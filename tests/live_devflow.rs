//! tests/live_devflow.rs — drive dev-flow against a throwaway repo, ticket to merged.
//!
//! The one test that exercises the CONDUCTOR: whether `workflows/dev-flow.md`, read by a live model,
//! actually produces correct behavior. The unit tests cover `otto state` and `otto poke`; nothing else
//! covers the prose, and the prose is where the two worst bugs in this project were found.
//!
//! Run from a normal terminal. Measured on a two-file repo for a one-function change: intake
//! 1min, prepare 2min, plan ~1min — but the first run of all took 50min before the workflow
//! told the conductor to size its effort to the change. Budget an hour and raise the timeouts
//! (below) rather than concluding a run is stuck.
//!
//! The remote is a local bare clone, so the push and the merge are real operations against a
//! throwaway — no network, no gh, no chance of touching a real PR. That is also a case the
//! workflow must handle honestly: no PR host means `prNumber: null` with a recorded reason,
//! never a pretend PR.
//!
//!   cargo test --test live_devflow -- --ignored --nocapture live_devflow_merge
//!   cargo test --test live_devflow -- --ignored --nocapture live_devflow_verify

mod support;

use std::path::Path;
use std::time::Duration;
use support::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StopAt {
    /// Phases 0-4 only: ends at "ready to push", nothing outward (2 gates).
    Verify,
    /// The whole pipeline: push and merge too (4 gates).
    Merge,
}

fn env_secs(key: &str, default: u64) -> Duration {
    Duration::from_secs(std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default))
}

#[test]
#[ignore = "spawns a real claude session, builds a scratch repo, and spends real money — run with --ignored"]
fn live_devflow_verify() {
    run_devflow(StopAt::Verify);
}

#[test]
#[ignore = "spawns a real claude session, builds a scratch repo, and spends real money — run with --ignored"]
fn live_devflow_merge() {
    run_devflow(StopAt::Merge);
}

