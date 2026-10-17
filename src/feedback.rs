//! Feedback about otto, from the wakes that use it.
//!
//! otto's worst bugs were found by a live model reading the harness, and the live tests cover only
//! the cases someone thought to write. Real runs cover the rest — skills nobody wrote for otto, real
//! repos, a run on its thirtieth day — and what they hit, the journal can't say. A wake that met the
//! contract but had to work around otto to do it (the harness and the instructions it runs
//! disagree, a refusal that doesn't say what to do instead) looks like any other; only the wake
//! knows. So it can say, with `otto state feedback`, and a person reads it with `otto feedback`.
//! Adapted from paperclip's suggestion box.
//!
//! - **Kept beside the runs, not in one**: `$OTTO_HOME/feedback.jsonl`, one report a line. It is
//!   about otto, not the run; the run's journal only notes that one was sent (`feedback-sent`).
//! - **A side channel, never a cost to the work**: the wake's command always exits 0, a wake gets
//!   `PER_WAKE`, and nothing notifies anyone. It is not the inbox, which is for the run's work.
//! - **Stamped by otto**: the run, the wake, what it wraps and which harness it was given, so the
//!   wake writes only what happened — and a report from before a harness fix can be told apart.
//! - **The wake's own words**: the harness forbids pasting from a ticket, a log or a page, because
//!   whoever reads these acts on them.

use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::event::Event;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const FILE: &str = "feedback.jsonl";
const LOCK: &str = ".feedback.lock";
/// One problem, said plainly, fits.
pub const MAX_BYTES: usize = 2048;
/// Reports one wake may send: enough for the friction that matters, too few to narrate a wake.
pub const PER_WAKE: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub id: u32,
    pub at: Timestamp,
    pub run: String,
    /// The wake that sent it; `None` when no wake was running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<u32>,
    /// What the run wraps: `skill legenda`, `instructions /path/flow.md`, `goal-only`.
    pub wraps: String,
    /// The harness the wake was given, hashed (`fnv`), so a report can be read against the harness
    /// it was about.
    pub harness: String,
    pub text: String,
    /// When a person marked it handled (`otto feedback --done`).
    #[serde(rename = "handledAt", default, skip_serializing_if = "Option::is_none")]
    pub handled_at: Option<Timestamp>,
}

fn path() -> PathBuf {
    crate::paths::otto_home().join(FILE)
}

/// Held while the file is read and written, so two wakes reporting at once never take one number.
struct Lock(std::fs::File);

impl Lock {
    fn take() -> Result<Self, OttoError> {
        let home = crate::paths::otto_home();
        std::fs::create_dir_all(&home)?;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(home.join(LOCK))?;
        fs4::fs_std::FileExt::lock_exclusive(&file)?;
        Ok(Self(file))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs4::fs_std::FileExt::unlock(&self.0);
    }
}

/// Every report, oldest first. A line that doesn't parse is skipped: the file is append-only and
/// otto's own, so one would be a torn write, not a report.
pub fn all() -> Vec<Report> {
    std::fs::read_to_string(path())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// A wake reports friction with otto itself. Refused when it is empty, long, or one too many for
/// the wake — `state::commands::feedback` turns every refusal into a notice, never a failure.
pub fn send(run: &str, text: &str) -> Result<Report, OttoError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(OttoError::usage("say what happened: what got in the way, and what would have helped"));
    }
    if text.len() > MAX_BYTES {
        return Err(OttoError::usage(format!(
            "a report is at most {MAX_BYTES} bytes (this one is {}) — one problem each, said plainly",
            text.len()
        )));
    }
    let id = crate::paths::resolve_run_id(run)?;
    let state = crate::state::read_run(&id)?;
    let wake = state.wake.as_ref().filter(|w| w.outcome.is_none()).map(|w| w.n);
    let wraps = match &state.wraps.reference {
        Some(reference) => format!("{} {reference}", crate::state::commands::kind_str(state.wraps.kind)),
        None => crate::state::commands::kind_str(state.wraps.kind).to_string(),
    };

    let report = {
        let _lock = Lock::take()?;
        let reports = all();
        if wake.is_some() && reports.iter().filter(|r| r.run == id && r.wake == wake).count() >= PER_WAKE {
            return Err(OttoError::conflict(format!(
                "this wake has sent {PER_WAKE} already — that is the most a wake sends; carry on with the work"
            )));
        }
        let report = Report {
            id: reports.iter().map(|r| r.id).max().unwrap_or(0) + 1,
            at: Timestamp::now(),
            run: id.clone(),
            wake,
            wraps,
            harness: crate::fnv::hex(crate::wake::prompt::HARNESS.as_bytes()),
            text: text.to_string(),
            handled_at: None,
        };
        let line = serde_json::to_string(&report)? + "\n";
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path())?;
        std::io::Write::write_all(&mut file, line.as_bytes())?;
        file.sync_all()?;
        report
    };
    crate::state::transaction(&id, |dir, _state| {
        crate::event::record(dir, &Event::FeedbackSent { report: report.id, wake: report.wake })
    })?;
    Ok(report)
}

