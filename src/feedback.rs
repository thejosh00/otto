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
//! It is also the backlog a run builds otto from, so a person files here too (`otto feedback add`,
//! or the web page), and each item carries how far it has got:
//!
//! - **Who said it decides what it is.** A person's item says what they want. A wake's is evidence:
//!   a wake that read something hostile can be steered, so a fix starts from reproducing it. Only a
//!   person's way in stamps `person`. That refuses a wake by its `OTTO_RUN_ID`, which is a guard
//!   against a mistake, not a lock: the lock is that a person approves whatever gets built.
//! - **One at a time.** `next` offers nothing while an item is being built, so a run can't stack up
//!   work faster than a person reviews it. Reopened items go first (a fix that didn't hold), then
//!   the oldest; the parts of a split item take its place in the queue, in order, so the slices of
//!   a big task land together rather than behind everything filed since.
//! - **Only ever forward, unless reopened**: `open → building (a change) → landed → verified`, or
//!   `closed` with the reason why. A split item waits as `split` until its parts are over.
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
/// What a person asks for can take more saying than a wake's report of one problem.
pub const PERSON_MAX_BYTES: usize = 16 * 1024;
/// Reports one wake may send: enough for the friction that matters, too few to narrate a wake.
pub const PER_WAKE: usize = 2;

/// Who filed it, which decides how far to trust what it says (above).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Wake,
    Person,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Open,
    /// Handed to a change, which is out with a person or the lander.
    Building,
    /// Its change landed; not yet shown to have fixed it.
    Landed,
    Verified,
    /// Broken into parts, which are what gets built.
    Split,
    Closed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Building => "building",
            Status::Landed => "landed",
            Status::Verified => "verified",
            Status::Split => "split",
            Status::Closed => "closed",
        }
    }

    fn is_over(self) -> bool {
        matches!(self, Status::Verified | Status::Closed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub id: u32,
    pub at: Timestamp,
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub status: Status,
    /// The run a wake's report came from; empty for a person's.
    #[serde(default)]
    pub run: String,
    /// The wake that sent it; `None` when no wake was running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<u32>,
    /// What the run wraps: `skill legenda`, `instructions /path/flow.md`, `goal-only`.
    #[serde(default)]
    pub wraps: String,
    /// The harness the wake was given, hashed (`fnv`), so a report can be read against the harness
    /// it was about.
    pub harness: String,
    pub text: String,
    /// When it left the queue: verified, closed, or marked with `otto feedback --done`.
    #[serde(rename = "handledAt", default, skip_serializing_if = "Option::is_none")]
    pub handled_at: Option<Timestamp>,
    /// The change building it, or the one that did: `7/7`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    /// Why it was closed or reopened, most recent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(rename = "duplicateOf", default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<u32>,
    /// The item this is a part of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    #[serde(rename = "reopenedAt", default, skip_serializing_if = "Option::is_none")]
    pub reopened_at: Option<Timestamp>,
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
/// otto's own, so one would be a torn write, not a report. One handled before reports had a status
/// reads as closed.
pub fn all() -> Vec<Report> {
    std::fs::read_to_string(path())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Report>(line).ok())
        .map(|mut report| {
            if report.handled_at.is_some() && !report.status.is_over() {
                report.status = Status::Closed;
            }
            report
        })
        .collect()
}

fn next_id(reports: &[Report]) -> u32 {
    reports.iter().map(|r| r.id).max().unwrap_or(0) + 1
}

fn append(report: &Report) -> Result<(), OttoError> {
    let line = serde_json::to_string(report)? + "\n";
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path())?;
    std::io::Write::write_all(&mut file, line.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn write_all(reports: &[Report]) -> Result<(), OttoError> {
    let mut text = String::new();
    for report in reports {
        text.push_str(&serde_json::to_string(report)?);
        text.push('\n');
    }
    crate::state::write_atomic(&path(), &text)
}

/// Change the file under its lock, and write it back only if `change` succeeds.
fn edit<T>(change: impl FnOnce(&mut Vec<Report>) -> Result<T, OttoError>) -> Result<T, OttoError> {
    let _lock = Lock::take()?;
    let mut reports = all();
    let out = change(&mut reports)?;
    write_all(&reports)?;
    Ok(out)
}

fn index_of(reports: &[Report], id: u32) -> Result<usize, OttoError> {
    reports
        .iter()
        .position(|r| r.id == id)
        .ok_or_else(|| OttoError::not_found(format!("no report {id} — `otto feedback --all` lists every one")))
}

/// A person says what they want built.
pub fn add(text: &str) -> Result<Report, OttoError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(OttoError::usage("say what you want: what should change, and how you'd know it had"));
    }
    if text.len() > PERSON_MAX_BYTES {
        return Err(OttoError::usage(format!("at most {PERSON_MAX_BYTES} bytes (this is {})", text.len())));
    }
    let _lock = Lock::take()?;
    let report = Report {
        id: next_id(&all()),
        at: Timestamp::now(),
        source: Source::Person,
        status: Status::Open,
        run: String::new(),
        wake: None,
        wraps: String::new(),
        harness: crate::fnv::hex(crate::wake::prompt::HARNESS.as_bytes()),
        text: text.to_string(),
        handled_at: None,
        change: None,
        why: None,
        duplicate_of: None,
        parent: None,
        reopened_at: None,
    };
    append(&report)?;
    Ok(report)
}

