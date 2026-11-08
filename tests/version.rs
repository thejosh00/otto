//! tests/version.rs — `otto --version` names the commit it was built from, and that answer can be
//! trusted: it follows git with no source file changed, says when the tree differs, and copes with
//! there being no git at all.
//!
//! The real binary shows the form. The traps need a repository in a known state, so they run the
//! real `build.rs` in a tiny crate planted in a throwaway one, which builds in moments and doesn't
//! depend on the state of this checkout.

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(dir: &Path, program: &str, args: &[&str]) -> String {
    let out = Command::new(program).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

#[test]
fn the_real_binary_carries_the_commit_in_both_flags() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let in_a_repo = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|o| o.status.success() && repo.join(".git").exists())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    for flag in ["--version", "-V"] {
        let said = run(repo, support::OTTO, &[flag]);
        let commit = said
            .strip_prefix(concat!("otto ", env!("CARGO_PKG_VERSION"), " ("))
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or_else(|| panic!("{flag} said {said:?}"));
        match &in_a_repo {
            Some(head) => assert_eq!(commit.trim_end_matches("-dirty"), head, "{flag} said {said:?}"),
            None => assert_eq!(commit, "commit unknown", "{flag} said {said:?}"),
        }
    }
}

/// A tiny crate using the real build.rs, in a git repository of its own, printing what it was told.
struct Planted {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
}

impl Planted {
    fn new() -> Planted {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        write_crate(&repo);
        let planted = Planted { _dir: dir, root, repo };
        planted.git(&planted.repo, &["init", "-q", "-b", "main"]);
        // The lockfile as Cargo writes it, so that building leaves the tree as committed.
        run(&planted.repo, env!("CARGO"), &["generate-lockfile", "-q", "--offline"]);
        planted.git(&planted.repo, &["add", "-A"]);
        planted.git(&planted.repo, &["commit", "-qm", "first"]);
        planted
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    fn head(&self, dir: &Path) -> String {
        self.git(dir, &["rev-parse", "--short", "HEAD"])
    }

    fn target(&self) -> PathBuf {
        self.root.join("target")
    }

    /// Builds in `dir` against the shared target directory and says what the binary reports.
    fn built(&self, dir: &Path) -> String {
        self.build_with(dir, &self.target(), None, &[]);
        run(dir, self.target().join("debug/probe").to_str().unwrap(), &[])
    }

    fn build_with(&self, dir: &Path, target: &Path, path_prefix: Option<&Path>, flags: &[&str]) {
        let mut cargo = Command::new(env!("CARGO"));
        cargo.args(["build", "-q", "--offline"]).args(flags).current_dir(dir).env("CARGO_TARGET_DIR", target);
        // As a shell started there would have it: current_dir alone leaves this process's.
        cargo.env("PWD", dir);
        for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
            cargo.env_remove(var);
        }
        if let Some(prefix) = path_prefix {
            cargo.env("PATH", format!("{}:{}", prefix.display(), std::env::var("PATH").unwrap()));
        }
        let out = cargo.output().unwrap();
        assert!(out.status.success(), "cargo build: {}", String::from_utf8_lossy(&out.stderr));
    }
}

fn write_crate(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("build.rs"), dir.join("build.rs")).unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    fs::write(dir.join("src/main.rs"), "fn main() { println!(\"{}\", env!(\"OTTO_BUILD_COMMIT\")); }\n").unwrap();
    fs::write(dir.join("notes.txt"), "tracked\n").unwrap();
    fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();
}

#[test]
fn a_commit_with_no_source_change_is_reported() {
    let p = Planted::new();
    assert_eq!(p.built(&p.repo), p.head(&p.repo));
    p.git(&p.repo, &["commit", "-q", "--allow-empty", "-m", "nothing changed"]);
    assert_eq!(p.built(&p.repo), p.head(&p.repo));
}

#[test]
fn a_commit_is_seen_when_the_branch_was_only_in_packed_refs() {
    let p = Planted::new();
    p.git(&p.repo, &["pack-refs", "--all"]);
    assert!(!p.repo.join(".git/refs/heads/main").exists());
    assert_eq!(p.built(&p.repo), p.head(&p.repo));
    p.git(&p.repo, &["commit", "-q", "--allow-empty", "-m", "the ref's file appears"]);
    assert_eq!(p.built(&p.repo), p.head(&p.repo));
}

