//! tests/live_sandbox_refused.rs — a run refused for disagreeing with its sandbox record, and what a real model
//! makes of the wake after a person has put it right and answered. The refusal itself spends nothing and has its
//! own tests (`tests/sandbox.rs`). What only a model can show is the question it's told about afterwards: that it
//! carries on with its work, and doesn't ask again or take the answer as being about the work.
//!
//! No sandbox here: the record names the plain launcher, as what's compared is the same either way.
//! ~2 minutes, two wakes.
//!
//!   cargo test --test live_sandbox_refused -- --ignored --nocapture

mod support;

use std::time::Duration;
use support::*;

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_sandbox_refused() {
    let mut checks = Checks::new();
    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - a run whose work is one line per wake");
    let run_id = format!("refused-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    let run = start_run(
        &mut checks,
        &run_id,
        "Each wake, append one line to artifacts/wakes.md saying this wake happened, and write the handoff. That is all the work there is, each time. Then stop, leaving the run running so that the period brings the next wake.",
        &["--perpetual", "--period", "6h"],
    );
    if !await_field(&run, "wake.n", "1", Duration::from_secs(120)) || !await_idle(&run, Duration::from_secs(240)) {
        checks.bail(&run, "the first wake never finished");
    }
    let wakes = std::fs::read_to_string(run.artifact("wakes.md")).unwrap_or_default();
    checks.check(wakes.lines().count() == 1, "the first wake did its work", format!("artifacts/wakes.md: {wakes:?}"));

    checks.step("2 - it gets a record, then its run.json is edited past it");
    let run_json = run.dir.join("run.json");
    let read = || -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(&run_json).unwrap()).unwrap() };
    let original = read()["launcher"].clone();
    let id = run.dir.file_name().unwrap().to_str().unwrap().to_string();
    let records = otto_data_dir().join("sandbox/runs");
    std::fs::create_dir_all(&records).unwrap();
    let record = records.join(format!("{id}.json"));
    std::fs::write(&record, serde_json::json!({"launcher": original["kind"], "workdir": original["workdir"], "write": original["repos"].as_array().cloned().unwrap_or_default()}).to_string()).unwrap();
    let edit = |launcher: serde_json::Value| {
        let mut state = read();
        state["launcher"] = launcher;
        std::fs::write(&run_json, state.to_string()).unwrap();
    };
    let mut widened = original.clone();
    widened["repos"] = serde_json::json!(["/Users/nobody/.ssh"]);
    edit(widened);

    let refused = otto(&["wake", &id, "--detach", "none"]);
    let said = String::from_utf8_lossy(&refused.stderr).to_string();
    checks.check(refused.status.code() == Some(2) && said.contains(".ssh"), "otto wake refused, saying what differed", format!("exit {:?}: {said}", refused.status.code()));
    checks.check(run.events("sandbox-refused") == 1, "the journal says so", "no sandbox-refused line");
    checks.check(run.status() == "blocked", "the run is visibly stopped", format!("status {}", run.status()));
    checks.check(run.field("wake.n").and_then(|n| n.as_u64()) == Some(1), "no wake started", "a wake started");

    checks.step("3 - a person puts it right and answers");
    edit(original);
    let answered = otto(&["answer", &id, "--choice", "Fixed"]);
    checks.check(answered.status.success(), "answered", String::from_utf8_lossy(&answered.stderr));
    if !await_field(&run, "wake.n", "2", Duration::from_secs(60)) || !await_idle(&run, Duration::from_secs(240)) {
        checks.bail(&run, "the wake after the answer never finished");
    }

    checks.step("4 - it went back to its work, and asked nothing");
    let wakes = std::fs::read_to_string(run.artifact("wakes.md")).unwrap_or_default();
    checks.check(wakes.lines().count() == 2, "the second wake did its work", format!("artifacts/wakes.md: {wakes:?}"));
    checks.check(run.events("gate-opened") == 1, "it asked nothing of its own", format!("{} gates", run.events("gate-opened")));
    checks.check(run.field("gate").is_none_or(|g| g.is_null()), "no question is open", "a question is open");
    checks.check(!run.status().contains("blocked"), "it isn't blocked", format!("status {}", run.status()));
    checks.check(run.events("sandbox-refused") == 1, "and it wasn't refused again", "refused again");
    if let Some(transcript) = run.wake_transcript() {
        checks.note(format!("the second wake's transcript: {transcript}"));
    }
    run.journal_dump();
    checks.finish();
}
