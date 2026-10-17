//! tests/live_feedback.rs — prove a wake tells otto's maker when otto gets in its way, against a real
//! model.
//!
//! What `cargo test` cannot reach: whether a wake whose instructions collide with otto's own rules —
//! here, a status line in exactly the words `otto state handoff` refuses — reports it with
//! `otto state feedback` without being told to (`harness/wake.md`, "When otto gets in your way"),
//! in its own words, and keeps it out of the work. Models rarely volunteer feedback, so this is the
//! test of whether the harness wording is enough.
//!
//! Two wakes, the work and the check of its done, ~2 minutes.
//!
//!   cargo test --test live_feedback -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::Duration;
use support::*;

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_FEEDBACK_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(420);
    Duration::from_secs(secs)
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_feedback() {
    let timeout = timeout();
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - start clash-flow, whose status line otto refuses");
    let run_id = format!("clash-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    let instructions = repo_path("test/fixtures/clash-flow.md");
    let run = start_run(
        &mut checks,
        &run_id,
        "Write the greeting clash-flow asks for",
        &["--instructions", instructions.to_str().unwrap(), "--until", "artifacts/hello.md exists and the run is done"],
    );

    checks.step(&format!("2 - wait for it to finish (up to {}s)", timeout.as_secs()));
    if !await_field(&run, "status", "done", timeout) {
        checks.bail(&run, "never finished");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the last wake has exited", "a wake is still running");
    let refused = run.events("handoff-refused");
    checks.note(format!("handoffs refused for otto's words: {refused}"));

    checks.step("3 - it said what got in its way");
    let all: Value = serde_json::from_str(&stdout(&otto(&["feedback", "--all", "--json"]))).unwrap_or(Value::Null);
    let ours: Vec<&Value> = all.as_array().map(|reports| reports.iter().filter(|r| r["run"] == run_id.as_str()).collect()).unwrap_or_default();
    checks.check(!ours.is_empty(), format!("{} report(s)", ours.len()), "no feedback — the clash went unreported");
    checks.check(ours.len() <= 4, "no more than two a wake", format!("{} reports", ours.len()));
    checks.check(run.events("feedback-sent") == ours.len(), "the journal notes each one", "the journal and the file disagree");
    for report in &ours {
        let text = report["text"].as_str().unwrap_or("");
        checks.note(format!("wake {}: {text}", report["wake"]));
        checks.check(report["wake"].is_u64(), "stamped with its wake", "no wake on it");
    }
    let about_the_clash = ours.iter().any(|r| {
        let text = r["text"].as_str().unwrap_or("").to_lowercase();
        ["where this is", "handoff", "word for word", "verbatim", "status board", "refus"].iter().any(|w| text.contains(w))
    });
    checks.check(about_the_clash, "it is about the clash", "none of them is about the status line");

    checks.step("4 - and kept it out of the work");
    let handoff = std::fs::read_to_string(run.handoff_path()).unwrap_or_default().to_lowercase();
    checks.check(!handoff.contains("feedback"), "the handoff doesn't mention it", "the handoff mentions it");
    checks.check(
        std::fs::read_to_string(run.artifact("hello.md")).is_ok_and(|t| !t.trim().is_empty()),
        "artifacts/hello.md",
        "no greeting",
    );

    finish_run(checks, &run, "done", None, None);
}