/// What to build now, if anything: nothing while something is being built, else whatever was
/// reopened, else the oldest, with a split item's parts in its place.
pub fn next() -> Option<Report> {
    let reports = all();
    if reports.iter().any(|r| r.status == Status::Building) {
        return None;
    }
    reports
        .into_iter()
        .filter(|r| r.status == Status::Open)
        .min_by_key(|r| (r.reopened_at.is_none(), r.parent.unwrap_or(r.id), r.id))
}

/// Why a run building from this should wake, for its check script: something to start, something
/// landed to verify, or the change being built back with its author or over. A note alone won't do
/// it: a check script stands in front of a run's wake, so one saying "nothing new" would leave a
/// landed change or a person's "change this" unread until the safety net.
pub fn due() -> Option<String> {
    let reports = all();
    if let Some(report) = reports.iter().find(|r| r.status == Status::Landed) {
        return Some(format!("report {} landed and wants verifying", report.id));
    }
    if let Some(report) = reports.iter().find(|r| r.status == Status::Building && change_wants_its_author(r)) {
        return Some(format!("report {}'s change wants its author", report.id));
    }
    next().map(|report| format!("report {} is next", report.id))
}

/// Over, sent back, or gone. An unreadable one is a wake too: only a wake can sort that out.
fn change_wants_its_author(report: &Report) -> bool {
    let found = report.change.as_deref().and_then(|text| {
        let r = crate::changes::ChangeRef::parse(text).ok()?;
        let dir = crate::paths::run_dir(&r.run).ok()?;
        crate::changes::index_or_empty(&dir).changes.into_iter().find(|c| c.n == r.n)
    });
    found.is_none_or(|c| c.state.is_over() || c.whose_move() == crate::changes::Move::Author)
}

/// Its change is out. Refused while another is, which is how `next`'s one-at-a-time holds for a run
/// that didn't ask it first.
pub fn building(id: u32, change: &str) -> Result<Report, OttoError> {
    // The run's whole id, not its short form: what's unambiguous today may not stay so.
    let change = crate::changes::ChangeRef::parse(change)?;
    let change = format!("{}/{}", change.run, change.n);
    edit(|reports| {
        if let Some(other) = reports.iter().find(|r| r.status == Status::Building && r.id != id) {
            return Err(OttoError::conflict(format!("report {} is being built already — one at a time", other.id)));
        }
        let i = index_of(reports, id)?;
        require(&reports[i], Status::Open)?;
        reports[i].status = Status::Building;
        reports[i].change = Some(change);
        Ok(reports[i].clone())
    })
}

pub fn landed(id: u32) -> Result<Report, OttoError> {
    edit(|reports| {
        let i = index_of(reports, id)?;
        require(&reports[i], Status::Building)?;
        reports[i].status = Status::Landed;
        Ok(reports[i].clone())
    })
}

pub fn verified(id: u32) -> Result<Report, OttoError> {
    edit(|reports| {
        let i = index_of(reports, id)?;
        require(&reports[i], Status::Landed)?;
        finish(&mut reports[i], Status::Verified);
        let done = reports[i].clone();
        settle_parent(reports, done.parent);
        Ok(done)
    })
}

/// It didn't hold, or shouldn't have been closed. Back to the front of the queue.
pub fn reopen(id: u32, why: &str) -> Result<Report, OttoError> {
    let why = said(why, "say why it's back")?;
    edit(|reports| {
        let i = index_of(reports, id)?;
        if !matches!(reports[i].status, Status::Landed | Status::Verified | Status::Closed) {
            return Err(OttoError::conflict(format!("report {id} is {}, so there's nothing to reopen", reports[i].status.as_str())));
        }
        let parent = reports[i].parent;
        let report = &mut reports[i];
        report.status = Status::Open;
        report.handled_at = None;
        report.change = None;
        report.duplicate_of = None;
        report.reopened_at = Some(Timestamp::now());
        report.why = Some(why);
        let done = report.clone();
        if let Some(parent) = parent {
            if let Some(parent) = reports.iter_mut().find(|r| r.id == parent) {
                parent.status = Status::Split;
                parent.handled_at = None;
            }
        }
        Ok(done)
    })
}

