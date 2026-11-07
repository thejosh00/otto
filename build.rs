//! Records which commit this binary was built from, for `otto --version`.
//!
//! The answer has to be trustable, so this script must rerun whenever it could change, and nothing
//! it records may differ between two builds of one commit: `install.sh` compares binaries byte for
//! byte to keep its rollback copy, so no build time and no path of the checkout.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    // Checkouts sharing a target directory share Cargo's record of what's fresh, whose outside paths
    // are absolute. Without this, a checkout whose files are older than another's last build is
    // handed that build, its code and its commit. PWD is the one thing Cargo compares that differs.
    println!("cargo:rerun-if-env-changed=PWD");
    println!("cargo:rustc-env=OTTO_BUILD_COMMIT={}", commit(&dir).unwrap_or_else(|| "commit unknown".into()));
}

/// `24eea03`, or `24eea03-dirty` when a tracked file differs from it. None without a usable git, or
/// when the source isn't the top of a repository: an archive unpacked inside some other project
/// would otherwise report that project's commit.
fn commit(dir: &Path) -> Option<String> {
    if !git(dir, &["rev-parse", "--show-prefix"])?.is_empty() {
        return None;
    }
    watch_git_state(dir);
    let hash = git(dir, &["rev-parse", "--short", "HEAD"])?;
    let changes = git(dir, &["--no-optional-locks", "status", "--porcelain", "--untracked-files=no"])?;
    Some(if changes.is_empty() { hash } else { format!("{hash}-dirty") })
}

/// Everything the answer reads. Naming any path stops Cargo watching the package's own files, so
/// the tracked files are named too, or undoing an edit wouldn't flip the mark back. Untracked and
/// ignored files aren't, so a scratch file or `target/` doesn't cause a rebuild. The index is, or
/// staging a new file wouldn't show; the price is a rebuild when `git status` happens to rewrite it.
///
/// Paths go through git because a linked worktree's `.git` is a file, its `HEAD` and index are in
/// one directory and its refs in another. A named path that doesn't exist makes Cargo rerun this on
/// every build, so of git's own files only what exists is named.
fn watch_git_state(dir: &Path) {
    let git_path = |name: &str| git(dir, &["rev-parse", "--git-path", name]).map(|p| dir.join(p));
    for path in ["HEAD", "index", "packed-refs"].iter().filter_map(|name| git_path(name)) {
        watch_if_there(&path);
    }
    // Detached, HEAD is the whole answer. On a branch, the ref may only be in packed-refs, and the
    // next commit then creates its file: only the directory it appears in can show that.
    if let Some(path) = git(dir, &["symbolic-ref", "-q", "HEAD"]).and_then(|name| git_path(&name)) {
        match path.ancestors().find(|p| p.exists()) {
            Some(there) if there == path => watch_if_there(&path),
            Some(_) => watch_if_there(path.parent().unwrap_or(&path)),
            None => {}
        }
    }
    if let Some(files) = git(dir, &["ls-files", "-z"]) {
        for file in files.split('\0').filter(|f| !f.is_empty()) {
            // Even if it's gone: nothing else would show it coming back. It costs a rerun per build
            // while the tree is dirty anyway.
            println!("cargo:rerun-if-changed={file}");
        }
    }
}

fn watch_if_there(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// What git printed, or None if it isn't there or said no. Its own environment is cleared so a
/// build started from a git hook still asks about this checkout.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}
