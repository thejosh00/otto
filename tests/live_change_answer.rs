//! tests/live_change_answer.rs — prove an author whose question about a change is answered "leave it"
//! gets that change back into the person's review without inventing a commit, against a real model.
//!
//! What `cargo test` cannot reach: what a wake does with the answer. An answer leaves the change with
//! its author, and the person can approve only a change in review. The same commit submitted again
//! used to change nothing, so the one way back was a new commit nobody had asked for — a comment, a
//! blank line — and a second version to read. So: told the answer changes nothing, does the wake
//! submit the same commit again (`harness/wake.md`, "Changes to code", and the note the answer
//! leaves), and is the change then in review, still at version 1, its branch where it was?
//!
//! Everything runs in an `OTTO_HOME` of its own under the scratch repo. Two wakes, ~8 minutes.
//!
//!   cargo test --test live_change_answer -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::{Duration, Instant};
use support::*;

fn env_secs(key: &str, default: u64) -> Duration {
    Duration::from_secs(std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default))
}

/// The change, as the author run keeps it.
fn change(run: &Run) -> Option<Value> {
    let text = std::fs::read_to_string(run.dir.join("changes/index.json")).ok()?;
    let index: Value = serde_json::from_str(&text).ok()?;
    index["changes"].as_array()?.first().cloned()
}

