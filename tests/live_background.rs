//! tests/live_background.rs — prove a job that runs past a wake is picked up from what it wrote
//! down, against a real model.
//!
//! What `cargo test` cannot reach: a wake told to kick a slow job off and check back has no way
//! back but the next wake, and the process it started may die when it ends. The harness says the
//! job writes its result under `artifacts/` and the next wake trusts the file, not the process
//! ("Waiting", on background jobs). Whether a real wake does that — rather than sitting through
//! the job, or arming a timer to look at a process id — is prose, and only a live model shows it.
//!
//! Two or three wakes and a minute's job, ~4 minutes.
//!
//!   cargo test --test live_background -- --ignored --nocapture

mod support;

use std::time::Duration;
use support::*;

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_background() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - start build-flow, whose build takes a minute");
    let run_id = format!("build-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    let instructions = repo_path("test/fixtures/build-flow.md");
    let run = start_run(
        &mut checks,
        &run_id,
        "Run build 7 and record what it printed",
        &[
            "--instructions",
            instructions.to_str().unwrap(),
            "--until",
            "artifacts/result.md holds what the build printed, and the run is done",
        ],
    );

    checks.step("2 - the first wake starts it and stops, without waiting");
    if !await_field(&run, "wake.outcome", "complete", Duration::from_secs(420)) {
        checks.bail(&run, "the first wake never finished");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the first wake has exited", "a wake is still running");
    checks.check(run.status() == "sleeping", "it sleeps until the build is done", format!("status={}", run.status()));
    checks.check(!run.artifact("result.md").exists(), "no result yet: it didn't sit through the build", "it waited for the build");

    // Wake it ourselves rather than poke every run in the home; give the build its minute first.
    checks.step("3 - later wakes finish from what the build wrote down");
    for attempt in 1..=3 {
        if run.is_over() {
            break;
        }
        std::thread::sleep(Duration::from_secs(75));
        checks.note(format!("waking it (attempt {attempt})"));
        let _ = otto(&["wake", &run_id]);
        std::thread::sleep(Duration::from_secs(5));
        if !await_idle(&run, Duration::from_secs(420)) {
            checks.bail(&run, "a wake never exited");
        }
    }
    checks.check(run.status() == "done", "status=done", format!("status={}", run.status()));
    let result = std::fs::read_to_string(run.artifact("result.md")).unwrap_or_default();
    checks.check(result.contains("build 7 ok"), "artifacts/result.md holds what it printed", format!("result.md: {}", result.trim()));
    checks.note(format!("wakes: {}", run.events("wake-started")));

    finish_run(checks, &run, "done", None, None);
}
