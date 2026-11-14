//! tests/live_summary_added.rs — prove a wake that gives a summary to a document it already published
//! knows from otto's reply that the summary is on it, against a real model.
//!
//! What `cargo test` cannot reach: what a wake makes of the reply. otto once answered the same text
//! published again with a summary as "unchanged, and its summary kept with it", which reads as though
//! the new summary was ignored — so a wake spent a step publishing again, or going to look. So: is the
//! summary on version 1 with no version 2, was it told so in words that say so, and did it carry on?
//!
//! One wake, ~3 minutes.
//!
//!   cargo test --test live_summary_added -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::Duration;
use support::*;

fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Each command the wake ran, from its transcript, in order. A transcript can carry the same call on
/// more than one line, so each is counted once, by its id.
fn commands(transcript: &str) -> Vec<String> {
    let mut ran = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in transcript.lines().filter(|l| l.contains("\"tool_use\"")) {
        let line: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        let blocks = line["message"]["content"].as_array().cloned().unwrap_or_default();
        for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
            if let (Some(id), Some(command)) = (b["id"].as_str(), b["input"]["command"].as_str()) {
                if seen.insert(id.to_string()) {
                    ran.push(command.to_string());
                }
            }
        }
    }
    ran
}

/// How many times a command publishes. What it writes into a file with a here-document is text, not
/// a command: a wake that writes its summary and publishes it in one go publishes once, and one that
/// writes down the command it ran earlier doesn't publish at all.
fn publishes(command: &str) -> usize {
    let mut count = 0;
    let mut until: Option<String> = None;
    for line in command.lines() {
        if let Some(end) = &until {
            if line.trim() == end {
                until = None;
            }
            continue;
        }
        let (run, rest) = line.split_once("<<").unwrap_or((line, ""));
        count += run.matches("state publish").count();
        let word = rest.trim_start_matches('-').trim_start().trim_start_matches(['\'', '"']);
        let word: String = word.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if !word.is_empty() {
            until = Some(word);
        }
    }
    count
}

/// The two shapes real wakes gave it, which an earlier count got wrong: no model needed.
#[test]
fn a_here_document_is_not_a_command() {
    let wrote_then_published = "cat > artifacts/s.md <<'EOF'\nShort.\nEOF\notto state publish r --key guide --file artifacts/guide.md --summary-file artifacts/s.md 2>&1; echo \"exit=$?\"";
    assert_eq!(publishes(wrote_then_published), 1);
    let quoted_it = "cat > artifacts/published.md <<'EOF'\nCommand: `otto state publish r --key guide`\nEOF\notto state handoff r --stdin <<'EOF'\notto state publish\nEOF\notto state log r --event cycle";
    assert_eq!(publishes(quoted_it), 0);
    assert_eq!(publishes("wc -w a.md && otto state publish r --key guide --file a.md 2>&1; echo done"), 1);
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_summary_added() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - it publishes a long guide, then is asked for a summary on that same guide (up to 10min)");
    let run_id = format!("summary-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    let fixture = repo_path("test/fixtures/summary-flow.md");
    let run = start_run(
        &mut checks,
        &run_id,
        "Write a guide to keeping a paper notebook of the day's work, publish it, then give it a summary",
        &[
            "--instructions", fixture.to_str().unwrap(),
            "--until", "the guide is published with its summary, and artifacts/published.md says what otto said",
            "--policy", "verifyDone=false",
        ],
    );
    if !await_field(&run, "status", "done", Duration::from_secs(600)) {
        checks.bail(&run, "never finished");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the wake has exited", "a wake is still running");
    let told = std::fs::read_to_string(run.artifact("published.md")).unwrap_or_default();
    println!("published.md:\n{told}");

    checks.step("2 - the summary is on version 1, and there is no version 2");
    let index: Value = std::fs::read_to_string(run.dir.join("outputs/index.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let outputs = index["outputs"].as_array().cloned().unwrap_or_default();
    checks.check(outputs.len() == 1, "one output", format!("{} outputs, not one", outputs.len()));
    let guide = outputs.iter().find(|o| o["key"] == "guide").cloned().unwrap_or(Value::Null);
    let versions = guide["versions"].as_array().cloned().unwrap_or_default();
    checks.check(versions.len() == 1, "the guide has one version", format!("the guide has {} versions", versions.len()));
    let v1 = versions.first().cloned().unwrap_or(Value::Null);
    let text = v1["file"].as_str().and_then(|p| std::fs::read_to_string(run.dir.join(p)).ok()).unwrap_or_default();
    let summary = v1["summary"].as_str().and_then(|p| std::fs::read_to_string(run.dir.join(p)).ok()).unwrap_or_default();
    println!("the guide: {} words; its summary, {} words:\n{summary}", words(&text), words(&summary));
    checks.check(words(&text) > 500, "the guide is over a page", "the guide isn't over a page, so nothing asked for a summary");
    checks.check(!summary.trim().is_empty() && words(&summary) <= 500, "v1 carries a summary of at most a page", "v1 has no summary");
    checks.check(run.events("output-published") == 1, "one version was ever published", format!("{} versions were published", run.events("output-published")));

    checks.step("3 - otto said so, and the wake carried on");
    let reply = "summary added — the text is the same, so no new version";
    match run.wake_transcript() {
        Some(said) => {
            let ran = commands(&said);
            let published: Vec<usize> = ran.iter().enumerate().flat_map(|(i, c)| std::iter::repeat_n(i, publishes(c))).collect();
            for i in ran.iter().enumerate().filter(|(_, c)| publishes(c) > 0).map(|(i, _)| i) {
                println!("it ran: {}", ran[i]);
            }
            checks.check(said.contains(reply), "it was told its summary was added", "its transcript doesn't show the reply");
            checks.check(published.len() == 2, "it published twice: the guide, then the guide with its summary", format!("it published {} times, not twice", published.len()));
            // Anything after the last publish that reads the guide's output back is a step the reply should have saved.
            let after = published.last().map_or(&ran[..], |i| &ran[i + 1..]);
            let looked: Vec<&String> = after.iter().filter(|c| c.contains("otto outputs") || c.contains("outputs/guide") || c.contains("outputs/index.json")).collect();
            checks.check(looked.is_empty(), "it didn't go and look whether the summary was on", format!("it went to look afterwards: {looked:?}"));
        }
        None => {
            checks.note("its transcript couldn't be read, so how many times it published is unknown");
            checks.check(told.contains(reply), "published.md quotes the reply", "published.md doesn't quote the reply");
        }
    }
    checks.check(told.contains("summary added"), "what it wrote down says the summary was added", "what it wrote down doesn't say the summary was added");
    let shown = stdout(&otto(&["outputs", &run_id, "guide"]));
    checks.check(shown.contains(summary.lines().find(|l| !l.trim().is_empty()).unwrap_or("\u{0}")), "`otto outputs` opens on the summary", format!("`otto outputs` shows: {}", shown.chars().take(400).collect::<String>()));

    finish_run(checks, &run, "done", None, None);
}
