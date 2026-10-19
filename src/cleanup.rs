//! What `$OTTO_HOME` holds that no agent uses, and removing it — `otto cleanup`.
//!
//! Everything an agent uses is its directory in `runs/`. What else piles up is directories too:
//!
//! - `archive/<id>` — an agent deleted with its data kept (`otto delete`, `DataChoice::Keep`).
//! - `deleting/<id>-<secs>` — what a delete moved out of `runs/` and then couldn't remove.
//! - `runs/<dir>` with no `run.json` — not a run, so nothing in otto will ever read it: a number
//!   claimed for a run that was never written, or a directory something else left there.
//!
//! Only these are listed, and only what is listed can be removed: a path the web page sends is
//! looked up in the list, never joined onto `$OTTO_HOME` and removed.

use crate::clock::Timestamp;
use crate::error::OttoError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// An agent's data, kept when it was deleted.
    Kept,
    /// What a delete couldn't finish removing.
    Leftover,
    /// A directory in `runs/` that isn't a run.
    Stray,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unused {
    /// Where it is under `$OTTO_HOME` — `archive/7` — which is also how it is named to remove it.
    pub path: String,
    pub kind: Kind,
    /// What it was, in a line: `#7 legenda — Watch the release notes and draft a summary`.
    pub what: String,
    pub bytes: u64,
    /// When its agent last ran (or was made, if it never ran) when it was one; otherwise when
    /// anything in it last changed.
    pub last_used: Option<Timestamp>,
}

/// A directory in `runs/` this young may be a run being created — its number claimed, its
/// `run.json` not written yet — so it isn't listed.
const STRAY_GRACE: Duration = Duration::from_secs(10 * 60);

/// Everything no agent uses, least recently used first.
pub fn find() -> Vec<Unused> {
    find_with(STRAY_GRACE)
}

fn find_with(grace: Duration) -> Vec<Unused> {
    let home = crate::paths::otto_home();
    let mut found = Vec::new();
    for (sub, kind) in [("archive", Kind::Kept), ("deleting", Kind::Leftover)] {
        found.extend(subdirs(&home.join(sub)).iter().map(|dir| describe(dir, kind)));
    }
    for dir in subdirs(&crate::paths::runs_dir()) {
        let hidden = dir.file_name().and_then(|n| n.to_str()).is_none_or(|n| n.starts_with('.'));
        if hidden || dir.join("run.json").exists() {
            continue;
        }
        let (_, changed) = walk(&dir);
        if changed.and_then(|t| t.elapsed().ok()).is_some_and(|age| age >= grace) {
            found.push(describe(&dir, Kind::Stray));
        }
    }
    found.sort_by_key(|u| u.last_used);
    found
}

/// Remove these, each named as `find` lists it. Every one is checked against the list before any
/// is removed, so a path that isn't there removes nothing.
pub fn remove(paths: &[String]) -> Result<Vec<Unused>, OttoError> {
    let listed = find();
    let mut chosen = Vec::new();
    for path in paths {
        let item = listed.iter().find(|u| &u.path == path).ok_or_else(|| {
            OttoError::not_found(format!("{path} isn't something `otto cleanup` lists — nothing was removed"))
        })?;
        if !chosen.iter().any(|c: &Unused| c.path == item.path) {
            chosen.push(item.clone());
        }
    }
    let home = crate::paths::otto_home();
    for item in &chosen {
        std::fs::remove_dir_all(home.join(&item.path))
            .map_err(|e| OttoError::new(format!("couldn't remove {}: {e}", item.path), 4))?;
    }
    Ok(chosen)
}

fn describe(dir: &Path, kind: Kind) -> Unused {
    let (bytes, changed) = walk(dir);
    let changed = changed.map(|t| Timestamp::at(t.into()));
    let state = crate::state::read_state(dir).ok();
    let what = match (&state, kind) {
        (Some(s), Kind::Kept) => format!("{} — {}", s.label(), s.goal),
        (Some(s), _) => format!("left over from deleting {}", s.label()),
        (None, Kind::Kept) => "an agent's data — its run.json can't be read".to_string(),
        (None, Kind::Leftover) => "left over from a delete".to_string(),
        (None, Kind::Stray) if bytes == 0 => "not a run — an empty directory".to_string(),
        (None, Kind::Stray) => "not a run — it has no run.json".to_string(),
    };
    // From its record when it has one: what a delete itself wrote — stopping it — is not use, and
    // an agent that never ran was last used when it was made.
    let used = state.as_ref().map(|s| {
        let finished = crate::core::last_run(dir, s).map(|l| l.at);
        let started = s.wake.as_ref().map(|w| w.started_at);
        finished.into_iter().chain(started).max().unwrap_or(s.created_at)
    });
    Unused { path: crate::paths::under_home(dir), kind, what, bytes, last_used: used.or(changed) }
}

