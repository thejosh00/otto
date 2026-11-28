//! `otto chat-edits` — the folders a run's chats may edit (plans/threads-agents.md).
//!
//! A chat changes nothing (DESIGN.md §7), except where the person has said otherwise: a run whose
//! chats should record what the person tells them — a thread's log, its decisions, its summary —
//! has those folders named here, by the person, and nowhere else. Later steps hold such a chat to
//! them (a hook refuses edits outside them), take the run's wake lock for it, snapshot the folders
//! before it and diff them after, and tell the next wake what changed.
//!
//! `$OTTO_HOME/chat-edits/<run id>.json`, beside `approvals/` and for the same reason: never
//! `run.json`, which a wake rewrites, so a run can't widen what its own chats may touch. The
//! command is refused from a wake or a chat. An unsandboxed wake can write this file anyway, as it
//! can write anything; a sandboxed one can't, once the sandbox closes `chat-edits/` as it closes
//! `approvals/`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::OttoError;

/// The most a folder may hold, `.git` aside: what a snapshot before each chat copies.
pub const MAX_FILES: usize = 2_000;
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;

#[derive(clap::Args, Debug)]
pub struct ChatEditsArgs {
    /// The run: its number or name (a unique prefix of the name will do)
    pub id: String,
    /// The folders its chats may edit, replacing any it had. Omit them, and --clear, to see what it
    /// has
    pub dirs: Vec<PathBuf>,
    /// Take them all away: its chats edit nothing again
    #[arg(long, conflicts_with = "dirs")]
    pub clear: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    /// The run's id when it was written. A record whose `run` isn't the run asked about counts
    /// for nothing.
    run: String,
    /// Canonical, absolute.
    dirs: Vec<String>,
    set_at: String,
}

pub fn dir() -> PathBuf {
    crate::paths::otto_home().join("chat-edits")
}

/// A run's record. `run` is a run id, which is one path component: anything else has none.
fn path(run: &str) -> Result<PathBuf, OttoError> {
    if run.is_empty() || run == "." || run == ".." || run.contains(['/', '\\', '\0']) {
        return Err(OttoError::usage(format!("{run:?} is not a run id")));
    }
    Ok(dir().join(format!("{run}.json")))
}

/// The folders the run's chats may edit, as recorded: none when it has no record, or one that
/// can't be read or isn't its own.
pub fn read(run: &str) -> Vec<PathBuf> {
    let Some(text) = path(run).ok().and_then(|p| std::fs::read_to_string(p).ok()) else { return vec![] };
    match serde_json::from_str::<Record>(&text) {
        Ok(record) if record.run == run => record.dirs.into_iter().map(PathBuf::from).collect(),
        _ => vec![],
    }
}

fn write(run: &str, dirs: &[PathBuf]) -> Result<(), OttoError> {
    let path = path(run)?;
    let record = Record {
        run: run.to_string(),
        dirs: dirs.iter().map(|d| d.display().to_string()).collect(),
        set_at: crate::clock::Timestamp::now().to_string(),
    };
    std::fs::create_dir_all(dir())?;
    crate::state::write_atomic(&path, &format!("{}\n", serde_json::to_string_pretty(&record)?))
}

