//! tests/summary_replies.rs — what otto says when a summary joins a version that already exists, through
//! the real binary: `otto state publish` of the same text, and `otto state change submit` of the same
//! commit. A wake reads the reply, so it has to say what happened: added, replaced, or nothing new. It
//! once said "its summary kept with it" for both, which reads as though the new one was ignored.
//! No tmux, no model.

mod support;

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
    /// A run called `author`, and a repo with one commit on `fix` to hand back.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&repo, &["checkout", "-q", "-b", "fix"]);
        std::fs::write(repo.join("f"), "first").unwrap();
        git(&repo, &["add", "f"]);
        git(&repo, &["commit", "-q", "-m", "first"]);
        git(&repo, &["checkout", "-q", "main"]);
        let s = Scratch { _dir: dir, root };
        let out = s.otto(&["state", "init", "--name", "author", "--perpetual", "--period", "1h", "--goal", "g", "--repo", repo.to_str().unwrap(), "--workdir", repo.to_str().unwrap()]);
        assert!(out.status.success(), "state init: {}", String::from_utf8_lossy(&out.stderr));
        s
    }

    fn otto(&self, args: &[&str]) -> Output {
        support::otto_command()
            .args(args)
            .current_dir(&self.root)
            .env("OTTO_HOME", self.root.join("home"))
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .output()
            .unwrap()
    }

    fn write(&self, name: &str, text: &str) -> String {
        let path = self.root.join(name);
        std::fs::write(&path, text).unwrap();
        path.display().to_string()
    }

    fn run_dir(&self) -> PathBuf {
        let runs = self.root.join("home/runs");
        std::fs::read_dir(&runs).unwrap().map(|e| e.unwrap().path()).find(|p| p.is_dir()).expect("a run")
    }

    /// Publish `family-map` and give back what otto said, which must be a success.
    fn publish(&self, flags: &[&str]) -> String {
        let mut args = vec!["state", "publish", "author", "--key", "family-map", "--title", "The family map"];
        args.extend_from_slice(flags);
        let out = self.otto(&args);
        assert!(out.status.success(), "publish {flags:?}: {}", said(&out));
        said(&out)
    }

    /// Hand `fix` back and give back what otto said, which must be a success.
    fn submit(&self, flags: &[&str]) -> String {
        let repo = self.root.join("repo");
        let mut args = vec!["state", "change", "submit", "author", "--repo", repo.to_str().unwrap(), "--branch", "fix", "--base", "main", "--title", "A fix"];
        args.extend_from_slice(flags);
        let out = self.otto(&args);
        assert!(out.status.success(), "submit {flags:?}: {}", said(&out));
        said(&out)
    }
}

fn said(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
fn publishing_the_same_text_says_whether_its_summary_was_added_replaced_or_the_same() {
    let s = Scratch::new();
    let map = s.write("map.md", "The family map: who is in it, and how each of them is related to the others.\n");
    let (first, second) = (s.write("sum.md", "Who is who.\n"), s.write("sum2.md", "Who is who, and where.\n"));
    let summary = s.run_dir().join("outputs/family-map/v1.summary.md");

    assert_eq!(s.publish(&["--file", &map]).trim(), "family-map@1");

    let added = s.publish(&["--file", &map, "--summary-file", &first]);
    assert_eq!(added.trim(), "family-map@1: summary added — the text is the same, so no new version");
    assert_eq!(std::fs::read_to_string(&summary).unwrap().trim(), "Who is who.");

    let replaced = s.publish(&["--file", &map, "--summary-file", &second]);
    assert_eq!(replaced.trim(), "family-map@1: summary replaced with this one — the text is the same, so no new version");
    assert_eq!(std::fs::read_to_string(&summary).unwrap().trim(), "Who is who, and where.");

    let same = s.publish(&["--file", &map, "--summary-file", &second]);
    assert_eq!(same.trim(), "family-map@1 unchanged — the same as its latest version, so no new one");
    assert_eq!(s.publish(&["--file", &map]).trim(), "family-map@1 unchanged — the same as its latest version, so no new one");
    assert_eq!(std::fs::read_to_string(&summary).unwrap().trim(), "Who is who, and where.", "publishing without one leaves its summary");
    assert!(!s.run_dir().join("outputs/family-map/v2.md").exists(), "none of that made a second version");
}

#[test]
fn submitting_the_same_commit_says_whether_its_write_up_was_added_or_its_summary_replaced() {
    let s = Scratch::new();
    let whole = s.write("whole.md", &"word ".repeat(600));
    let (first, second) = (s.write("sum.md", "Shouts it, and why.\n"), s.write("sum2.md", "Shouts it, and why not.\n"));

    // Handed back with only what it says; then the same commit with the whole of it and a new summary.
    let v1 = s.submit(&["--summary-file", &first, "--no-evidence", "a test of the replies"]);
    assert!(!v1.contains("the same commit"), "{v1}");
    let added = s.submit(&["--file", &whole, "--summary-file", &second]);
    assert_eq!(added.lines().next(), Some("change 1/1 v1: whole write-up added, and its summary replaced with this one — the same commit, so no new version"), "{added}");

    // A better summary of the same write-up.
    let replaced = s.submit(&["--file", &whole, "--summary-file", &first]);
    assert_eq!(replaced.lines().next(), Some("change 1/1 v1: summary replaced with this one — the same commit, so no new version"), "{replaced}");
    let shown = said(&s.otto(&["changes", "show", "author/1"]));
    assert!(shown.contains("Shouts it, and why.") && !shown.contains("why not"), "the reply is true of what a person sees: {shown}");

    // The same of both again: nothing happened, and nothing claims otherwise.
    let same = s.submit(&["--file", &whole, "--summary-file", &first]);
    assert_eq!(same.lines().next(), Some("change 1/1 v1 unchanged — the same commit as its latest version, so nothing new for the person"), "{same}");

    for reply in [&v1, &added, &replaced, &same] {
        assert!(!reply.contains("kept with it"), "{reply}");
    }
}

#[test]
fn submitting_the_same_commit_with_its_write_up_and_the_summary_it_had_says_only_the_write_up_was_added() {
    let s = Scratch::new();
    let whole = s.write("whole.md", &"word ".repeat(600));
    let summary = s.write("sum.md", "Shouts it, and why.\n");
    s.submit(&["--summary-file", &summary, "--no-evidence", "a test of the replies"]);
    let added = s.submit(&["--file", &whole, "--summary-file", &summary]);
    assert_eq!(added.lines().next(), Some("change 1/1 v1: whole write-up added, under the summary it already had — the same commit, so no new version"), "{added}");
}
