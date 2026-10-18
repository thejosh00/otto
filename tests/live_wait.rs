//! tests/live_wait.rs — prove a wait with a limit ends at its limit, against a real model.
//!
//! What `cargo test` cannot reach: whether a wake whose instructions say how long is too long
//! bounds the wait with `arm-timer --waiting-for --give-up-after` (`harness/wake.md`, "Waiting"),
//! and whether the wake that finds it run out stops waiting and asks rather than sleeping again.
//! CI here stays pending for good, and the instructions say two minutes is the most it takes.
//!
//! Three or four short wakes a minute apart, ~5 minutes.
//!
//!   cargo test --test live_wait -- --ignored --nocapture

mod support;

use std::time::Duration;
use support::*;

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_wait() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - start ci-flow, with CI pending for good");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let status = std::env::temp_dir().join(format!("otto-ci-status-{stamp}"));
    write_file(&status, "pending\n");
    let run_id = format!("wait-{stamp}");
    let instructions = repo_path("test/fixtures/ci-flow.md");
    let fact = format!("ciStatusFile={}", status.display());
    let run = start_run(
        &mut checks,
        &run_id,
        "Report CI's result for build 7",
        &[
            "--instructions",
            instructions.to_str().unwrap(),
            "--fact",
            &fact,
            "--until",
            "the person has CI's result for build 7, and the run is done",
        ],
    );

    checks.step("2 - it waits, with a limit");
    if !await_field(&run, "wake.outcome", "complete", Duration::from_secs(420)) {
        checks.bail(&run, "the first wake never finished");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the first wake has exited", "a wake is still running");
    checks.check(run.status() == "sleeping", "it sleeps on CI", format!("status={}", run.status()));
    let limited = run.field_str("wait.giveUpAt");
    checks.check(!limited.is_empty(), format!("waiting for \"{}\" until {limited}", run.field_str("wait.for")), "no limit on the wait");

    // Wake it ourselves, a minute apart, rather than poke every run in the home.
    checks.step("3 - past the limit, it stops waiting and asks");
    for attempt in 1..=5 {
        if run.field_str("gate").starts_with('{') || run.is_over() {
            break;
        }
        std::thread::sleep(Duration::from_secs(65));
        checks.note(format!("waking it (attempt {attempt}): wait {}", run.field_str("wait.for")));
        let _ = otto(&["wake", &run_id]);
        std::thread::sleep(Duration::from_secs(5));
        if !await_idle(&run, Duration::from_secs(420)) {
            checks.bail(&run, "a wake never exited");
        }
    }
    checks.check(run.field_str("gate").starts_with('{'), "it asks the person", format!("status={}, no question", run.status()));
    let question = run.gate_files_matching(".md").last().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    checks.check(question.to_lowercase().contains("ci"), "about CI", format!("asked: {}", question.trim()));
    checks.note(format!("waits begun: {}, sleeps refused past the limit: {}", run.events("waiting"), run.events("wait-overdue")));
    let _ = std::fs::remove_file(&status);

    finish_run(checks, &run, "awaiting_human", None, None);
}