fn clear(run: &str) -> Result<(), OttoError> {
    match std::fs::remove_file(path(run)?) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// A renumbered run keeps its folders.
pub fn moved(from: &str, to: &str) {
    if let (Ok(old), Ok(new)) = (path(from), path(to)) {
        if let Some(mut record) = std::fs::read_to_string(&old).ok().and_then(|t| serde_json::from_str::<Record>(&t).ok()) {
            record.run = to.to_string();
            if let Ok(text) = serde_json::to_string_pretty(&record) {
                if crate::state::write_atomic(&new, &format!("{text}\n")).is_ok() {
                    let _ = std::fs::remove_file(old);
                }
            }
        }
    }
}

/// `dir` as it may be recorded — canonical — or why it may not be.
pub fn check(dir: &Path) -> Result<PathBuf, OttoError> {
    let shown = dir.display();
    let canonical = dir.canonicalize().map_err(|e| OttoError::usage(format!("{shown}: {e}")))?;
    if !canonical.is_dir() {
        return Err(OttoError::usage(format!("{shown} is not a folder")));
    }
    let home = crate::paths::home_dir();
    let home = home.canonicalize().unwrap_or(home);
    if canonical == Path::new("/") || canonical == home {
        return Err(OttoError::usage(format!(
            "{shown} is too much to hand a chat: name the folder its updates belong in, not the whole of {}",
            if canonical == home { "your home" } else { "the disk" }
        )));
    }
    let otto = crate::paths::otto_home();
    let otto = otto.canonicalize().unwrap_or(otto);
    let claude = home.join(".claude");
    for (closed, what) in [(&otto, "otto's own"), (&claude, "Claude's settings and memory")] {
        if canonical.starts_with(closed) || closed.starts_with(&canonical) {
            return Err(OttoError::usage(format!("{shown} holds or is inside {what} ({}): a chat edits none of it", closed.display())));
        }
    }
    let (files, bytes) = size(&canonical)?;
    if files > MAX_FILES || bytes > MAX_BYTES {
        return Err(OttoError::usage(format!(
            "{shown} holds {files} files, {} MB, past the {MAX_FILES} files or {} MB a chat's snapshot copies (`.git` not counted): name a smaller folder",
            bytes / (1024 * 1024),
            MAX_BYTES / (1024 * 1024)
        )));
    }
    Ok(canonical)
}

/// Files and bytes under `dir`, `.git` aside, links not followed (a link counts as a file).
fn size(dir: &Path) -> Result<(usize, u64), OttoError> {
    let (mut files, mut bytes) = (0usize, 0u64);
    let mut todo = vec![dir.to_path_buf()];
    while let Some(at) = todo.pop() {
        for entry in std::fs::read_dir(&at)? {
            let entry = entry?;
            let meta = std::fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                if entry.file_name() != ".git" {
                    todo.push(entry.path());
                }
            } else {
                files += 1;
                bytes += meta.len();
                if files > MAX_FILES || bytes > MAX_BYTES {
                    return Ok((files, bytes));
                }
            }
        }
    }
    Ok((files, bytes))
}

/// Refused from a wake or a chat, before anything is read or written.
fn person_only() -> Result<(), OttoError> {
    if std::env::var_os("OTTO_RUN_ID").is_some() || std::env::var_os(crate::chat::CHAT_ENV).is_some() {
        return Err(OttoError::usage(
            "`otto chat-edits` is the person's to run: what a run's chats may edit is refused from a wake or a chat, and nothing was changed. Ask them to run it",
        ));
    }
    Ok(())
}

