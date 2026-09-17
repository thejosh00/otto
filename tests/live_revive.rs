//! tests/live_revive.rs — prove a run survives having its wake killed, and a reboot.
//!
//! The claim under test is the one the whole design rests on: a run's truth is on disk, so losing the
//! process that was working on it costs nothing but time. Concretely — `kill -9` a wake mid-flight
//! and the run keeps its state, releases its lock, gets noticed as stranded, and carries on from
//! exactly where it was.
//!
//! Three things `cargo test` (the unit suite) cannot reach:
//!
//!   1. flock really is released by the kernel on SIGKILL, with no bookkeeping and nothing
//!      to time out. Everything in poke's liveness rests on that.
//!   2. A wake that started and then died is *recorded*, not instantly respawned. Without
//!      that, poke restarts a reliably-crashing wake every five minutes forever and pays
//!      each time.
//!   3. The installed launchd agent does it all unassisted, which is what "survives a
//!      reboot" actually means — see `live_revive_launchd`, its own test below.
//!
//!   cargo test --test live_revive -- --ignored --nocapture live_revive
//!   cargo test --test live_revive -- --ignored --nocapture live_revive_launchd

mod support;

use std::time::{Duration, Instant};
use support::*;

const AGENT: &str = "com.joshuahill.otto-poke";

fn ticks() -> u32 {
    std::env::var("OTTO_LIVE_REVIVE_TICKS").ok().and_then(|s| s.parse().ok()).unwrap_or(2)
}

fn tick_seconds() -> u32 {
    std::env::var("OTTO_LIVE_REVIVE_TICK_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(120)
}

/// The action field of the `otto poke --verbose` decision line for one run — that line reads
/// `<stamp> <action> <run-id> <reason...>`, whitespace-separated.
fn poke_action(run_id: &str, args: &[&str]) -> Option<String> {
    let mut full = vec!["poke", "--verbose"];
    full.extend_from_slice(args);
    let out = otto(&full);
    let text = format!("{}{}", stdout(&out), String::from_utf8_lossy(&out.stderr));
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 3 && fields[2] == run_id {
            return Some(fields[1].to_string());
        }
    }
    None
}

fn kill9(pid: u32) {
    let group_kill = std::process::Command::new("kill").args(["-9", &format!("-{pid}")]).status();
    if group_kill.map(|s| s.success()).unwrap_or(false) {
        return;
    }
    let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
}