/// It won't be built: not otto's, a duplicate, already fixed, or no longer wanted. Whatever of it
/// hadn't finished goes with it.
pub fn close(id: u32, why: &str, duplicate_of: Option<u32>) -> Result<Report, OttoError> {
    let why = said(why, "say why it's closing")?;
    edit(|reports| {
        let i = index_of(reports, id)?;
        if reports[i].status.is_over() {
            return Err(OttoError::conflict(format!("report {id} is {} already", reports[i].status.as_str())));
        }
        if let Some(other) = duplicate_of {
            index_of(reports, other)?;
            if other == id {
                return Err(OttoError::usage("a report can't be a duplicate of itself"));
            }
        }
        reports[i].why = Some(why.clone());
        reports[i].duplicate_of = duplicate_of;
        finish(&mut reports[i], Status::Closed);
        let done = reports[i].clone();
        for part in reports.iter_mut().filter(|r| r.parent == Some(id) && !r.status.is_over()) {
            part.why = Some(format!("closed with {id}: {why}"));
            finish(part, Status::Closed);
        }
        settle_parent(reports, done.parent);
        Ok(done)
    })
}

/// Too big for one change: what's built instead is these, each landing on its own, in this order.
/// They keep who said it, so a wake's report stays evidence however it was divided.
pub fn split(id: u32, parts: &[String]) -> Result<Vec<Report>, OttoError> {
    if parts.len() < 2 {
        return Err(OttoError::usage("a split is two parts or more"));
    }
    let parts: Vec<String> = parts.iter().map(|p| said(p, "a part says what it builds")).collect::<Result<_, _>>()?;
    if let Some(long) = parts.iter().find(|p| p.len() > MAX_BYTES) {
        return Err(OttoError::usage(format!("a part is at most {MAX_BYTES} bytes (one is {})", long.len())));
    }
    edit(|reports| {
        let i = index_of(reports, id)?;
        require(&reports[i], Status::Open)?;
        if reports[i].parent.is_some() {
            return Err(OttoError::conflict(format!("report {id} is a part already; close it and split its parent's work differently")));
        }
        reports[i].status = Status::Split;
        let whole = reports[i].clone();
        let mut made = Vec::new();
        for text in parts {
            let part = Report {
                id: next_id(reports),
                at: Timestamp::now(),
                status: Status::Open,
                text,
                parent: Some(id),
                why: None,
                change: None,
                reopened_at: None,
                duplicate_of: None,
                handled_at: None,
                ..whole.clone()
            };
            reports.push(part.clone());
            made.push(part);
        }
        Ok(made)
    })
}

fn said(text: &str, otherwise: &str) -> Result<String, OttoError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(OttoError::usage(otherwise));
    }
    Ok(text.to_string())
}

fn require(report: &Report, status: Status) -> Result<(), OttoError> {
    if report.status == status {
        return Ok(());
    }
    Err(OttoError::conflict(format!("report {} is {}, not {}", report.id, report.status.as_str(), status.as_str())))
}

fn finish(report: &mut Report, status: Status) {
    report.status = status;
    report.handled_at = Some(Timestamp::now());
}

