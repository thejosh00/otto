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
    "wake-reported-error", "budget-warning", "wake-complete", "wake-incomplete", "usage-limited",
    "budget-exhausted", "lock-acquired", "lock-released", "lock-broken", "lock-lost", "authorized",
    "wake-killed", "spawn-abandoned", "check-ran", "note-added", "notes-delivered", "notes-given-late", "note-dropped",
    "check-set", "renamed", "renumbered", "period-set", "check-cleared", "notified", "message-sent",
    "message-done", "paused", "unpaused", "start-failed", "handoff-refused", "output-published", "output-state",
    "stages-set", "done-claimed", "done-verified", "done-reopened", "feedback-sent", "waiting",
    "wait-overdue", "chat-started", "chat-ended", "change-submitted", "change-moved",
];

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
    WakeKilled {
        reason: String,
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
    },
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
}
