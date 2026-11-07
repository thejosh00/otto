//! tests/live_land_override.rs — prove the lander lands a change the way the person chose, told
//! plainly, when they chose other than its repository's process, against a real model.
//!
//! What `cargo test` cannot reach: what the lander makes of what `claim` prints. A change approved
//! to push in a repository whose process opens pull requests was claimed as `mode` push with `land`
//! still saying "A pull request onto the base", and the lander had to dig through the change's
//! history to find which won. So a stand-in author hands back one change on such a repository, the
//! person approves it with --push, and it must land on the base as the commit approved, with no such
//! words in anything the lander was given.
//!
//! One wake, ~2 minutes. An `OTTO_HOME` of its own under the scratch repo.
//!
//!   cargo test --test live_land_override -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::{Duration, Instant};
use support::*;

const TIMEOUT: Duration = Duration::from_secs(900);

fn change(run: &Run, n: u64) -> Value {
    let index: Value = std::fs::read_to_string(run.dir.join("changes/index.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    index["changes"].as_array().and_then(|all| all.iter().find(|c| c["n"] == n).cloned()).unwrap_or(Value::Null)
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_land_override() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions, and a home of its own");
    require_launcher(&mut checks);
    let mut scratch = ScratchRepo::new(&workspace_dir(), "land-override");
    let home = scratch.path().join("otto-home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("OTTO_HOME", &home);
    checks.ok(format!("OTTO_HOME={}", home.display()));

    checks.step("1 - a repo on the standard process, which opens pull requests, with a real (local) remote");
    let origin = scratch.origin_path();
    let repo = scratch.repo_path("greeter");
    git_ok(scratch.path(), &["init", "--quiet", "--bare", "--initial-branch=main", origin.to_str().unwrap()]);
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "--quiet", "--initial-branch=main"]);
    write_file(&repo.join("README.md"), "# greeter\n\nSays hello.\n\n## Testing\n\n```\npython3 -m unittest\n```\n");
    write_file(&repo.join("greet.py"), "def greet(name: str) -> str:\n    return f\"Hello, {name}!\"\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "greeter: hello"]);
    git_ok(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git_ok(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git_ok(&repo, &["remote", "set-head", "origin", "main"]);
    let workdir = scratch.path().to_str().unwrap().to_string();

    checks.step("2 - a lander, wrapping land-flow, and a stand-in author with one change");
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

    let worktree = scratch.path().join(".work-trees/farewell");
    git_ok(&repo, &["worktree", "add", "--quiet", "-b", "author/farewell", worktree.to_str().unwrap(), "main"]);
    write_file(&worktree.join("farewell.py"), "def farewell(name: str) -> str:\n    return f\"Goodbye, {name}!\"\n");
    git_ok(&worktree, &["add", "-A"]);
    git_ok(&worktree, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "a farewell"]);
    let approved = stdout(&git(&worktree, &["rev-parse", "HEAD"]));
    let summary = scratch.path().join("summary.md");
    write_file(&summary, "A farewell, beside the greeting.\n");
    let out = otto(&["state", "change", "submit", &author.id, "--repo", repo.to_str().unwrap(), "--branch", "author/farewell", "--title", "A farewell", "--summary-file", summary.to_str().unwrap(), "--tests", "no tests touch it"]);
    checks.check(out.status.success(), "handed back", format!("submit failed: {}", String::from_utf8_lossy(&out.stderr)));
    let _ = otto(&["stop", &author.id, "--reason", "a stand-in: it writes nothing"]);

    checks.step(&format!("3 - approve it to push, against its process (up to {}min)", TIMEOUT.as_secs() / 60));
    let out = otto(&["changes", "approve", "author/1@v1", "--push"]);
    checks.check(out.status.success(), "approved", format!("approve failed: {}", String::from_utf8_lossy(&out.stderr)));
    let d = &change(&author, 1)["decision"];
    checks.check(d["mode"] == "push" && d["process"] == "standard" && d["overridden"] == true, "to push, by the person's choice, under standard", format!("the decision: {d}"));
    checks.check(!d.to_string().contains("pull request"), "with no words about a pull request for the lander to weigh", format!("the decision: {d}"));

    let start = Instant::now();
    let state = loop {
        let c = change(&author, 1);
        let state = c["state"].as_str().unwrap_or("").to_string();
        if (state == "landed" && !c["cleaned"].is_null()) || state == "revising" || state == "needs-you" || start.elapsed() > TIMEOUT {
            break state;
        }
        let _ = otto(&["poke"]);
        std::thread::sleep(Duration::from_secs(15));
    };

    checks.step("4 - it landed the way the person chose");
    checks.check(state == "landed", "it landed", format!("the change is {state}"));
    let main_now = stdout(&git(&origin, &["rev-parse", "main"]));
    checks.check(main_now == approved, "origin/main is the very commit approved", format!("origin/main is {main_now}, approved {approved}"));
    let moments = change(&author, 1)["timeline"].to_string();
    checks.check(!moments.contains("pull/"), "no pull request came into it", format!("its timeline: {moments}"));

    // Whether it had to go looking is the model's account; shown for whoever reads the transcript.
    let reports = stdout(&otto(&["feedback", "--all", "--json"]));
    checks.note(format!("what the lander reported about otto: {reports}"));
    println!("\nthe lander's output:\n{}", lander.wake_log(80));
    lander.journal_dump();
    if checks.fail_count() > 0 {
        scratch.keep();
    }
    let _ = otto(&["stop", &lander.id, "--reason", "live test finished"]);
    checks.finish();
}
