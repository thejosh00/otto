//! tests/live_devflow.rs — drive dev-flow against a throwaway repo, ticket to landed.
//!
//! The one test that exercises the CONDUCTOR: whether `workflows/dev-flow.md`, read by a live model,
//! actually produces correct behavior. The unit tests cover `otto state` and `otto poke`; nothing else
//! covers the prose, and the prose is where the two worst bugs in this project were found.
//!
//! dev-flow never pushes: it hands its branch back as a change, and the lander lands it as the
//! repository's process says. So each variant checks that nothing left the machine until the
//! person decided, and then what the decision did:
//!
//! - `live_devflow_land` gives its repo a process of its own, whose words say how a branch is named,
//!   and approves the change: the lander lands it, and the run finishes on the note that says so.
//! - `live_devflow_drop` drops it: nothing ever reaches the remote, and the run still finishes.
//!
//! Both share one `OTTO_HOME` of their own under the workspace, and one lander in it — the
//! environment is the process's, and the two run side by side. The remotes are local bare repos:
//! no network, no gh, no chance of touching a real PR. Measured before dev-flow handed back changes:
//! ~10min a variant, most of it planning and implementing. Raise the timeouts (below) rather than
//! concluding a run is stuck.
//!
//!   cargo test --test live_devflow -- --ignored --nocapture live_devflow_land
//!   cargo test --test live_devflow -- --ignored --nocapture live_devflow_drop

mod support;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use support::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Decide {
    /// Approve the change: the lander lands it.
    Land,
    /// Drop it: it never lands, and the run finishes anyway.
    Drop,
}

impl Decide {
    /// In the run id and the scratch dir, so the two variants can run side by side.
    fn label(self) -> &'static str {
        match self {
            Decide::Land => "devflow-land",
            Decide::Drop => "devflow-drop",
        }
    }
}

/// How many variants run in this process, so the last to finish stops the lander.
const VARIANTS: usize = 2;
static FINISHED: AtomicUsize = AtomicUsize::new(0);

/// The `OTTO_HOME` both variants share, and the id of the lander named in it — made once, by
/// whichever variant gets here first.
fn shared_home() -> &'static (PathBuf, String) {
    static HOME: OnceLock<(PathBuf, String)> = OnceLock::new();
    HOME.get_or_init(|| {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let home = workspace_dir().join(".work-trees").join(format!("otto-scratch-devflow-home-{stamp}"));
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("OTTO_HOME", &home);
        let land_flow = repo_path("workflows/land-flow.md");
        let workdir = workspace_dir().join(".work-trees").display().to_string();
        let out = otto(&[
            "state", "init", "--name", "lander", "--perpetual", "--period", "10m",
            "--instructions", land_flow.to_str().unwrap(),
            "--repo", &workdir, "--workdir", &workdir,
            "--goal", "Land the changes I approve, and clean up after them",
        ]);
        assert!(out.status.success(), "the lander: {}", String::from_utf8_lossy(&out.stderr));
        let lander = stdout(&out).lines().last().unwrap_or("1").to_string();
        assert!(otto(&["config", "lander", "lander"]).status.success(), "otto config lander failed");
        (home, lander)
    })
}

/// This variant is done with the shared home: the last one out stops the lander, and removes the
/// home unless something failed.
fn leave_home(failed: bool) {
    static ANY_FAILED: AtomicUsize = AtomicUsize::new(0);
    if failed {
        ANY_FAILED.fetch_add(1, Ordering::SeqCst);
    }
    if FINISHED.fetch_add(1, Ordering::SeqCst) + 1 < VARIANTS {
        return;
    }
    let _ = otto(&["stop", "lander", "--reason", "live test finished"]);
    if ANY_FAILED.load(Ordering::SeqCst) == 0 && std::env::var("OTTO_TEST_KEEP").as_deref() != Ok("1") {
        let _ = std::fs::remove_dir_all(&shared_home().0);
    } else {
        eprintln!("kept the otto home at {}", shared_home().0.display());
    }
}

fn env_secs(key: &str, default: u64) -> Duration {
    Duration::from_secs(std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default))
}