/// A split item is over when its parts are: verified if any of them got built, else closed.
fn settle_parent(reports: &mut [Report], parent: Option<u32>) {
    let Some(parent) = parent else { return };
    let parts = || reports.iter().filter(|r| r.parent == Some(parent));
    if !parts().all(|r| r.status.is_over()) {
        return;
    }
    let status = if parts().any(|r| r.status == Status::Verified) { Status::Verified } else { Status::Closed };
    if let Some(report) = reports.iter_mut().find(|r| r.id == parent && r.status == Status::Split) {
        finish(report, status);
    }
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
        if wake.is_some() && reports.iter().filter(|r| r.run == id && r.wake == wake && r.parent.is_none()).count() >= PER_WAKE {
            return Err(OttoError::conflict(format!(
                "this wake has sent {PER_WAKE} already — that is the most a wake sends; carry on with the work"
            )));
        }
        let report = Report {
            id: next_id(&reports),
            at: Timestamp::now(),
            source: Source::Wake,
            status: Status::Open,
            run: id.clone(),
            wake,
            wraps,
            harness: crate::fnv::hex(crate::wake::prompt::HARNESS.as_bytes()),
            text: text.to_string(),
            handled_at: None,
            change: None,
            why: None,
            duplicate_of: None,
            parent: None,
            reopened_at: None,
        };
        append(&report)?;
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
    edit(|reports| {
        for id in ids {
            index_of(reports, *id)?;
        }
        let mut marked = Vec::new();
        for id in ids {
            let i = index_of(reports, *id)?;
            if reports[i].handled_at.is_some() {
                continue;
            }
            let status = if reports[i].status == Status::Landed { Status::Verified } else { Status::Closed };
            finish(&mut reports[i], status);
            marked.push(*id);
            let parent = reports[i].parent;
            settle_parent(reports, parent);
        }
        Ok(marked)
    })
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

    fn status_of(id: u32) -> Status {
        all().into_iter().find(|r| r.id == id).unwrap().status
    }

    fn a_change() -> String {
        test_init("f-build", "a goal").unwrap();
        "f-build/1".to_string()
    }

    /// A report from before the file had any of this still reads, as what it was.
    #[test]
    fn an_old_report_reads_as_a_wake_s_and_closed_if_it_was_handled() {
        let _h = TempHome::new();
        std::fs::create_dir_all(crate::paths::otto_home()).unwrap();
        let old = r#"{"id":1,"at":"2026-10-21T01:32:15Z","run":"7","wake":3,"wraps":"goal-only","harness":"abc","text":"open"}
{"id":2,"at":"2026-10-21T01:32:15Z","run":"7","wake":3,"wraps":"goal-only","harness":"abc","text":"done","handledAt":"2026-10-22T01:32:15Z"}
"#;
        std::fs::write(crate::paths::otto_home().join(FILE), old).unwrap();
        let reports = all();
        assert_eq!((reports[0].source, reports[0].status), (Source::Wake, Status::Open));
        assert_eq!((reports[1].source, reports[1].status), (Source::Wake, Status::Closed));
        assert_eq!(next().unwrap().id, 1);
    }

    #[test]
    fn a_person_s_item_says_so_and_can_say_more() {
        let _h = TempHome::new();
        assert!(add("  ").is_err());
        assert!(add(&"x".repeat(PERSON_MAX_BYTES + 1)).is_err());
        let report = add(&"x".repeat(MAX_BYTES + 1)).unwrap();
        assert_eq!((report.source, report.status), (Source::Person, Status::Open));
        assert_eq!(report.run, "");
        assert_eq!(all().len(), 1, "and it's kept");
    }

    #[test]
    fn next_is_the_oldest_and_nothing_while_one_is_building() {
        let _h = TempHome::new();
        let change = a_change();
        assert!(next().is_none());
        add("first").unwrap();
        send("f-build", "second").unwrap();
        assert_eq!(next().unwrap().id, 1);
        building(1, &change).unwrap();
        assert!(next().is_none(), "one at a time");
        let err = building(2, &change).unwrap_err();
        assert!(err.message.contains("report 1"), "{}", err.message);
        assert_eq!(status_of(2), Status::Open, "refused means unchanged");
        landed(1).unwrap();
        assert_eq!(next().unwrap().id, 2, "landed lets the next one go");
    }

    #[test]
    fn it_only_goes_forward_and_a_refusal_changes_nothing() {
        let _h = TempHome::new();
        let change = a_change();
        add("a").unwrap();
        assert!(landed(1).is_err(), "nothing to land yet");
        assert!(verified(1).is_err());
        assert!(building(1, "not a change").is_err());
        assert!(building(1, "no-such-run/1").is_err());
        assert_eq!(status_of(1), Status::Open);
        assert_eq!(building(1, &change).unwrap().change.as_deref(), Some("f-build/1"));
        assert!(verified(1).is_err(), "not landed yet");
        landed(1).unwrap();
        let done = verified(1).unwrap();
        assert_eq!(done.status, Status::Verified);
        assert!(done.handled_at.is_some());
    }

    #[test]
    fn a_fix_that_didn_t_hold_goes_to_the_front() {
        let _h = TempHome::new();
        let change = a_change();
        add("older still").unwrap();
        add("the fix").unwrap();
        building(2, &change).unwrap();
        landed(2).unwrap();
        verified(2).unwrap();
        assert!(reopen(1, "open items aren't reopened").is_err());
        assert!(reopen(2, " ").is_err(), "a reason is required");
        let back = reopen(2, "the same complaint came back").unwrap();
        assert_eq!((back.status, back.handled_at, back.change), (Status::Open, None, None));
        assert_eq!(back.why.as_deref(), Some("the same complaint came back"));
        assert_eq!(next().unwrap().id, 2, "ahead of the older one");
    }

    #[test]
    fn closing_says_why_and_can_name_the_duplicate() {
        let _h = TempHome::new();
        add("one").unwrap();
        add("one again").unwrap();
        assert!(close(2, "", None).is_err());
        assert!(close(2, "same", Some(9)).is_err());
        assert!(close(2, "same", Some(2)).is_err());
        assert_eq!(status_of(2), Status::Open);
        let closed = close(2, "same", Some(1)).unwrap();
        assert_eq!((closed.status, closed.duplicate_of), (Status::Closed, Some(1)));
        assert!(close(2, "again", None).is_err(), "over already");
        assert_eq!(next().unwrap().id, 1);
    }

    /// A big task's slices come before anything filed after it, in order, and it's over when they
    /// are. They keep who said it: dividing a wake's report doesn't make it a person's.
    #[test]
    fn a_split_item_is_built_as_its_parts_in_its_place() {
        let _h = TempHome::new();
        let change = a_change();
        send("f-build", "a big complaint").unwrap();
        add("filed after").unwrap();
        assert!(split(1, &["only one".to_string()]).is_err());
        let parts = split(1, &["first slice".to_string(), "second slice".to_string()]).unwrap();
        assert_eq!(parts.iter().map(|p| p.id).collect::<Vec<_>>(), vec![3, 4]);
        assert!(parts.iter().all(|p| p.source == Source::Wake && p.parent == Some(1)));
        assert_eq!(status_of(1), Status::Split);
        assert!(split(3, &["a".to_string(), "b".to_string()]).is_err(), "one level");

        assert_eq!(next().unwrap().id, 3);
        building(3, &change).unwrap();
        landed(3).unwrap();
        assert_eq!(next().unwrap().id, 4, "the next slice waits only for the last to land");
        verified(3).unwrap();
        assert_eq!(status_of(1), Status::Split, "one part is left");
        building(4, &change).unwrap();
        landed(4).unwrap();
        verified(4).unwrap();
        assert_eq!(status_of(1), Status::Verified, "over when they are");
        assert!(all().iter().find(|r| r.id == 1).unwrap().handled_at.is_some());
        assert_eq!(next().unwrap().id, 2);

        reopen(4, "it came back").unwrap();
        assert_eq!(status_of(1), Status::Split, "and not over once one is back");
    }

    #[test]
    fn closing_a_split_item_closes_what_s_left_of_it() {
        let _h = TempHome::new();
        add("big").unwrap();
        split(1, &["a".to_string(), "b".to_string()]).unwrap();
        close(1, "no longer wanted", None).unwrap();
        assert_eq!((status_of(2), status_of(3)), (Status::Closed, Status::Closed));
        assert!(next().is_none());
    }

    /// Everything that wants the run has to get past its check script.
    #[test]
    fn due_says_what_wants_a_wake_and_stays_quiet_while_a_change_waits_on_you() {
        use crate::changes::tests::{r, repo, submit_fix};
        let _h = TempHome::new();
        test_init("f-due", "a goal").unwrap();
        assert_eq!(due(), None);
        add("a thing").unwrap();
        assert!(due().unwrap().contains("report 1 is next"));

        let repo = repo();
        crate::changes::submit("f-due", &submit_fix(&repo, "Shout")).unwrap();
        building(1, "f-due/1").unwrap();
        assert_eq!(due(), None, "it's with the person");

        crate::changes::revise(&r("f-due", 1), "shout louder").unwrap();
        assert!(due().unwrap().contains("wants its author"), "sent back");

        crate::changes::drop_change(&r("f-due", 1), Some("not this way")).unwrap();
        assert!(due().unwrap().contains("wants its author"), "dropped: a wake closes or rebuilds it");

        landed(1).unwrap();
        assert!(due().unwrap().contains("wants verifying"));
        verified(1).unwrap();
        assert_eq!(due(), None);
    }

    #[test]
    fn due_wakes_for_a_change_it_can_t_find() {
        let _h = TempHome::new();
        let change = a_change();
        add("a thing").unwrap();
        building(1, &change).unwrap();
        assert!(due().unwrap().contains("wants its author"));
    }

    #[test]
    fn parts_don_t_use_up_the_wake_s_reports() {
        let _h = TempHome::new();
        test_init("f-parts", "a goal").unwrap();
        wake_running("f-parts", 1);
        send("f-parts", "big").unwrap();
        split(1, &["a".to_string(), "b".to_string()]).unwrap();
        send("f-parts", "the wake's second").unwrap();
    }
}