pub fn chat_edits(args: ChatEditsArgs) -> Result<(), OttoError> {
    let changing = args.clear || !args.dirs.is_empty();
    if changing {
        person_only()?;
    }
    let id = crate::paths::resolve_run_id(&args.id)?;
    let short = crate::paths::short_id(&id);
    if args.clear {
        clear(&id)?;
        println!("its chats edit nothing now");
        return Ok(());
    }
    if !args.dirs.is_empty() {
        let mut dirs: Vec<PathBuf> = Vec::new();
        for dir in &args.dirs {
            let dir = check(dir)?;
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        write(&id, &dirs)?;
        println!("a chat about {} may now edit:", crate::paths::label_of(&id));
        for dir in &dirs {
            println!("  {}", crate::changes::home_relative(&dir.to_string_lossy()));
        }
        return Ok(());
    }
    let dirs = read(&id);
    if dirs.is_empty() {
        println!("its chats edit nothing — `otto chat-edits {short} <folder>` to let them record in one");
    } else {
        for dir in &dirs {
            println!("{}", crate::changes::home_relative(&dir.to_string_lossy()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;

    /// Folders to name, outside otto's home (`TempHome` points `OTTO_HOME` at its own folder).
    struct Work(tempfile::TempDir);

    impl Work {
        fn new() -> Self {
            Work(tempfile::tempdir().unwrap())
        }

        fn folder(&self, name: &str) -> PathBuf {
            let dir = self.0.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }
    }

    #[test]
    fn a_folder_is_recorded_canonical_and_read_back_for_its_own_run_only() {
        let _home = TempHome::new();
        let work = Work::new();
        let thread = work.folder("thread");
        let recorded = check(&thread.join("..").join("thread")).unwrap();
        assert_eq!(recorded, thread.canonicalize().unwrap());
        write("7", std::slice::from_ref(&recorded)).unwrap();
        assert_eq!(read("7"), vec![recorded.clone()]);
        assert!(read("8").is_empty());
        // A record copied to another run's name is not that run's.
        std::fs::copy(path("7").unwrap(), path("8").unwrap()).unwrap();
        assert!(read("8").is_empty(), "the record names its run");
        clear("7").unwrap();
        assert!(read("7").is_empty());
        clear("7").expect("clearing nothing is not an error");
    }

    #[test]
    fn too_much_or_the_wrong_place_is_refused() {
        let home = TempHome::new();
        let work = Work::new();
        assert!(check(Path::new("/")).is_err());
        assert!(check(&crate::paths::home_dir()).unwrap_err().message.contains("your home"));
        assert!(check(&work.0.path().join("nowhere")).is_err());
        let file = work.0.path().join("a-file");
        std::fs::write(&file, "x").unwrap();
        assert!(check(&file).unwrap_err().message.contains("not a folder"));
        let claude = crate::paths::home_dir().join(".claude");
        if claude.is_dir() {
            assert!(check(&claude).unwrap_err().message.contains("Claude's"));
        }
        let runs = crate::paths::otto_home().join("runs");
        std::fs::create_dir_all(&runs).unwrap();
        assert!(check(&runs).unwrap_err().message.contains("otto's own"), "inside otto's home");
        // A folder holding otto's home is refused as well as one inside it.
        let holding = work.folder("holds-otto");
        std::fs::create_dir_all(holding.join("otto")).unwrap();
        std::env::set_var("OTTO_HOME", holding.join("otto"));
        let held = check(&holding);
        std::env::set_var("OTTO_HOME", home.path());
        assert!(held.unwrap_err().message.contains("otto's own"), "holding otto's home");
    }

    #[test]
    fn a_folder_past_the_snapshot_bound_is_refused_and_git_is_not_counted() {
        let _home = TempHome::new();
        let work = Work::new();
        let big = work.folder("big");
        std::fs::create_dir_all(big.join(".git/objects")).unwrap();
        for i in 0..(MAX_FILES + 10) {
            std::fs::write(big.join(".git/objects").join(i.to_string()), "").unwrap();
        }
        check(&big).expect(".git doesn't count");
        for i in 0..=MAX_FILES {
            std::fs::write(big.join(i.to_string()), "").unwrap();
        }
        assert!(check(&big).unwrap_err().message.contains("files"));
    }

    #[test]
    fn a_renumbered_run_keeps_its_folders() {
        let _home = TempHome::new();
        let thread = check(&Work::new().folder("thread")).unwrap();
        write("2026-10-01-old", std::slice::from_ref(&thread)).unwrap();
        moved("2026-10-01-old", "4");
        assert_eq!(read("4"), vec![thread]);
        assert!(read("2026-10-01-old").is_empty());
    }

    #[test]
    fn changing_them_is_refused_from_a_wake_or_a_chat() {
        // TempHome holds the environment's lock, so setting a variable under it is safe.
        let _home = TempHome::new();
        for var in ["OTTO_RUN_ID", crate::chat::CHAT_ENV] {
            let before = std::env::var_os(var);
            std::env::set_var(var, "7");
            let refused = chat_edits(ChatEditsArgs { id: "7".into(), dirs: vec![], clear: true });
            match before {
                Some(v) => std::env::set_var(var, v),
                None => std::env::remove_var(var),
            }
            assert!(refused.unwrap_err().message.contains("the person's to run"), "{var}");
        }
    }

    #[test]
    fn run_ids_that_are_not_one_path_component_have_no_record() {
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            assert!(path(bad).is_err(), "{bad:?}");
            assert!(read(bad).is_empty());
        }
    }
}
