//! tests/live_message.rs — prove a wake delivers a result to the person, against a real model.
//!
//! What `cargo test` cannot reach: whether a wake, handed instructions that only say "let me know
//! what you found", turns that into a message in the person's inbox (`harness/wake.md`, "Messages")
//! rather than saying it into a terminal nobody is watching — and whether the handoff it writes
//! gets past the plain-language check on the sections a person reads, first time or after the
//! refusal tells it why.
//!
//! One wake, ~2 minutes.
//!
//!   cargo test --test live_message -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::Duration;
use support::*;

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_MESSAGE_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(420);
    Duration::from_secs(secs)
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_message() {
    let timeout = timeout();
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);
    // The wake calls `otto state message`, which only this build has.
    use_this_build_first_on_path();

    checks.step("1 - start report-flow, wrapped as an instructions file");
    let run_id = format!("message-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    let instructions = repo_path("test/fixtures/report-flow.md");
    let run = start_run(
        &mut checks,
        &run_id,
        "Find out what this run's history says so far, and let me know",
        &[
            "--instructions",
            instructions.to_str().unwrap(),
            "--until",
            "the person has been told what was found, and the run is done",
        ],
    );

    checks.step(&format!("2 - wait for it to finish (up to {}s)", timeout.as_secs()));
    if !await_field(&run, "status", "done", timeout) {
        checks.bail(&run, "never finished");
    }
    checks.check(await_idle(Duration::from_secs(120)), "the wake has exited", "a wake is still running");

    checks.step("3 - it told the person, through the inbox");
    let sent = run.events("message-sent");
    checks.check(sent >= 1, format!("{sent} message(s) sent"), "no message was sent — the result went nowhere a person would see");
    let messages_dir = run.dir.join("messages");
    let metas: Vec<Value> = std::fs::read_dir(&messages_dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .filter_map(|p| std::fs::read_to_string(p).ok())
                .filter_map(|t| serde_json::from_str(&t).ok())
                .collect()
        })
        .unwrap_or_default();
    if let Some(meta) = metas.first() {
        let kind = meta["kind"].as_str().unwrap_or("");
        checks.check(matches!(kind, "report" | "update"), format!("kind {kind}"), format!("an odd kind for a finding: {kind}"));
        let title = meta["title"].as_str().unwrap_or("");
        checks.check(!title.is_empty() && title.len() <= 120, format!("titled \"{title}\""), "no usable title");
        let body = meta["body"].as_str().map(|b| std::fs::read_to_string(run.dir.join(b)).unwrap_or_default()).unwrap_or_default();
        checks.check(body.trim().len() > 40, format!("a body of {} bytes", body.len()), "the body says next to nothing");
        if body.chars().any(|c| c.is_ascii_digit()) {
            checks.ok("the body carries the count it was asked for");
        } else {
            checks.note("the body has no number in it — did it count the journal?");
        }
    } else {
        checks.bad("no message file under messages/");
    }

    let inbox: Value = serde_json::from_str(&stdout(&otto(&["inbox", "--json"]))).unwrap_or(Value::Null);
    let ours: Vec<&Value> = inbox["items"].as_array().map(|items| items.iter().filter(|i| i["run"] == run_id.as_str()).collect()).unwrap_or_default();
    checks.check(
        ours.iter().any(|i| i["open"] == true && i["kind"] != "question"),
        "it is waiting in `otto inbox`",
        "the inbox does not show it",
    );

    checks.step("4 - the line a person reads is plain");
    let handoff = std::fs::read_to_string(run.handoff_path()).unwrap_or_default();
    let refused = run.events("handoff-refused");
    checks.note(format!("handoffs refused for otto's words before one was accepted: {refused}"));
    let said: String = handoff
        .lines()
        .skip_while(|l| !l.starts_with("## Where this is"))
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .collect::<Vec<_>>()
        .join(" ");
    checks.note(format!("where this is: {}", said.trim()));
    checks.check(!said.trim().is_empty(), "it says where the run stands", "Where this is is empty");
    let words: Vec<String> = said.to_lowercase().split(|c: char| !c.is_alphanumeric()).map(str::to_string).collect();
    for word in ["wake", "wakes", "tick", "ticked", "gate", "handoff", "poke"] {
        checks.check(!words.iter().any(|w| w == word), format!("no \"{word}\""), format!("it says \"{word}\": {said}"));
    }

    checks.step("5 - marking it done empties the inbox");
    let open: Vec<String> = run
        .field("messages")
        .and_then(|m| m.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    for id in &open {
        let done = otto(&["inbox", "done", &run_id, id]);
        checks.check(done.status.success(), format!("otto inbox done {id}"), format!("otto inbox done failed: {}", String::from_utf8_lossy(&done.stderr)));
    }
    checks.check(run.field("messages").is_none(), "nothing of it left open", "run.json still lists an open message");

    finish_run(checks, &run, "done", None, None);
}
