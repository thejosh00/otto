//! tests/live_gate.rs — prove the gate round-trip end to end, against a real model.
//!
//! The cheapest live check there is, and the one to run  after any change to the engine.
//! It proves what `cargo test` structurally cannot: that a real model, handed the harness prompt,
//! produces correct behavior — writes its artifact, opens a gate, stops in a legal state, and then
//! reads a verbatim answer back off disk on a *different* wake with none of the first wake's context.
//!
//! ~4 minutes, two wakes, roughly $0.60.
//!
//!   cargo test --test live_gate -- --ignored --nocapture

mod support;

use std::time::Duration;
use support::*;

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_GATE_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    Duration::from_secs(secs)
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_gate() {
    let timeout = timeout();
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - start hello-flow, wrapped as an instructions file");
    let run_id = format!("gate-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    let instructions = repo_path("test/fixtures/hello-flow.md");
    let run = start_run(
        &mut checks,
        &run_id,
        "Smoke-test otto's machinery: greet, gate, farewell.",
        &[
            "--instructions",
            instructions.to_str().unwrap(),
            "--until",
            "artifacts/farewell.md exists and the run is done",
        ],
    );
    checks.check(run.dir.is_dir(), "run directory exists", format!("no run directory at {}", run.dir.display()));

    checks.step(&format!("2 - wait for the gate (up to {}s)", timeout.as_secs()));
    if !await_field(&run, "status", "awaiting_human", timeout) {
        checks.bail(&run, "never reached a gate");
    }
    checks.ok(format!("parked on a gate after {}min", checks.elapsed().as_secs() / 60));

    let gate_file_rel = run.field_str("gate.file");
    let gate_file = run.dir.join(&gate_file_rel);
    checks.check(gate_file.is_file(), format!("gate file on disk: {gate_file_rel}"), "no gate file");
    let gate_text = std::fs::read_to_string(&gate_file).unwrap_or_default();
    checks.check(gate_text.contains("## Question"), "carries a question", "gate file has no question");
    checks.check(gate_text.contains("_unanswered_"), "marked unanswered", "gate file is not marked unanswered");

    checks.check(await_idle(Duration::from_secs(120)), "the wake that asked has exited", "a wake is still running at the gate");

    checks.step("3 - the artifact the instructions asked for");
    let greeting = run.artifact("greeting.md");
    checks.check(greeting.is_file(), "greeting.md written", "no greeting.md");
    let greeting_text = std::fs::read_to_string(&greeting).unwrap_or_default();
    if greeting_text.contains(&run_id) {
        checks.ok("names its own run");
    } else {
        checks.note("greeting.md does not name the run");
    }

    checks.step("4 - the handoff - the only thing the next wake gets");
    let handoff = run.handoff_path();
    checks.check(handoff.is_file(), "handoff written", "no handoff");
    let cap: u64 = run.field("policy.handoffMaxBytes").and_then(|v| v.as_u64()).unwrap_or(8192);
    let bytes = std::fs::metadata(&handoff).map(|m| m.len()).unwrap_or(0);
    checks.check(bytes <= cap, format!("handoff is {bytes}B, under the {cap}B cap"), format!("handoff is {bytes}B, over the {cap}B cap"));
    let handoff_text = std::fs::read_to_string(&handoff).unwrap_or_default();
    checks.check(
        handoff_text.contains("## Next wake must"),
        "says what the next wake must do",
        "handoff has no 'Next wake must'",
    );

    checks.step("5 - answer it from the CLI");
    if !answer_gate(&mut checks, &run, "greet-approval", "Approve — the greeting looks right.", timeout) {
        checks.bail(&run, "could not answer the gate");
    }
    let gate_field = run.field("gate");
    checks.check(
        matches!(gate_field, None | Some(serde_json::Value::Null)),
        "gate cleared",
        "gate still open",
    );

    checks.step("6 - a second wake carries it to done");
    let _ = otto(&["wake", &run_id]);
    if !await_field(&run, "status", "done", timeout) {
        checks.bail(&run, "never finished");
    }
    checks.ok(format!("reached done after {}min", checks.elapsed().as_secs() / 60));

    // The point of the whole exercise: a wake with none of the first wake's context read the
    // answer back off disk and acted on it.
    let farewell = run.artifact("farewell.md");
    checks.check(farewell.is_file(), "farewell.md written", "no farewell.md");
    let farewell_text = std::fs::read_to_string(&farewell).unwrap_or_default().to_lowercase();
    checks.check(
        farewell_text.contains("approve"),
        "the second wake quoted the answer it never heard",
        "farewell.md does not carry the verbatim answer",
    );

    checks.step("7 - nothing left pending");
    let next_wake_at = run.field_str("nextWakeAt");
    checks.check(next_wake_at.is_empty() || next_wake_at == "null", "no stray timer", "nextWakeAt is still set");
    let completed = run.events("wake-complete");
    checks.check(completed >= 2, format!("{completed} wakes completed"), "expected at least 2 completed wakes");
    let incomplete = run.events("wake-incomplete");
    if incomplete == 0 {
        checks.ok("no wake failed its contract");
    } else {
        checks.note(format!("{incomplete} wake(s) failed the contract — see the journal"));
    }

    finish_run(checks, &run, "done", None, None);
}