#[test]
fn a_tracked_change_says_dirty_and_undoing_it_says_clean() {
    let p = Planted::new();
    let head = p.head(&p.repo);
    assert_eq!(p.built(&p.repo), head);

    fs::write(p.repo.join("notes.txt"), "edited\n").unwrap();
    assert_eq!(p.built(&p.repo), format!("{head}-dirty"));
    p.git(&p.repo, &["checkout", "--", "notes.txt"]);
    assert_eq!(p.built(&p.repo), head);

    // Only staged, with the file on disk as a new one: the index is what differs.
    fs::write(p.repo.join("added.txt"), "new\n").unwrap();
    assert_eq!(p.built(&p.repo), head, "untracked doesn't count");
    p.git(&p.repo, &["add", "added.txt"]);
    assert_eq!(p.built(&p.repo), format!("{head}-dirty"));
    p.git(&p.repo, &["reset", "-q", "added.txt"]);
    assert_eq!(p.built(&p.repo), head);
}

#[test]
fn deleting_a_tracked_file_says_dirty_and_restoring_it_says_clean() {
    let p = Planted::new();
    let head = p.head(&p.repo);
    assert_eq!(p.built(&p.repo), head);
    fs::remove_file(p.repo.join("notes.txt")).unwrap();
    assert_eq!(p.built(&p.repo), format!("{head}-dirty"));
    // Put back by hand, so the index is left alone and only the file itself can show it.
    fs::write(p.repo.join("notes.txt"), "tracked\n").unwrap();
    assert_eq!(p.built(&p.repo), head);
}

#[test]
fn untracked_and_ignored_files_neither_mark_it_nor_rebuild_it() {
    let p = Planted::new();
    let head = p.head(&p.repo);
    assert_eq!(p.built(&p.repo), head);
    let bin = p.target().join("debug/probe");
    let before = fs::metadata(&bin).unwrap().modified().unwrap();

    fs::write(p.repo.join("scratch.txt"), "x").unwrap();
    fs::write(p.repo.join("ignored.txt"), "x").unwrap();
    assert_eq!(p.built(&p.repo), head);
    assert_eq!(fs::metadata(&bin).unwrap().modified().unwrap(), before, "a rebuild for nothing");

    // git status can rewrite the index, which costs a rebuild and must not change the answer.
    p.git(&p.repo, &["status"]);
    assert_eq!(p.built(&p.repo), head);
}

#[test]
fn a_detached_head_is_named_and_the_next_move_is_seen() {
    let p = Planted::new();
    let first = p.head(&p.repo);
    p.git(&p.repo, &["commit", "-q", "--allow-empty", "-m", "second"]);
    let second = p.head(&p.repo);
    assert_eq!(p.built(&p.repo), second);
    p.git(&p.repo, &["checkout", "-q", "--detach", "HEAD~1"]);
    assert_eq!(p.built(&p.repo), first);
    p.git(&p.repo, &["commit", "-q", "--allow-empty", "-m", "on no branch"]);
    assert_eq!(p.built(&p.repo), p.head(&p.repo));
}

#[test]
fn linked_worktrees_sharing_a_target_directory_each_report_their_own() {
    let p = Planted::new();
    let linked = p.root.join("linked");
    p.git(&p.repo, &["worktree", "add", "-q", "-b", "other", linked.to_str().unwrap()]);
    assert!(linked.join(".git").is_file());
    p.git(&linked, &["commit", "-q", "--allow-empty", "-m", "only in the worktree"]);

    let main_head = p.head(&p.repo);
    let linked_head = p.head(&linked);
    assert_ne!(main_head, linked_head);

    // Back and forth, nothing changing in between: each must still name its own commit.
    assert_eq!(p.built(&p.repo), main_head);
    assert_eq!(p.built(&linked), linked_head);
    assert_eq!(p.built(&p.repo), main_head);
    assert_eq!(p.built(&linked), linked_head);

    p.git(&linked, &["commit", "-q", "--allow-empty", "-m", "moved, in the worktree"]);
    assert_eq!(p.built(&linked), p.head(&linked));
    fs::write(linked.join("notes.txt"), "edited\n").unwrap();
    assert_eq!(p.built(&linked), format!("{}-dirty", p.head(&linked)));
    p.git(&linked, &["checkout", "--", "notes.txt"]);
    assert_eq!(p.built(&linked), p.head(&linked));
    assert_eq!(p.built(&p.repo), main_head);

    // A worktree on a detached HEAD, which is how the lander and the checks build.
    p.git(&linked, &["checkout", "-q", "--detach", "HEAD~1"]);
    assert_eq!(p.built(&linked), p.head(&linked));
}

