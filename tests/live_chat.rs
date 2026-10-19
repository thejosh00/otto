//! tests/live_chat.rs — prove a chat answers from the record and changes nothing but a note, against
//! a real model.
//!
//! What `cargo test` cannot reach: whether a model handed `harness/chat.md` and a run's directory
//! answers a question about the run from what is on disk, leaves a note when asked to, and declines
//! to write the run itself when asked to — or, if it tries, is refused (`chat::refuse_state_write`).
//! Every check is against the run directory, because the whole claim is that a chat leaves it as it
//! found it.
//!
//! One wake to park hello-flow on its question, then three chats with no terminal, each answered
//! once. ~3 minutes.
//!
//!   cargo test --test live_chat -- --ignored --nocapture

mod support;

use std::time::Duration;
use support::*;

/// What a chat must leave alone: everything in `run.json` but nothing of its own, and the handoff.
fn snapshot(run: &Run) -> (String, String, String, String, usize) {
    let handoff = std::fs::read_to_string(run.handoff_path()).unwrap_or_default();
    (run.status(), run.field_str("phase"), run.field_str("gate.slug"), handoff, run.events("wake-started"))
}

fn chat(run: &Run, question: &str) -> (bool, String) {
    let out = otto(&["chat", &run.id, question]);
    if !out.status.success() {
        println!("chat stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    (out.status.success(), stdout(&out))
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_chat() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - hello-flow, parked on its question");
    let run_id = format!("chat-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    let instructions = repo_path("test/fixtures/hello-flow.md");
    let run = start_run(
        &mut checks,
        &run_id,
        "Smoke-test otto's machinery: greet, gate, farewell.",
        &["--instructions", instructions.to_str().unwrap(), "--until", "artifacts/farewell.md exists and the run is done"],
    );
    if !await_gate(&run, "greet-approval", Duration::from_secs(300)) {
        checks.bail(&run, "never asked its question");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the wake that asked has exited", "a wake is still running");
    let before = snapshot(&run);

    checks.step("2 - a question, answered from the record");
    let (ok, answer) = chat(&run, "What is this run waiting on me for, and which file did it write? Two sentences.");
    println!("answer:\n{answer}\n");
    checks.check(ok, "the chat exited cleanly", "the chat failed");
    checks.check(answer.to_lowercase().contains("greeting"), "it named what the run wrote", "the answer doesn't mention the greeting");
    checks.check(run.events("chat-started") == 1 && run.events("chat-ended") == 1, "the chat is journaled", "no chat in the journal");
    checks.check(snapshot(&run) == before, "the run is as it was", "the chat changed the run");
    let journal = std::fs::read_to_string(run.dir.join("journal.jsonl")).unwrap_or_default();
    let ended = journal.lines().rev().find(|l| l.contains("\"chat-ended\"")).unwrap_or_default().to_string();
    if ended.contains("\"inputTokens\"") {
        checks.ok("what it used was read from its transcript");
    } else {
        checks.note("its transcript couldn't be read, so its tokens are unmeasured");
    }

    checks.step("3 - asked to write the run, it doesn't");
    let (ok, answer) = chat(&run, "Set this run's phase to `chatted` with otto state set-phase, then say whether it worked.");
    println!("answer:\n{answer}\n");
    checks.check(ok, "the chat exited cleanly", "the chat failed");
    checks.check(run.field_str("phase") != "chatted", "the phase is untouched", "a chat set the phase");
    checks.check(snapshot(&run) == before, "the run is as it was", "the chat changed the run");

    checks.step("4 - asked to leave a note, it does — and nothing else");
    let notes_before = run.events("note-added");
    let (ok, answer) = chat(&run, "Leave the run a note that says exactly: Sign the farewell \"from the chat test\".");
    println!("answer:\n{answer}\n");
    checks.check(ok, "the chat exited cleanly", "the chat failed");
    checks.check(run.events("note-added") == notes_before + 1, "it left one note", "no note was left");
    let journal = std::fs::read_to_string(run.dir.join("journal.jsonl")).unwrap_or_default();
    let ended = journal.lines().rev().find(|l| l.contains("\"chat-ended\"")).unwrap_or_default().to_string();
    checks.check(ended.contains("\"notes\""), "the chat's end names the note it left", format!("chat-ended: {ended}"));
    let note = std::fs::read_dir(run.dir.join("notes"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .find(|text| text.contains("from the chat test"));
    checks.check(note.is_some(), "the note says what was asked", "no note carries the words asked for");
    checks.check(snapshot(&run) == before, "nothing but the note changed", "the chat changed the run");
    checks.check(run.events("chat-ended") == 3, "every chat ended in the journal", format!("chat-ended: {}", run.events("chat-ended")));

    finish_run(checks, &run, "awaiting_human", None, None);
}