/// Poke (this home only) until `done` says so, or the time is up.
fn poke_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if done() {
            return true;
        }
        let _ = otto(&["poke"]);
        std::thread::sleep(Duration::from_secs(20));
    }
    done()
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_change_answer() {
    let ask_timeout = env_secs("OTTO_LIVE_CHANGE_ANSWER_ASK_TIMEOUT", 1200);
    let back_timeout = env_secs("OTTO_LIVE_CHANGE_ANSWER_BACK_TIMEOUT", 900);
    let mut checks = Checks::new();

    checks.step("0 - preconditions, and a home of its own");
    require_launcher(&mut checks);
    let scratch = ScratchRepo::new(&workspace_dir(), "change-answer");
    let home = scratch.path().join("otto-home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("OTTO_HOME", &home);
    checks.ok(format!("OTTO_HOME={}", home.display()));

    checks.step("1 - scratch repo with a real (local) remote");
    let origin = scratch.origin_path();
    let repo = scratch.repo_path("greeter");
    git_ok(scratch.path(), &["init", "--quiet", "--bare", "--initial-branch=main", origin.to_str().unwrap()]);
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "--quiet", "--initial-branch=main"]);
    write_file(&repo.join("greet.py"), "\"\"\"Greetings.\"\"\"\n\n\ndef greet(name: str) -> str:\n    \"\"\"Say hello to `name`.\"\"\"\n    return f\"Hello, {name}!\"\n");
    write_file(
        &repo.join("test_greet.py"),
        "import unittest\n\nfrom greet import greet\n\n\nclass TestGreet(unittest.TestCase):\n    def test_greet_names_the_person(self) -> None:\n        self.assertEqual(greet(\"Ada\"), \"Hello, Ada!\")\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n",
    );
    write_file(&repo.join(".gitignore"), "__pycache__/\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "greeter: hello"]);
    git_ok(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git_ok(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git_ok(&repo, &["remote", "set-head", "origin", "main"]);
    otto(&["config", "project", repo.to_str().unwrap(), "--process", "direct"]);
    let workdir = scratch.path().to_str().unwrap().to_string();

    checks.step(&format!("2 - an author hands its change back, and asks about it (up to {}min)", ask_timeout.as_secs() / 60));
    // Goal only: what it knows about changes and asking is the harness, nothing else.
    let goal = format!(
        "In {}: add a farewell(name) function to greet.py returning \"Goodbye, <name>!\", mirroring greet() including its \
         docstring style, with a unit test beside the existing one, and hand it back as a change for me to review. Whether \
         farewell should also take the parting word as a second argument is mine to decide: once the change is submitted, ask \
         me about the change itself (not at a gate), with the options \"Add it\" and \"Leave it\", default \"Leave it\". Then do \
         what I answer, and see that the change is waiting for my review again afterwards",
        repo.display()
    );
    let author = start_run(
        &mut checks,
        "author",
        &goal,
        &["--repo", repo.to_str().unwrap(), "--workdir", &workdir, "--until", "the farewell change has landed on main", "--policy", "verifyDone=false"],
    );
    let start = Instant::now();
    while !change(&author).is_some_and(|c| c["state"] == "needs-you") && start.elapsed() < ask_timeout && !author.is_over() {
        std::thread::sleep(Duration::from_secs(10));
    }
    let c = change(&author).unwrap_or(Value::Null);
    if c["state"] != "needs-you" {
        println!("the change: {}", serde_json::to_string_pretty(&c).unwrap_or_default());
        checks.bail(&author, "it never asked about its change");
    }
    checks.check(await_idle(&author, Duration::from_secs(300)), "its wake has exited", "its wake is still running");
    let c = change(&author).unwrap_or(Value::Null);
    checks.check(c["state"] == "needs-you", "the change waits on an answer", format!("the change is {}", c["state"]));
    checks.check(c["ask"]["askedIn"] == "review", "it asked while the change was in review", format!("asked in {}", c["ask"]["askedIn"]));
    checks.check(author.gate_files_matching(".md").is_empty(), "it opened no gate", "it opened a gate as well");
    let branch = c["branch"].as_str().unwrap_or("").to_string();
    let v1 = c["versions"][0]["sha"].as_str().unwrap_or("").to_string();
    let head_before = stdout(&git(&repo, &["rev-parse", &branch]));
    checks.check(!v1.is_empty() && head_before == v1, format!("its branch {branch} is at v1"), format!("its branch is at {head_before}, v1 is {v1}"));

    checks.step(&format!("3 - answered \"Leave it\": the same commit comes back to review (up to {}min)", back_timeout.as_secs() / 60));
    let out = otto(&["changes", "answer", "author/1", "--text", "Leave it"]);
    checks.check(out.status.success(), "answered", format!("answer failed: {}", String::from_utf8_lossy(&out.stderr)));
    let c = change(&author).unwrap_or(Value::Null);
    checks.check(c["state"] == "revising", "the answer leaves it with its author", format!("after the answer it is {}", c["state"]));
    // That it can't be approved from there is the unit test's to show: this home has no lander, and
    // approving is refused for that before anything else.
    let listed = stdout(&otto(&["changes", "--json"]));
    checks.check(listed.contains("\"yours\": 0"), "and nothing waits on you", format!("otto changes: {}", listed.chars().take(300).collect::<String>()));
    let back = poke_until(back_timeout, || change(&author).is_some_and(|c| c["state"] == "review") && !author.wake_running());
    let c = change(&author).unwrap_or(Value::Null);
    if !back {
        println!("the change: {}", serde_json::to_string_pretty(&c).unwrap_or_default());
        checks.bail(&author, "the change never came back to review");
    }
    checks.ok("the change is in review again");
    let versions = c["versions"].as_array().map_or(0, |v| v.len());
    checks.check(versions == 1, "still version 1: nothing new for the person to read", format!("it has {versions} versions: a commit was invented to get it back"));
    let head_after = stdout(&git(&repo, &["rev-parse", &branch]));
    checks.check(head_after == head_before, "its branch didn't move", format!("its branch moved from {head_before} to {head_after}"));
    checks.check(author.events("change-submitted") == 1, "one version was ever submitted", format!("{} were submitted", author.events("change-submitted")));
    let timeline = serde_json::to_string(&c["timeline"]).unwrap_or_default();
    checks.check(timeline.contains("stands as it is after your answer"), "the change says its author let it stand", format!("its timeline: {timeline}"));
    checks.check(c["decision"].is_null(), "back in review is not approved", "it carries a decision");
    match author.wake_transcript() {
        Some(said) => {
            for line in said.lines().filter(|l| l.contains("\"tool_use\"") && l.contains("state change submit")) {
                let line: Value = serde_json::from_str(line).unwrap_or(Value::Null);
                let blocks = line["message"]["content"].as_array().cloned().unwrap_or_default();
                for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                    println!("it ran: {}", b["input"]["command"].as_str().unwrap_or_default());
                }
            }
            checks.check(said.contains("back in the person's review"), "it was told the change is back in review", "its transcript doesn't show otto saying so");
        }
        None => checks.note("its transcript couldn't be read, so what it was told is unknown"),
    }
    let listed = stdout(&otto(&["changes", "--json"]));
    checks.check(listed.contains("\"yours\": 1"), "otto changes says one waits on you", "otto changes doesn't count it");
    checks.check(author.gate_files_matching(".md").is_empty(), "and it asked nothing at a gate", "it opened a gate");

    let status = author.status();
    finish_run(checks, &author, &status, None, None);
}