fn run_devflow(stop_at: StopAt) {
    let plan_timeout = env_secs("OTTO_LIVE_DEVFLOW_PLAN_TIMEOUT", 2400);
    let verify_timeout = env_secs("OTTO_LIVE_DEVFLOW_VERIFY_TIMEOUT", 3600);
    let outward_timeout = env_secs("OTTO_LIVE_DEVFLOW_OUTWARD_TIMEOUT", 2400);
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - scratch repo with a real (local) remote");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let mut scratch = ScratchRepo::new(&workspace_dir(), "devflow");
    let origin = scratch.origin_path();
    let repo = scratch.repo_path("greeter");
    git_ok(scratch.path(), &["init", "--quiet", "--bare", "--initial-branch=main", origin.to_str().unwrap()]);
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "--quiet", "--initial-branch=main"]);
    write_file(
        &repo.join("greet.py"),
        "\"\"\"Greetings.\"\"\"\n\n\ndef greet(name: str) -> str:\n    \"\"\"Say hello to `name`.\"\"\"\n    return f\"Hello, {name}!\"\n",
    );
    write_file(
        &repo.join("test_greet.py"),
        "import unittest\n\nfrom greet import greet\n\n\nclass TestGreet(unittest.TestCase):\n    def test_greet_names_the_person(self) -> None:\n        self.assertEqual(greet(\"Ada\"), \"Hello, Ada!\")\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n",
    );
    // Clear bytecode first: a change that alters neither file size nor the mtime second can
    // leave a stale __pycache__ winning, which hands back a false green.
    write_file(
        &repo.join("Makefile"),
        "test:\n\tfind . -name __pycache__ -prune -exec rm -rf {} +\n\tpython3 -m unittest discover -v\n\n.PHONY: test\n",
    );
    write_file(&repo.join(".gitignore"), "__pycache__/\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(
        &repo,
        &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "greeter: hello"],
    );
    git_ok(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git_ok(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git_ok(&repo, &["remote", "set-head", "origin", "main"]);
    let seed = stdout(&git(&origin, &["rev-parse", "main"]));
    checks.ok(format!("repo at {} (origin main @ {})", repo.display(), &seed[..8.min(seed.len())]));
    let make_test_baseline = run_make_test(&repo);
    checks.check(make_test_baseline, "its baseline is green", "the scratch repo starts red");

    checks.step("2 - start dev-flow (push gated: askBeforePush=true)");
    let run_id = format!("devflow-{stamp}");
    let instructions = repo_path("workflows/dev-flow.md");
    let request = format!(
        "In {}: add a farewell(name) function to greet.py returning \"Goodbye, <name>!\", mirroring greet() \
         including its docstring style, with a unit test alongside the existing one. Acceptance: make test \
         passes and farewell(\"Ada\") == \"Goodbye, Ada!\".",
        repo.display()
    );
    let run = start_run(
        &mut checks,
        &run_id,
        &request,
        &[
            "--instructions",
            instructions.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--phase",
            "intake",
            "--until",
            "the change is merged to origin/main and the worktree is reaped",
            "--policy",
            "askBeforePush=true",
            "--policy",
            "autoMergeWhenGreen=false",
        ],
    );

    checks.step(&format!("3 - the plan gate (up to {}min)", plan_timeout.as_secs() / 60));
    checks.note("intake, then prepare (manage-pr sets up the worktree), then planning");
    if !await_gate(&run, "plan-review", plan_timeout) {
        checks.bail(&run, &format!("no plan gate within {}min", plan_timeout.as_secs() / 60));
    }
    checks.ok(format!("plan-review gate after {}min", checks.elapsed().as_secs() / 60));
    checks.check(is_nonempty_file(&run.artifact("spec.md")), "artifacts/spec.md", "no spec was written");
    let spec_text = std::fs::read_to_string(run.artifact("spec.md")).unwrap_or_default().to_lowercase();
    if spec_text.contains("criteri") {
        checks.ok("the spec states acceptance criteria");
    } else {
        checks.note("the spec does not obviously state acceptance criteria — read it");
    }
    let main_repo = run.field_str("facts.mainRepo");
    checks.check(main_repo == repo.display().to_string(), "facts.mainRepo resolved to the scratch repo", format!("facts.mainRepo={main_repo}"));
    let branch = run.field_str("facts.branch");
    let worktree = run.field_str("facts.worktree");
    let base = run.field_str("facts.base");
    checks.check(base == "main", "facts.base=main (from origin/HEAD)", format!("facts.base={base}"));
    checks.check(!branch.is_empty(), format!("facts.branch={branch}"), "no branch recorded");
    let wt = std::path::PathBuf::from(&worktree);
    if !worktree.is_empty() && wt.is_dir() {
        checks.ok(format!("worktree at {worktree}"));
        let head_branch = stdout(&git(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]));
        checks.check(head_branch == branch, format!("the worktree is on {branch}"), "the worktree is on the wrong branch");
    } else {
        checks.bad(format!("facts.worktree={worktree} is not a directory"));
    }
    checks.check(repo_is_locked(&repo), "the repo is locked by the run", "the repo was never locked");
    checks.check(is_nonempty_file(&run.artifact("plan.md")), "artifacts/plan.md", "no plan was written");
    let plan_text = std::fs::read_to_string(run.artifact("plan.md")).unwrap_or_default();
    if plan_text.contains("greet.py") {
        checks.ok("the plan names the file it will change");
    } else {
        checks.note("the plan does not mention greet.py — read it before approving");
    }
    checks.note(format!("read it: {} — or watch: otto attach {run_id}", run.artifact("plan.md").display()));
    if !answer_gate(&mut checks, &run, "plan-review", "Approve — implement it as written.", Duration::from_secs(300)) {
        checks.bail(&run, "could not answer the plan gate");
    }

    checks.step(&format!("4 - the result gate (up to {}min)", verify_timeout.as_secs() / 60));
    checks.note("implementing and verifying; subagents do the writing");
    if !await_gate(&run, "result-review", verify_timeout) {
        checks.bail(&run, &format!("no result gate within {}min", verify_timeout.as_secs() / 60));
    }
    checks.ok(format!("result-review gate after {}min", checks.elapsed().as_secs() / 60));
    let commits: u32 = stdout(&git(&wt, &["rev-list", "--count", &format!("origin/main..{branch}")])).parse().unwrap_or(0);
    checks.check(commits >= 1, format!("{commits} commit(s) on the branch"), "nothing was committed");
    let dirty = !stdout(&git(&wt, &["status", "--porcelain"])).is_empty();
    checks.check(!dirty, "the worktree is clean", "uncommitted changes left behind");
    let greet_py = std::fs::read_to_string(wt.join("greet.py")).unwrap_or_default();
    checks.check(greet_py.contains("farewell"), "greet.py has farewell()", "farewell() was never written");
    checks.check(run_make_test(&wt), "make test passes in the worktree", "the suite fails there");
    checks.check(is_nonempty_file(&run.artifact("test-report.md")), "artifacts/test-report.md", "no test report");

    checks.step("5 - nothing has left this machine");
    let branch_ref = format!("refs/heads/{branch}");
    let branch_on_origin = git(&origin, &["show-ref", "--quiet", "--verify", &branch_ref]).status.success();
    checks.check(!branch_on_origin, format!("origin has no {branch}"), "the branch is on origin before any push gate");
    let head_at_verify = stdout(&git(&wt, &["rev-parse", "HEAD"]));
    let push_authorized = otto(&["state", "check-authorized", &run_id, "--action", "push", "--head", &head_at_verify]).status.success();
    checks.check(!push_authorized, "push is not authorized", "a push is somehow already authorized");

    if stop_at == StopAt::Verify {
        checks.step("6 - stop at ready-to-push (option 3)");
        if !answer_gate(&mut checks, &run, "result-review", "Stop here — leave it at ready-to-push.", Duration::from_secs(300)) {
            checks.bail(&run, "could not answer the result gate");
        }
        checks.check(await_field(&run, "status", "done", outward_timeout), "status=done", format!("status={}", run.status()));
        checks.check(is_nonempty_file(&run.artifact("ready-to-push.md")), "artifacts/ready-to-push.md", "no ready-to-push summary");
        let branch_on_origin = git(&origin, &["show-ref", "--quiet", "--verify", &branch_ref]).status.success();
        checks.check(!branch_on_origin, "still nothing on origin", "something pushed anyway");
        checks.check(!repo_is_locked(&repo), "lock released", "the lock was never released");
        if checks.fail_count() > 0 {
            scratch.keep();
        }
        finish_run(checks, &run, "done", Some(&wt), Some(&repo));
        return;
    }

    checks.step("6 - the push gate");
    if !answer_gate(&mut checks, &run, "result-review", "Go to publish.", Duration::from_secs(300)) {
        checks.bail(&run, "could not answer the result gate");
    }
    if !await_gate(&run, "push-confirm", outward_timeout) {
        checks.bail(&run, "no push-confirm gate — publish did not ask first");
    }
    checks.ok("push-confirm gate is open");
    let branch_on_origin = git(&origin, &["show-ref", "--quiet", "--verify", &branch_ref]).status.success();
    checks.check(!branch_on_origin, "still nothing pushed", "the branch reached origin BEFORE the gate was answered");
    if !answer_gate(&mut checks, &run, "push-confirm", "Approved — push it.", Duration::from_secs(300)) {
        checks.bail(&run, "could not answer the push gate");
    }

    checks.step("7 - the push, and the authorization behind it");
    let deadline = std::time::Instant::now() + outward_timeout;
    loop {
        if !run.field_str("facts.pushedSha").is_empty() {
            break;
        }
        if matches!(run.status().as_str(), "failed" | "blocked") {
            break;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let pushed = run.field_str("facts.pushedSha");
    if pushed.is_empty() {
        checks.bail(&run, "nothing was pushed after approval");
    }
    checks.ok(format!("facts.pushedSha={}", &pushed[..8.min(pushed.len())]));
    let auth_push_head = run.field_str("authorizations.push.head");
    checks.check(auth_push_head == pushed, "the authorization names the pushed commit", format!("authorized {auth_push_head} but pushed {pushed}"));
    let auth_push_by = run.field_str("authorizations.push.by");
    checks.check(auth_push_by == "human", "authorized by a human, via a gate", "the push authorization did not come from a gate");
    let origin_branch_sha = stdout(&git(&origin, &["rev-parse", &branch_ref]));
    checks.check(origin_branch_sha == pushed, format!("origin/{branch} is at the approved commit"), "origin holds a different commit");
    let travels = otto(&["state", "check-authorized", &run_id, "--action", "push", "--head", &seed]).status.success();
    checks.check(!travels, "it covers no other commit", "the authorization travels to other commits");
    let merge_from_push = otto(&["state", "check-authorized", &run_id, "--action", "merge", "--head", &pushed]).status.success();
    checks.check(!merge_from_push, "merge still needs its own approval", "approving a push also approved a merge");
    let main_sha = stdout(&git(&origin, &["rev-parse", "main"]));
    checks.check(main_sha == seed, "main is untouched so far", "main moved early");

    checks.step("8 - the merge");
    if !await_gate(&run, "merge-confirm", outward_timeout) {
        checks.bail(&run, &format!("no merge-confirm gate within {}min", outward_timeout.as_secs() / 60));
    }
    checks.ok(format!("merge-confirm gate after {}min", checks.elapsed().as_secs() / 60));
    if !answer_gate(&mut checks, &run, "merge-confirm", "Approved — merge it.", Duration::from_secs(300)) {
        checks.bail(&run, "could not answer the merge gate");
    }
    if !await_field(&run, "status", "done", outward_timeout) {
        checks.note("not done yet — checking what landed");
    }
    let merged = run.field_str("facts.mergedSha");
    checks.check(!merged.is_empty(), format!("facts.mergedSha={}", &merged[..8.min(merged.len())]), "no merge was recorded");
    let auth_merge_head = run.field_str("authorizations.merge.head");
    checks.check(auth_merge_head == pushed, "the merge authorization names that commit", "the merge authorization does not match what was pushed");
    let ancestor = git(&origin, &["merge-base", "--is-ancestor", &pushed, "main"]).status.success();
    checks.check(ancestor, "origin/main contains the change", "main does not contain the pushed commit");
    let authorized_events = run.events("authorized");
    checks.check(authorized_events >= 2, "two authorizations: push and merge", "expected one authorization per outward action");

    checks.step("9 - cleanup — the phase otto's contract cannot enforce");
    // A real run went `land → done` and left its worktree behind, unmentioned in the journal.
    // Both contract checks passed: `done` is a legal stopping state and the handoff was
    // written. Nothing generic can verify that a declared phase's durableOutput exists
    // (DESIGN.md §19.1), so this assertion is the only thing standing between "merged" and
    // "finished".
    checks.check(run.status() == "done", "status=done", format!("status={}", run.status()));
    checks.check(!repo_is_locked(&repo), "lock released", "the repo lock outlived the run");
    let worktree_list = stdout(&git(&repo, &["worktree", "list"]));
    if worktree_list.contains(&branch) {
        checks.bad("the worktree for the branch was never reaped — cleanup did not run");
    } else {
        checks.ok("worktree reaped");
    }
    let cleanup_ran = run.events("cleanup-complete") >= 1 || run.field_str("phase") == "cleanup";
    checks.check(
        cleanup_ran,
        "cleanup ran as its own phase",
        format!("no evidence cleanup ran (phase={}) — land must not set status done itself", run.field_str("phase")),
    );
    if wt.is_dir() {
        checks.note(format!("worktree still at {} (cleanup should have offered to reap it)", wt.display()));
    } else {
        checks.ok("worktree reaped");
    }
    if is_nonempty_file(&run.artifact("summary.md")) {
        checks.ok("artifacts/summary.md");
    } else {
        checks.note("no summary artifact");
    }

    if checks.fail_count() > 0 {
        scratch.keep();
    }
    finish_run(checks, &run, "done", Some(&wt), Some(&repo));
}

fn is_nonempty_file(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file() && m.len() > 0).unwrap_or(false)
}

fn run_make_test(dir: &Path) -> bool {
    std::process::Command::new("make").arg("test").current_dir(dir).output().map(|o| o.status.success()).unwrap_or(false)
}