/// The real directories in `dir`, not following symlinks.
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()
}

/// Its size in bytes, and when anything in it — itself included — last changed. Symlinks are
/// counted as themselves, never followed.
pub fn walk(dir: &Path) -> (u64, Option<SystemTime>) {
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return (0, None) };
    let mut newest = meta.modified().ok();
    if !meta.is_dir() {
        return (meta.len(), newest);
    }
    let mut bytes = 0;
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let (b, changed) = walk(&entry.path());
        bytes += b;
        newest = newest.max(changed);
    }
    (bytes, newest)
}

/// `0 B`, `840 B`, `64 KB`, `1.2 MB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut n = bytes as f64 / 1024.0;
    let mut unit = 0;
    while n >= 1024.0 && unit < UNITS.len() - 1 {
        n /= 1024.0;
        unit += 1;
    }
    if n < 10.0 {
        format!("{n:.1} {}", UNITS[unit])
    } else {
        format!("{n:.0} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{delete_run, DataChoice};
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;

    fn deleted(id: &str, data: DataChoice) -> crate::core::Deleted {
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() }); // has-session: no
        delete_run(id, data, &mut exec).unwrap()
    }

    #[test]
    fn a_kept_agent_is_listed_by_what_it_was_and_removed_only_when_asked() {
        let _h = TempHome::new();
        test_init("2026-10-19-kept", "Watch the release notes").unwrap();
        let kept = deleted("kept", DataChoice::Keep);
        assert_eq!(kept.kept.as_deref(), Some("archive/2026-10-19-kept"));

        let found = find();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].path.as_str(), found[0].kind), ("archive/2026-10-19-kept", Kind::Kept));
        assert!(found[0].what.contains("Watch the release notes"), "{}", found[0].what);
        assert!(found[0].bytes > 0 && found[0].last_used.is_some());

        let removed = remove(&["archive/2026-10-19-kept".to_string()]).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(find().is_empty());
        assert!(!crate::paths::otto_home().join("archive/2026-10-19-kept").exists());
    }

    #[test]
    fn the_same_id_kept_twice_gets_two_places() {
        let _h = TempHome::new();
        test_init("2026-10-19-twice", "a goal").unwrap();
        deleted("twice", DataChoice::Keep);
        test_init("2026-10-19-twice", "a goal").unwrap();
        assert_eq!(deleted("twice", DataChoice::Keep).kept.as_deref(), Some("archive/2026-10-19-twice-2"));
        assert_eq!(find().len(), 2);
    }

    /// The web page sends a path; anything that isn't listed — a live run, otto's config, a path
    /// out of `$OTTO_HOME` — is refused, and nothing else in the same request goes either.
    #[test]
    fn only_what_is_listed_can_be_removed() {
        let _h = TempHome::new();
        test_init("2026-10-19-live", "a goal").unwrap();
        test_init("2026-10-19-gone", "a goal").unwrap();
        deleted("gone", DataChoice::Keep);
        for path in ["runs/2026-10-19-live", "config.json", "../elsewhere", "archive", "archive/../runs/2026-10-19-live"] {
            let err = remove(&["archive/2026-10-19-gone".to_string(), path.to_string()]).expect_err(path);
            assert_eq!(err.code, 3, "{path}");
        }
        assert!(crate::state::read_run("2026-10-19-live").is_ok());
        assert_eq!(find().len(), 1, "the listed one stays too when the request is refused");
    }

    #[test]
    fn a_directory_in_runs_that_is_not_a_run_is_listed_once_it_is_not_new() {
        let _h = TempHome::new();
        test_init("2026-10-19-real", "a goal").unwrap();
        std::fs::create_dir_all(crate::paths::runs_dir().join("12")).unwrap();
        assert!(find().is_empty(), "a run being created must not be offered for removal");
        let found = find_with(Duration::ZERO);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].path.as_str(), found[0].kind), ("runs/12", Kind::Stray));
        assert!(found[0].what.contains("empty"));
        assert!(!found.iter().any(|u| u.path.contains(".locks")), "otto's own directories are not strays");
    }

    #[test]
    fn sizes_read_as_a_person_would_say_them() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(840), "840 B");
        assert_eq!(size(64 * 1024), "64 KB");
        assert_eq!(size(1_250_000), "1.2 MB");
    }
}
