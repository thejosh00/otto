//! tests/live_land_tests.rs — prove the lander runs a repo's tests on what lands, every time,
//! against a real model.
//!
//! What `cargo test` cannot reach: how the lander, reading `workflows/land-flow.md`, treats a change
//! on a base that hasn't moved. With no rebase to do, it once took the author's word for it and
//! pushed. So a stand-in author hands back three changes on one repo, each saying its tests pass,
//! and that repo's tests need an install step, as a fresh worktree of a real repo does:
//!
//! - one is lying, and must come back to its author with main untouched;
//! - one is lying and rewrites its own README to name a weaker test command, which must not work:
//!   the command comes from the base branch;
//! - one is telling the truth, and is approved while a tool its tests start with is missing, so
//!   `make test` exits 127. That says nothing about the change, so it must go to the person, not its
//!   author. Told it's fixed, the lander lands it, tested, as the very commit approved.
//!
//! The authors write no code with a model; the lander is the only thing awake. About four wakes,
//! ~5 minutes. An `OTTO_HOME` of its own under the scratch repo.
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

/// Commit these files on a new branch in a worktree of its own, and return the commit.
fn commit_on_branch(repo: &Path, worktree: &Path, branch: &str, files: &[(&str, &str)]) -> String {
    git_ok(repo, &["worktree", "add", "--quiet", "-b", branch, worktree.to_str().unwrap(), "main"]);
    for (path, content) in files {
        write_file(&worktree.join(path), content);
    }
    git_ok(worktree, &["add", "-A"]);
    git_ok(worktree, &["-c", "user.email=author@local", "-c", "user.name=author", "commit", "--quiet", "-m", branch]);
    stdout(&git(worktree, &["rev-parse", "HEAD"]))
}

