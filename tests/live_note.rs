//! tests/live_note.rs — prove a note left while a wake works reaches it before it asks, against a
//! real model.
//!
//! What `cargo test` cannot reach: a note added mid-wake was never in that wake's prompt, so otto
//! refuses the question it goes on to ask, with the note in the refusal (`notes::refuse_if_late`).
//! Whether the wake then reads the refusal as the person speaking — and so writes the colour it was
//! told instead of asking anyway, or retrying the same question — is prose (`harness/wake.md`,
//! "Notes"), and only a live model can show it.
//!
//! Two wakes, the work and the check of its done, ~2 minutes.
//!
//!   cargo test --test live_note -- --ignored --nocapture

mod support;

use std::time::{Duration, Instant};
use support::*;

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_NOTE_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(420);
    Duration::from_secs(secs)
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_note() {
    let timeout = timeout();
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - start colour-flow, which asks the person for a colour");
    let run_id = format!("note-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    let instructions = repo_path("test/fixtures/colour-flow.md");
    let run = start_run(
        &mut checks,
        &run_id,
        "Write the colour the person picks",
        &[
            "--instructions",
            instructions.to_str().unwrap(),
            "--until",
            "artifacts/colour.md names the colour the person picked, and the run is done",
        ],
    );

    // Once the palette is written, the wake's next move is the question. A note left any earlier
    // is one it can find on its own, reading the run at the start, and it never asks at all.
    checks.step("2 - with the palette written, before it asks, say which");
    let deadline = Instant::now() + Duration::from_secs(180);
    while !run.artifact("palette.md").is_file() && !run.is_over() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if !run.artifact("palette.md").is_file() {
        checks.bail(&run, "no palette within 3min");
    }
    if run.events("gate-opened") > 0 {
        checks.bail(&run, "it asked before the note could be left — the test lost its race, not the run");
    }
    let noted = otto(&["note", &run_id, "Make it blue."]);
    checks.check(noted.status.success(), "left a note: \"Make it blue.\"", String::from_utf8_lossy(&noted.stderr));

    checks.step(&format!("3 - it finishes without asking (up to {}s)", timeout.as_secs()));
    let deadline = Instant::now() + timeout;
    while !run.is_over() && run.events("gate-opened") == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(2));
    }
    let late = run.events("notes-given-late");
    checks.check(late == 1, "otto refused it once, with the note", format!("refusals with the note: {late}"));
    // `gate` is the path this test is for; `done` means it found the note in the run before
    // asking, so what was refused is the done — right too, but say so.
    let journal = std::fs::read_to_string(run.dir.join("journal.jsonl")).unwrap_or_default();
    let before: Vec<String> = journal
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v["event"] == "notes-given-late")
        .map(|v| v["before"].as_str().unwrap_or("").to_string())
        .collect();
    match before.first().map(String::as_str) {
        Some("gate") => checks.ok("what was refused was its question"),
        Some(other) => checks.note(format!("what was refused was its {other}: it found the note before asking")),
        None => {}
    }
    let asked = run.events("gate-opened");
    checks.check(asked == 0, "it never asked what it had been told", "it asked anyway");
    if asked > 0 {
        checks.bail(&run, "a gate is open");
    }
    checks.check(run.status() == "done", "status=done", format!("status={}", run.status()));
    checks.check(await_idle(&run, Duration::from_secs(120)), "the last wake has exited", "a wake is still running");

    checks.step("4 - blue, delivered, and checked");
    let colour = std::fs::read_to_string(run.artifact("colour.md")).unwrap_or_default();
    checks.check(
        colour.to_lowercase().contains("blue"),
        "artifacts/colour.md says blue",
        format!("artifacts/colour.md says: {}", colour.trim()),
    );
    checks.check(run.events("notes-delivered") == 1, "the note was delivered", "the note was never delivered");
    checks.check(
        run.events("done-claimed") >= 1 && run.events("done-verified") == 1,
        "its done was checked by a fresh wake",
        "its done was never checked",
    );

    finish_run(checks, &run, "done", None, None);
}
