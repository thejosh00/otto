//! tests/live_driver_blocked.rs — prove a driver wake that finds another agent blocked tells the
//! person once and records that it looked, so its check doesn't wake it for the same one again,
//! against a real model.
//!
//! What `cargo test` cannot reach: whether a model reading `workflows/drive-flow.md` writes
//! `artifacts/blocked-seen` the way `scripts/driver-check.sh` reads it. `tests/driver_check.rs`
//! runs the flow's command itself; here a wake has to find it, run it, and leave the rest of what
//! it keeps alone. Without the file the check woke the driver every period an agent stayed blocked.
//!
//! The driver is mid-plan: one item out and still open, so the blocked agent is its only cue.
//!
//! One run, one wake, ~3 minutes.
//!
//!   cargo test --test live_driver_blocked -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use support::*;

const ITEM: &str = "Plan item 0b.1: Why a wake started

Add `cause` to the `wake-started` event: period, check changed, check safety net, or a person.

How I'd know: `otto show <run> --journal` says why each wake started.

Plan: plans/v3.md, \"Phase 0b: debuggability\".
";

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_DRIVER_BLOCKED_TIMEOUT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    Duration::from_secs(secs)
}

/// A run with no wake yet. Only the driver gets one.
fn create(checks: &mut Checks, id: &str, name: &str, goal: &str, extra: &[&str]) -> Run {
    let mut args = vec![
        "state",
        "init",
        "--id",
        id,
        "--name",
        name,
        "--goal",
        goal,
        "--perpetual",
        "--period",
        "15m",
    ];
    args.extend_from_slice(extra);
    let made = otto(&args);
    checks.check(
        made.status.success(),
        format!("created {name}"),
        String::from_utf8_lossy(&made.stderr),
    );
    Run::new(id)
}

fn block(checks: &mut Checks, run: &Run, reason: &str) {
    let set = otto(&[
        "state",
        "set-status",
        &run.id,
        "--status",
        "blocked",
        "--because",
        "budget",
        "--reason",
        reason,
    ]);
    checks.check(
        set.status.success(),
        format!("{} is blocked: {reason}", run.id),
        String::from_utf8_lossy(&set.stderr),
    );
}

fn queue() -> Vec<Value> {
    let all: Value = serde_json::from_str(&stdout(&otto(&["feedback", "--all", "--json"])))
        .unwrap_or(Value::Null);
    all.as_array().cloned().unwrap_or_default()
}

fn await_first_wake_over(run: &Run, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if run.events("wake-complete") + run.events("wake-incomplete") > 0 && !run.wake_running() {
            return true;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    false
}

/// Everything written under a directory of the run, in one string.
fn written_under(dir: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return String::new();
    };
    let mut all = String::new();
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() {
            all.push_str(&written_under(&path));
        } else {
            all.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            all.push('\n');
        }
    }
    all
}

