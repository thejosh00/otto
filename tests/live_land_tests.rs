//! tests/live_land_tests.rs — prove the lander runs a repo's tests on what lands, every time,
//! against a real model.
//!
//! What `cargo test` cannot reach: whether the lander, reading `workflows/land-flow.md`, tests a
//! change whose base hasn't moved. With no rebase to do, it once took the author's word for it and
//! pushed. So a stand-in author hands back two changes on an unmoved base, each saying its tests
//! pass: one is lying, and must come back to its author with main untouched; the other is telling
//! the truth, and must land, tested, as the very commit approved.
//!
//! The authors write no code with a model; the lander is the only thing awake. Two wakes, ~2
//! minutes. An `OTTO_HOME` of its own under the scratch repo.
//!
//!   cargo test --test live_land_tests -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::path::Path;
use std::time::{Duration, Instant};
use support::*;

const TIMEOUT: Duration = Duration::from_secs(1200);

/// Change `n` of the author's, as otto keeps it.
fn change(run: &Run, n: u64) -> Value {
    let index: Value = std::fs::read_to_string(run.dir.join("changes/index.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    index["changes"].as_array().and_then(|all| all.iter().find(|c| c["n"] == n).cloned()).unwrap_or(Value::Null)
}

/// Poke (this home only) until the change has been dealt with one way or the other.
fn await_outcome(run: &Run, n: u64) -> String {
    let start = Instant::now();
    loop {
        let state = change(run, n)["state"].as_str().unwrap_or("").to_string();
        if matches!(state.as_str(), "revising" | "landed") && !run_is_landing(n, run) {
            return state;
        }
        if start.elapsed() > TIMEOUT {
            return format!("still {state} after {}min", TIMEOUT.as_secs() / 60);
        }
        let _ = otto(&["poke"]);
        std::thread::sleep(Duration::from_secs(15));
    }
}

/// A landed change isn't finished with until its worktree and branch are gone.
fn run_is_landing(n: u64, run: &Run) -> bool {
    let c = change(run, n);
    c["state"] == "landed" && c["cleaned"].is_null()
}

/// Commit `path` = `content` on a new branch in a worktree of its own, and return the commit.
fn commit_on_branch(repo: &Path, worktree: &Path, branch: &str, path: &str, content: &str) -> String {
    git_ok(repo, &["worktree", "add", "--quiet", "-b", branch, worktree.to_str().unwrap(), "main"]);
    write_file(&worktree.join(path), content);
    git_ok(worktree, &["add", "-A"]);
    git_ok(worktree, &["-c", "user.email=author@local", "-c", "user.name=author", "commit", "--quiet", "-m", branch]);
    stdout(&git(worktree, &["rev-parse", "HEAD"]))
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_land_tests() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions, and a home of its own");
    require_launcher(&mut checks);
    let mut scratch = ScratchRepo::new(&workspace_dir(), "land-tests");
    let home = scratch.path().join("otto-home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("OTTO_HOME", &home);
    checks.ok(format!("OTTO_HOME={}", home.display()));

    checks.step("1 - a repo whose README says how to test it, with a real (local) remote");
    let origin = scratch.origin_path();
    let repo = scratch.repo_path("greeter");
    let test_runs = scratch.path().join("test-runs.log");
    git_ok(scratch.path(), &["init", "--quiet", "--bare", "--initial-branch=main", origin.to_str().unwrap()]);
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "--quiet", "--initial-branch=main"]);
    write_file(&repo.join("README.md"), "# greeter\n\nSays hello.\n\n## Testing\n\n```\nmake test\n```\n");
    write_file(&repo.join("greet.py"), "def greet(name: str) -> str:\n    return f\"Hello, {name}!\"\n");
    write_file(
        &repo.join("test_greet.py"),
        "import unittest\n\nfrom greet import greet\n\n\nclass TestGreet(unittest.TestCase):\n    def test_greet_names_the_person(self) -> None:\n        self.assertEqual(greet(\"Ada\"), \"Hello, Ada!\")\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n",
    );
    // The record of what was really tested, and where: the lander's word isn't the evidence either.
    write_file(
        &repo.join("Makefile"),
        &format!("test:\n\t@echo \"$$(git rev-parse HEAD) $$(pwd)\" >> {}\n\tpython3 -m unittest discover -v\n\n.PHONY: test\n", test_runs.display()),
    );
    write_file(&repo.join(".gitignore"), "__pycache__/\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "greeter: hello"]);
    git_ok(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git_ok(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git_ok(&repo, &["remote", "set-head", "origin", "main"]);
    otto(&["config", "project", repo.to_str().unwrap(), "--process", "direct"]);
    let workdir = scratch.path().to_str().unwrap().to_string();
    let main_before = stdout(&git(&origin, &["rev-parse", "main"]));

    checks.step("2 - a lander, wrapping land-flow, and a stand-in author with two changes");
    let out = otto(&["state", "init", "--name", "author", "--perpetual", "--repo", repo.to_str().unwrap(), "--workdir", &workdir, "--goal", "Stand in for a run that writes code"]);
    checks.check(out.status.success(), "the author exists", format!("state init failed: {}", String::from_utf8_lossy(&out.stderr)));
    let author = Run::new(stdout(&out).lines().last().unwrap_or("2").to_string());

    let land_flow = repo_path("workflows/land-flow.md");
    let out = otto(&[
        "state", "init", "--name", "lander", "--perpetual", "--period", "10m",
        "--instructions", land_flow.to_str().unwrap(),
        "--repo", &workdir, "--workdir", &workdir,
        "--goal", "Land the changes I approve, and clean up after them",
    ]);
    checks.check(out.status.success(), "the lander exists", format!("state init failed: {}", String::from_utf8_lossy(&out.stderr)));
    let lander = Run::new(stdout(&out).lines().last().unwrap_or("1").to_string());
    let out = otto(&["config", "lander", "lander"]);
    checks.check(out.status.success(), "it is the lander", "otto config lander failed");

    let worktrees = scratch.path().join(".work-trees");
    let broken_at = worktrees.join("warmer");
    let broken = commit_on_branch(&repo, &broken_at, "author/warmer", "greet.py", "def greet(name: str) -> str:\n    return f\"Hi there, {name}!\"\n");
    let good_at = worktrees.join("farewell");
    let good = commit_on_branch(&repo, &good_at, "author/farewell", "farewell.py", "def farewell(name: str) -> str:\n    return f\"Goodbye, {name}!\"\n");

    let summary = scratch.path().join("summary.md");
    write_file(&summary, "A small change to the greeter. All the tests pass.\n");
    for (branch, title) in [("author/warmer", "A warmer greeting"), ("author/farewell", "A farewell")] {
        let out = otto(&[
            "state", "change", "submit", &author.id, "--repo", repo.to_str().unwrap(), "--branch", branch, "--title", title,
            "--summary-file", summary.to_str().unwrap(), "--tests", "make test: 1 passed",
        ]);
        checks.check(out.status.success(), format!("{branch} handed back"), format!("submit failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    // It has done its part: nothing it is told from here on should spend a wake.
    let _ = otto(&["stop", &author.id, "--reason", "a stand-in: it writes nothing"]);

    checks.step(&format!("3 - approve the one whose test fails, on a base that hasn't moved (up to {}min)", TIMEOUT.as_secs() / 60));
    let out = otto(&["changes", "approve", "author/1@v1"]);
    checks.check(out.status.success(), "approved", format!("approve failed: {}", String::from_utf8_lossy(&out.stderr)));
    checks.check(stdout(&out).contains("woke"), "the lander was woken", format!("the lander wasn't woken: {}", stdout(&out)));
    let outcome = await_outcome(&author, 1);
    let main_now = stdout(&git(&origin, &["rev-parse", "main"]));
    let bad_landed = main_now != main_before;
    checks.check(!bad_landed, "origin/main hasn't moved: the failing change didn't land", format!("origin/main moved to {main_now}: a change with a failing test landed"));
    checks.check(outcome == "revising", "it went back to its author", format!("the change is {outcome}"));
    let moments = change(&author, 1)["timeline"].to_string();
    checks.check(moments.contains("couldn't land it") && moments.to_lowercase().contains("test"), "saying the tests failed", format!("its timeline: {moments}"));
    let ran = std::fs::read_to_string(&test_runs).unwrap_or_default();
    checks.check(ran.contains(&broken), "make test really ran on that commit", format!("the test log says: {ran:?}"));
    if bad_landed || outcome != "revising" {
        println!("\nthe lander's output:\n{}", lander.wake_log(60));
        lander.journal_dump();
        scratch.keep();
        checks.bail(&lander, "a change with a failing test and an unmoved base wasn't stopped");
    }

    checks.step(&format!("4 - approve the honest one, also on an unmoved base (up to {}min)", TIMEOUT.as_secs() / 60));
    let out = otto(&["changes", "approve", "author/2@v1"]);
    checks.check(out.status.success(), "approved", format!("approve failed: {}", String::from_utf8_lossy(&out.stderr)));
    checks.check(stdout(&out).contains("woke"), "the lander was woken", format!("the lander wasn't woken: {}", stdout(&out)));
    let outcome = await_outcome(&author, 2);
    checks.check(outcome == "landed", "it landed", format!("the change is {outcome}"));
    let main_now = stdout(&git(&origin, &["rev-parse", "main"]));
    checks.check(main_now == good, "origin/main is the very commit approved", format!("origin/main is {main_now}, approved {good}"));
    let ran = std::fs::read_to_string(&test_runs).unwrap_or_default();
    let line = ran.lines().find(|l| l.starts_with(&good)).unwrap_or("");
    checks.check(!line.is_empty(), "make test ran on it first", format!("the test log says: {ran:?}"));
    checks.check(!line.is_empty() && !line.contains(good_at.to_str().unwrap()), "in the lander's own worktree, not its author's", format!("it ran at: {line}"));
    checks.check(locks_held() == 0, "no repo lock is held", "a repo lock is still held");

    println!("\nwhat was tested, and where:\n{ran}");
    println!("the lander's output:\n{}", lander.wake_log(60));
    lander.journal_dump();
    if checks.fail_count() > 0 {
        scratch.keep();
    }
    let _ = otto(&["stop", &lander.id, "--reason", "live test finished"]);
    checks.finish();
}