/// The run's first change, as it keeps it.
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
#[ignore = "spawns real claude sessions, builds a scratch repo, and spends real money — run with --ignored"]
fn live_devflow_land() {
    run_devflow(Decide::Land);
}

#[test]
#[ignore = "spawns real claude sessions, builds a scratch repo, and spends real money — run with --ignored"]
fn live_devflow_drop() {
    run_devflow(Decide::Drop);
}

fn run_devflow(decide: Decide) {
    let plan_timeout = env_secs("OTTO_LIVE_DEVFLOW_PLAN_TIMEOUT", 2400);
    let submit_timeout = env_secs("OTTO_LIVE_DEVFLOW_SUBMIT_TIMEOUT", 3600);
    let land_timeout = env_secs("OTTO_LIVE_DEVFLOW_LAND_TIMEOUT", 1800);
    let mut checks = Checks::new();

    checks.step("0 - preconditions, and a home of its own with a lander in it");
    require_launcher(&mut checks);
    let (home, lander) = shared_home();
    checks.ok(format!("OTTO_HOME={}", home.display()));

    checks.step("1 - scratch repo with a real (local) remote");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let mut scratch = ScratchRepo::new(&workspace_dir(), decide.label());
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
    checks.check(run_make_test(&repo), "its baseline is green", "the scratch repo starts red");

    // Landing, the repo follows a process of its own, whose words name the branch: whether the run
    // read them is on disk as the branch it made. Dropping, it follows standard.
    let want_process = match decide {
        Decide::Land => {
            let process = format!("devflow-live-{stamp}");
            let out = otto(&[
                "config", "process", &process, "--mode", "push",
                "--start", "On a branch named farewell/<slug>, in a worktree made with the manage-pr skill",
            ]);
            checks.check(out.status.success(), format!("process {process}"), format!("otto config process failed: {}", String::from_utf8_lossy(&out.stderr)));
            let out = otto(&["config", "project", repo.to_str().unwrap(), "--process", &process]);
            checks.check(out.status.success(), "the repo follows it", format!("otto config project failed: {}", String::from_utf8_lossy(&out.stderr)));
            process
        }
        Decide::Drop => "standard".to_string(),
    };

    checks.step("2 - start dev-flow");
    let run_id = format!("{}-{stamp}", decide.label());
    let instructions = repo_path("workflows/dev-flow.md");
    let request = format!(
        "In {}: add a farewell(name) function to greet.py returning \"Goodbye, <name>!\", mirroring greet() \
         including its docstring style, with a unit test alongside the existing one. Acceptance: make test \
         passes and farewell(\"Ada\") == \"Goodbye, Ada!\".",
        repo.display()
    );
    let workdir = scratch.path().display().to_string();
    let run = start_run(
        &mut checks,
        &run_id,
        &request,
        &[
            "--instructions", instructions.to_str().unwrap(),
            "--repo", repo.to_str().unwrap(), "--workdir", &workdir,
            "--phase", "intake",
            "--until", "the change has landed on origin/main, or been dropped, and the ticket update and summary are written",
        ],
    );

    checks.step(&format!("3 - the plan gate (up to {}min)", plan_timeout.as_secs() / 60));
    checks.note("intake, then prepare (manage-pr sets up the worktree), then planning");
    if !await_gate(&run, "plan-review", plan_timeout) {
        checks.bail(&run, &format!("no plan gate within {}min", plan_timeout.as_secs() / 60));
    }
    checks.ok(format!("plan-review gate after {}min", checks.elapsed().as_secs() / 60));
    checks.check(is_nonempty_file(&run.artifact("spec.md")), "artifacts/spec.md", "no spec was written");
    let main_repo = run.field_str("facts.mainRepo");
    checks.check(main_repo == repo.display().to_string(), "facts.mainRepo resolved to the scratch repo", format!("facts.mainRepo={main_repo}"));
    let process = run.field_str("facts.process");
    checks.check(process == want_process, format!("facts.process={process}"), format!("facts.process={process}, not {want_process}"));
    let branch = run.field_str("facts.branch");
    let worktree = run.field_str("facts.worktree");
    let base = run.field_str("facts.base");
    checks.check(base == "main", "facts.base=main (from origin/HEAD)", format!("facts.base={base}"));
    checks.check(!branch.is_empty(), format!("facts.branch={branch}"), "no branch recorded");
    if decide == Decide::Land {
        checks.check(branch.starts_with("farewell/"), "named as the process says it starts", format!("{branch} isn't farewell/<slug>: the process's words weren't followed"));
    }
    let wt = PathBuf::from(&worktree);
    if !worktree.is_empty() && wt.is_dir() {
        checks.ok(format!("worktree at {worktree}"));
        let head_branch = stdout(&git(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]));
        checks.check(head_branch == branch, format!("the worktree is on {branch}"), "the worktree is on the wrong branch");
    } else {
        checks.bad(format!("facts.worktree={worktree} is not a directory"));
    }
    // `prepare` holds the lock only while it sets up the worktree, and releases it before `plan`:
    // a run that kept it through the plan-review gate would block every other run on this repo
    // for as long as the review took.
    checks.check(run.events("lock-acquired") >= 1, "prepare locked the repo", "the repo was never locked");
    checks.check(!repo_is_locked(&repo), "and released it before the gate", "the lock is held across the plan-review gate");
    checks.check(await_idle(&run, Duration::from_secs(300)), "the wake that asked has exited", "a wake is still running at the plan gate");
    check_todo(&mut checks, &run, &[("intake", "x"), ("prepare", "x"), ("plan", "~"), ("implement", " "), ("cleanup", " ")]);
    checks.check(is_nonempty_file(&run.artifact("plan.md")), "artifacts/plan.md", "no plan was written");
    checks.note(format!("read it: {} — or watch: otto attach {run_id}", run.artifact("plan.md").display()));
    if !answer_gate(&mut checks, &run, "plan-review", "Approve — implement it as written.", Duration::from_secs(300)) {
        checks.bail(&run, "could not answer the plan gate");
    }

    checks.step(&format!("4 - handed back as a change (up to {}min)", submit_timeout.as_secs() / 60));
    checks.note("implementing and verifying; subagents do the writing");
    let start = Instant::now();
    while run.events("change-submitted") == 0 && start.elapsed() < submit_timeout && !run.is_over() {
        std::thread::sleep(Duration::from_secs(10));
    }
    if run.events("change-submitted") == 0 {
        checks.bail(&run, &format!("no change was submitted within {}min", submit_timeout.as_secs() / 60));
    }
    checks.ok(format!("change submitted after {}min", checks.elapsed().as_secs() / 60));
    checks.check(await_idle(&run, Duration::from_secs(300)), "its wake has exited", "a wake is still running after submitting");
    let c = change(&run).unwrap_or(Value::Null);
    checks.check(c["state"] == "review", "the change waits for review", format!("the change is {}", c["state"]));
    checks.check(c["branch"] == branch.as_str(), format!("from {branch}"), format!("from {}, not facts.branch", c["branch"]));
    checks.check(run.field_str("facts.change") == "1", "facts.change=1", format!("facts.change={}", run.field_str("facts.change")));
    checks.check(
        run.gate_files_matching(".md").iter().all(|g| g.display().to_string().contains("plan-review")),
        "no gate but the plan's — the change is the result review",
        "it opened a gate besides plan-review",
    );
    let commits: u32 = stdout(&git(&wt, &["rev-list", "--count", &format!("origin/main..{branch}")])).parse().unwrap_or(0);
    checks.check(commits >= 1, format!("{commits} commit(s) on the branch"), "nothing was committed");
    checks.check(stdout(&git(&wt, &["status", "--porcelain"])).is_empty(), "the worktree is clean", "uncommitted changes left behind");
    checks.check(std::fs::read_to_string(wt.join("greet.py")).unwrap_or_default().contains("farewell"), "greet.py has farewell()", "farewell() was never written");
    checks.check(run_make_test(&wt), "make test passes in the worktree", "the suite fails there");
    checks.check(is_nonempty_file(&run.artifact("test-report.md")), "artifacts/test-report.md", "no test report");
    checks.check(is_nonempty_file(&run.artifact("change.md")), "artifacts/change.md — the review", "no change summary");
    check_todo(&mut checks, &run, &[("verify", "x"), ("submit", "x"), ("review", "~"), ("cleanup", " ")]);

    checks.step("5 - nothing has left this machine");
    let branch_ref = format!("refs/heads/{branch}");
    let on_origin = |b: &str| git(&origin, &["show-ref", "--quiet", "--verify", b]).status.success();
    checks.check(!on_origin(&branch_ref), format!("origin has no {branch}"), "the branch is on origin before anyone approved it");
    checks.check(stdout(&git(&origin, &["rev-parse", "main"])) == seed, "origin/main is untouched", "origin/main moved before anyone approved it");

    let reference = format!("{run_id}/1");
    match decide {
        Decide::Land => {
            checks.step(&format!("6 - approve it; the lander lands it as the process says (up to {}min)", land_timeout.as_secs() / 60));
            let out = otto(&["changes", "approve", &reference]);
            checks.check(out.status.success(), "approved", format!("approve failed: {}", String::from_utf8_lossy(&out.stderr)));
            let d = change(&run).unwrap_or(Value::Null)["decision"].clone();
            checks.check(d["mode"] == "push" && d["process"] == want_process.as_str(), "approved to push, as its process says", format!("the decision: {d}"));
            let landed = poke_until(land_timeout, || change(&run).is_some_and(|c| c["state"] == "landed" && !c["cleaned"].is_null()));
            if !landed {
                println!("the change: {}", serde_json::to_string_pretty(&change(&run).unwrap_or(Value::Null)).unwrap_or_default());
                Run::new(lander.clone()).journal_dump();
                checks.bail(&run, "the change didn't land and get cleaned up");
            }
            let sha = change(&run).unwrap_or(Value::Null)["landed"]["sha"].as_str().unwrap_or("").to_string();
            checks.ok(format!("landed at {}", &sha[..8.min(sha.len())]));
            checks.check(stdout(&git(&origin, &["show", "main:greet.py"])).contains("Goodbye"), "origin/main has the farewell", "origin/main has no farewell");

            checks.step("7 - told it landed, the run finishes");
            let finished = poke_until(land_timeout, || run.is_over());
            checks.check(finished && run.status() == "done", "status=done", format!("status={}", run.status()));
            let landed_sha = run.field_str("facts.landedSha");
            checks.check(!sha.is_empty() && landed_sha == sha, "facts.landedSha is the commit on main", format!("facts.landedSha={landed_sha}, landed at {sha}"));
            check_todo(&mut checks, &run, &[("submit", "x"), ("review", "x"), ("cleanup", "x")]);
        }
        Decide::Drop => {
            checks.step("6 - drop it");
            let out = otto(&["changes", "drop", &reference, "--text", "Not needed after all — we're keeping greetings one-way."]);
            checks.check(out.status.success(), "dropped", format!("drop failed: {}", String::from_utf8_lossy(&out.stderr)));
            // A drop is heard at the run's next wake; its day-long timer stands in for that here.
            let out = otto(&["wake", &run_id]);
            checks.check(out.status.success(), "woke the run, as its timer would", format!("otto wake failed: {}", String::from_utf8_lossy(&out.stderr)));

            checks.step("7 - the run finishes without it, and the lander cleans up after it");
            let finished = poke_until(land_timeout, || run.is_over() && change(&run).is_some_and(|c| !c["cleaned"].is_null()));
            checks.check(finished && run.status() == "done", "status=done", format!("status={}", run.status()));
            checks.check(change(&run).is_some_and(|c| c["state"] == "dropped"), "the change stays dropped", "the change isn't dropped");
            checks.check(run.field_str("facts.landedSha").is_empty(), "nothing recorded as landed", "a landedSha was recorded for a dropped change");
            checks.check(!on_origin(&branch_ref), "still nothing on origin", "something reached origin anyway");
            checks.check(stdout(&git(&origin, &["rev-parse", "main"])) == seed, "origin/main untouched", "origin/main moved");
            check_todo(&mut checks, &run, &[("submit", "x"), ("review", "-"), ("cleanup", "x")]);
        }
    }

    checks.step("8 - nothing left behind");
    // A real run once went straight from merging to `done`, skipping its cleanup unmentioned. Both
    // contract checks passed: `done` is a legal stopping state and the handoff was written. Nothing
    // generic can verify that a declared phase's durableOutput exists (DESIGN.md §19.1), so these
    // assertions are what stands between "landed" and "finished".
    checks.check(is_nonempty_file(&run.artifact("summary.md")), "artifacts/summary.md", "no summary — cleanup did not run");
    checks.check(!repo_is_locked(&repo), "no repo lock held", "a repo lock outlived the run");
    let worktrees = stdout(&git(&repo, &["worktree", "list"]));
    checks.check(!worktrees.contains(&branch), "the lander reaped the worktree", format!("worktrees left: {worktrees}"));
    checks.check(await_idle(&run, Duration::from_secs(300)), "the finishing wake has exited", "a wake is still running at done");
    check_nothing_left_open(&mut checks, &run);

    let failed = checks.fail_count() > 0;
    if failed {
        scratch.keep();
    }
    leave_home(failed);
    finish_run(checks, &run, "done", None, None);
}

