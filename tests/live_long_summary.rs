//! tests/live_long_summary.rs — prove a wake with over a page to say gets the reviewer something they can
//! read in a minute before there is a version to approve, against a real model.
//!
//! What `cargo test` cannot reach: what a wake makes of otto refusing it. otto once took a long summary
//! alone and only then said to submit the same commit again with a short one. A person approved in between,
//! so the second submit was refused as already landing and the version stayed with no short summary. So:
//! is there ever a version without its summary, and does the refusal tell a wake what to do?
//!
//! One wake, ~3 minutes. An `OTTO_HOME` of its own under the scratch repo.
//!
//!   cargo test --test live_long_summary -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::Duration;
use support::*;

fn index(run: &Run) -> Value {
    std::fs::read_to_string(run.dir.join("changes/index.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_long_summary() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions, and a home of its own");
    require_launcher(&mut checks);
    let mut scratch = ScratchRepo::new(&workspace_dir(), "long-summary");
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

    checks.step("2 - it hands the change back with far too much to say (up to 15min)");
    let fixture = repo_path("test/fixtures/long-summary-flow.md");
    let goal = format!("In {}: add a farewell(name) function to greet.py returning \"Goodbye, <name>!\", with a unit test beside the existing one", repo.display());
    let run = start_run(
        &mut checks,
        "long",
        &goal,
        &[
            "--instructions", fixture.to_str().unwrap(),
            "--repo", repo.to_str().unwrap(), "--workdir", &workdir,
            "--until", "the change is handed back, and artifacts/submitted.md says what otto said",
            "--policy", "verifyDone=false",
        ],
    );
    if !await_field(&run, "status", "done", Duration::from_secs(900)) {
        checks.bail(&run, "never finished");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the wake has exited", "a wake is still running");
    println!("submitted.md:\n{}", std::fs::read_to_string(run.artifact("submitted.md")).unwrap_or_default());

    checks.step("3 - there was never a version without its summary");
    let i = index(&run);
    let change = &i["changes"][0];
    let versions = change["versions"].as_array().cloned().unwrap_or_default();
    checks.check(versions.len() == 1, "one version", "not exactly one version");
    let timeline: Vec<String> = change["timeline"].as_array().map(|t| t.iter().filter_map(|m| m["what"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    println!("its timeline:\n{}", timeline.join("\n"));
    checks.check(
        !timeline.iter().any(|w| w.contains("added the whole")),
        "the whole came with the first version, so nothing waited to be approved without it",
        "the version existed first and the whole was added to it afterwards",
    );

    checks.step("4 - what the reviewer opens on");
    let latest = versions.last().cloned().unwrap_or(Value::Null);
    let summary = latest["summary"].as_str().and_then(|p| std::fs::read_to_string(run.dir.join(p)).ok()).unwrap_or_default();
    let whole = latest["file"].as_str().and_then(|p| std::fs::read_to_string(run.dir.join(p)).ok()).unwrap_or_default();
    println!("its summary, {} words:\n{summary}\nthe whole: {} words", words(&summary), words(&whole));
    checks.check(words(&whole) > 500, "the whole of it is kept", "the notes aren't kept as the whole");
    checks.check(!summary.trim().is_empty() && words(&summary) <= 500, "the summary is at most a page", "the summary isn't a page or less");
    checks.check(locks_held() == 0, "no repo lock is held", "a repo lock is still held");

    checks.step("5 - the refusal itself, in otto's own words rather than the model's");
    let branch = change["branch"].as_str().unwrap_or_default().to_string();
    let title = change["title"].as_str().unwrap_or_default().to_string();
    let worktree = change["worktree"].as_str().map(std::path::PathBuf::from).unwrap_or_else(|| repo.clone());
    write_file(&worktree.join("NOTES"), "one more commit\n");
    git_ok(&worktree, &["add", "NOTES"]);
    git_ok(&worktree, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "one more"]);
    let long = scratch.path().join("long-notes.md");
    write_file(&long, &"word ".repeat(501));
    let again = otto(&["state", "change", "submit", "long", "--repo", repo.to_str().unwrap(), "--branch", &branch, "--title", &title, "--summary-file", long.to_str().unwrap(), "--no-evidence", "a test of the refusal"]);
    let said = String::from_utf8_lossy(&again.stderr).to_string();
    println!("a new commit with 501 words alone: {said}");
    checks.check(!again.status.success() && said.contains("--file") && said.contains("--summary-file"), "refused, saying how to go on", "not refused, or the reply doesn't say how to go on");
    let after = index(&run)["changes"][0]["versions"].as_array().map_or(0, Vec::len);
    checks.check(after == 1, "the refusal left no version to approve", "a version exists with no short summary");

    if checks.fail_count() > 0 {
        scratch.keep();
    }
    finish_run(checks, &run, "done", None, None);
}
