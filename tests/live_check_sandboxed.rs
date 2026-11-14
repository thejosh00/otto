//! tests/live_check_sandboxed.rs — whether a real wake, told only to watch something, writes a check script that
//! still works when poke runs it under the run's sandbox record (plans/v3.md, 1.3). The harness
//! (`harness/wake.md`, "Cheaper than a tick: check scripts") no longer offers `gh` as the example, because `gh`
//! doesn't work under a profile and a check that fails wakes its run every period. That wording is read by a
//! model, so only a model can show what it leads to.
//!
//! The wake itself is not sandboxed: the record names the plain launcher, as `live_sandbox_refused` does, and
//! the run agrees with it. What runs under the profile is the script the wake left, when poke comes to it.
//! That a profile closes what it should has its own tests, with no model (`tests/sandbox.rs`).
//!
//! Uses the network: the watched repository is a public one on GitHub that doesn't change.
//! ~2 minutes, one wake.
//!
//!   cargo test --test live_check_sandboxed -- --ignored --nocapture

mod support;

use std::time::Duration;
use support::*;

/// Public, tiny, and its default branch hasn't moved in years: so "nothing new" is the right answer.
const WATCHED: &str = "https://github.com/octocat/Hello-World";

/// Whether a profile can be entered from here. Not from inside a sandboxed wake (`tests/sandbox.rs`).
fn can_nest() -> bool {
    std::process::Command::new("/usr/bin/sandbox-exec")
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Whether a script runs `gh`: a word of its own, or the end of a path, on a line that isn't a comment.
fn calls_gh(script: &str) -> bool {
    script
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .any(|line| {
            line.split(|c: char| c.is_whitespace() || ";|&()`\"'$=".contains(c))
                .any(|word| word == "gh" || word.ends_with("/gh"))
        })
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_check_sandboxed() {
    let mut checks = Checks::new();
    checks.step("0 - preconditions");
    require_launcher(&mut checks);
    if !can_nest() {
        checks.bad("already inside a sandbox, so a different profile can't start here: run this from outside one, before spending a wake on it");
        checks.finish();
        return;
    }
    // A home of this test's own, and not the one `support` makes under `$TMPDIR`. The profile leaves
    // `$TMPDIR` open to writes unless the otto home is inside it, in which case it keeps the whole of it
    // closed (`sandbox::temp_dirs`). So with the usual home the script would run under a profile no real
    // run gets, one where `mktemp` fails. Under the target directory, as `tests/sandbox.rs` puts its
    // scratch, the profile is the real shape. The working directory stays where `support` put it.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let home = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("otto-live-check-{stamp}"))
        .join("home");
    std::fs::create_dir_all(&home).unwrap();
    let home = home.canonicalize().unwrap();
    assert!(
        !home.starts_with("/private/tmp"),
        "CARGO_TARGET_DIR is under /private/tmp, which the profile leaves open"
    );
    std::fs::copy(own_home().join("config.json"), home.join("config.json")).unwrap();
    std::env::set_var("OTTO_HOME", &home);
    checks.ok(format!("OTTO_HOME={}", home.display()));

    checks.step("1 - a run whose work is watching");
    let run_id = format!("watch-{stamp}");
    let goal = format!(
        "Watch the public git repository {WATCHED} for new commits on its default branch. There is nothing to do about the commits already there: this wake, record the current head as the one already seen, and that is all. Whenever a later wake finds a newer commit, add one line about it to artifacts/commits.md and record it as seen."
    );
    let run = start_run(
        &mut checks,
        &run_id,
        &goal,
        &["--perpetual", "--period", "6h"],
    );
    if !await_field(&run, "wake.n", "1", Duration::from_secs(120))
        || !await_idle(&run, Duration::from_secs(360))
    {
        checks.bail(&run, "the first wake never finished");
    }
    checks.check(
        run.events("wake-complete") == 1 && run.status() == "sleeping",
        "the first wake completed and the run sleeps",
        format!(
            "status {}, {} wake-complete",
            run.status(),
            run.events("wake-complete")
        ),
    );

    checks.step("2 - the wake set a check of its own, and it doesn't use gh");
    let check = run.field("check").filter(|c| !c.is_null());
    let Some(check) = check else {
        println!("wake output tail:\n{}", run.wake_log(40));
        checks.bail(
            &run,
            "the wake set no check script, so there is nothing to run under a profile",
        );
    };
    println!("       run.json check: {check}");
    checks.check(
        check["setByWake"] == true,
        "check.setByWake is true",
        "the check wasn't set by the wake",
    );
    checks.check(
        check["pinned"] == true,
        "it is a standing check (set-check)",
        "it was armed with one sleep, not set as the run's standing check",
    );
    let script_path = run.dir.join(check["script"].as_str().unwrap_or_default());
    let script = std::fs::read_to_string(&script_path).unwrap_or_default();
    println!(
        "\n--- {} ---\n{script}\n--- end of script ---",
        script_path.display()
    );
    checks.check(
        !script.is_empty(),
        "the script is there",
        format!("nothing at {}", script_path.display()),
    );
    checks.check(
        !calls_gh(&script),
        "the script doesn't call gh",
        "the script calls gh, which doesn't work under a profile",
    );
    if script.contains("|| true") || script.contains("||true") {
        checks.note("the script has an `|| true` in it: read above whether it hides a failed read");
    }

    checks.step("3 - the run gets a sandbox record it agrees with");
    let run_json = run.dir.join("run.json");
    let read = || -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(&run_json).unwrap()).unwrap()
    };
    let launcher = read()["launcher"].clone();
    let id = run.dir.file_name().unwrap().to_str().unwrap().to_string();
    let records = otto_data_dir().join("sandbox/runs");
    std::fs::create_dir_all(&records).unwrap();
    let record = serde_json::json!({"launcher": launcher["kind"], "workdir": launcher["workdir"], "write": launcher["repos"].as_array().cloned().unwrap_or_default()});
    std::fs::write(records.join(format!("{id}.json")), record.to_string()).unwrap();
    checks.ok(format!("record: {record}"));
    let profile = otto_data_dir()
        .join("sandbox/profiles")
        .join(format!("{id}.check.sb"));
    checks.check(
        !profile.exists(),
        "no check has run under a profile yet",
        "a check profile is already there",
    );

    checks.step("4 - its wake comes due, and poke runs the check under the profile");
    let line_for_run = |said: &str| {
        said.lines()
            .find(|l| l.split_whitespace().nth(2) == Some(id.as_str()))
            .map(str::to_string)
    };
    let mut poked = String::new();
    // The wake can have one last write of `run.json` in hand when it looks finished, and this edit takes
    // no lock: if that undid it, poke says the run isn't due yet, and it is done again (`tests/check_history.rs`).
    for _ in 0..10 {
        let mut state = read();
        state["nextWakeAt"] = "2020-01-01T00:00:00Z".into();
        std::fs::write(&run_json, serde_json::to_string_pretty(&state).unwrap()).unwrap();
        let out = otto_command()
            .env("OTTO_BIN", OTTO)
            .args(["poke", "--verbose"])
            .output()
            .unwrap();
        poked = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if !line_for_run(&poked).is_some_and(|l| l.contains(" skip ") && l.contains("due at")) {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    println!(
        "\n--- otto poke --verbose ---\n{}\n--- end of poke ---",
        poked.trim_end()
    );
    let line = line_for_run(&poked).unwrap_or_default();
    checks.check(
        line.contains("no change; sleeping again"),
        "poke: checked, no change; sleeping again",
        format!("poke said of the run: {line:?}"),
    );
    checks.check(
        profile.is_file(),
        format!("it ran under {}", profile.display()),
        "no check profile was written, so the script didn't run under one",
    );

    let after = run.field("check").unwrap_or_default();
    println!("       run.json check: {after}");
    checks.check(
        after["lastResult"] == "no-change",
        "check.lastResult is no-change",
        format!(
            "check.lastResult is {}, note {}",
            after["lastResult"], after["lastNote"]
        ),
    );
    checks.note(format!("what the script printed: {}", after["lastNote"]));
    let history = std::fs::read_to_string(run.dir.join("check-history.jsonl")).unwrap_or_default();
    println!("       check-history.jsonl: {}", history.trim_end());
    let last: serde_json::Value = history
        .lines()
        .last()
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or_default();
    checks.check(
        last["code"] == 0,
        "the script exited 0 under the profile",
        format!("the last check-history line: {last}"),
    );

    checks.step("5 - and no second wake started");
    // Were one started it would take a moment to show; a no-change pass starts nothing, so this is short.
    std::thread::sleep(Duration::from_secs(3));
    checks.check(
        run.events("wake-started") == 1,
        "one wake in all",
        format!("{} wakes started", run.events("wake-started")),
    );
    checks.check(
        run.field("wake.n").and_then(|n| n.as_u64()) == Some(1),
        "wake.n is still 1",
        format!("wake.n is {}", run.field_str("wake.n")),
    );
    checks.check(
        run.status() == "sleeping",
        "the run sleeps on",
        format!("status {}", run.status()),
    );
    checks.check(
        run.events("sandbox-refused") == 0,
        "nothing was refused",
        "the run was refused: it disagrees with the record this test wrote",
    );

    if let Some(transcript) = run.wake_transcript() {
        checks.note(format!(
            "the wake's transcript is {} bytes",
            transcript.len()
        ));
    }
    run.journal_dump();
    // Whatever happened, nothing more is to be spent on it.
    let _ = otto(&["stop", &run.id, "--reason", "live test finished"]);
    checks.finish();
}
