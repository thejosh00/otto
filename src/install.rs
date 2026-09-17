//! `otto install` — symlinks otto's Claude Code skills into `~/.claude/skills/`
//! so otto's skills are discoverable by any Claude Code session — not only one
//! running inside a particular sandbox/launcher.
//!
//! Skills installed: `manage-pr`. (v1 also installed an `otto` skill — the conductor's
//! operating procedure — which v2 replaced with the embedded harness prompt.)
//! (the worktree/PR-prep skill `dev-flow` delegates to) — both live under `<repo>/skills/`
//! in the otto checkout this is run from.

use crate::error::OttoError;
use std::path::{Path, PathBuf};

const SKILLS: &[&str] = &["manage-pr"];

/// Where the otto checkout is, so `otto install` can symlink `skills/<name>` even though
/// the `otto` binary itself may be installed somewhere else entirely (e.g.
/// `~/.local/bin/otto`). Priority: `--repo`, then `$OTTO_REPO`, then walking up from the
/// current directory looking for `harness/wake.md`. No hardcoded fallback path —
/// otto has no business assuming where its own source happens to live.
fn resolve_repo(explicit: Option<&str>) -> Result<PathBuf, OttoError> {
    if let Some(path) = explicit {
        return Ok(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var("OTTO_REPO") {
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("harness").join("wake.md").is_file() {
            return Ok(dir);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    Err(OttoError::usage(
        "cannot find the otto checkout — pass --repo <path>, set $OTTO_REPO, or run `otto install` from \
         inside the checkout",
    ))
}

#[derive(clap::Args, Debug)]
pub struct InstallArgs {
    /// path to the otto checkout (default: $OTTO_REPO, or detected from the current
    /// directory)
    #[arg(long)]
    pub repo: Option<String>,
}

enum LinkOutcome {
    Created,
    AlreadyCorrect,
    Replaced,
}

fn ensure_symlink(link: &Path, target: &Path) -> Result<LinkOutcome, OttoError> {
    match std::fs::symlink_metadata(link) {
        Err(_) => {
            std::os::unix::fs::symlink(target, link)?;
            Ok(LinkOutcome::Created)
        }
        Ok(meta) if meta.file_type().is_symlink() => {
            let current = std::fs::read_link(link)?;
            if current == target {
                Ok(LinkOutcome::AlreadyCorrect)
            } else {
                std::fs::remove_file(link)?;
                std::os::unix::fs::symlink(target, link)?;
                Ok(LinkOutcome::Replaced)
            }
        }
        Ok(_) => Err(OttoError::conflict(format!(
            "{} exists and is not a symlink — remove it or move it aside, then re-run `otto install`",
            link.display()
        ))),
    }
}

/// `~/.claude/skills`, overridable via `$OTTO_CLAUDE_SKILLS_DIR` — used only by tests, so
/// they never touch the real user's skill directory.
fn skills_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OTTO_CLAUDE_SKILLS_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    crate::paths::home_dir().join(".claude").join("skills")
}

pub fn install(args: InstallArgs) -> Result<(), OttoError> {
    let repo = resolve_repo(args.repo.as_deref())?;
    let skills_dir = skills_dir();
    std::fs::create_dir_all(&skills_dir)?;

    for name in SKILLS {
        let source = repo.join("skills").join(name);
        if !source.join("SKILL.md").is_file() {
            return Err(OttoError::conflict(format!("{} has no SKILL.md — is {} the otto checkout?", source.display(), repo.display())));
        }
        let link = skills_dir.join(name);
        match ensure_symlink(&link, &source)? {
            LinkOutcome::Created => println!("installed {} -> {}", link.display(), source.display()),
            LinkOutcome::AlreadyCorrect => println!("already installed: {}", link.display()),
            LinkOutcome::Replaced => println!("relinked {} -> {}", link.display(), source.display()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;

    fn make_repo(dir: &Path) {
        // The marker `resolve_repo` walks up looking for. It moved from the conductor skill
        // (deleted in v2) to the harness prompt, which is the file that now identifies a checkout.
        std::fs::create_dir_all(dir.join("harness")).unwrap();
        std::fs::write(dir.join("harness").join("wake.md"), "harness").unwrap();
        for name in SKILLS {
            std::fs::create_dir_all(dir.join("skills").join(name)).unwrap();
            std::fs::write(dir.join("skills").join(name).join("SKILL.md"), "---\nname: x\n---\nbody").unwrap();
        }
    }

    #[test]
    fn installs_fresh_symlinks() {
        let home = TempHome::new();
        let repo = home.path().join("repo");
        make_repo(&repo);
        install(InstallArgs { repo: Some(repo.display().to_string()) }).unwrap();
        for name in SKILLS {
            let link = home.path().join(".claude").join("skills").join(name);
            assert_eq!(std::fs::read_link(&link).unwrap(), repo.join("skills").join(name));
        }
    }

    #[test]
    fn reinstalling_is_idempotent() {
        let home = TempHome::new();
        let repo = home.path().join("repo");
        make_repo(&repo);
        install(InstallArgs { repo: Some(repo.display().to_string()) }).unwrap();
        install(InstallArgs { repo: Some(repo.display().to_string()) }).unwrap();
        let name = SKILLS[0];
        let link = home.path().join(".claude").join("skills").join(name);
        assert_eq!(std::fs::read_link(&link).unwrap(), repo.join("skills").join(name));
    }

    #[test]
    fn relinks_a_symlink_pointing_elsewhere() {
        let home = TempHome::new();
        let repo = home.path().join("repo");
        make_repo(&repo);
        let skills_dir = home.path().join(".claude").join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let name = SKILLS[0];
        std::os::unix::fs::symlink("/nowhere", skills_dir.join(name)).unwrap();
        install(InstallArgs { repo: Some(repo.display().to_string()) }).unwrap();
        assert_eq!(std::fs::read_link(skills_dir.join(name)).unwrap(), repo.join("skills").join(name));
    }

    #[test]
    fn refuses_to_clobber_a_real_file() {
        let home = TempHome::new();
        let repo = home.path().join("repo");
        make_repo(&repo);
        let skills_dir = home.path().join(".claude").join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let name = SKILLS[0];
        std::fs::write(skills_dir.join(name), "not a symlink").unwrap();
        let result = install(InstallArgs { repo: Some(repo.display().to_string()) });
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(skills_dir.join(name)).unwrap(), "not a symlink");
    }

    #[test]
    fn errors_without_a_resolvable_repo() {
        // CWD and $OTTO_REPO are both process-global, so this test serializes on the same
        // mutex every other env/CWD-mutating test in the crate uses.
        let _lock = crate::paths::test_support::lock_env();
        let previous_repo = std::env::var_os("OTTO_REPO");
        std::env::remove_var("OTTO_REPO");
        let dir = tempfile::tempdir().unwrap();
        let previous_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let result = resolve_repo(None);
        std::env::set_current_dir(previous_dir).unwrap();
        match previous_repo {
            Some(v) => std::env::set_var("OTTO_REPO", v),
            None => std::env::remove_var("OTTO_REPO"),
        }
        assert!(result.is_err());
    }
}