/// Cargo calls a build fresh when no watched file is newer than it, and these files are all older
/// than the other checkout's build: without care this hands over that build, code and commit.
#[test]
fn a_checkout_with_older_files_is_not_handed_another_checkouts_build() {
    let p = Planted::new();
    fs::write(p.repo.join("src/main.rs"), "fn main() { println!(\"{} two\", env!(\"OTTO_BUILD_COMMIT\")); }\n").unwrap();
    p.git(&p.repo, &["commit", "-qam", "different code"]);
    let (older, newer) = (p.root.join("older"), p.root.join("newer"));
    p.git(&p.repo, &["worktree", "add", "-q", "--detach", older.to_str().unwrap(), "HEAD~1"]);
    p.git(&p.repo, &["worktree", "add", "-q", "--detach", newer.to_str().unwrap(), "HEAD"]);
    let clone = p.root.join("clone");
    p.git(&p.root, &["clone", "-q", p.repo.to_str().unwrap(), clone.to_str().unwrap()]);
    let copy = p.root.join("archive");
    write_crate(&copy);

    let (one, two) = (p.head(&older), p.head(&newer));
    let two_says = format!("{two} two");
    assert_eq!(p.built(&older), one);
    assert_eq!(p.built(&newer), two_says);
    assert_eq!(p.built(&older), one);
    assert_eq!(p.built(&clone), two_says);
    assert_eq!(p.built(&older), one);
    assert_eq!(p.built(&copy), "commit unknown");
    assert_eq!(p.built(&newer), two_says);
    assert_eq!(p.built(&newer), two_says, "and no change on staying put");
}

#[test]
fn no_repository_says_the_commit_is_unknown() {
    let p = Planted::new();
    let copy = p.root.join("archive");
    write_crate(&copy);
    assert_eq!(p.built(&copy), "commit unknown");
}

#[test]
fn a_source_copy_inside_someone_elses_repository_does_not_claim_their_commit() {
    let p = Planted::new();
    let inside = p.repo.join("vendor/copy");
    write_crate(&inside);
    assert_eq!(p.built(&inside), "commit unknown");
}

#[test]
fn a_git_that_will_not_run_says_the_commit_is_unknown() {
    let p = Planted::new();
    let stubs = p.root.join("stubs");
    fs::create_dir_all(&stubs).unwrap();
    fs::write(stubs.join("git"), "#!/bin/sh\nexit 127\n").unwrap();
    fs::set_permissions(stubs.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    p.build_with(&p.repo, &p.target(), Some(&stubs), &[]);
    assert_eq!(run(&p.repo, p.target().join("debug/probe").to_str().unwrap(), &[]), "commit unknown");
}

/// Release, as `install.sh` builds it and compares it: a debug build's symbols name its target
/// directory, which has nothing to do with this.
#[test]
fn building_one_commit_twice_gives_the_same_bytes() {
    let p = Planted::new();
    let (one, two) = (p.root.join("t1"), p.root.join("t2"));
    p.build_with(&p.repo, &one, None, &["--release"]);
    p.build_with(&p.repo, &two, None, &["--release"]);
    let first = fs::read(one.join("release/probe")).unwrap();
    assert!(first == fs::read(two.join("release/probe")).unwrap(), "two target directories differ");
    fs::remove_dir_all(&one).unwrap();
    p.build_with(&p.repo, &one, None, &["--release"]);
    assert!(first == fs::read(one.join("release/probe")).unwrap(), "a build from clean differs");
}
