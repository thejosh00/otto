//! tests/live_verify.rs — a claim of done is checked against the world, by a wake that did not
//! make it.
//!
//! The failure this exists for is DESIGN.md §19.1: `dev-flow` went `land → done`, left its worktree
//! behind, and said nothing — and both contract checks passed. Here the claim is set up by hand, so
//! the premise is certain: steps 1–3 of `test/fixtures/note-flow.md` are done, the cleanup is not,
//! and the claiming wake's own account (handoff and todo) says it is. Only looking at the world
//! tells the difference, which is the whole of what the checker is for.
//!
//! - `live_verify_catches_a_skipped_cleanup`: the checker must reopen, naming the worktree; the
//!   next wake must clean up and claim again; a second check confirms. Three wakes, ~6 minutes.
//! - `live_verify_confirms_finished_work`: everything was done; one wake confirms. ~2 minutes.
//!
//!   cargo test --test live_verify -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use support::*;

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_VERIFY_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(1200);
    Duration::from_secs(secs)
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_verify_catches_a_skipped_cleanup() {
    run_case(false);
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_verify_confirms_finished_work() {
    run_case(true);
}

fn run_case(cleaned_up: bool) {
    let timeout = timeout();
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - the work, done by hand: worktree, commit, merge");
    let label = if cleaned_up { "verify-done" } else { "verify-skip" };
    let mut scratch = ScratchRepo::new(&workspace_dir(), label);
    let repo = scratch.repo_path("greeter");
    let worktree = PathBuf::from(format!("{}-note", repo.display()));
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "--quiet", "--initial-branch=main"]);
    write_file(&repo.join("greet.py"), "def greet(name):\n    return f\"Hello, {name}!\"\n");
    commit_all(&repo, "greeter: hello");
    git_ok(&repo, &["worktree", "add", "--quiet", "-b", "note", worktree.to_str().unwrap()]);
    write_file(&worktree.join("NOTE.md"), "A note.\n");
    commit_all(&worktree, "note");
    git_ok(&repo, &["merge", "--quiet", "--no-edit", "note"]);
    if cleaned_up {
        git_ok(&repo, &["worktree", "remove", worktree.to_str().unwrap()]);
        git_ok(&repo, &["branch", "--quiet", "-d", "note"]);
    }
    checks.check(repo.join("NOTE.md").is_file(), "NOTE.md is on main", "the merge did not land");
    checks.check(
        worktree.is_dir() != cleaned_up,
        if cleaned_up { "the worktree is gone" } else { "the worktree is still there — the skipped cleanup" },
        "the scratch repo is not in the state this case needs",
    );

    checks.step("2 - a run whose last wake claims done, saying it cleaned up");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let run_id = format!("{label}-{stamp}");
    let instructions = repo_path("test/fixtures/note-flow.md");
    let repo_fact = format!("repo={}", repo.display());
    let created = otto(&[
        "state",
        "init",
        "--id",
        &run_id,
        "--goal",
        "Add NOTE.md to greeter's main through a worktree, then clean the worktree up.",
        "--until",
        "NOTE.md is on main, and the note worktree and branch are gone (git worktree list shows only the main checkout)",
        "--instructions",
        instructions.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--workdir",
        scratch.path().to_str().unwrap(),
        "--fact",
        &repo_fact,
    ]);
    checks.check(created.status.success(), format!("created {run_id}"), String::from_utf8_lossy(&created.stderr));
    let run = Run::new(&run_id);
    // The claiming wake's own account, and it is the same either way: it says it cleaned up.
    write_file(
        &run.artifact("todo.md"),
        "# note-flow\n- [x] Make the note worktree\n- [x] Write and commit NOTE.md\n- [x] Merge note into main\n\
         - [x] Remove the worktree and the note branch\n",
    );
    let handoff = format!(
        "# {run_id} · cleanup · wake 0\n## Goal\nAdd NOTE.md to greeter's main through a worktree, then clean the worktree up.\n\
         ## Where this is\nFinished: the note is on main and everything is tidied away.\n## Decided\nNothing needed deciding.\n\
         ## This wake did\nMade the note worktree, committed NOTE.md, merged note into main, removed the worktree and the branch.\n\
         ## Next wake must\nNothing; the run is done.\n## Needs\nnothing\n## Don't re-derive\nartifacts/todo.md\n"
    );
    let wrote = std::process::Command::new(otto_bin())
        .args(["state", "handoff", &run_id, "--stdin"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(handoff.as_bytes())?;
            child.wait()
        })
        .map(|s| s.success())
        .unwrap_or(false);
    checks.check(wrote, "the handoff says it is all done", "could not write the handoff");
    let claimed = otto(&["state", "set-status", &run_id, "--status", "done", "--reason", "merged the note and cleaned up"]);
    checks.check(claimed.status.success(), "claimed done", String::from_utf8_lossy(&claimed.stderr));
    checks.check(run.status() == "sleeping", "a claim, not done: the run waits to be checked", format!("status={}", run.status()));
    checks.check(run.field_str("doneClaim.byWake") == "0", "doneClaim recorded", "no doneClaim in run.json");

    checks.step(&format!("3 - wake it: the checker, and whatever follows (up to {}min)", timeout.as_secs() / 60));
    let woke = otto(&["wake", &run_id]);
    checks.check(woke.status.success(), "the checking wake started", String::from_utf8_lossy(&woke.stderr));
    if !await_over(&run, timeout) {
        scratch.keep();
        checks.bail(&run, &format!("not over within {}min (status={})", timeout.as_secs() / 60, run.status()));
    }
    checks.check(await_idle(&run, Duration::from_secs(300)), "no wake still running", "a wake is still running");

    checks.step("4 - what the checks decided");
    let verdicts: Vec<Value> = journal(&run)
        .into_iter()
        .filter(|v| matches!(v.get("event").and_then(Value::as_str), Some("done-verified" | "done-reopened")))
        .collect();
    for v in &verdicts {
        checks.note(format!(
            "| {}: {}",
            v["event"].as_str().unwrap_or("?"),
            v.get("reason").and_then(Value::as_str).unwrap_or("")
        ));
    }
    let reopened = verdicts.iter().filter(|v| v["event"] == "done-reopened").count();
    let confirmed = verdicts.iter().filter(|v| v["event"] == "done-verified").count();
    if cleaned_up {
        checks.check(reopened == 0, "nothing was missing, and the check said so", format!("reopened {reopened} time(s) on finished work"));
        checks.check(confirmed == 1, "confirmed once", format!("{confirmed} confirmation(s)"));
        checks.check(run.field_str("wake.n") == "1", "by the one checking wake", format!("took {} wakes", run.field_str("wake.n")));
    } else {
        checks.check(reopened >= 1, "the first check reopened it", "the claim was confirmed with the worktree still there");
        let first = verdicts.first().map(|v| v.to_string().to_lowercase()).unwrap_or_default();
        checks.check(
            first.contains("done-reopened") && first.contains("worktree"),
            "and named the worktree as what was missing",
            format!("the first verdict was: {first}"),
        );
        checks.check(confirmed == 1, "a later check confirmed it", format!("{confirmed} confirmation(s)"));
        checks.check(!worktree.exists(), "the worktree is gone now", "the worktree is still there");
        let branches = stdout(&git(&repo, &["branch", "--list", "note"]));
        checks.check(branches.trim().is_empty(), "and the note branch", format!("branches: {branches}"));
    }
    checks.check(run.status() == "done", "status=done", format!("status={}", run.status()));
    checks.check(run.field_str("lastVerdict.confirmed") == "true", "lastVerdict says what was checked", "no confirmed lastVerdict");
    checks.check(repo.join("NOTE.md").is_file(), "NOTE.md still on main", "the note went missing");
    checks.check(run.field("doneClaim").is_none(), "no claim left behind", "doneClaim outlived the run");

    if checks.fail_count() > 0 {
        scratch.keep();
    }
    finish_run(checks, &run, "done", None, None);
}

fn commit_all(dir: &Path, message: &str) {
    git_ok(dir, &["add", "-A"]);
    git_ok(dir, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", message]);
}

fn journal(run: &Run) -> Vec<Value> {
    std::fs::read_to_string(run.dir.join("journal.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Until the run is done, failed, stopped or blocked.
fn await_over(run: &Run, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if matches!(run.status().as_str(), "done" | "failed" | "stopped" | "blocked") {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}
