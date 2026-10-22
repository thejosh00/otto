//! tests/live_changes.rs — prove code handed back as a change gets landed, against a real model.
//!
//! What `cargo test` cannot reach: whether a wake told only to "hand it back for review" commits on
//! a branch of its own and submits a change (`harness/wake.md`, "Changes to code") rather than
//! asking at a gate or pushing; whether the lander, wrapping `workflows/land-flow.md`, lands an
//! approved change onto a `main` that moved meanwhile — rebasing in a worktree of its own, checking
//! the commit, pushing, cleaning up — and whether the author, told it landed, does what follows.
//!
//! Everything runs in an `OTTO_HOME` of its own under the scratch repo, so the lander this test
//! names in `config.json` is never yours. Three wakes or so, ~10 minutes.
//!
//!   cargo test --test live_changes -- --ignored --nocapture

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
fn live_changes() {
    let submit_timeout = env_secs("OTTO_LIVE_CHANGES_SUBMIT_TIMEOUT", 1200);
    let land_timeout = env_secs("OTTO_LIVE_CHANGES_LAND_TIMEOUT", 1200);
    let mut checks = Checks::new();

    checks.step("0 - preconditions, and a home of its own");
    require_launcher(&mut checks);
    let mut scratch = ScratchRepo::new(&workspace_dir(), "changes");
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
    write_file(&repo.join("Makefile"), "test:\n\tfind . -name __pycache__ -prune -exec rm -rf {} +\n\tpython3 -m unittest discover -v\n\n.PHONY: test\n");
    write_file(&repo.join(".gitignore"), "__pycache__/\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "greeter: hello"]);
    git_ok(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git_ok(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git_ok(&repo, &["remote", "set-head", "origin", "main"]);
    let workdir = scratch.path().to_str().unwrap().to_string();

    checks.step("2 - a lander, wrapping land-flow, named in this home's config");
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

    checks.step(&format!("3 - an author hands its change back (up to {}min)", submit_timeout.as_secs() / 60));
    let fixture = repo_path("test/fixtures/change-flow.md");
    let goal = format!(
        "In {}: add a farewell(name) function to greet.py returning \"Goodbye, <name>!\", mirroring greet() including its \
         docstring style, with a unit test beside the existing one",
        repo.display()
    );
    let author = start_run(
        &mut checks,
        "author",
        &goal,
        &[
            "--instructions", fixture.to_str().unwrap(),
            "--repo", repo.to_str().unwrap(), "--workdir", &workdir,
            "--until", "the farewell change has landed on main, and artifacts/after-landing.md names the commit it landed at",
            "--policy", "verifyDone=false",
        ],
    );
    let start = Instant::now();
    while author.events("change-submitted") == 0 && start.elapsed() < submit_timeout && !author.is_over() {
        std::thread::sleep(Duration::from_secs(10));
    }
    if author.events("change-submitted") == 0 {
        checks.bail(&author, "no change was submitted");
    }
    checks.check(await_idle(&author, Duration::from_secs(300)), "its wake has exited", "its wake is still running");
    let c = change(&author).unwrap_or(Value::Null);
    checks.check(c["state"] == "review", "the change waits for review", format!("the change is {}", c["state"]));
    checks.check(author.gate_files_matching(".md").is_empty(), "it asked no question first — the change is the question", "it opened a gate as well");
    let branch = c["branch"].as_str().unwrap_or("").to_string();
    checks.check(!branch.is_empty() && branch != "main", format!("on branch {branch}"), "no branch of its own");
    let remote_main_before = stdout(&git(&origin, &["rev-parse", "main"]));
    let pushed = stdout(&git(&origin, &["branch", "--list"]));
    checks.check(!pushed.contains(&branch), "it pushed nothing", format!("the remote has its branch: {pushed}"));
    let patch = c["versions"][0]["patch"].as_str().map(|p| std::fs::read_to_string(author.dir.join(p)).unwrap_or_default()).unwrap_or_default();
    checks.check(patch.contains("Goodbye"), "the kept patch is the farewell", "the patch doesn't say Goodbye");
    let listed = stdout(&otto(&["changes", "--json"]));
    checks.check(listed.contains("\"yours\": 1"), "otto changes says one waits on you", "otto changes doesn't count it");

    checks.step("4 - main moves on meanwhile, so landing it takes a clean rebase");
    write_file(&repo.join("README.md"), "# greeter\n\nSays hello.\n");
    git_ok(&repo, &["add", "README.md"]);
    git_ok(&repo, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "a readme"]);
    git_ok(&repo, &["push", "--quiet", "origin", "main"]);

    checks.step(&format!("5 - approve it; the lander lands it (up to {}min)", land_timeout.as_secs() / 60));
    // Its remote is a local one, with no pull requests: it pushes straight to main.
    otto(&["config", "project", repo.to_str().unwrap(), "--process", "direct"]);
    let out = otto(&["changes", "approve", "author/1"]);
    checks.check(out.status.success(), "approved", format!("approve failed: {}", String::from_utf8_lossy(&out.stderr)));
    checks.check(stdout(&out).contains("woke"), "the lander was woken", format!("the lander wasn't woken: {}", stdout(&out)));
    let landed = poke_until(land_timeout, || change(&author).is_some_and(|c| c["state"] == "landed" && !c["cleaned"].is_null()));
    if !landed {
        let c = change(&author).unwrap_or(Value::Null);
        println!("the change: {}", serde_json::to_string_pretty(&c).unwrap_or_default());
        lander.journal_dump();
        checks.bail(&lander, "the change didn't land and get cleaned up");
    }
    let c = change(&author).unwrap_or(Value::Null);
    let sha = c["landed"]["sha"].as_str().unwrap_or("").to_string();
    let main_now = stdout(&git(&origin, &["rev-parse", "main"]));
    checks.check(main_now != remote_main_before, "origin/main moved", "origin/main didn't move");
    let greet = stdout(&git(&origin, &["show", "main:greet.py"]));
    checks.check(greet.contains("Goodbye"), "origin/main has the farewell", "origin/main has no farewell");
    let readme = git(&origin, &["show", "main:README.md"]);
    checks.check(readme.status.success(), "and the readme it was rebased onto", "the readme commit was lost — a force push?");
    let worktrees = stdout(&git(&repo, &["worktree", "list"]));
    checks.check(worktrees.lines().count() == 1, "only the main checkout is left", format!("worktrees left: {worktrees}"));
    checks.check(!git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).status.success(), "its branch is gone", "its branch is still there");
    checks.check(stdout(&otto(&["changes", "--due", "--json"])) == "[]", "nothing is due", "something is still due");
    checks.check(locks_held() == 0, "no repo lock is held", "a repo lock is still held");

    checks.step("6 - the author is told, and does what follows landing");
    let finished = poke_until(Duration::from_secs(900), || author.is_over());
    checks.check(finished && author.status() == "done", "the author finished", format!("the author is {}", author.status()));
    let after = std::fs::read_to_string(author.artifact("after-landing.md")).unwrap_or_default();
    let short = &sha[..7.min(sha.len())];
    checks.check(!short.is_empty() && after.contains(short), format!("after-landing.md names {short}"), format!("after-landing.md says: {after:?}"));

    if checks.fail_count() > 0 {
        scratch.keep();
    }
    let _ = otto(&["stop", "lander", "--reason", "live test finished"]);
    finish_run(checks, &author, "done", None, None);
}
