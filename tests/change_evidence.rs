//! tests/change_evidence.rs — `otto state change submit` won't take a new commit with neither
//! `--evidence` nor `--no-evidence`, through the real binary: what a wake types, what it is told,
//! the exit code, and what is left on disk. No tmux, no model.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
    run: String,
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

impl Scratch {
    /// A run, and a repo with a commit on `fix` to hand back.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&repo, &["checkout", "-q", "-b", "fix"]);
        std::fs::write(root.join("summary.md"), "What it does.\n").unwrap();
        std::fs::write(root.join("after.txt"), "1 passed\n").unwrap();
        let mut s = Scratch { _dir: dir, root, run: String::new() };
        s.commit("first");
        git(&repo, &["checkout", "-q", "main"]);
        let out = s.otto(&["state", "init", "--name", "author", "--perpetual", "--period", "1h", "--goal", "g", "--repo", repo.to_str().unwrap(), "--workdir", repo.to_str().unwrap()]);
        assert!(out.status.success(), "state init: {}", String::from_utf8_lossy(&out.stderr));
        s.run = "author".to_string();
        s
    }

    fn commit(&self, what: &str) {
        let repo = self.root.join("repo");
        git(&repo, &["checkout", "-q", "fix"]);
        std::fs::write(repo.join("f"), what).unwrap();
        git(&repo, &["add", "f"]);
        git(&repo, &["commit", "-q", "-m", what]);
        git(&repo, &["checkout", "-q", "main"]);
    }

    fn otto(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_otto"))
            .args(args)
            .current_dir(&self.root)
            .env("OTTO_HOME", self.root.join("home"))
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .output()
            .unwrap()
    }

    fn submit(&self, flags: &[&str]) -> Output {
        let (repo, summary) = (self.root.join("repo"), self.root.join("summary.md"));
        let mut args = vec!["state", "change", "submit", &self.run, "--repo", repo.to_str().unwrap(), "--branch", "fix", "--base", "main", "--title", "A fix", "--summary-file", summary.to_str().unwrap()];
        args.extend_from_slice(flags);
        self.otto(&args)
    }

    fn evidence(&self) -> String {
        self.root.join("after.txt").display().to_string()
    }

    fn run_dir(&self) -> PathBuf {
        let runs = self.root.join("home/runs");
        std::fs::read_dir(&runs).unwrap().map(|e| e.unwrap().path()).find(|p| p.is_dir()).expect("a run")
    }

    /// Every file in the run's directory and what it holds: "nothing written" means this is the same.
    fn on_disk(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    out.insert(p.clone(), Vec::new());
                    walk(&p, out);
                } else {
                    out.insert(p.clone(), std::fs::read(&p).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.run_dir(), &mut out);
        out
    }

    fn versions(&self) -> Vec<Value> {
        let Ok(text) = std::fs::read_to_string(self.run_dir().join("changes/index.json")) else { return Vec::new() };
        let index: Value = serde_json::from_str(&text).unwrap();
        index["changes"].as_array().and_then(|c| c.first()).and_then(|c| c["versions"].as_array().cloned()).unwrap_or_default()
    }
}

fn said(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// Refused as a usage error, with nothing in the run's directory changed.
fn assert_refused(s: &Scratch, flags: &[&str]) -> String {
    let before = s.on_disk();
    let out = s.submit(flags);
    assert_eq!(out.status.code(), Some(1), "{flags:?} should be a usage error: {}", said(&out));
    assert!(s.on_disk() == before, "{flags:?} wrote something in the run's directory");
    said(&out)
}

#[test]
fn a_new_commit_with_neither_flag_is_refused_with_nothing_written() {
    let s = Scratch::new();
    let message = assert_refused(&s, &[]);
    assert!(message.contains("--evidence") && message.contains("--no-evidence"), "it says what to do, both ways: {message}");
    assert!(s.versions().is_empty(), "no change was created");
    assert!(!s.run_dir().join("changes").exists(), "no change directory");
    let journal = std::fs::read_to_string(s.run_dir().join("journal.jsonl")).unwrap_or_default();
    assert!(!journal.contains("change-submitted"), "{journal}");
}

#[test]
fn a_next_version_with_neither_flag_is_refused_and_the_change_stays_at_its_last() {
    let s = Scratch::new();
    assert!(s.submit(&["--evidence", &s.evidence()]).status.success());
    s.commit("second");
    assert_refused(&s, &[]);
    assert_eq!(s.versions().len(), 1, "no v2");
}

#[test]
fn a_reason_is_accepted_and_kept_on_the_version() {
    let s = Scratch::new();
    let out = s.submit(&["--no-evidence", "plan only, no code"]);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(s.versions()[0]["noEvidence"], "plan only, no code");
    let shown = s.otto(&["changes", "show", "author/1"]);
    assert!(said(&shown).contains("no evidence: plan only, no code"), "{}", said(&shown));
}

#[test]
fn both_flags_together_are_refused() {
    let s = Scratch::new();
    let message = assert_refused(&s, &["--evidence", &s.evidence(), "--no-evidence", "why"]);
    assert!(message.contains("not both"), "{message}");
}

#[test]
fn an_empty_reason_is_refused() {
    let s = Scratch::new();
    for empty in ["", "   "] {
        let message = assert_refused(&s, &["--no-evidence", empty]);
        assert!(message.contains("needs its reason"), "{message}");
    }
}

#[test]
fn a_reason_on_the_same_commit_changes_nothing_and_says_so() {
    let s = Scratch::new();
    assert!(s.submit(&["--no-evidence", "first reason"]).status.success());
    let out = s.submit(&["--no-evidence", "another reason"]);
    assert!(out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("kept as it was"), "{}", said(&out));
    assert_eq!(s.versions().len(), 1);
    assert_eq!(s.versions()[0]["noEvidence"], "first reason");
}

// The next two are guards, and pass on the base commit too: what must not change.

#[test]
fn evidence_alone_still_succeeds() {
    let s = Scratch::new();
    let out = s.submit(&["--evidence", &s.evidence()]);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(s.versions()[0]["evidence"][0]["name"], "after.txt");
}

#[test]
fn the_same_commit_again_with_neither_flag_still_works() {
    let s = Scratch::new();
    assert!(s.submit(&["--evidence", &s.evidence()]).status.success());
    let out = s.submit(&[]);
    assert!(out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("unchanged"), "{}", said(&out));
    assert_eq!(s.versions().len(), 1, "no new version");
    assert_eq!(s.versions()[0]["evidence"].as_array().map(Vec::len), Some(1), "its evidence kept");
}
