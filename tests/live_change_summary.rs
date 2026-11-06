//! tests/live_change_summary.rs — prove a wake's correction to a change it handed back reaches the
//! person, against a real model.
//!
//! What `cargo test` cannot reach: what a wake does with otto's reply when it submits the same
//! commit with a corrected summary. otto keeps the summary as it was, because the person may have
//! read it. A wake once got a reply that only spoke of evidence, left the mistake standing and
//! said so where the person might not look. So: does the reply tell it the summary stayed, and does
//! the right sentence then reach the person?
//!
//! One wake, ~2 minutes. An `OTTO_HOME` of its own under the scratch repo.
//!
//!   cargo test --test live_change_summary -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::Duration;
use support::*;

/// Everything the person can read from this run: what each version of its change says, and every
/// message it sent them.
fn what_the_person_can_read(run: &Run) -> (String, String) {
    let index: Value = std::fs::read_to_string(run.dir.join("changes/index.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let latest = index["changes"][0]["versions"].as_array().and_then(|v| v.last().cloned()).unwrap_or(Value::Null);
    let summary = latest["summary"].as_str().and_then(|p| std::fs::read_to_string(run.dir.join(p)).ok()).unwrap_or_default();
    let messages = std::fs::read_dir(run.dir.join("messages"))
        .map(|d| d.filter_map(|e| e.ok()).filter_map(|e| std::fs::read_to_string(e.path()).ok()).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    (summary, messages)
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_change_summary() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions, and a home of its own");
    require_launcher(&mut checks);
    let mut scratch = ScratchRepo::new(&workspace_dir(), "summary");
    let home = scratch.path().join("otto-home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("OTTO_HOME", &home);

    checks.step("1 - scratch repo with a local remote");
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
    write_file(&repo.join("Makefile"), "test:\n\tfind . -name __pycache__ -prune -exec rm -rf {} +\n\tpython3 -m unittest discover -v\n\n.PHONY: test\n");
    write_file(&repo.join(".gitignore"), "__pycache__/\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "greeter: hello"]);
    git_ok(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git_ok(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git_ok(&repo, &["remote", "set-head", "origin", "main"]);
    let workdir = scratch.path().to_str().unwrap().to_string();

    checks.step("2 - it hands the change back with a wrong sentence, then corrects it (up to 15min)");
    let fixture = repo_path("test/fixtures/correct-flow.md");
    let goal = format!("In {}: add a farewell(name) function to greet.py returning \"Goodbye, <name>!\", with a unit test beside the existing one", repo.display());
    let run = start_run(
        &mut checks,
        "corrects",
        &goal,
        &[
            "--instructions", fixture.to_str().unwrap(),
            "--repo", repo.to_str().unwrap(), "--workdir", &workdir,
            "--until", "the reviewer can see the right file name, and artifacts/corrected.md says how",
            "--policy", "verifyDone=false",
        ],
    );
    if !await_field(&run, "status", "done", Duration::from_secs(900)) {
        checks.bail(&run, "never finished");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the wake has exited", "a wake is still running");

    checks.step("3 - what it was told, and what it made of it");
    let corrected = std::fs::read_to_string(run.artifact("corrected.md")).unwrap_or_default();
    println!("corrected.md:\n{corrected}");
    checks.check(!corrected.trim().is_empty(), "it wrote down what it did", "no artifacts/corrected.md");

    checks.step("4 - the right file name reached the person");
    let (summary, messages) = what_the_person_can_read(&run);
    println!("the latest version's summary:\n{summary}\nits messages:\n{messages}");
    let fixed_in_summary = summary.contains("greet.py") && !summary.contains("in hello.py");
    checks.check(fixed_in_summary || messages.contains("greet.py"), "the person can read the right file name", "the person is still told only hello.py");
    checks.check(locks_held() == 0, "no repo lock is held", "a repo lock is still held");

    checks.step("5 - the reply itself, in otto's own words rather than the model's");
    let index: Value = std::fs::read_to_string(run.dir.join("changes/index.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let branch = index["changes"][0]["branch"].as_str().unwrap_or_default().to_string();
    let title = index["changes"][0]["title"].as_str().unwrap_or_default().to_string();
    let another = scratch.path().join("another-summary.md");
    write_file(&another, "A summary no version has said yet.\n");
    let again = otto(&["state", "change", "submit", "corrects", "--repo", repo.to_str().unwrap(), "--branch", &branch, "--title", &title, "--summary-file", another.to_str().unwrap()]);
    let reply = stdout(&again);
    println!("submitting the same commit with another summary: {reply}");
    checks.check(reply.contains("summary kept as it was") && reply.contains("as a message"), "the reply says the summary stayed, and how to reach the person", "the reply doesn't say the summary stayed and how to reach the person");

    if checks.fail_count() > 0 {
        scratch.keep();
    }
    finish_run(checks, &run, "done", None, None);
}
