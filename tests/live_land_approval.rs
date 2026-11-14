//! tests/live_land_approval.rs — prove the lander confirms an approved change one way, the change's
//! own check, and asks nothing, against a real model.
//!
//! What `cargo test` cannot reach: which check a model runs when it has read both the harness and
//! its instructions. The harness told every wake to run `otto state check-authorized` before a push,
//! every time; land-flow told the lander to run `otto state change check`. For an approved change
//! `check-authorized` refuses and says to open a gate, which the lander is never to do about a
//! landing. So a stand-in author hands back one change on a repository whose process pushes, the
//! person approves it, and it must land on the base as the commit approved, with no gate opened and,
//! in the commands the lander's sessions ran, `change check` and never `check-authorized`.
//!
//! One wake, ~2 minutes. An `OTTO_HOME` of its own under the scratch repo.
//!
//!   cargo test --test live_land_approval -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::{Duration, Instant};
use support::*;

const TIMEOUT: Duration = Duration::from_secs(900);

fn change(run: &Run, n: u64) -> Value {
    let index: Value = std::fs::read_to_string(run.dir.join("changes/index.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    index["changes"].as_array().and_then(|all| all.iter().find(|c| c["n"] == n).cloned()).unwrap_or(Value::Null)
}

/// Every command the lander's sessions ran, and how many of those sessions' transcripts could be
/// read. Every wake's session, from the journal's `wake-started` — `wake.session` is only the latest,
/// and landing and cleaning up may be two wakes. Only what the model ran, not what it was given to
/// read: the harness names `check-authorized` itself.
fn commands_run(run: &Run) -> (usize, Vec<String>) {
    let journal = std::fs::read_to_string(run.dir.join("journal.jsonl")).unwrap_or_default();
    let mut sessions: Vec<String> = journal
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| v["event"] == "wake-started")
        .filter_map(|v| v["session"].as_str().map(str::to_string))
        .collect();
    let latest = run.field_str("wake.session");
    if !latest.is_empty() && !sessions.contains(&latest) {
        sessions.push(latest);
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let projects: Vec<_> = std::fs::read_dir(home.join(".claude/projects")).map(|d| d.flatten().map(|p| p.path()).collect()).unwrap_or_default();
    let (mut read, mut commands) = (0, Vec::new());
    for session in sessions {
        let Some(text) = projects.iter().find_map(|p| std::fs::read_to_string(p.join(format!("{session}.jsonl"))).ok()) else { continue };
        read += 1;
        for entry in text.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
            if entry["type"] != "assistant" {
                continue;
            }
            for block in entry["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use" {
                    if let Some(command) = block["input"]["command"].as_str() {
                        commands.push(command.to_string());
                    }
                }
            }
        }
    }
    (read, commands)
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_land_approval() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions, and a home of its own");
    require_launcher(&mut checks);
    let mut scratch = ScratchRepo::new(&workspace_dir(), "land-approval");
    let home = scratch.path().join("otto-home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("OTTO_HOME", &home);
    checks.ok(format!("OTTO_HOME={}", home.display()));

    checks.step("1 - a repo on the direct process, where approval pushes, with a real (local) remote");
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
    let out = otto(&["config", "project", repo.to_str().unwrap(), "--process", "direct"]);
    checks.check(out.status.success(), "its process is direct", format!("config project failed: {}", String::from_utf8_lossy(&out.stderr)));
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
    let out = otto(&["state", "change", "submit", &author.id, "--repo", repo.to_str().unwrap(), "--branch", "author/farewell", "--title", "A farewell", "--summary-file", summary.to_str().unwrap(), "--tests", "no tests touch it", "--no-evidence", "a stand-in: the test's own commit"]);
    checks.check(out.status.success(), "handed back", format!("submit failed: {}", String::from_utf8_lossy(&out.stderr)));
    let _ = otto(&["stop", &author.id, "--reason", "a stand-in: it writes nothing"]);

    checks.step(&format!("3 - approve it, plainly (up to {}min)", TIMEOUT.as_secs() / 60));
    let out = otto(&["changes", "approve", "author/1@v1"]);
    checks.check(out.status.success(), "approved", format!("approve failed: {}", String::from_utf8_lossy(&out.stderr)));
    let d = &change(&author, 1)["decision"];
    checks.check(d["mode"] == "push", "to push, as its process says", format!("the decision: {d}"));

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

    checks.step("4 - it landed, as the commit approved");
    checks.check(state == "landed", "it landed", format!("the change is {state}"));
    let main_now = stdout(&git(&origin, &["rev-parse", "main"]));
    checks.check(main_now == approved, "origin/main is the very commit approved", format!("origin/main is {main_now}, approved {approved}"));

    checks.step("5 - the lander asked nothing");
    checks.check(lander.events("gate-opened") == 0, "it opened no gate", format!("it opened {} gate(s)", lander.events("gate-opened")));
    checks.check(lander.field_str("gate.slug").is_empty(), "and has none open", format!("its gate is {}", lander.field_str("gate.slug")));

    checks.step("6 - and confirmed the approval one way: the change's own check");
    let (sessions, commands) = commands_run(&lander);
    if sessions == 0 {
        checks.skip("none of the lander's transcripts could be read, which says nothing either way");
    } else {
        checks.note(format!("{sessions} session(s), {} command(s)", commands.len()));
        let checked: Vec<&String> = commands.iter().filter(|c| c.contains("state change check")).collect();
        checks.check(!checked.is_empty(), "it ran `change check`", "no command it ran was `otto state change check`");
        let authorized: Vec<&String> = commands.iter().filter(|c| c.contains("check-authorized")).collect();
        checks.check(authorized.is_empty(), "it never ran `check-authorized`", format!("it ran: {authorized:?}"));
    }

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
