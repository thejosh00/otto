//! The person's approval of a change, kept outside its run
//!
//! `$OTTO_HOME/approvals/<run id>/<n>.json`, one per change: written when the person approves it
//! (`changes::approve`), before the run's index says approved, and removed when they take it back
//! (`revise` without a pull request, `drop`). A later approval of the same change replaces it.
//!
//! It is outside the run directory because a wake can write its own run's index, and a sandboxed
//! wake cannot write here (`sandbox`: `approvals/` is closed after the record's grants, so no
//! record's `write` list opens it). It holds everything a check needs about what was approved, so
//! none of it has to come from the index.
//!
//! Nothing reads it as the authority yet: `changes::check` still reads the run's index. That is
//! the next change. Until then the record is written, kept in step by the person's commands, and
//! unused.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::changes::LandMode;
use crate::clock::Timestamp;
use crate::error::OttoError;

/// What the person approved: which version of which change, and how it lands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// The run's id when it was written. A record whose `run` isn't the run asked about doesn't count.
    pub run: String,
    pub n: u32,
    pub version: u32,
    pub sha: String,
    pub patch_id: String,
    /// The repository's main checkout, absolute, and the branch it lands on, as they were approved.
    pub repo: String,
    pub base: String,
    pub branch: String,
    pub mode: LandMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overridden: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merge_when_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_changes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub at: Timestamp,
    /// The person said to merge its pull request, and the commit the pull request had when they did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_asked: Option<MergeAsked>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeAsked {
    pub at: Timestamp,
    pub head: String,
}

pub fn dir() -> PathBuf {
    crate::paths::otto_home().join("approvals")
}

/// A run's records. `run` is a run id, which is one path component: anything else has no records.
pub fn run_dir(run: &str) -> Result<PathBuf, OttoError> {
    if run.is_empty() || run == "." || run == ".." || run.contains(['/', '\\', '\0']) {
        return Err(OttoError::usage(format!("{run:?} is not a run id")));
    }
    Ok(dir().join(run))
}

pub fn path(run: &str, n: u32) -> Result<PathBuf, OttoError> {
    Ok(run_dir(run)?.join(format!("{n}.json")))
}

/// Write the record, replacing the one its change had. An error says why in plain words: the
/// caller writes this before the run's index, so a failure here leaves the change unapproved.
pub fn write(record: &Record) -> Result<(), OttoError> {
    let path = path(&record.run, record.n)?;
    let text = serde_json::to_string_pretty(record)?;
    let denied = |e: &std::io::Error| e.kind() == std::io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(1 | 13));
    let refused = |why: &str, denied: bool| {
        let where_ = crate::changes::home_relative(&dir().to_string_lossy());
        if denied {
            OttoError::conflict(format!(
                "not approved: approving is the person's, and this looks like it is running inside a sandbox, which can't write {where_}/ ({why})"
            ))
        } else {
            OttoError::conflict(format!("not approved: the approval couldn't be written under {where_}/ ({why})"))
        }
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| refused(&e.to_string(), denied(&e)))?;
    }
    // `write_atomic` gives back words, not the error it had: the two the system uses for "not allowed".
    crate::state::write_atomic(&path, &format!("{text}\n"))
        .map_err(|e| refused(&e.message, e.message.contains("Operation not permitted") || e.message.contains("Permission denied")))
}

/// The change's record, or `None` when it has none or it can't be read as one.
pub fn read(run: &str, n: u32) -> Option<Record> {
    let text = std::fs::read_to_string(path(run, n).ok()?).ok()?;
    serde_json::from_str(&text).ok()
}

/// Change the record a change has, if it has one. A change with no record is left without one:
/// only approving makes it.
pub fn update(run: &str, n: u32, f: impl FnOnce(&mut Record)) -> Result<(), OttoError> {
    match read(run, n) {
        Some(mut record) => {
            f(&mut record);
            write(&record)
        }
        None => Ok(()),
    }
}

/// Take the record away. Having none already is fine.
pub fn remove(run: &str, n: u32) -> Result<(), OttoError> {
    let path = path(run, n)?;
    match std::fs::remove_file(&path) {
        Ok(()) => {
            // The run's directory goes with its last record; not empty is the usual answer.
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(OttoError::conflict(format!("its approval on record couldn't be removed ({}): {e}", crate::changes::home_relative(&path.to_string_lossy())))),
    }
}

/// A renumbered run's records follow it. Best effort: a record left behind names a run that no
/// longer exists, so it counts for nothing and the person approves again.
pub fn moved(from: &str, to: &str) {
    if let (Ok(old), Ok(new)) = (run_dir(from), run_dir(to)) {
        if old.is_dir() {
            let _ = std::fs::rename(&old, &new);
        }
    }
}
