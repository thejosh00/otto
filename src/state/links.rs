//! A wake can write its own run directory, and otto, outside any sandbox, writes and reads there too.
//! So a link a wake leaves there is a way to aim otto at any file the person owns: a `journal.jsonl`
//! linked to `~/.zshenv` had otto append to the person's shell startup file. Otto never writes through
//! a link in a run directory, and never reads through one to decide what to do. The one exception is a
//! file it only shows the person (`reach`), and only when the link leads somewhere the run already has.

use crate::error::OttoError;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The first link at or below the run directory on the way to `path`, if `path` is in one. A path
/// outside `$OTTO_HOME/runs` isn't a run's to plant links in, so it isn't checked here.
pub fn link_in_run_dir(path: &Path) -> Option<PathBuf> {
    let runs = crate::paths::runs_dir();
    let rest = path.strip_prefix(&runs).ok()?;
    let mut at = runs;
    for part in rest.components() {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => return Some(at),
            Ok(_) => {}
            // Nothing there yet, so nothing below it either: what otto makes from here on is its own.
            Err(_) => return None,
        }
    }
    None
}

/// Refuse `path` if a link stands anywhere between its run directory and it.
pub fn refuse_links(path: &Path) -> Result<(), OttoError> {
    match link_in_run_dir(path) {
        None => Ok(()),
        Some(link) => Err(OttoError::conflict(format!(
            "{}: a link was found in the run directory ({}) and was not followed. A wake can write its own run \
             directory, so otto never writes, or reads to decide anything, through a link there. Look at what it \
             points to before removing it.",
            path.display(),
            link.display()
        ))),
    }
}

/// `std::fs::read_to_string` for a file otto reads to decide what to do: never through a link.
pub fn read_to_string(path: impl AsRef<Path>) -> std::io::Result<String> {
    let path = path.as_ref();
    refuse_links(path).map_err(|e| std::io::Error::other(e.message))?;
    std::fs::read_to_string(path)
}

/// `std::fs::read`, likewise.
pub fn read(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    let path = path.as_ref();
    refuse_links(path).map_err(|e| std::io::Error::other(e.message))?;
    std::fs::read(path)
}

/// Open for appending, creating it if missing, never through a link: checked on the way down, and
/// `O_NOFOLLOW` on the last step so a link planted between the check and the open is refused too.
pub fn open_append(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    refuse_links(path).map_err(|e| std::io::Error::other(e.message))?;
    std::fs::OpenOptions::new().create(true).append(true).custom_flags(libc::O_NOFOLLOW).open(path).map_err(|e| {
        if e.raw_os_error() == Some(libc::ELOOP) {
            std::io::Error::other(format!(
                "{}: a link was found in the run directory and was not followed",
                path.display()
            ))
        } else {
            e
        }
    })
}