/// `dev-flow`'s phases, in order: its todo has one item per phase, each starting with the name.
const PHASES: [&str; 8] = ["intake", "prepare", "plan", "implement", "verify", "submit", "review", "cleanup"];

/// The mark on a phase's todo item — the first item whose text starts with the phase's name — as
/// the file has it: `x`, ` `, `~`, `-` or `!`.
fn phase_mark(todo: &str, phase: &str) -> Option<char> {
    todo.lines()
        .filter_map(|line| line.strip_prefix("- [").or_else(|| line.strip_prefix("* [")))
        .find(|rest| rest.get(3..).is_some_and(|text| text.trim_start_matches(['`', '*', ' ']).to_lowercase().starts_with(phase)))
        .and_then(|rest| rest.chars().next())
}

/// The todo as a gate (or `done`) finds it: an item for every phase, and each phase named in
/// `want` marked with one of the marks given for it.
fn check_todo(checks: &mut Checks, run: &Run, want: &[(&str, &str)]) {
    let todo = std::fs::read_to_string(run.artifact("todo.md")).unwrap_or_default();
    if todo.trim().is_empty() {
        checks.bad("no artifacts/todo.md — intake should have written it first");
        return;
    }
    for line in todo.lines() {
        checks.note(format!("| {line}"));
    }
    let missing: Vec<&str> = PHASES.iter().copied().filter(|phase| phase_mark(&todo, phase).is_none()).collect();
    checks.check(missing.is_empty(), "the todo has an item for every phase", format!("the todo has no item for {missing:?}"));
    for (phase, marks) in want {
        let mark = phase_mark(&todo, phase);
        checks.check(
            mark.is_some_and(|m| marks.contains(m)),
            format!("{phase} is [{}]", mark.unwrap_or('?')),
            format!("{phase} is {mark:?}, expected [{marks}]"),
        );
    }
}

/// `done`, with every item `[x]` or `[-]`: what otto counts, and what it would flag.
fn check_nothing_left_open(checks: &mut Checks, run: &Run) {
    match run.shown_progress() {
        Some((done, total)) => {
            checks.check(done == total, format!("every item accounted for ({done}/{total})"), format!("{} item(s) left open", total - done))
        }
        None => checks.bad("otto show counts no todo"),
    }
    checks.check(!run.shown_left_open(), "otto show flags nothing left open", "otto show flags items left open at done");
}

fn is_nonempty_file(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file() && m.len() > 0).unwrap_or(false)
}

fn run_make_test(dir: &Path) -> bool {
    std::process::Command::new("make").arg("test").current_dir(dir).output().map(|o| o.status.success()).unwrap_or(false)
}
