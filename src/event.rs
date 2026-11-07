//! Otto's own journal events, typed.
//!
//! The journal is the audit trail — DESIGN.md §13 calls it "the only honest account" of a run a
//! week later. Before this module, an event was a `&str` name plus a `serde_json::json!({...})`
//! bag of fields, written inline at each of some twenty call sites across `state::commands`,
//! `wake`, `state::locks` and `state::authorize`. Nothing enumerated the vocabulary, `otto logs`
//! could only ever render generic `key=value` pairs, and a test asserting on the shape of one
//! (`journal.contains("\"inputTokens\":150")`) was checking a string, not a schema.
//!
//! This covers only the events **otto's own control flow** emits — the ones a person never
//! chooses the name or shape of. `otto state log` stays exactly as free-form as DESIGN.md
//! describes ("one `log --event error` per real failure"): a wrapped skill or wake can journal
//! any event name with any fields, and that has to stay open-ended or wrapping an arbitrary
//! skill (DESIGN.md §1) would mean pre-declaring its vocabulary. That path still goes through
//! the untyped `state::journal`.

use crate::clock::Timestamp;
use crate::state::{Budget, CheckResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The name of every event otto writes, as the journal has it. Anything else in a journal, a wake
/// wrote with `otto state log`, and that is what a run's activity feed shows (`core::activity`).
/// By name rather than by type, because otto also writes some of these untyped (the lock events).
/// A test in `doc_examples` keeps this in step with `Event`, so a variant added without its name
/// here fails rather than putting otto's bookkeeping in the feed.
pub const OTTO_EVENTS: &[&str] = &[
    "run-created", "phase-changed", "status-changed", "facts-recorded", "gate-opened", "gate-closed",
    "gate-expired", "timer-armed", "tick", "noop-tick", "wake-started", "wake-spent", "usage-unavailable",
    "sandbox-refused",
    "wake-reported-error", "budget-warning", "wake-complete", "wake-incomplete", "usage-limited",
    "budget-exhausted", "lock-acquired", "lock-released", "lock-broken", "lock-lost", "authorized",
    "wake-killed", "spawn-abandoned", "check-ran", "note-added", "notes-delivered", "notes-given-late", "note-dropped",
    "check-set", "renamed", "renumbered", "period-set", "policy-set", "check-cleared", "notified", "message-sent",
    "message-done", "paused", "unpaused", "start-failed", "handoff-refused", "output-published", "output-state",
    "stages-set", "done-claimed", "done-verified", "done-reopened", "feedback-sent", "feedback-assigned", "waiting",
    "wait-overdue", "chat-started", "chat-ended", "change-submitted", "change-moved",
];

/// What started a wake, as `wake-started` records it. It crosses a process boundary: whoever starts
/// a wake knows why, and the wake that records it is a child, told with a hidden `--cause`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum WakeCause {
    /// The run's period came round, with no check in front of it.
    Period,
    /// A timer the last wake armed for itself came due.
    Timer,
    /// A check script in front of a due wake said something changed.
    CheckChanged,
    /// A check script failed or timed out, and poke woke the run to look.
    CheckError,
    /// A check said "nothing new" enough times in a row that poke woke the run anyway.
    CheckSafetyNet,
    /// A person answered its gate.
    GateAnswered,
    /// Its gate's time ran out, and a wake has to record the silence.
    GateExpired,
    /// Something was said for it to read: a person's note, a reply to its message, or word about a
    /// change of its that wants more work.
    Note,
    /// A person started it: `otto run`, `otto wake`, `otto resume`, the web page's buttons.
    Person,
    /// A person approved a change, and this is the lander being started to land it.
    ChangeApproved,
    /// A change of this run's landed.
    ChangeLanded,
    /// Again, after a wake that didn't finish or a start that produced no wake.
    Retry,
    /// Something was asked of the run while its last wake was running, too late for that wake.
    AskedMidWake,
    /// Poke found nothing running, no gate and no wake time.
    Stranded,
    /// Straight after the wake that claimed done: this one checks the claim.
    FollowOnClaim,
    /// Straight after the check that sent a done claim back: this one does the fix.
    FollowOnReopen,
    /// Straight after a wake that a person's answer arrived too late for: this one is given it.
    FollowOnAnswer,
}