/// Append bytes to a file, never through a link.
pub fn append(path: &Path, bytes: &[u8]) -> Result<(), OttoError> {
    let mut file = open_append(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

/// Where a link in a run directory may lead for otto to follow it, to show the person what's there
/// and for nothing else: the run's own directory, its workdir, and the repos it may write — the
/// sandbox record's `write` list when it has one (a wake can't write that), else `launcher.repos`.
/// Each is resolved, so a root that is itself a link counts as where it really is.
pub fn reach(run_dir: &Path, state: &crate::state::RunState) -> Vec<PathBuf> {
    let id = run_dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let home = crate::paths::home_dir();
    let repos: Vec<PathBuf> = match crate::sandbox::read_record(&id) {
        Ok(record) => record.write.iter().filter_map(|p| crate::sandbox::normalize(p, &home).ok()).collect(),
        Err(_) => state.launcher.repos.iter().map(PathBuf::from).collect(),
    };
    [Some(run_dir.to_path_buf()), crate::config::workdir_for(state)]
        .into_iter()
        .flatten()
        .chain(repos)
        .filter_map(|root| root.canonicalize().ok())
        .collect()
}

/// Whether `path`, followed through any links, lands inside `reach`. A path that doesn't resolve
/// (missing, a dangling link, a loop) is not in reach.
pub fn in_reach(run_dir: &Path, state: &crate::state::RunState, path: &Path) -> bool {
    let Ok(target) = path.canonicalize() else { return false };
    reach(run_dir, state).iter().any(|root| target.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;

    fn run_dir(id: &str) -> PathBuf {
        let dir = crate::paths::runs_dir().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn journal_append_through_a_link_is_refused_and_the_target_is_untouched() {
        let _h = TempHome::new();
        let dir = run_dir("r1");
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("zshenv");
        std::fs::write(&victim, "# the person's startup file\n").unwrap();
        std::os::unix::fs::symlink(&victim, dir.join("journal.jsonl")).unwrap();
        let err = crate::state::journal(&dir, "x", serde_json::json!({"say": "$(touch /tmp/ran)"})).unwrap_err();
        assert!(err.message.contains("a link was found in the run directory"), "{}", err.message);
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "# the person's startup file\n");
    }

    #[test]
    fn a_linked_directory_on_the_way_is_refused_too() {
        let _h = TempHome::new();
        let dir = run_dir("r1");
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.join("gates")).unwrap();
        let err = crate::state::write_atomic(&dir.join("gates/1-q.md"), "question").unwrap_err();
        assert!(err.message.contains("a link was found in the run directory"), "{}", err.message);
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none(), "nothing written through it");
    }

    #[test]
    fn run_json_and_handoff_writes_through_a_link_are_refused() {
        let _h = TempHome::new();
        let dir = run_dir("r1");
        let outside = tempfile::tempdir().unwrap();
        for name in ["run.json", crate::state::commands::HANDOFF_FILE] {
            let victim = outside.path().join(name);
            std::fs::write(&victim, "theirs\n").unwrap();
            std::os::unix::fs::symlink(&victim, dir.join(name)).unwrap();
            let err = crate::state::write_atomic(&dir.join(name), "ours\n").unwrap_err();
            assert!(err.message.contains("a link was found in the run directory"), "{name}: {}", err.message);
            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "theirs\n");
            assert!(std::fs::symlink_metadata(dir.join(name)).unwrap().file_type().is_symlink(), "{name}: not replaced either");
        }
    }

    #[test]
    fn run_json_read_through_a_link_is_refused() {
        let _h = TempHome::new();
        let dir = run_dir("r1");
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("run.json"), "{}").unwrap();
        std::os::unix::fs::symlink(outside.path().join("run.json"), dir.join("run.json")).unwrap();
        let err = crate::state::read_run("r1").unwrap_err();
        assert!(err.message.contains("a link was found in the run directory"), "{}", err.message);
    }

    #[test]
    fn a_todo_linked_into_its_own_run_dir_is_shown_and_one_linked_out_of_reach_is_not() {
        let _h = TempHome::new();
        crate::state::commands::test_init("t-link", "a goal").unwrap();
        let dir = crate::paths::run_dir("t-link").unwrap();
        let todo = dir.join(crate::todo::TODO_FILE);
        std::fs::create_dir_all(dir.join("plan")).unwrap();
        std::fs::write(dir.join("plan/steps.md"), "- [ ] one\n").unwrap();
        std::os::unix::fs::symlink(dir.join("plan/steps.md"), &todo).unwrap();
        assert_eq!(crate::todo::read(&dir).map(|t| t.counts.total), Some(1), "into its own run directory");

        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(elsewhere.path().join("secret.md"), "- [ ] not yours\n").unwrap();
        std::fs::remove_file(&todo).unwrap();
        std::os::unix::fs::symlink(elsewhere.path().join("secret.md"), &todo).unwrap();
        assert!(crate::todo::read(&dir).is_none(), "out of reach");
    }

    #[test]
    fn a_todo_linked_into_a_repo_the_sandbox_record_lets_it_write_is_shown() {
        let _h = TempHome::new();
        crate::state::commands::test_init("t-repo", "a goal").unwrap();
        let dir = crate::paths::run_dir("t-repo").unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("todo.md"), "- [ ] one\n").unwrap();
        std::os::unix::fs::symlink(repo.path().join("todo.md"), dir.join(crate::todo::TODO_FILE)).unwrap();
        assert!(crate::todo::read(&dir).is_none(), "not yet in reach");
        let records = crate::paths::otto_home().join("sandbox/runs");
        std::fs::create_dir_all(&records).unwrap();
        let record = serde_json::json!({"launcher": "claude", "write": [repo.path().display().to_string()]});
        std::fs::write(records.join("t-repo.json"), record.to_string()).unwrap();
        assert_eq!(crate::todo::read(&dir).map(|t| t.counts.total), Some(1));
    }

    #[test]
    fn paths_outside_the_runs_dir_are_not_checked() {
        let _h = TempHome::new();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("real"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path().join("real"), outside.path().join("link")).unwrap();
        assert!(link_in_run_dir(&outside.path().join("link")).is_none());
    }
}
