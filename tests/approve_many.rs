//! tests/approve_many.rs — approving several changes at once is all or nothing, through the real
//! binary: one refusal leaves every named change in review, whatever order the list is in, and if
//! saving fails partway the error says what is approved. A stub stands in for the model.

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const OTTO: &str = env!("CARGO_BIN_EXE_otto");

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
    lander: String,
    author: String,
}

impl Scratch {
    /// A lander, and an author with three changes in review: 1 (A) has a v2, 2 (B) and 3 (C) don't.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let stub = root.join("stub");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nprintf '# h\\n## Goal\\ng\\n## Where this is\\nHere.\\n## Decided\\nn\\n## This wake did\\nn\\n## Next wake must\\nn\\n## Needs\\nnothing\\n## Don'\"'\"'t re-derive\\nn\\n' | '{OTTO}' state handoff \"$OTTO_RUN_ID\" --stdin >/dev/null\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Named `claude` too, so nothing here can ever reach the real one.
        std::fs::write(
            root.join("home/config.json"),
            serde_json::json!({"launchers": [{"name": "claude", "command": stub.display().to_string()}]}).to_string(),
        )
        .unwrap();
        std::fs::write(root.join("summary.md"), "What it does.\n").unwrap();
        let mut s = Scratch { _dir: dir, root, lander: String::new(), author: String::new() };
        s.init_repo("repo");
        s.ok(&["config", "project", s.root.join("repo").to_str().unwrap(), "--process", "direct"]);
        s.lander = s.run("lander", "repo");
        s.ok(&["config", "lander", &s.lander.clone()]);
        s.author = s.run("author", "repo");
        let author = s.author.clone();
        s.submit(&author, "repo", "a", "A");
        s.commit("repo", "a", "again");
        s.submit(&author, "repo", "a", "A");
        s.submit(&author, "repo", "b", "B");
        s.submit(&author, "repo", "c", "C");
        s
    }

    fn init_repo(&self, name: &str) {
        let repo = self.root.join(name);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    }

    fn otto(&self, args: &[&str]) -> Output {
        Command::new(OTTO)
            .args(args)
            .current_dir(self.root.join("repo"))
            .env("OTTO_HOME", self.root.join("home"))
            .env("TMUX_TMPDIR", &self.root)
            .env("OTTO_BIN", OTTO)
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT")
            .output()
            .unwrap_or_else(|e| panic!("could not run otto {args:?}: {e}"))
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.otto(args);
        assert!(output.status.success(), "otto {args:?}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    /// Refused with exit 2 or 1; what it said.
    fn refused(&self, args: &[&str], code: i32) -> String {
        let output = self.otto(args);
        assert_eq!(output.status.code(), Some(code), "otto {args:?}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stderr).to_string()
    }

    fn run(&self, name: &str, repo: &str) -> String {
        let repo = self.root.join(repo);
        let repo = repo.to_str().unwrap();
        self.ok(&["run", "--goal", "g", "--name", name, "--perpetual", "--period", "1h", "--detach", "none", "--repo", repo, "--workdir", repo]);
        let list: Value = serde_json::from_str(&self.ok(&["state", "list", "--json"])).unwrap();
        list.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap()["id"].as_str().unwrap().to_string()
    }

    /// A commit on this branch, made if it's new, with the main branch checked out again after.
    fn commit(&self, repo: &str, branch: &str, what: &str) {
        let repo = self.root.join(repo);
        if git(&repo, &["branch", "--list", branch]).is_empty() {
            git(&repo, &["checkout", "-q", "-b", branch, "main"]);
        } else {
            git(&repo, &["checkout", "-q", branch]);
        }
        std::fs::write(repo.join(branch), what).unwrap();
        git(&repo, &["add", branch]);
        git(&repo, &["commit", "-q", "-m", what]);
        git(&repo, &["checkout", "-q", "main"]);
    }

    /// The branch's next version, with a commit first if it has none yet.
    fn submit(&self, run: &str, repo: &str, branch: &str, title: &str) {
        let path = self.root.join(repo);
        if git(&path, &["branch", "--list", branch]).is_empty() {
            self.commit(repo, branch, "first");
        }
        let summary = self.root.join("summary.md");
        self.ok(&[
            "state", "change", "submit", run, "--repo", path.to_str().unwrap(), "--branch", branch, "--title", title,
            "--summary-file", summary.to_str().unwrap(), "--tests", "none", "--no-evidence", "a stub wake",
        ]);
    }

    fn index(&self, run: &str) -> Value {
        let text = std::fs::read_to_string(self.root.join("home/runs").join(run).join("changes/index.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn state_and_decision(&self, run: &str, n: u64) -> (String, bool) {
        let index = self.index(run);
        let c = index["changes"].as_array().unwrap().iter().find(|c| c["n"] == n).unwrap().clone();
        (c["state"].as_str().unwrap().to_string(), !c["decision"].is_null())
    }

    /// `otto changes --due`: how many wait on the lander.
    fn due(&self) -> usize {
        let due: Value = serde_json::from_str(&self.ok(&["changes", "--due"])).unwrap();
        due.as_array().unwrap().len()
    }

    /// Every one of the author's changes is waiting for review with no decision, and nothing is due.
    fn all_in_review(&self, said: &str) {
        for n in 1..=3 {
            assert_eq!(self.state_and_decision(&self.author, n), ("review".to_string(), false), "change {n} after: {said}");
        }
        assert_eq!(self.due(), 0, "--due after: {said}");
    }

    fn a(&self, version: &str) -> String {
        format!("{}/1@{version}", self.author)
    }
    fn b(&self) -> String {
        format!("{}/2@v1", self.author)
    }
    fn c(&self) -> String {
        format!("{}/3@v1", self.author)
    }

    fn lander_wakes_for_approval(&self) -> usize {
        let path = self.root.join("home/runs").join(&self.lander).join("journal.jsonl");
        let journal = std::fs::read_to_string(path).unwrap();
        journal.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).filter(|l| l["event"] == "wake-started" && l["cause"] == "change-approved").count()
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).current_dir(dir).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Step 1 of the report, in both orders: A@v1 is stale. It says nothing new is approved, and that's true.
#[test]
fn a_stale_version_anywhere_in_the_list_approves_nothing() {
    let s = Scratch::new();
    for list in [[s.b(), s.a("v1")], [s.a("v1"), s.b()]] {
        let said = s.refused(&["changes", "approve", &list[0], &list[1]], 2);
        assert!(said.contains("v2 of") && said.contains("Nothing new is approved"), "{said}");
        s.all_in_review(&said);
    }
}

/// Step 2: after a refusal, the corrected list approves everything it names, and wakes the lander once.
#[test]
fn the_corrected_retry_approves_all_of_it_and_wakes_the_lander_once() {
    let s = Scratch::new();
    s.refused(&["changes", "approve", &s.b(), &s.a("v1")], 2);
    assert_eq!(s.lander_wakes_for_approval(), 0, "a refusal starts nothing");
    let before = s.lander_wakes_for_approval();
    s.ok(&["changes", "approve", &s.b(), &s.a("v2")]);
    for n in [1, 2] {
        assert_eq!(s.state_and_decision(&s.author, n).1, true, "change {n} is decided");
    }
    assert_eq!(s.state_and_decision(&s.author, 3), ("review".to_string(), false), "C wasn't named");
    let until = Instant::now() + Duration::from_secs(30);
    while s.lander_wakes_for_approval() == before {
        assert!(Instant::now() < until, "the lander never woke");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(s.lander_wakes_for_approval(), before + 1, "once, for both");
}

/// Step 3: one that can't be approved leaves the rest in review wherever it is in the list, and
/// names its full reference.
#[test]
fn one_not_in_review_leaves_the_rest_in_review_in_either_order_and_is_named_in_full() {
    let s = Scratch::new();
    s.ok(&["changes", "approve", &s.c(), "--no-wake"]);
    let c = format!("{}/3", s.author);
    for list in [[s.b(), s.c()], [s.c(), s.b()]] {
        let said = s.refused(&["changes", "approve", &list[0], &list[1], "--no-wake"], 2);
        assert!(said.contains(&format!("{c} can't be approved")) && said.contains("Nothing new is approved"), "{said}");
        assert_eq!(s.state_and_decision(&s.author, 2), ("review".to_string(), false), "B after: {said}");
        assert_eq!(s.due(), 1, "only C, from before: {said}");
    }
}

/// Every other refusal: no version named, one that doesn't exist, a change or a run that doesn't
/// exist, a change named twice.
#[test]
fn every_refusal_in_the_checks_approves_nothing() {
    let s = Scratch::new();
    let refusals: [(String, &str, i32); 4] = [
        (format!("{}/1", s.author), "its latest is v2", 1),
        (s.a("v9"), "no v9", 1),
        (format!("{}/99@v1", s.author), "no change 99", 3),
        (s.b(), "named twice", 1),
    ];
    for (bad, expect, code) in &refusals {
        let said = s.refused(&["changes", "approve", &s.b(), bad, "--no-wake"], *code);
        assert!(said.contains(expect) && said.contains("Nothing new is approved"), "{bad}: {said}");
        s.all_in_review(&said);
    }
    // Refused as the list is read, before any change is looked at.
    let said = s.refused(&["changes", "approve", &s.b(), "no-such-run/1@v1", "--no-wake"], 3);
    assert!(said.contains("no such run"), "{said}");
    s.all_in_review(&said);
}

/// `--pr` where one of the repositories has no remote: the one with a remote named first isn't approved.
#[test]
fn a_pull_request_with_nowhere_to_go_approves_nothing() {
    let s = Scratch::new();
    s.init_repo("with-remote");
    let path = s.root.join("with-remote");
    git(&path, &["remote", "add", "origin", path.to_str().unwrap()]);
    let other = s.run("other", "with-remote");
    s.submit(&other, "with-remote", "d", "D");
    let d = format!("{other}/1@v1");
    let said = s.refused(&["changes", "approve", &d, &s.b(), "--pr", "--no-wake"], 1);
    assert!(said.contains("no remote") && said.contains("Nothing new is approved"), "{said}");
    assert_eq!(s.state_and_decision(&other, 1), ("review".to_string(), false), "{said}");
    s.all_in_review(&said);
}

/// Saving fails after the first is written: what is approved stands, so the error names it, and it
/// mustn't say nothing new is approved.
#[test]
fn a_failure_while_saving_names_what_is_approved() {
    let s = Scratch::new();
    let other = s.run("other", "repo");
    s.submit(&other, "repo", "d", "D");
    let changes = s.root.join("home/runs").join(&other).join("changes");
    let keep = std::fs::metadata(&changes).unwrap().permissions();
    std::fs::set_permissions(&changes, std::fs::Permissions::from_mode(0o555)).unwrap();
    let output = s.otto(&["changes", "approve", &s.b(), &format!("{other}/1@v1"), "--no-wake"]);
    std::fs::set_permissions(&changes, keep).unwrap();

    let said = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!output.status.success(), "{said}");
    assert!(said.contains(&format!("{}/2 approved", s.author)) && said.contains("the lander lands"), "{said}");
    assert!(!said.to_lowercase().contains("nothing new is approved"), "{said}");
    assert_eq!(s.state_and_decision(&s.author, 2), ("approved".to_string(), true));
    assert_eq!(s.state_and_decision(&other, 1), ("review".to_string(), false));
    assert_eq!(s.due(), 1);
}

/// As they were: one, and a list with nothing wrong.
#[test]
fn a_single_approve_and_an_all_valid_list_still_approve() {
    let s = Scratch::new();
    assert!(s.ok(&["changes", "approve", &s.a("v2"), "--no-wake"]).contains("approved"));
    assert_eq!(s.due(), 1);
    let out = s.ok(&["changes", "approve", &s.b(), &s.c(), "--no-wake"]);
    assert_eq!(out.lines().filter(|l| l.contains(" approved ")).count(), 2, "{out}");
    assert_eq!(s.due(), 3);
}