/// Whether the record beside the wake lock is the wake a kill acted on. flock can't say who holds
/// it, so this is as far as otto can honestly go: the record says who took it last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KillHolder {
    /// The record names this wake, by number and pid. It took the lock; `signalLanded` says whether
    /// it was still there, since a wake killed outright leaves this record behind too.
    ThisWake,
    /// The record is someone else's: a leftover from an earlier wake, a wake too new to have its
    /// number, or something that isn't a wake. See `holderRecord`, which is not this wake's.
    NotThisWake,
    /// No record, or none that can be read: taken by a binary from before they were written.
    NotKnown,
}

impl KillHolder {
    pub fn of(wake: Option<&crate::state::Wake>, record: Option<&crate::liveness::Holder>) -> Self {
        match (wake, record) {
            (_, None) => Self::NotKnown,
            (Some(wake), Some(record)) if record.wake == Some(wake.n) && wake.pid.is_none_or(|pid| pid == record.pid) => Self::ThisWake,
            (_, Some(_)) => Self::NotThisWake,
        }
    }
}

impl WakeCause {
    /// As the journal and `--cause` both spell it.
    pub fn label(self) -> String {
        clap::ValueEnum::to_possible_value(&self).expect("none is skipped").get_name().to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    RunCreated {
        wraps: String,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        goal: String,
        #[serde(rename = "doneCondition", skip_serializing_if = "Option::is_none")]
        done_condition: Option<String>,
        perpetual: bool,
        phase: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        launcher: String,
    },
    PhaseChanged {
        from: String,
        to: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    StatusChanged {
        from: String,
        to: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Only when `to` is `blocked`: the `BlockedCause` label, so the journal says why.
        #[serde(skip_serializing_if = "Option::is_none")]
        because: Option<String>,
        /// Only when `to` is `done`: the todo items still open at that moment (`crate::todo`).
        #[serde(rename = "leftOpen", default, skip_serializing_if = "Vec::is_empty")]
        left_open: Vec<String>,
    },
    /// A wake said the run is done; a fresh wake checks before it is (`state::DoneClaim`).
    DoneClaimed {
        wake: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(rename = "leftOpen", default, skip_serializing_if = "Vec::is_empty")]
        left_open: Vec<String>,
    },
    /// The check found the claim holds. A `status-changed` to `done` follows it.
    DoneVerified {
        reason: String,
        #[serde(rename = "claimedBy")]
        claimed_by: u32,
    },
    /// The check found the claim does not hold, and the run went back to work.
    DoneReopened {
        reason: String,
        reopens: u32,
    },
    FactsRecorded {
        facts: Map<String, Value>,
    },
    GateOpened {
        gate: String,
        slug: String,
        phase: String,
        #[serde(rename = "expiresAt", skip_serializing_if = "Option::is_none")]
        expires_at: Option<Timestamp>,
        /// The output version the question is about (`design@1`), when it is about one.
        #[serde(skip_serializing_if = "Option::is_none")]
        about: Option<String>,
    },
    GateClosed {
        gate: String,
        slug: String,
        #[serde(rename = "askedAt")]
        asked_at: Timestamp,
        #[serde(rename = "answeredAt")]
        answered_at: Timestamp,
        answer: String,
    },
    GateExpired {
        gate: String,
        slug: String,
        #[serde(rename = "askedAt")]
        asked_at: Timestamp,
        #[serde(rename = "answeredAt")]
        answered_at: Timestamp,
        answer: String,
    },
    TimerArmed {
        #[serde(rename = "nextWakeAt")]
        next_wake_at: Timestamp,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        /// What the wake this brings is for, when that is more than a time coming round. Poke reads
        /// it when it starts that wake: `run.json` can't carry it, because an older binary drops
        /// fields it doesn't know, and a rollback must not lose more than a label.
        #[serde(skip_serializing_if = "Option::is_none")]
        cause: Option<WakeCause>,
    },
    Tick {
        #[serde(rename = "ticksWithoutProgress")]
        ticks_without_progress: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    NoopTick {
        #[serde(rename = "ticksWithoutProgress")]
        ticks_without_progress: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    WakeStarted {
        wake: u32,
        #[serde(rename = "deadlineAt")]
        deadline_at: Timestamp,
        launcher: String,
        /// The claude session the wake runs as, so its transcript can be found after the next wake
        /// has replaced `wake.session` in `run.json` — `otto chat` lists them. Absent before 2026-10.
        #[serde(skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// What started it. Absent before it was recorded: read that as "not recorded", never as a
        /// guess.
        #[serde(skip_serializing_if = "Option::is_none")]
        cause: Option<WakeCause>,
    },
    WakeSpent {
        turns: u64,
        #[serde(rename = "spentWakes")]
        spent_wakes: u32,
        /// The run's tokens so far, this wake's included — what a token budget is measured against.
        #[serde(rename = "spentTokens", default)]
        spent_tokens: u64,
        #[serde(rename = "inputTokens")]
        input_tokens: u64,
        #[serde(rename = "outputTokens")]
        output_tokens: u64,
        #[serde(rename = "cacheRead")]
        cache_read: u64,
        #[serde(rename = "cacheCreation")]
        cache_creation: u64,
        #[serde(rename = "toolErrors")]
        tool_errors: u64,
    },
    UsageUnavailable {
        note: String,
    },
    /// The sandbox wrapper wouldn't start anything, or `otto wake` or `otto chat` wouldn't because the run
    /// disagrees with its record, and why. No token, or any of a token, in `why`.
    SandboxRefused {
        why: String,
    },
    WakeReportedError {
        #[serde(rename = "exitCode")]
        exit_code: i32,
        #[serde(rename = "timedOut")]
        timed_out: bool,
        turns: u64,
    },
    BudgetWarning {
        #[serde(rename = "fractionSpent")]
        fraction_spent: f64,
        budget: Budget,
    },
    WakeComplete {
        status: String,
        /// Whether it moved the run: otto saw its work change, or it said `tick --progress`.
        progress: bool,
    },
    WakeIncomplete {
        reason: String,
        consecutive: u32,
    },
    /// A wake ran into the account's usage limit. Not counted as incomplete: the run sleeps until
    /// the limit resets instead (`wake::record_usage_limited`).
    UsageLimited {
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<String>,
        #[serde(rename = "resetsAt", skip_serializing_if = "Option::is_none")]
        resets_at: Option<Timestamp>,
    },
    BudgetExhausted {
        reason: String,
    },
    LockAcquired {
        repo: String,
    },
    LockReleased {
        repo: String,
    },
    LockBroken {
        repo: String,
        #[serde(rename = "previousOwner")]
        previous_owner: String,
        reason: String,
    },
    LockLost {
        repo: String,
        #[serde(rename = "takenBy")]
        taken_by: String,
        reason: String,
    },
    Authorized {
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        item: Option<String>,
        head: String,
        by: String,
        source: String,
    },
    /// Poke's own bookkeeping — the process-control counterpart to `wake-incomplete`. A killed
    /// wake cannot record its own failure, so poke journals this and calls `record_incomplete`
    /// on its behalf (see `spawner::kill`).
    ///
    /// A kill records the evidence it acted on. All but `reason` are absent on lines from before
    /// they were recorded: read those as "not recorded", never as false or none.
    WakeKilled {
        reason: String,
        /// The wake the decision acted on: its number and recorded pid, from `run.json`.
        #[serde(skip_serializing_if = "Option::is_none")]
        wake: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(rename = "deadlineAt", skip_serializing_if = "Option::is_none")]
        deadline_at: Option<Timestamp>,
        /// That wake's outcome, written even when it has none: `null` is what the decision saw.
        outcome: Option<crate::state::WakeOutcome>,
        #[serde(skip_serializing_if = "Option::is_none")]
        holder: Option<KillHolder>,
        /// The record beside the lock, whoever's it was. `holder` says whether it is this wake's.
        #[serde(rename = "holderRecord", skip_serializing_if = "Option::is_none")]
        holder_record: Option<crate::liveness::Holder>,
        #[serde(rename = "signalLanded", skip_serializing_if = "Option::is_none")]
        signal_landed: Option<bool>,
        #[serde(rename = "tmuxKilled", skip_serializing_if = "Option::is_none")]
        tmux_killed: Option<bool>,
    },
    /// Poke gave up spawning this run after repeated attempts produced no wake. Journaled once
    /// per streak, not every pass — see `poke::spawn_or_back_off`.
    SpawnAbandoned {
        reason: String,
    },
    /// An opt-in check script (DESIGN.md §8) reported something other than "no change" — a
    /// no-change result never reaches the journal, only `run.json`'s own `check` field, or a
    /// tight cadence over days would spam the journal the way `heartbeatAt` used to (§17).
    CheckRan {
        result: CheckResult,
        #[serde(rename = "consecutiveNoChange")]
        consecutive_no_change: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// A person left a note for the run (`otto note`). The text is in `file`, verbatim.
    NoteAdded {
        note: String,
        file: String,
        standing: bool,
    },
    /// A completed wake carried these one-off notes, so they are delivered and leave `run.json`.
    NotesDelivered {
        notes: Vec<String>,
        wake: u32,
    },
    /// A wake was about to ask a question (`gate`), claim done (`done`) or confirm a claim
    /// (`confirm`) without these notes, added while it ran. Refused once, with the notes, which are
    /// now given to it.
    NotesGivenLate {
        notes: Vec<String>,
        wake: u32,
        before: String,
    },
    /// A wake told otto's maker what got in its way (`feedback`). The report is in
    /// `$OTTO_HOME/feedback.jsonl`, not here: it is about otto, not the run.
    FeedbackSent {
        report: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        wake: Option<u32>,
    },
    /// The driver run filed work in the queue (`feedback::assign`). The item is in
    /// `$OTTO_HOME/feedback.jsonl`, not here.
    FeedbackAssigned {
        report: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        wake: Option<u32>,
    },
    /// A wake started waiting on something with a limit (`arm-timer --waiting-for`).
    Waiting {
        #[serde(rename = "for")]
        what: String,
        #[serde(rename = "giveUpAt", skip_serializing_if = "Option::is_none")]
        give_up_at: Option<Timestamp>,
    },
    /// A wake tried to sleep again for a wait whose limit had passed, and otto refused.
    WaitOverdue {
        #[serde(rename = "for")]
        what: String,
        since: Timestamp,
        #[serde(rename = "giveUpAt")]
        give_up_at: Timestamp,
    },
    /// A person withdrew a note before delivery, or retired a standing one.
    NoteDropped {
        note: String,
        standing: bool,
    },
    /// A standing check script was set — by a person (`otto check`) or a wake (`set-check`).
    CheckSet {
        script: String,
        #[serde(rename = "wakeAfter")]
        wake_after: u32,
        /// `person` or `wake`.
        by: String,
    },
    /// A person renamed the run (`otto rename`).
    Renamed { from: String, to: String },
    /// `otto renumber` gave a run from before numbering its run number; `from` is the old id.
    Renumbered { from: String },
    /// A person changed how often the run wakes (`otto period`).
    PeriodSet {
        #[serde(rename = "periodMinutes")]
        period_minutes: u64,
        #[serde(rename = "previousMinutes")]
        previous_minutes: u64,
        /// The time of day its period wakes are due, when it has one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<crate::clock::TimeOfDay>,
        #[serde(rename = "previousAt", default, skip_serializing_if = "Option::is_none")]
        previous_at: Option<crate::clock::TimeOfDay>,
        /// The days of the week its period wakes fall on, when it keeps to some.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        days: Option<crate::clock::Weekdays>,
        #[serde(rename = "previousDays", default, skip_serializing_if = "Option::is_none")]
        previous_days: Option<crate::clock::Weekdays>,
    },
    /// A person changed the run's policy (`otto policy`): each key it set, with what it was before
    /// (`null` when it had no value of its own).
    PolicySet { set: serde_json::Map<String, serde_json::Value>, previous: serde_json::Map<String, serde_json::Value> },
    /// The run's standing check script was removed; every wake is a full one again.
    CheckCleared {
        script: String,
        /// `person` or `wake`.
        by: String,
    },
    /// Poke sent a desktop notification about this run — once per `key`, so the journal says
    /// when a person was told, not just when the thing happened. See `notify`.
    Notified {
        key: String,
        title: String,
        message: String,
    },
    /// A wake sent the person a message (`otto state message`): a report, a draft, an update.
    MessageSent {
        message: String,
        kind: crate::messages::MessageKind,
        title: String,
        file: String,
    },
    /// The person marked a message done (`otto inbox done`), with what they replied, if anything.
    MessageDone {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reply: Option<String>,
        /// The note the reply became, when there was one.
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// A person paused the run (`otto pause`): no scheduled wake until `until`, or until resumed.
    Paused {
        #[serde(skip_serializing_if = "Option::is_none")]
        until: Option<Timestamp>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The pause ended: `person` (`otto resume`) or `time` (its `until` passed).
    Unpaused {
        by: String,
    },
    /// A wake never got going — claude exited at once, having done nothing. Retried on a backoff
    /// for as long as it takes, and never counted toward blocking the run (`wake::couldnt_start`).
    StartFailed {
        reason: String,
        consecutive: u32,
    },
    /// `otto state handoff` refused a handoff — past its cap, or speaking to the person in otto's
    /// own words (`plain`). Journaled so how often a wake has to rewrite is measured, not guessed.
    HandoffRefused {
        reason: String,
    },
    /// A wake published a version of an output (`otto state publish`): its first, when `version`
    /// is 1. See `outputs`.
    OutputPublished {
        output: String,
        version: u32,
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        stage: Option<String>,
        /// Where its text is, relative to the run directory — or `link`, for work kept elsewhere.
        #[serde(skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        link: Option<String>,
    },
    /// An output was marked waiting, done or dropped — by the `person` ("I sent it") or a `wake`.
    OutputState {
        output: String,
        state: crate::outputs::OutputState,
        by: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// A wake declared the steps its work moves through (`otto state stages`).
    StagesSet {
        stages: Vec<String>,
    },
    /// A wake submitted a version of a change — code on a branch, to be reviewed and landed (`changes`).
    ChangeSubmitted {
        change: u32,
        version: u32,
        title: String,
        sha: String,
        branch: String,
    },
    /// A change moved on: approved, sent back, dropped, landed, cleaned up. `by` is `person`,
    /// `author`, or the lander's run id; `what` says it in words, as the change's timeline does.
    ChangeMoved {
        change: u32,
        state: crate::changes::ChangeState,
        by: String,
        what: String,
    },
    /// A person opened a chat about the run (`otto chat`). Not a wake: it changes nothing but the
    /// notes the person leaves from it.
    ChatStarted {
        session: String,
        launcher: String,
    },
    /// The chat ended. What it used, read from its transcript the way a wake's is — absent when
    /// that can't be read — and the notes left while it was open.
    ChatEnded {
        session: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        turns: Option<u64>,
        #[serde(rename = "inputTokens", skip_serializing_if = "Option::is_none")]
        input_tokens: Option<u64>,
        #[serde(rename = "outputTokens", skip_serializing_if = "Option::is_none")]
        output_tokens: Option<u64>,
        #[serde(rename = "cacheRead", skip_serializing_if = "Option::is_none")]
        cache_read: Option<u64>,
        #[serde(rename = "cacheCreation", skip_serializing_if = "Option::is_none")]
        cache_creation: Option<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        notes: Vec<String>,
    },
}

/// Write one typed event to `path`'s journal, stamped with the current time. The caller must
/// already hold the run's lock — this does not acquire one, matching the untyped `journal()` it
/// sits beside. Prefer `state::log_event` from outside `state` itself, which takes the lock too.
pub fn record(path: &std::path::Path, event: &Event) -> Result<(), crate::error::OttoError> {
    let mut value = serde_json::to_value(event)?;
    if let Value::Object(map) = &mut value {
        map.insert("ts".to_string(), Value::String(crate::clock::now_iso()));
    }
    crate::state::append_journal_line(path, &value)
}

#[cfg(test)]
mod tests {
    /// A line from before a kill recorded its evidence still reads, with none of it invented.
    #[test]
    fn an_old_wake_killed_line_still_reads() {
        let old: super::Event = serde_json::from_str(r#"{"event":"wake-killed","reason":"wake is 16m past its deadline"}"#).unwrap();
        let super::Event::WakeKilled { reason, wake, deadline_at, holder, signal_landed, tmux_killed, .. } = old else {
            panic!("not a wake-killed")
        };
        assert_eq!(reason, "wake is 16m past its deadline");
        assert!(wake.is_none() && deadline_at.is_none() && holder.is_none() && signal_landed.is_none() && tmux_killed.is_none());
    }

    use super::*;

    /// The tag lands as `"event": "kebab-case-name"`, matching the untyped journal's own
    /// `"event"` key so `otto logs` and every existing test keep reading the same shape.
    #[test]
    fn the_tag_is_kebab_case_and_named_event() {
        let value = serde_json::to_value(Event::WakeComplete { status: "sleeping".into(), progress: true }).unwrap();
        assert_eq!(value["event"], "wake-complete");
        assert_eq!(value["status"], "sleeping");
    }

    #[test]
    fn absent_optional_fields_are_omitted_not_null() {
        let value = serde_json::to_value(Event::StatusChanged {
            from: "running".into(),
            to: "done".into(),
            reason: None,
            because: None,
            left_open: vec![],
        })
        .unwrap();
        assert!(value.get("reason").is_none(), "a None field must not appear, not appear as null");
        assert!(value.get("leftOpen").is_none(), "nor an empty list");
    }

    /// Three things have to spell a cause the same: the journal, the flag it crosses processes on,
    /// and what is read back.
    #[test]
    fn a_cause_is_spelled_the_same_in_the_journal_and_on_the_command_line() {
        for cause in <WakeCause as clap::ValueEnum>::value_variants() {
            let journal = serde_json::to_value(cause).unwrap();
            assert_eq!(journal, cause.label().as_str());
            assert_eq!(<WakeCause as clap::ValueEnum>::from_str(&cause.label(), false).as_ref(), Ok(cause));
        }
    }

    /// Every line written before causes were recorded has to go on reading.
    #[test]
    fn a_wake_started_line_with_no_cause_reads_and_stays_without_one() {
        let old = serde_json::json!({
            "event": "wake-started", "wake": 3, "deadlineAt": "2026-09-12T12:00:00Z", "launcher": "claude"
        });
        let event: Event = serde_json::from_value(old).unwrap();
        assert!(matches!(event, Event::WakeStarted { cause: None, .. }));
        assert!(serde_json::to_value(&event).unwrap().get("cause").is_none(), "nothing is made up on the way back");
    }
}
