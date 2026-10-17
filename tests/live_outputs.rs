//! tests/live_outputs.rs — prove a wake keeps its work where a person can find it, against a real
//! model.
//!
//! What `cargo test` cannot reach: whether a wake, handed instructions that only say "I'll want to
//! find this again" and "ask me whether the design is right", publishes each document as an output
//! (`harness/wake.md`, "Outputs") — the research, the design and the finished greeting — and
//! whether, asked to change the design, it publishes a second version of the *same* output rather
//! than a new one; whether its question is about that design, so the answer lands on v1 as its
//! review; and whether the message at the end points at the work.
//!
//! Three or four wakes, ~6-10 minutes.
//!
//!   cargo test --test live_outputs -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::{Duration, Instant};
use support::*;

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_OUTPUTS_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(480);
    Duration::from_secs(secs)
}

fn index(run: &Run) -> Value {
    std::fs::read_to_string(run.dir.join("outputs/index.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

fn outputs(run: &Run) -> Vec<Value> {
    index(run)["outputs"].as_array().cloned().unwrap_or_default()
}

/// The output the open question is about, from `gate.about` (`design@1`).
fn asked_about(run: &Run) -> Option<(String, u64)> {
    let about = run.field_str("gate.about");
    let (key, n) = about.split_once('@')?;
    Some((key.to_string(), n.parse().ok()?))
}

fn await_gates(run: &Run, n: usize, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if run.events("gate-opened") >= n && run.status() == "awaiting_human" {
            return true;
        }
        if run.is_over() {
            return false;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    false
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_outputs() {
    let timeout = timeout();
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);
    // The wakes call `otto state publish`, which only this build has.

    checks.step("1 - start design-flow, wrapped as an instructions file");
    let run_id = format!("outputs-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    let instructions = repo_path("test/fixtures/design-flow.md");
    let run = start_run(
        &mut checks,
        &run_id,
        "Research, design and write a one-line greeting, with my approval of the design",
        &["--instructions", instructions.to_str().unwrap(), "--until", "the greeting is written, I have been told, and the run is done"],
    );

    checks.step(&format!("2 - it researches, designs, and asks about the design (up to {}s)", timeout.as_secs()));
    if !await_gates(&run, 1, timeout) {
        checks.bail(&run, "never asked about the design");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the wake that asked has exited", "a wake is still running at the question");
    let made = outputs(&run);
    checks.check(made.len() >= 2, format!("{} outputs published: research and design", made.len()), format!("only {} output(s) published", made.len()));
    let stages: Vec<String> =
        index(&run)["stages"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if stages.len() >= 3 {
        checks.ok(format!("it declared its steps: {}", stages.join(" → ")));
    } else {
        checks.note(format!("it declared no stages for three named steps ({stages:?})"));
    }
    let Some((design_key, v)) = asked_about(&run) else {
        checks.bail(&run, "the question is not about an output — `--about` was not used");
    };
    checks.ok(format!("the question is about {design_key} v{v}"));
    let design = made.iter().find(|o| o["key"] == design_key.as_str()).cloned().unwrap_or(Value::Null);
    let file = design["versions"][0]["file"].as_str().unwrap_or("");
    checks.check(
        std::fs::read_to_string(run.dir.join(file)).is_ok_and(|t| t.trim().len() > 40),
        format!("{file} holds the design"),
        "the design's text is not on disk",
    );

    checks.step("3 - ask for a change");
    let change = "Needs changes: make the greeting say good morning, and keep it under ten words.";
    if !answer_gate(&mut checks, &run, "", change, timeout) {
        checks.bail(&run, "could not answer the design question");
    }

    checks.step(&format!("4 - it revises the same design and asks again (up to {}s)", timeout.as_secs()));
    if !await_gates(&run, 2, timeout) {
        checks.bail(&run, "never asked about the revised design");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the wake that asked again has exited", "a wake is still running");
    let design = outputs(&run).into_iter().find(|o| o["key"] == design_key.as_str()).unwrap_or(Value::Null);
    let versions = design["versions"].as_array().cloned().unwrap_or_default();
    checks.check(
        versions.len() >= 2,
        format!("{design_key} now has {} versions", versions.len()),
        "the revision is not a new version of the design — a new key, or not published",
    );
    let review = versions.first().and_then(|v| v["reviews"][0]["answer"].as_str().map(str::to_string)).unwrap_or_default();
    checks.check(review == change, "v1 carries the answer as its review, verbatim", format!("v1's review is {review:?}"));
    let v2 = versions.get(1).and_then(|v| v["file"].as_str()).and_then(|f| std::fs::read_to_string(run.dir.join(f)).ok()).unwrap_or_default();
    checks.check(v2.to_lowercase().contains("good morning"), "v2 says good morning, as asked", "v2 does not carry the change asked for");
    if let Some((key, n)) = asked_about(&run) {
        checks.check(key == design_key && n >= 2, format!("the new question is about {key} v{n}"), format!("the new question is about {key} v{n}"));
    } else {
        checks.bad("the new question is not about the design");
    }

    checks.step("5 - approve, and it finishes");
    if !answer_gate(&mut checks, &run, "", "Approve", timeout) {
        checks.bail(&run, "could not approve the design");
    }
    if !await_field(&run, "status", "done", timeout) {
        checks.bail(&run, "never finished");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the last wake has exited", "a wake is still running");
    let made = outputs(&run);
    checks.check(made.len() >= 3, format!("{} outputs in all: research, design, greeting", made.len()), "the greeting was not published");
    let sent: Vec<Value> = std::fs::read_dir(run.dir.join("messages"))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .filter_map(|p| std::fs::read_to_string(p).ok())
                .filter_map(|t| serde_json::from_str(&t).ok())
                .collect()
        })
        .unwrap_or_default();
    checks.check(!sent.is_empty(), format!("{} message(s) sent", sent.len()), "it never told the person");
    checks.check(
        sent.iter().any(|m| m["output"].is_string()),
        "a message points at its output",
        "no message points at an output — the inbox can't open the work",
    );

    checks.step("6 - a person can find it all again");
    let listed = stdout(&otto(&["outputs", &run_id]));
    checks.check(listed.contains(&design_key), "`otto outputs` lists the design", format!("`otto outputs` says: {listed}"));
    let v1 = stdout(&otto(&["outputs", &run_id, &design_key, "--version", "1"]));
    checks.check(v1.contains("your review") && v1.contains("good morning"), "v1 prints with your review", format!("v1 prints as: {v1}"));
    checks.check(run.events("output-published") >= 4, format!("{} versions published in all", run.events("output-published")), "fewer versions than outputs");

    finish_run(checks, &run, "done", None, None);
}