/// Poke until the lander has stopped on a question, or the change has moved on without one.
fn await_question(run: &Run, lander: &Run, n: u64) -> String {
    let start = Instant::now();
    loop {
        let state = change(run, n)["state"].as_str().unwrap_or("").to_string();
        if !lander.wake_running() && !lander.field_str("gate.slug").is_empty() {
            return "asked".to_string();
        }
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

    checks.step("1 - a repo whose README says how to install and test it, with a real (local) remote");
    let origin = scratch.origin_path();
    let repo = scratch.repo_path("greeter");
    let test_runs = scratch.path().join("test-runs.log");
    git_ok(scratch.path(), &["init", "--quiet", "--bare", "--initial-branch=main", origin.to_str().unwrap()]);
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "--quiet", "--initial-branch=main"]);
    // Its test runner comes from a mirror, so a fresh worktree can't run the tests (make exits 127,
    // as `bun run test` does) until it has been installed. The runner starts with a tool from outside
    // the repo: taking that away is a 127 after a clean install, through no fault of any change.
    let mirror = scratch.path().join("mirror");
    let tool = mirror.join("pyrun");
    let tool_text = "exec python3 \"$@\"\n";
    write_file(&tool, tool_text);
    write_file(&mirror.join("runner"), &format!("#!/bin/sh\nexec sh {} -m unittest discover -v\n", tool.display()));
    write_file(&repo.join("README.md"), "# greeter\n\nSays hello.\n\n## Setup\n\n```\nmake install\n```\n\n## Testing\n\n```\nmake test\n```\n");
    write_file(&repo.join("greet.py"), "def greet(name: str) -> str:\n    return f\"Hello, {name}!\"\n");
    write_file(
        &repo.join("test_greet.py"),
        "import unittest\n\nfrom greet import greet\n\n\nclass TestGreet(unittest.TestCase):\n    def test_greet_names_the_person(self) -> None:\n        self.assertEqual(greet(\"Ada\"), \"Hello, Ada!\")\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n",
    );
    // The record of what was really tested, and where: the lander's word isn't the evidence either.
    let makefile = format!(
        "install:\n\tmkdir -p .deps\n\tcp {mirror}/runner .deps/runner\n\tchmod +x .deps/runner\n\t@echo \"install $$(git rev-parse HEAD) $$(pwd)\" >> {log}\n\n\
         test:\n\t@echo \"test $$(git rev-parse HEAD) $$(pwd)\" >> {log}\n\t.deps/runner\n\n.PHONY: install test\n",
        mirror = mirror.display(),
        log = test_runs.display(),
    );
    write_file(&repo.join("Makefile"), &makefile);
    write_file(&repo.join(".gitignore"), "__pycache__/\n.deps/\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "greeter: hello"]);
    git_ok(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git_ok(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git_ok(&repo, &["remote", "set-head", "origin", "main"]);
    otto(&["config", "project", repo.to_str().unwrap(), "--process", "direct"]);
    let workdir = scratch.path().to_str().unwrap().to_string();
    let main_before = stdout(&git(&origin, &["rev-parse", "main"]));

    checks.step("2 - a lander, wrapping land-flow, and a stand-in author with three changes");
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
    let warmer_greet = "def greet(name: str) -> str:\n    return f\"Hi there, {name}!\"\n";
    let broken = commit_on_branch(&repo, &worktrees.join("warmer"), "author/warmer", &[("greet.py", warmer_greet)]);
    // Its code fails `make test`, and its own README now says the tests are a subset that passes.
    let check_makefile = format!("{makefile}\ncheck:\n\t@echo \"check $$(git rev-parse HEAD) $$(pwd)\" >> {log}\n\tpython3 -m unittest -v test_smoke\n\n.PHONY: check\n", log = test_runs.display());
    let smoke_test = "import unittest\n\nfrom greet import greet\n\n\nclass TestSmoke(unittest.TestCase):\n    def test_greet_names_the_person(self) -> None:\n        self.assertIn(\"Ada\", greet(\"Ada\"))\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n";
    let weakened = commit_on_branch(
        &repo,
        &worktrees.join("casual"),
        "author/casual",
        &[
            ("greet.py", "def greet(name: str) -> str:\n    return f\"Hey, {name}!\"\n"),
            ("README.md", "# greeter\n\nSays hello.\n\n## Setup\n\n```\nmake install\n```\n\n## Testing\n\nBefore landing:\n\n```\nmake check\n```\n\n`make test` is the whole suite, for nights.\n"),
            ("Makefile", &check_makefile),
            ("test_smoke.py", smoke_test),
        ],
    );
    let good_at = worktrees.join("farewell");
    let good = commit_on_branch(&repo, &good_at, "author/farewell", &[
            ("farewell.py", "def farewell(name: str) -> str:\n    return f\"Goodbye, {name}!\"\n"),
            ("test_farewell.py", "import unittest\n\nfrom farewell import farewell\n\n\nclass TestFarewell(unittest.TestCase):\n    def test_farewell_names_the_person(self) -> None:\n        self.assertEqual(farewell(\"Ada\"), \"Goodbye, Ada!\")\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n"),
        ]);

    let summary = scratch.path().join("summary.md");
    write_file(&summary, "A small change to the greeter. All the tests pass.\n");
    for (branch, title) in [("author/warmer", "A warmer greeting"), ("author/casual", "A casual greeting, and a quicker way to test"), ("author/farewell", "A farewell")] {
        let out = otto(&[
            "state", "change", "submit", &author.id, "--repo", repo.to_str().unwrap(), "--branch", branch, "--title", title,
            "--summary-file", summary.to_str().unwrap(), "--tests", "the tests pass", "--no-evidence", "a stand-in: the test's own commit",
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
    checks.check(ran.contains(&format!("install {broken}")), "the install ran first, in a fresh worktree", format!("the test log says: {ran:?}"));
    checks.check(ran.contains(&format!("test {broken}")), "make test really ran on that commit", format!("the test log says: {ran:?}"));
    if bad_landed || outcome != "revising" {
        println!("\nthe lander's output:\n{}", lander.wake_log(60));
        lander.journal_dump();
        scratch.keep();
        checks.bail(&lander, "a change with a failing test and an unmoved base wasn't stopped");
    }

    checks.step(&format!("4 - approve the one whose own README names a weaker test command (up to {}min)", TIMEOUT.as_secs() / 60));
    // Approved while its last wake is still finishing, nothing would wake it: it would wait out its period.
    checks.check(await_idle(&lander, TIMEOUT), "the lander has gone back to sleep", "the lander never finished its wake");
    let out = otto(&["changes", "approve", "author/2@v1"]);
    checks.check(out.status.success(), "approved", format!("approve failed: {}", String::from_utf8_lossy(&out.stderr)));
    let outcome = await_outcome(&author, 2);
    let main_now = stdout(&git(&origin, &["rev-parse", "main"]));
    checks.check(main_now == main_before, "origin/main hasn't moved: editing its README didn't get it past its check", format!("origin/main moved to {main_now}: the change chose its own test command"));
    checks.check(outcome == "revising", "it went back to its author", format!("the change is {outcome}"));
    let ran = std::fs::read_to_string(&test_runs).unwrap_or_default();
    checks.check(ran.contains(&format!("test {weakened}")), "the base branch's make test ran on it, not its make check", format!("the test log says: {ran:?}"));

    checks.step(&format!("5 - approve the honest one while the tool its tests start with is missing (up to {}min)", TIMEOUT.as_secs() / 60));
    let main_before = stdout(&git(&origin, &["rev-parse", "main"]));
    std::fs::remove_file(&tool).unwrap();
    checks.check(await_idle(&lander, TIMEOUT), "the lander has gone back to sleep", "the lander never finished its wake");
    let out = otto(&["changes", "approve", "author/3@v1"]);
    checks.check(out.status.success(), "approved", format!("approve failed: {}", String::from_utf8_lossy(&out.stderr)));
    let outcome = await_question(&author, &lander, 3);
    let main_now = stdout(&git(&origin, &["rev-parse", "main"]));
    checks.check(main_now == main_before, "origin/main hasn't moved: nothing untested landed", format!("origin/main moved to {main_now}: it landed without its tests having run"));
    checks.check(outcome == "asked", "the lander asked the person", format!("the change is {outcome}, with no question"));
    checks.check(change(&author, 3)["state"] != "revising", "it didn't go back to its author", "it went back to its author");
    checks.check(locks_held() == 0, "no repo lock is held across the question", "a repo lock is held across the question");
    if outcome != "asked" {
        println!("\nthe lander's output:\n{}", lander.wake_log(60));
        lander.journal_dump();
        scratch.keep();
        checks.bail(&lander, "a run that couldn't start didn't go to the person");
    }
    let question = lander.gate_files_matching(".md").iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect::<Vec<_>>().join("\n");
    println!("\nwhat it asked:\n{question}");

    checks.step(&format!("6 - the tool is back: it lands, tested (up to {}min)", TIMEOUT.as_secs() / 60));
    write_file(&tool, tool_text);
    let slug = lander.field_str("gate.slug");
    answer_gate(&mut checks, &lander, &slug, "pyrun is back. Try again.", TIMEOUT);
    let outcome = await_outcome(&author, 3);
    checks.check(outcome == "landed", "it landed", format!("the change is {outcome}"));
    let main_now = stdout(&git(&origin, &["rev-parse", "main"]));
    checks.check(main_now == good, "origin/main is the very commit approved", format!("origin/main is {main_now}, approved {good}"));
    let ran = std::fs::read_to_string(&test_runs).unwrap_or_default();
    checks.check(ran.contains(&format!("install {good}")), "its install ran first", format!("the test log says: {ran:?}"));
    let line = ran.lines().find(|l| l.starts_with(&format!("test {good}"))).unwrap_or("");
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
