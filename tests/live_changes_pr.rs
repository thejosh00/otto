//! tests/live_changes_pr.rs — prove a change approved as a pull request is kept moving, against a
//! real model and a stand-in for GitHub.
//!
//! What `cargo test` cannot reach: whether the lander, wrapping `workflows/land-flow.md`, opens the
//! pull request; whether the babysitter otto makes, wrapping `workflows/babysit-flow.md`, reads a
//! reviewer's ask off it and hands it to the change's author rather than fixing it itself; whether
//! the author makes the change and hands it back, and the lander pushes that to the same pull
//! request without asking the person; whether an approved, passing pull request comes back to the
//! person to merge, and the lander merges it when they say.
//!
//! GitHub is `test/fixtures/gh-stub.py`, first on `PATH`: pull requests on the scratch repo's local
//! bare remote, which the test edits to play the reviewer. Everything runs in an `OTTO_HOME` of its
//! own. Eight wakes or so, ~20 minutes.
//!
//!   cargo test --test live_changes_pr -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::path::Path;
use std::time::{Duration, Instant};
use support::*;

fn env_secs(key: &str, default: u64) -> Duration {
    Duration::from_secs(std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default))
}

fn change(run: &Run) -> Value {
    std::fs::read_to_string(run.dir.join("changes/index.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|i| i["changes"].as_array().and_then(|a| a.first().cloned()))
        .unwrap_or(Value::Null)
}

/// What every run is doing, for a failure to explain itself — a wake held back by the usage limit
/// reads differently from one that did the wrong thing.
fn say_where_runs_are() {
    println!("{}", stdout(&otto(&["ls"])));
}

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

/// The stand-in's state: its pull requests and every `gh` call it was given.
struct Hub {
    file: std::path::PathBuf,
}

impl Hub {
    fn read(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(&self.file).unwrap()).unwrap()
    }

    /// Play the reviewer: change the first pull request, and say it changed.
    fn update(&self, f: impl FnOnce(&mut Value)) {
        let mut state = self.read();
        if let Some(pr) = state["prs"].get_mut(0) {
            f(pr);
            pr["updatedAt"] = Value::String(time_now());
        }
        std::fs::write(&self.file, serde_json::to_string_pretty(&state).unwrap()).unwrap();
    }

    fn calls(&self, starting: &str) -> usize {
        self.read()["log"].as_array().map(|l| l.iter().filter(|c| c.as_str().is_some_and(|c| c.starts_with(starting))).count()).unwrap_or(0)
    }
}

fn time_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let out = std::process::Command::new("date").args(["-u", "-r", &secs.to_string(), "+%Y-%m-%dT%H:%M:%SZ"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_changes_pr() {
    let step_timeout = env_secs("OTTO_LIVE_CHANGES_PR_STEP_TIMEOUT", 1200);
    let mut checks = Checks::new();

    checks.step("0 - preconditions, a home of its own, and a GitHub that isn't");
    require_launcher(&mut checks);
    let mut scratch = ScratchRepo::new(&workspace_dir(), "changes-pr");
    // Kept unless everything passes: a run that went wrong is worth reading, and `bail` panics.
    scratch.keep();
    let home = scratch.path().join("otto-home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("OTTO_HOME", &home);
    let bin = scratch.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = repo_path("test/fixtures/gh-stub.py");
    write_file(&bin.join("gh"), &format!("#!/bin/sh\nexec /usr/bin/python3 -I {} \"$@\"\n", stub.display()));
    std::process::Command::new("chmod").args(["+x", bin.join("gh").to_str().unwrap()]).status().unwrap();
    let path = std::env::var("PATH").unwrap_or_default();
    // After this build of otto, which `require_launcher` put first.
    let (first, rest) = path.split_once(':').unwrap_or((&path, ""));
    std::env::set_var("PATH", format!("{first}:{}:{rest}", bin.display()));
    let origin = scratch.origin_path();
    let hub = Hub { file: scratch.path().join("gh-state.json") };
    std::fs::write(&hub.file, serde_json::json!({ "repo": "test/greeter", "remote": origin, "prs": [], "log": [] }).to_string()).unwrap();
    std::env::set_var("GH_STUB_STATE", &hub.file);
    checks.ok(format!("OTTO_HOME={}, gh is the stand-in", home.display()));

    checks.step("1 - scratch repo whose remote says github.com");
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
    write_file(&repo.join("CLAUDE.md"), "# greeter\n\nChanges land as pull requests on github.com/test/greeter, squash-merged. Tests: `make test`.\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "-m", "greeter: hello"]);
    let github = "https://github.com/test/greeter.git";
    git_ok(&repo, &["config", &format!("url.{}.insteadOf", origin.display()), github]);
    git_ok(&repo, &["remote", "add", "origin", github]);
    git_ok(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git_ok(&repo, &["remote", "set-head", "origin", "main"]);
    let workdir = scratch.path().to_str().unwrap().to_string();

    checks.step("2 - a lander of its own, and otto's babysitter, started");
    otto(&["config", "workdir", &workdir]);
    let land_flow = repo_path("workflows/land-flow.md");
    let out = otto(&[
        "state", "init", "--name", "lander", "--perpetual", "--period", "10m",
        "--instructions", land_flow.to_str().unwrap(), "--repo", &workdir, "--workdir", &workdir,
        "--goal", "Land the changes I approve, and clean up after them",
    ]);
    checks.check(out.status.success(), "the lander exists", "state init failed");
    let lander = Run::new(stdout(&out).lines().last().unwrap_or("1").to_string());
    otto(&["config", "lander", "lander"]);
    let check = scratch.path().join("changes-due.sh");
    write_file(&check, &format!("#!/bin/sh\nif [ \"$({} changes --due --json)\" = \"[]\" ]; then echo nothing; exit 0; fi\necho due; exit 1\n", otto_bin().display()));
    std::process::Command::new("chmod").args(["+x", check.to_str().unwrap()]).status().unwrap();
    let out = otto(&["check", "lander", "--script", check.to_str().unwrap(), "--wake-after", "0"]);
    checks.check(out.status.success(), "its check script is set", format!("otto check failed: {}", String::from_utf8_lossy(&out.stderr)));
    let made = stdout(&otto(&["poke"]));
    checks.check(made.contains("made the babysitter"), "otto made its babysitter", format!("poke said: {made}"));
    checks.check(made.contains("is the lander"), "and adopted the lander named by hand", format!("poke said: {made}"));
    let out = otto(&["resume", "babysitter"]);
    checks.check(out.status.success(), "the babysitter is started", format!("resume failed: {}", String::from_utf8_lossy(&out.stderr)));
    let babysitter = Run::new(stdout(&otto(&["state", "get", "babysitter", "--field", "id"])).trim().trim_matches('"').to_string());

    checks.step(&format!("3 - an author hands its change back (up to {}min)", step_timeout.as_secs() / 60));
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
            "--instructions", fixture.to_str().unwrap(), "--repo", repo.to_str().unwrap(), "--workdir", &workdir,
            "--until", "the farewell change has landed on main, and artifacts/after-landing.md names the commit it landed at",
            "--policy", "verifyDone=false",
        ],
    );
    let start = Instant::now();
    while author.events("change-submitted") == 0 && start.elapsed() < step_timeout && !author.is_over() {
        std::thread::sleep(Duration::from_secs(10));
    }
    if author.events("change-submitted") == 0 {
        checks.bail(&author, "no change was submitted");
    }
    checks.check(await_idle(&author, Duration::from_secs(300)), "its wake has exited", "its wake is still running");
    let branch = change(&author)["branch"].as_str().unwrap_or("").to_string();

    checks.step(&format!("4 - approved as a pull request: the lander opens it (up to {}min)", step_timeout.as_secs() / 60));
    let out = otto(&["changes", "approve", "author/1", "--pr"]);
    checks.check(out.status.success(), "approved as a pull request", format!("approve failed: {}", String::from_utf8_lossy(&out.stderr)));
    if !poke_until(step_timeout, || change(&author)["state"] == "pr-open") {
        say_where_runs_are();
        checks.bail(&lander, &format!("no pull request was opened; the change is {}", change(&author)["state"]));
    }
    let c = change(&author);
    // Counted on the hub, not by calls: a `pr create` that failed (its branch not pushed yet) and
    // was run again is fine; two pull requests for one change is not.
    let prs = hub.read()["prs"].as_array().map(Vec::len).unwrap_or(0);
    checks.check(prs == 1, "it opened one pull request", format!("{prs} pull requests for one change"));
    checks.check(hub.read()["prs"][0]["head"] == branch.as_str(), format!("from {branch}"), "from another branch");
    checks.check(c["pr"]["url"] == "https://github.com/test/greeter/pull/1", "the change knows its pull request", format!("pr: {}", c["pr"]));
    let main_before = stdout(&git(&origin, &["rev-parse", "main"]));

    checks.step(&format!("5 - a reviewer asks for something; it reaches the author, who hands back v2 (up to {}min)", step_timeout.as_secs() / 60));
    hub.update(|pr| {
        pr["ci"] = "SUCCESS".into();
        pr["reviewDecision"] = "CHANGES_REQUESTED".into();
        pr["reviews"] = serde_json::json!([{ "id": 501, "login": "sam", "state": "CHANGES_REQUESTED", "body": "One thing.", "at": time_now() }]);
        pr["review_comments"] = serde_json::json!([{
            "id": 9001, "path": "greet.py", "line": 10, "user": { "login": "sam" }, "created_at": time_now(),
            "body": "Please make farewell's docstring read exactly: Wave goodbye to `name`."
        }]);
    });
    let pushed_v2 = poke_until(step_timeout * 2, || {
        let c = change(&author);
        c["versions"].as_array().is_some_and(|v| v.len() >= 2) && c["state"] == "pr-open" && c["pr"]["head"] == c["versions"][1]["sha"]
    });
    let c = change(&author);
    let relayed = c["timeline"].as_array().map(|t| t.iter().any(|m| m["by"] == "reviewer" && m["what"].as_str().is_some_and(|w| w.to_lowercase().contains("goodbye")))).unwrap_or(false);
    checks.check(relayed, "the babysitter relayed sam's ask", "nothing on the change says sam's ask was relayed");
    if !pushed_v2 {
        babysitter.journal_dump();
        lander.journal_dump();
        say_where_runs_are();
        checks.bail(&author, &format!("v2 didn't reach the pull request; the change is {}", c["state"]));
    }
    let v2 = c["versions"][1]["sha"].as_str().unwrap_or("").to_string();
    let remote_branch = stdout(&git(&origin, &["rev-parse", &format!("refs/heads/{branch}")]));
    checks.check(remote_branch == v2, "the pull request's branch has v2", format!("its branch is at {remote_branch}, v2 is {v2}"));
    let v2_patch = c["versions"][1]["patch"].as_str().map(|p| std::fs::read_to_string(author.dir.join(p)).unwrap_or_default()).unwrap_or_default();
    checks.check(v2_patch.contains("Wave goodbye to"), "v2 does what sam asked", "v2's patch doesn't have the docstring sam asked for");
    checks.check(stdout(&git(&origin, &["rev-parse", "main"])) == main_before, "nothing reached main meanwhile", "main moved before the merge");
    checks.check(hub.read()["prs"].as_array().map(Vec::len) == Some(1), "and no second pull request", "a second pull request was opened");

    checks.step(&format!("6 - approved and passing: it comes back to the person to merge (up to {}min)", step_timeout.as_secs() / 60));
    hub.update(|pr| {
        pr["reviewDecision"] = "APPROVED".into();
        pr["ci"] = "SUCCESS".into();
        pr["reviews"].as_array_mut().unwrap().push(serde_json::json!({ "id": 502, "login": "sam", "state": "APPROVED", "body": "Thanks!", "at": time_now() }));
    });
    if !poke_until(step_timeout, || change(&author)["state"] == "ready-to-merge") {
        babysitter.journal_dump();
        checks.bail(&author, &format!("it never came back to merge; the change is {}", change(&author)["state"]));
    }
    checks.ok("ready to merge, and waiting on the person");
    checks.check(hub.calls("pr merge") == 0, "nobody merged it without asking", "it was merged before the person said");

    checks.step(&format!("7 - the person says merge it; the lander merges and cleans up (up to {}min)", step_timeout.as_secs() / 60));
    let out = otto(&["changes", "merge", "author/1"]);
    checks.check(out.status.success(), "merge asked for", format!("merge failed: {}", String::from_utf8_lossy(&out.stderr)));
    if !poke_until(step_timeout, || change(&author)["state"] == "merged" && !change(&author)["cleaned"].is_null()) {
        lander.journal_dump();
        checks.bail(&author, &format!("it didn't merge and clean up; the change is {}", change(&author)["state"]));
    }
    let greet = stdout(&git(&origin, &["show", "main:greet.py"]));
    checks.check(greet.contains("Wave goodbye to"), "main has v2, docstring and all", "main doesn't have what sam asked for");
    checks.check(hub.read()["prs"][0]["state"] == "MERGED", "the pull request is merged", "the pull request isn't merged");
    let worktrees = stdout(&git(&repo, &["worktree", "list"]));
    checks.check(worktrees.lines().count() == 1, "only the main checkout is left", format!("worktrees left: {worktrees}"));
    checks.check(locks_held() == 0, "no repo lock is held", "a repo lock is still held");

    checks.step("8 - the author is told, and does what follows");
    let finished = poke_until(Duration::from_secs(900), || author.is_over());
    checks.check(finished && author.status() == "done", "the author finished", format!("the author is {}", author.status()));
    let sha = change(&author)["landed"]["sha"].as_str().unwrap_or("").to_string();
    let after = std::fs::read_to_string(author.artifact("after-landing.md")).unwrap_or_default();
    checks.check(!sha.is_empty() && after.contains(&sha[..7]), format!("after-landing.md names {}", &sha[..7.min(sha.len())]), format!("after-landing.md says {after:?}"));
    println!("gh calls: {}", hub.read()["log"]);

    let _ = otto(&["stop", "lander", "--reason", "live test finished"]);
    let _ = otto(&["stop", "babysitter", "--reason", "live test finished"]);
    // Only once no wake is still writing into the scratch home: the author's last one may be
    // finishing its handoff, and removing the directory under it leaves half of it behind.
    if checks.fail_count() == 0 && await_idle(&author, Duration::from_secs(180)) && await_idle(&lander, Duration::from_secs(180)) && await_idle(&babysitter, Duration::from_secs(180)) {
        let _ = std::fs::remove_dir_all(scratch.path());
    }
    let _ = Path::new(&workdir);
    finish_run(checks, &author, "done", None, None);
}