fn current_uid() -> String {
    let out = std::process::Command::new("id").arg("-u").output().expect("id -u");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn launchd_ready() -> bool {
    let uid = current_uid();
    std::process::Command::new("launchctl")
        .arg("print")
        .arg(format!("gui/{uid}/{AGENT}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn start_nap(checks: &mut Checks, run_id: &str, ticks: u32, tick_seconds: u32) -> Run {
    let instructions = repo_path("test/fixtures/nap-flow.md");
    start_run(
        checks,
        run_id,
        "Sleep, wake, account for a tick, and repeat until ticksDone == ticksRequired.",
        &[
            "--instructions",
            instructions.to_str().unwrap(),
            "--phase",
            "nap",
            "--fact",
            &format!("tickSeconds={tick_seconds}"),
            "--fact",
            &format!("ticksRequired={ticks}"),
            "--fact",
            "ticksDone=0",
            "--until",
            "facts.ticksDone reaches facts.ticksRequired",
        ],
    )
}

#[test]
#[ignore = "spawns real claude wakes, kills one with SIGKILL, and spends real money — run with --ignored"]
fn live_revive() {
    let ticks_required = ticks();
    let tick_secs = tick_seconds();
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step(&format!("1 - start a nap-flow run ({ticks_required} ticks, {tick_secs}s apart)"));
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let run_id = format!("nap-{stamp}");
    let run = start_nap(&mut checks, &run_id, ticks_required, tick_secs);
    checks.ok(format!("run {run_id}"));

    checks.step("2 - the first tick and the timer it armed");
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let next_wake_at = run.field_str("nextWakeAt");
        if !next_wake_at.is_empty() && next_wake_at != "null" && run.field_str("facts.ticksDone") != "0" {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    let wake_at = run.field_str("nextWakeAt");
    let done_at_kill = run.field_str("facts.ticksDone");
    checks.check(!wake_at.is_empty() && wake_at != "null", format!("nextWakeAt={wake_at}"), "the timer was never armed");
    let done_at_kill_n: i64 = done_at_kill.parse().unwrap_or(0);
    checks.check(done_at_kill_n >= 1, format!("ticksDone={done_at_kill}"), "no tick was accounted for");
    checks.check(run.status() == "sleeping", "status=sleeping", format!("status={}", run.status()));
    checks.check(run.events("wake-complete") >= 1, "the first wake finished cleanly", "no completed wake");

    checks.step("3 - start a wake and kill -9 it mid-flight");
    // Force one now rather than waiting out the cadence; a wake started by hand is the same
    // process poke would start.
    let _ = otto(&["state", "arm-timer", &run_id, "--at", "2020-01-01T00:00:00Z"]);
    let _ = otto(&["poke", "--grace", "0"]);
    let mut pid: Option<u32> = None;
    for _ in 0..30 {
        let p = run.field_str("wake.pid");
        if !p.is_empty() && p != "null" {
            pid = p.parse().ok();
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let Some(pid) = pid else {
        checks.bail(&run, "poke started no wake");
    };
    checks.ok(format!("a wake is running (pid {pid})"));
    let ls_before_kill = stdout(&otto(&["ls"]));
    checks.check(ls_before_kill.contains("working (wake"), "and the lock reports it as running", "a wake is running but liveness does not see it");

    kill9(pid);
    std::thread::sleep(Duration::from_secs(2));
    // (1) The kernel released the flock with no cleanup step of otto's.
    let ls_after_kill = stdout(&otto(&["ls"]));
    checks.check(
        !ls_after_kill.contains("working (wake"),
        "the kernel released the wake lock on SIGKILL, unassisted",
        "the wake lock outlived the process — liveness would be permanently wrong",
    );
    let run_json_ok = std::fs::read_to_string(run.dir.join("run.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).is_some();
    checks.check(run_json_ok, "run.json survived intact", "run.json is corrupt after kill -9");
    checks.check(
        run.field_str("facts.ticksDone") == done_at_kill,
        format!("ticksDone still {done_at_kill} — the count is on disk, not in the process"),
        "ticksDone changed across the kill",
    );

    checks.step("4 - the crash is recorded, not instantly retried");
    // (2) The bound that stops a crash loop costing money forever.
    let before_incomplete = run.events("wake-incomplete");
    let action = poke_action(&run_id, &["--grace", "0"]);
    checks.note(format!("acted: {}", action.as_deref().unwrap_or("nothing")));
    checks.check(
        action.as_deref() == Some("recover"),
        "poke recorded the dead wake instead of respawning it",
        format!("expected 'recover', got '{}' — a crash loop would be unbounded", action.as_deref().unwrap_or("nothing")),
    );
    checks.check(
        run.events("wake-incomplete") > before_incomplete,
        format!("counted against the retry budget (incompleteWakes={})", run.field_str("incompleteWakes")),
        "the crash was not counted, so nothing bounds a repeat",
    );
    checks.check(
        run.status() == "sleeping" && !run.field_str("nextWakeAt").is_empty(),
        "left somewhere something will come back to",
        format!("status={}, nextWakeAt={} — the run is stranded", run.status(), run.field_str("nextWakeAt")),
    );

    checks.step("5 - inside the grace it defers; past it, it spawns");
    let _ = otto(&["state", "arm-timer", &run_id, "--at", "2020-01-01T00:00:00Z"]);
    let deferred = poke_action(&run_id, &["--dry-run", "--grace", "999"]);
    checks.check(
        deferred.as_deref() == Some("defer"),
        "defers while a wake could still be in flight",
        "did not defer — a merely-late wake would be double-started",
    );
    let spawned = poke_action(&run_id, &["--grace", "0"]);
    checks.check(spawned.as_deref() == Some("spawn"), "spawned a wake for the overdue run", format!("expected 'spawn', got '{}'", spawned.as_deref().unwrap_or("nothing")));
    await_idle(Duration::from_secs(600));
    let final_ticks: i64 = run.field_str("facts.ticksDone").parse().unwrap_or(0);
    if final_ticks > done_at_kill_n {
        checks.ok(format!("the run carried on: ticksDone={final_ticks}"));
    } else {
        checks.note("ticksDone did not advance — see the journal");
    }

    checks.step("6 - accounting");
    checks.check(run.events("wake-incomplete") >= 1, "the killed wake is in the journal as incomplete", "no wake-incomplete line for a wake we killed");
    // Every wake must have ended somewhere resumable; the validator is what guarantees it.
    let final_status = run.status();
    checks.check(
        final_status == "sleeping" || final_status == "done",
        format!("final status={final_status}"),
        format!("final status={final_status}"),
    );

    finish_run(checks, &run, &final_status, None, None);
}

/// The installed agent, unassisted: nothing in this test touches it. The agent runs every 5
/// minutes with the default 15m grace, so a wake overdue by more than that is fair game.
/// Independent of `live_revive` (its own nap-flow run), so either test can run alone. Skips
/// itself with a note if the agent isn't loaded — start it first with `otto agent start`.
#[test]
#[ignore = "needs the launchd agent installed (`otto agent start`) and spends real money — run with --ignored"]
fn live_revive_launchd() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);
    if !launchd_ready() {
        checks.skip(format!("agent {AGENT} is not loaded — run `otto agent start` first"));
        return;
    }
    checks.ok(format!("launchd agent {AGENT} is loaded"));

    checks.step("1 - start a nap-flow run and let its first tick land");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let run_id = format!("nap-launchd-{stamp}");
    let run = start_nap(&mut checks, &run_id, ticks(), tick_seconds());
    let deadline = Instant::now() + Duration::from_secs(600);
    while run.field_str("facts.ticksDone") == "0" && Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(2));
    }
    checks.check(run.field_str("facts.ticksDone") != "0", "the first tick landed", "no tick happened before the launchd step");

    checks.step("2 - the installed agent, unassisted");
    let before = run.events("wake-started");
    let twenty_min_ago = time::OffsetDateTime::now_utc() - time::Duration::minutes(20);
    let at = twenty_min_ago.format(&time::format_description::well_known::Rfc3339).expect("formats as RFC 3339");
    let _ = otto(&["state", "arm-timer", &run_id, "--at", &at]);
    checks.note("armed a wake 20 minutes overdue; waiting up to 12 minutes for the agent");
    let deadline = Instant::now() + Duration::from_secs(720);
    while run.events("wake-started") <= before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(10));
    }
    checks.check(
        run.events("wake-started") > before,
        "the launchd agent started a wake with no help from this test",
        "the agent never fired — check ~/.otto/logs/poke.log",
    );
    await_idle(Duration::from_secs(600));

    finish_run(checks, &run, &run.status(), None, None);
}
