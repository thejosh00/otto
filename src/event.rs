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
    },
    WakeSpent {
        turns: u64,
        #[serde(rename = "spentWakes")]
        spent_wakes: u32,
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
    },
    WakeIncomplete {
        reason: String,
        consecutive: u32,
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
    /// A person withdrew a note before delivery, or retired a standing one.
    NoteDropped {
        note: String,
        standing: bool,
    },
    /// Poke sent a desktop notification about this run — once per `key`, so the journal says
    /// when a person was told, not just when the thing happened. See `notify`.
    Notified {
        key: String,
        title: String,
        message: String,
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
        let value = serde_json::to_value(Event::WakeComplete { status: "sleeping".into() }).unwrap();
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
        })
        .unwrap();
        assert!(value.get("reason").is_none(), "a None field must not appear, not appear as null");
    }
}