/// Mark reports handled: read, and acted on or set aside. They stay in the file. Returns the ones
/// that weren't handled already; an unknown number is refused, naming the ones that exist.
pub fn mark_handled(ids: &[u32]) -> Result<Vec<u32>, OttoError> {
    let _lock = Lock::take()?;
    let mut reports = all();
    if let Some(missing) = ids.iter().find(|id| !reports.iter().any(|r| r.id == **id)) {
        return Err(OttoError::not_found(format!(
            "no report {missing} — `otto feedback --all` lists every one"
        )));
    }
    let now = Timestamp::now();
    let mut marked = Vec::new();
    for report in reports.iter_mut().filter(|r| ids.contains(&r.id) && r.handled_at.is_none()) {
        report.handled_at = Some(now);
        marked.push(report.id);
    }
    let mut text = String::new();
    for report in &reports {
        text.push_str(&serde_json::to_string(report)?);
        text.push('\n');
    }
    crate::state::write_atomic(&path(), &text)?;
    Ok(marked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;

    fn wake_running(id: &str, n: u32) {
        crate::state::transaction(id, |_p, s| {
            let now = Timestamp::now();
            s.wake = Some(crate::state::Wake {
                n,
                started_at: now,
                deadline_at: now,
                launcher: "claude".into(),
                pid: None,
                session: None,
                outcome: None,
            });
            Ok(())
        })
        .unwrap();
    }

    /// otto stamps what the wake can't be trusted to: which run, which wake, what it wraps, which
    /// harness. The run's journal says one was sent, and not what it said.
    #[test]
    fn a_report_is_kept_beside_the_runs_and_stamped_by_otto() {
        let _h = TempHome::new();
        test_init("f-send", "a goal").unwrap();
        wake_running("f-send", 3);
        let report = send("f-send", "  The refusal for my handoff named the word but not where it was.\n").unwrap();
        assert_eq!((report.id, report.wake), (1, Some(3)));
        assert_eq!(report.text, "The refusal for my handoff named the word but not where it was.");
        assert_eq!(report.harness, crate::fnv::hex(crate::wake::prompt::HARNESS.as_bytes()));
        assert_eq!(all().len(), 1);
        assert!(crate::paths::otto_home().join(FILE).is_file());
        let journal = std::fs::read_to_string(crate::paths::run_dir("f-send").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("\"event\":\"feedback-sent\""), "{journal}");
        assert!(!journal.contains("refusal for my handoff"), "the text stays out of the run's journal");
    }

    #[test]
    fn a_wake_sends_two_at_most_and_says_something_each_time() {
        let _h = TempHome::new();
        test_init("f-cap", "a goal").unwrap();
        assert!(send("f-cap", "   ").is_err());
        assert!(send("f-cap", &"x".repeat(MAX_BYTES + 1)).is_err());
        wake_running("f-cap", 1);
        send("f-cap", "one").unwrap();
        send("f-cap", "two").unwrap();
        let err = send("f-cap", "three").unwrap_err();
        assert!(err.message.contains("carry on"), "{}", err.message);
        wake_running("f-cap", 2);
        assert_eq!(send("f-cap", "the next wake has its own").unwrap().id, 3);
    }

    #[test]
    fn handled_reports_stay_and_say_when() {
        let _h = TempHome::new();
        test_init("f-done", "a goal").unwrap();
        for text in ["one", "two", "three"] {
            send("f-done", text).unwrap();
        }
        assert_eq!(mark_handled(&[1, 3]).unwrap(), vec![1, 3]);
        assert_eq!(mark_handled(&[3]).unwrap(), Vec::<u32>::new(), "already handled");
        let err = mark_handled(&[9]).unwrap_err();
        assert!(err.message.contains("no report 9"), "{}", err.message);
        let open: Vec<u32> = all().iter().filter(|r| r.handled_at.is_none()).map(|r| r.id).collect();
        assert_eq!(open, vec![2]);
        assert_eq!(all().len(), 3);
    }
}
