//! What a run's check script said, pass by pass, so "why didn't it wake?" still has an answer days
//! later. `run.json` keeps only the last result and the journal leaves out "nothing new", on purpose
//! (DESIGN.md §8.1), and on a run with a check nearly every pass is that.
//!
//! A file of its own in the run directory, one JSON object per line, for the same reason as
//! `wake-holder.json`: an older binary drops `run.json` fields it doesn't know and never opens this.
//! It sits outside everything `progress::mark` looks at, so a check finding nothing doesn't read as
//! the run moving. Only poke writes it, under the poke lock, so there is one writer at a time.

use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::state::CheckResult;
use serde::Serialize;

pub const FILE: &str = "check-history.jsonl";

/// About a week at a 15-minute period, for every run. Notes are capped, so this bounds the file.
const MAX_RECORDS: usize = 700;

/// One pass of a check script: poke's, never `otto check`'s trial or a dry run.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record<'a> {
    pub at: Timestamp,
    pub code: i32,
    pub timed_out: bool,
    pub result: CheckResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'a str>,
}

/// Add `record`, dropping the oldest past the cap. The whole file is replaced each time rather than
/// appended to, so a poke dying part way leaves the file as it was and never a torn last line. It
/// is a few hundred KB at most, and this happens once per pass.
pub fn add(run_id: &str, record: &Record) -> Result<(), OttoError> {
    let path = crate::paths::run_dir(run_id)?.join(FILE);
    let held = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        // Not "nothing there": replacing what can't be read would lose it.
        Err(e) => return Err(e.into()),
    };
    let held = String::from_utf8_lossy(&held);
    let lines: Vec<&str> = held.lines().collect();
    let kept = &lines[lines.len().saturating_sub(MAX_RECORDS - 1)..];

    let mut text = String::new();
    for line in kept {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str(&serde_json::to_string(record).expect("a record always serializes"));
    text.push('\n');
    crate::state::write_atomic(&path, &text)
}