/// The driver's check, as otto runs it: the real script, which looks for otto in `$HOME/.local/bin`,
/// so it is given a `HOME` where that is the otto under test. Its exit code and what it said.
fn driver_check(user: &Path, driver: &Run) -> (Option<i32>, String) {
    let mut command = Command::new("/bin/sh");
    command
        .arg(repo_path("scripts/driver-check.sh"))
        .env("HOME", user)
        .env("OTTO_HOME", own_home())
        .env("OTTO_RUN_ID", &driver.id);
    command.env("OTTO_NOTIFY", "off").env_remove("OTTO_RUN_DIR");
    let output = command.output().expect("run scripts/driver-check.sh");
    (
        output.status.code(),
        format!(
            "{}{}",
            stdout(&output),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn a_home_where_otto_is_this_build(root: &Path) -> PathBuf {
    let user = root.join("user");
    std::fs::create_dir_all(user.join(".local/bin")).expect("create the check's home");
    let link = user.join(".local/bin/otto");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(otto_bin(), &link).expect("link the otto under test");
    user
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_driver_blocked() {
    // The person's side of it: naming the driver is refused from a wake, and the wake running
    // these tests may well be one.
    std::env::remove_var("OTTO_RUN_ID");
    std::env::remove_var("OTTO_RUN_DIR");
    let timeout = timeout();
    let mut checks = Checks::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);
    let root = own_home()
        .parent()
        .expect("the test's home has a parent")
        .to_path_buf();
    let user = a_home_where_otto_is_this_build(&root);
    // Wakes run in the test's work directory, and the flow reads the plan from there.
    write_file(
        &root.join("work/plans/v3.md"),
        &std::fs::read_to_string(repo_path("plans/v3.md")).expect("plans/v3.md"),
    );

    checks.step("1 - a driver mid-plan: one item out and still open, nothing else to do");
    let flow = repo_path("workflows/drive-flow.md");
    let driver = create(
        &mut checks,
        &format!("blocked-driver-{stamp}"),
        "otto-driver",
        "Carry plans/v3.md to done, one item at a time",
        &["--instructions", flow.to_str().unwrap()],
    );
    let builder = create(
        &mut checks,
        &format!("blocked-build-{stamp}"),
        "otto-build",
        "Build otto from its feedback queue, one item at a time",
        &[],
    );
    let curator = create(
        &mut checks,
        &format!("blocked-curate-{stamp}"),
        "legenda-curate",
        "Find new things worth reading, once a day",
        &[],
    );
    let tutor = create(
        &mut checks,
        &format!("blocked-teach-{stamp}"),
        "lectio-teach",
        "Teach today's lessons",
        &[],
    );
    let named = otto(&["feedback", "driver", &driver.id]);
    checks.check(
        named.status.success(),
        "the person named the driver",
        String::from_utf8_lossy(&named.stderr),
    );

    let item = root.join("item.md");
    write_file(&item, ITEM);
    let assigned = otto_command()
        .args([
            "state",
            "assign",
            &driver.id,
            "--file",
            item.to_str().unwrap(),
        ])
        .env("OTTO_RUN_ID", &driver.id)
        .output()
        .unwrap();
    checks.check(
        assigned.status.success(),
        "its item 0b.1 is in the queue",
        String::from_utf8_lossy(&assigned.stderr),
    );
    let report = queue()
        .iter()
        .find(|r| r["assignedBy"] == driver.id.as_str())
        .map(|r| r["id"].to_string())
        .unwrap_or_default();
    checks.check(
        !report.is_empty(),
        format!("as report {report}"),
        "the queue doesn't show it",
    );

    let runs: Value =
        serde_json::from_str(&stdout(&otto(&["state", "list", "--json"]))).unwrap_or(Value::Null);
    let baseline: Vec<Value> = runs
        .as_array()
        .map(|runs| {
            runs.iter()
                .map(|r| {
                    let goal = r["goal"].as_str().unwrap_or("");
                    let hash = goal.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
                    serde_json::json!({ "id": r["id"], "name": r["name"], "goalHash": format!("{hash:016x}") })
                })
                .collect()
        })
        .unwrap_or_default();
    write_file(
        &driver.artifact("baseline.json"),
        &serde_json::to_string_pretty(&baseline).unwrap(),
    );
    let todo = format!(
        "# plans/v3.md\n\n- [x] 0.1 Only kill an unfinished wake\n- [x] 0.2 Don't signal a finished wake's pid\n- [ ] 0b.1 Why a wake started (out: report {report})\n- [ ] 0b.2 Evidence on a kill\n- [ ] 0b.3 Check history\n- [ ] 0b.4 Logs you can read\n"
    );
    write_file(&driver.artifact("todo.md"), &todo);
    let ledger = format!(
        "0.1 — report 1, change 1 — done: a finished wake is left alone (cargo test, and the journal after an install)\n0.2 — report 2, change 2 — done: no signal to a finished wake's pid (cargo test)\n0b.1 — report {report} — assigned, not yet built\n"
    );
    write_file(&driver.artifact("ledger.md"), &ledger);
    write_file(&driver.artifact("waiting-on"), &format!("{report}\n"));
    let (code, said) = driver_check(&user, &driver);
    checks.check(
        code == Some(0),
        format!("its check has nothing for it: {said}"),
        format!("its check would wake it anyway ({code:?}): {said}"),
    );

    checks.step("2 - another agent is blocked, and not by an install; the check wakes the driver");
    block(
        &mut checks,
        &curator,
        "token budget spent: 50M of 50M tokens used",
    );
    let at = curator.field_str("blocked.at");
    let key = format!("{} {at}", curator.id);
    let (code, said) = driver_check(&user, &driver);
    checks.check(
        code == Some(1) && said.contains("legenda-curate"),
        format!("the check: {said}"),
        format!("the check said ({code:?}): {said}"),
    );
    let woke = otto(&["wake", &driver.id]);
    checks.check(
        woke.status.success(),
        "woke the driver",
        String::from_utf8_lossy(&woke.stderr),
    );

    checks.step(&format!("3 - its wake ends (up to {}s)", timeout.as_secs()));
    if !await_first_wake_over(&driver, timeout) {
        checks.bail(&driver, "the driver's wake never ended");
    }
    checks.check(
        driver.events("wake-complete") == 1,
        "the wake completed",
        "the wake did not complete",
    );

    checks.step("4 - it told the person about the blocked agent");
    let told = format!(
        "{}{}",
        written_under(&driver.dir.join("messages")),
        written_under(&driver.dir.join("gates"))
    )
    .to_lowercase();
    let sent = driver.events("message-sent");
    let asked = driver.field("gate").is_some_and(|g| !g.is_null());
    checks.note(format!(
        "{sent} message(s), {}",
        if asked { "a gate open" } else { "no gate" }
    ));
    checks.check(
        sent > 0 || asked,
        "it sent a message or opened a gate",
        "it said nothing to the person",
    );
    checks.check(
        told.contains("legenda-curate") || told.contains(&curator.id),
        "which names the blocked agent",
        format!("nothing it sent names legenda-curate or {}", curator.id),
    );
    checks.check(
        told.contains("budget") || told.contains("token"),
        "and says why it is blocked",
        "nothing it sent says why",
    );

    checks.step("5 - it recorded that it looked, and left the rest alone");
    let seen = std::fs::read_to_string(driver.artifact("blocked-seen")).unwrap_or_default();
    checks.note(format!("artifacts/blocked-seen: {:?}", seen));
    checks.check(
        seen.lines().any(|l| l == key),
        format!("artifacts/blocked-seen has \"{key}\""),
        format!("artifacts/blocked-seen has no line \"{key}\""),
    );
    checks.check(
        seen.lines().count() == 1,
        "and nothing else",
        format!("{} lines", seen.lines().count()),
    );
    let waiting = std::fs::read_to_string(driver.artifact("waiting-on")).unwrap_or_default();
    checks.check(
        waiting.trim() == report,
        "waiting-on still names its item",
        format!("waiting-on is now {waiting:?}"),
    );
    let assigned = queue()
        .iter()
        .filter(|r| r["assignedBy"] == driver.id.as_str())
        .count();
    checks.check(
        assigned == 1,
        "it handed off nothing more",
        format!("{assigned} items from it in the queue"),
    );
    checks.check(
        curator.status() == "blocked",
        "the blocked agent is as it was",
        format!("legenda-curate is now {}", curator.status()),
    );

    checks.step("6 - the check doesn't wake it again for the same one, and does for another");
    for period in 2..=3 {
        let (code, said) = driver_check(&user, &driver);
        checks.check(
            code == Some(0),
            format!("period {period}: {said}"),
            format!("period {period}, a wake again ({code:?}): {said}"),
        );
    }
    block(
        &mut checks,
        &tutor,
        "token budget spent: 20M of 20M tokens used",
    );
    let (code, said) = driver_check(&user, &driver);
    checks.check(
        code == Some(1) && said.contains("lectio-teach"),
        format!("a different agent: {said}"),
        format!("a different agent blocked ({code:?}): {said}"),
    );

    for other in [&builder, &curator, &tutor] {
        let _ = otto(&["stop", &other.id, "--reason", "live test finished"]);
    }
    let _ = otto(&["feedback", "driver", "--clear"]);
    let ended = driver.status();
    finish_run(checks, &driver, &ended, None, None);
}
