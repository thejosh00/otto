//! tests/submit_base.rs — what `otto state change submit` measures a change against when the
//! checkout's `origin/<base>` lags the base branch (otto's landing pushes to the remote's URL, which
//! moves the remote and not the tracking ref), and what the same commit needs when only the base
//! moved. Through the real binary; a bare repository stands in for the remote. No tmux, no model.

mod support;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
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
    /// A remote and a clone of it at `init`, and a run.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let s = Scratch { _dir: dir, root };
        let (repo, origin) = (s.repo(), s.origin());
        std::fs::create_dir_all(s.root.join("home")).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        git(&s.root, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&repo, &["push", "-q", "-u", "origin", "main"]);
        std::fs::write(s.root.join("summary.md"), "What it does.\n").unwrap();
        std::fs::write(s.root.join("after.txt"), "1 passed\n").unwrap();
        let out = s.otto(&["state", "init", "--name", "author", "--perpetual", "--period", "1h", "--goal", "g", "--repo", repo.to_str().unwrap(), "--workdir", repo.to_str().unwrap()]);
        assert!(out.status.success(), "state init: {}", String::from_utf8_lossy(&out.stderr));
        s
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn origin(&self) -> PathBuf {
        self.root.join("origin.git")
    }

    /// A commit of `lines` lines in `file`, on whatever is checked out.
    fn commit(&self, file: &str, lines: usize) {
        std::fs::write(self.repo().join(file), "x\n".repeat(lines)).unwrap();
        git(&self.repo(), &["add", file]);
        git(&self.repo(), &["commit", "-q", "-m", file]);
    }

    /// As otto's landing does it: to the remote's URL, so `origin/main` here stays where it was.
    fn land(&self, what: &str) {
        git(&self.repo(), &["push", "-q", self.origin().to_str().unwrap(), &format!("{what}:refs/heads/main")]);
    }

    fn otto(&self, args: &[&str]) -> Output {
        support::otto_command().args(args).current_dir(&self.root).env("OTTO_HOME", self.root.join("home")).env_remove("OTTO_RUN_ID").env_remove("OTTO_RUN_DIR").output().unwrap()
    }

    fn submit(&self, flags: &[&str]) -> Output {
        let (repo, summary) = (self.repo(), self.root.join("summary.md"));
        let mut args = vec!["state", "change", "submit", "author", "--repo", repo.to_str().unwrap(), "--branch", "fix", "--base", "main", "--title", "A fix", "--summary-file", summary.to_str().unwrap()];
        args.extend_from_slice(flags);
        self.otto(&args)
    }

    fn run_dir(&self) -> PathBuf {
        let runs = self.root.join("home/runs");
        std::fs::read_dir(&runs).unwrap().map(|e| e.unwrap().path()).find(|p| p.is_dir()).expect("a run")
    }

    fn versions(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.run_dir().join("changes/index.json")).unwrap();
        let index: Value = serde_json::from_str(&text).unwrap();
        index["changes"][0]["versions"].as_array().cloned().unwrap()
    }
}

fn said(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// (files, lines added) of a version.
fn size(v: &Value) -> (usize, u64) {
    (v["stat"]["files"].as_array().unwrap().len(), v["stat"]["add"].as_u64().unwrap())
}

#[test]
fn a_change_is_measured_against_the_local_base_when_the_tracking_ref_lags_it() {
    let s = Scratch::new();
    // An earlier change landed: on main here and on the remote, and origin/main still before it.
    s.commit("landed.txt", 100);
    s.land("main");
    git(&s.repo(), &["checkout", "-q", "-b", "fix"]);
    s.commit("new.txt", 3);
    git(&s.repo(), &["checkout", "-q", "main"]);
    let out = s.submit(&["--evidence", s.root.join("after.txt").to_str().unwrap()]);
    assert!(out.status.success(), "{}", said(&out));
    let v = s.versions();
    assert_eq!(size(&v[0]), (1, 3), "only what is new, not what already landed: {}", v[0]["stat"]);
    assert_eq!(v[0]["commits"], 1);
}

#[test]
fn the_tracking_ref_is_the_base_when_it_is_ahead_of_the_local_branch_or_has_diverged() {
    // Ahead: the branch is cut from origin/main, which a fetch moved past the local main.
    let s = Scratch::new();
    s.commit("landed.txt", 100);
    git(&s.repo(), &["push", "-q", "origin", "main"]);
    git(&s.repo(), &["checkout", "-q", "-b", "fix"]);
    s.commit("new.txt", 3);
    git(&s.repo(), &["checkout", "-q", "main"]);
    git(&s.repo(), &["reset", "-q", "--hard", "HEAD~1"]);
    let out = s.submit(&["--evidence", s.root.join("after.txt").to_str().unwrap()]);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(size(&s.versions()[0]), (1, 3));

    // Diverged: a local main with a commit the remote hasn't, and a remote with one it hasn't.
    let s = Scratch::new();
    s.commit("theirs.txt", 100);
    git(&s.repo(), &["push", "-q", "origin", "main"]);
    git(&s.repo(), &["checkout", "-q", "-b", "fix"]);
    s.commit("new.txt", 3);
    git(&s.repo(), &["checkout", "-q", "main"]);
    git(&s.repo(), &["reset", "-q", "--hard", "HEAD~1"]);
    s.commit("mine.txt", 50);
    let out = s.submit(&["--evidence", s.root.join("after.txt").to_str().unwrap()]);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(size(&s.versions()[0]), (1, 3));
}

#[test]
fn the_same_commit_against_a_base_that_moved_is_a_new_version_that_keeps_its_evidence() {
    let s = Scratch::new();
    // The branch holds a commit that landed on the remote, and neither main nor origin/main here has it yet.
    git(&s.repo(), &["checkout", "-q", "-b", "fix"]);
    s.commit("landed.txt", 100);
    s.land("fix");
    s.commit("new.txt", 3);
    git(&s.repo(), &["checkout", "-q", "main"]);
    let out = s.submit(&["--evidence", s.root.join("after.txt").to_str().unwrap()]);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(size(&s.versions()[0]), (2, 103), "the base this checkout knew of is before the landing");

    git(&s.repo(), &["fetch", "-q", "origin", "main"]);
    // It doesn't stand as it was: the person hasn't read this measure of it. The refusal says so,
    // and not that the commit is new.
    let out = s.submit(&["--stands", "nothing to change"]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).contains("base branch has moved") && !said(&out).contains("A new commit is"), "{}", said(&out));
    assert_eq!(s.versions().len(), 1);
    let out = s.submit(&[]);
    assert!(out.status.success(), "the same commit asks for no evidence again: {}", said(&out));
    let v = s.versions();
    assert_eq!(v.len(), 2, "re-measured, so a new version for the person");
    assert_eq!(v[1]["sha"], v[0]["sha"]);
    assert_eq!(size(&v[1]), (1, 3));
    let kept = v[1]["evidence"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "{}", v[1]);
    assert_eq!(kept[0]["name"], "after.txt");
    let copy = s.run_dir().join(kept[0]["path"].as_str().unwrap());
    assert!(kept[0]["path"].as_str().unwrap().contains("v2.evidence"), "a copy of its own: {}", kept[0]["path"]);
    assert_eq!(std::fs::read_to_string(copy).unwrap(), "1 passed\n");

    // A commit that is new still needs its own.
    git(&s.repo(), &["checkout", "-q", "fix"]);
    s.commit("more.txt", 1);
    git(&s.repo(), &["checkout", "-q", "main"]);
    let out = s.submit(&[]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).contains("a new commit needs"), "{}", said(&out));
    assert_eq!(s.versions().len(), 2);
}
