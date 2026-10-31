//! The only writer of an otto run's durable state. Every mutation locks the run, rewrites
//! `run.json` atomically, and appends to `journal.jsonl` — nothing else may touch `run.json`.

pub mod authorize;
pub mod change;
pub mod commands;
pub mod locks;
pub mod ops;

use crate::error::OttoError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 2;
pub const TERMINAL: [Status; 3] = [Status::Done, Status::Failed, Status::Stopped];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[clap(rename_all = "snake_case")]
pub enum Status {
    Running,
    AwaitingHuman,
    Sleeping,
    Blocked,
    Done,
    Failed,
    Stopped,
}

impl Status {
    pub fn is_terminal(self) -> bool {
        TERMINAL.contains(&self)
    }
}

/// Why a run is `blocked`. The status alone says a person has to act; the cause says what
/// happened, and that is what decides the right action — looking at the logs and retrying the
/// wake is the answer to failures and beside the point for a stall, where nothing failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum BlockedCause {
    /// `policy.maxIncompleteWakes` wakes in a row did not finish
    WakeFailures,
    /// A ceiling set when the run was created (`--budget-wakes`, `--budget-hours`) was reached
    Budget,
    /// `policy.maxTicksWithoutProgress` ticks in a row changed nothing
    Stall,
    /// The wrapped instructions decided the run could not proceed
    Instructions,
    /// It said it was done more than `policy.maxDoneReopens` times, and each time a fresh wake
    /// checking the claim found it wasn't
    Unverified,
}

impl BlockedCause {
    /// The kebab-case name, as it appears in `run.json` and the journal.
    pub fn label(self) -> &'static str {
        match self {
            BlockedCause::WakeFailures => "wake-failures",
            BlockedCause::Budget => "budget",
            BlockedCause::Stall => "stall",
            BlockedCause::Instructions => "instructions",
            BlockedCause::Unverified => "unverified",
        }
    }
}

/// A wake said the run is done, and no other wake has checked yet. `done` is the one stopping
/// state nothing ever comes back from, so it is not taken on the word of the wake that did the
/// work: the run sleeps until now, and the next wake — cold, with none of this one's context —
/// checks the claim against the done condition and either confirms it or reopens the run
/// (`commands::verify`). Its presence is what makes a wake the checker, whatever the status says.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoneClaim {
    /// The wake that made the claim (0 when no wake had run). It cannot also confirm it.
    #[serde(rename = "byWake")]
    pub by_wake: u32,
    pub at: crate::clock::Timestamp,
    /// What the claiming wake said, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Todo items still `[ ]` or `[~]` when it claimed (`crate::todo`).
    #[serde(rename = "leftOpen", default, skip_serializing_if = "Vec::is_empty")]
    pub left_open: Vec<String>,
}

/// What the last check of a done claim found. Kept after the claim is gone: on a reopened run it is
/// why the run is back at work, and on a done one it is what was checked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub confirmed: bool,
    /// The checker's own words: what it checked, or what is missing.
    pub reason: String,
    /// The wake that checked.
    pub wake: u32,
    pub at: crate::clock::Timestamp,
}

/// The record behind a `blocked` status. Present exactly while the run is blocked — `transaction`
/// drops it the moment the status is anything else — so a stale reason can never outlive the
/// state it explained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blocked {
    pub cause: BlockedCause,
    /// What the blocker said, verbatim: the last wake failure, the budget line, the reason a wake
    /// gave `set-status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub at: crate::clock::Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[clap(rename_all = "kebab-case")]
pub enum WrapKind {
    Skill,
    Instructions,
    GoalOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wraps {
    pub kind: WrapKind,
    /// `ref` in JSON; `ref` is a Rust keyword.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeOutcome {
    Complete,
    // Covers a crash, a deadline kill, and a model that simply stopped talking — all of which
    // mean the same thing to the next wake.
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wake {
    pub n: u32,
    #[serde(rename = "startedAt")]
    pub started_at: crate::clock::Timestamp,
    #[serde(rename = "deadlineAt")]
    pub deadline_at: crate::clock::Timestamp,
    /// The launcher's name as the run recorded it (see `crate::config`).
    pub launcher: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<WakeOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[clap(rename_all = "kebab-case")]
// No `Bg` variant: `claude attach` on a re-adopted worker needs a process-identity probe that
// execs the setuid `/bin/ps`, which macOS Seatbelt blocks unconditionally inside a sandbox.
pub enum Detach {
    None,
    Tmux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "camelCase")]
#[clap(rename_all = "kebab-case")]
// A wake is unattended, so no permission prompt can ever be answered — `BypassPermissions` is
// the only mode known to be safe for an unattended run; the others are for a supervised one.
pub enum PermissionMode {
    AcceptEdits,
    Auto,
    BypassPermissions,
    Manual,
    DontAsk,
    Plan,
}

impl PermissionMode {
    /// Exactly what `claude --permission-mode` expects.
    pub fn as_claude_arg(self) -> &'static str {
        match self {
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Auto => "auto",
            PermissionMode::BypassPermissions => "bypassPermissions",
            PermissionMode::Manual => "manual",
            PermissionMode::DontAsk => "dontAsk",
            PermissionMode::Plan => "plan",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    pub mode: PermissionMode,
    #[serde(rename = "allowedTools", default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    #[serde(rename = "disallowedTools", default, skip_serializing_if = "Vec::is_empty")]
    pub disallowed_tools: Vec<String>,
}

fn default_detach() -> Detach {
    Detach::Tmux
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Launcher {
    /// The name of the launcher every wake runs under — `claude`, or one defined in
    /// `$OTTO_HOME/config.json` (see `crate::config`). Separate from `detach` on purpose: this
    /// decides argv shape and what the wake can see, not where it is backgrounded.
    pub kind: String,
    /// A run-level preference, deliberately not a field on `Wake`: a wake has no way to know
    /// how it was started, so recording it per wake made the second wake read the first wake's
    /// value instead of this one.
    #[serde(default = "default_detach")]
    pub detach: Detach,
    /// Extra repos the wake may touch: `--add-dir` for claude, plus the launcher's `grantFlag`
    /// when it has one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<String>,
    /// Where every wake runs, whoever starts it: absolute, resolved when the run was created from
    /// `--workdir` or the configured default. `None` on a run from before working directories
    /// existed, which follows today's default (`config::workdir_for`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

/// `wakes` and `hours` are the enforced dimensions. `usd`/`spent_usd` are vestigial — pricing a
/// wake needed `-p --output-format json`, and print mode is gone (see `wake::launcher`) — kept
/// only so a `run.json` written before that still deserializes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    #[serde(default)]
    pub wakes: u32,
    #[serde(default)]
    pub hours: u32,
    #[serde(default)]
    pub usd: f64,
    #[serde(rename = "spentWakes", default)]
    pub spent_wakes: u32,
    /// Tokens, counted the way `otto usage` totals them: input, output, and cache read and
    /// written. Not dollars — nothing can price a wake on a subscription — but a unit otto measures
    /// for every wake whose transcript it can read.
    #[serde(default)]
    pub tokens: u64,
    #[serde(rename = "spentTokens", default)]
    pub spent_tokens: u64,
    /// Never incremented — nothing can price a wake on a subscription.
    #[serde(rename = "spentUsd", default)]
    pub spent_usd: f64,
}

impl Budget {
    /// Fraction of the tightest limit already spent, or `None` when nothing is capped.
    /// `usd` is not considered — it would make the 80% warning read a number that's always
    /// zero, so a hand-edited dollar budget would look healthy forever instead of unenforceable.
    pub fn worst_fraction(&self, elapsed_hours: f64) -> Option<f64> {
        let mut worst: Option<f64> = None;
        let mut consider = |used: f64, limit: f64| {
            if limit > 0.0 {
                let fraction = used / limit;
                worst = Some(worst.map_or(fraction, |w: f64| w.max(fraction)));
            }
        };
        consider(self.spent_wakes as f64, self.wakes as f64);
        consider(elapsed_hours, self.hours as f64);
        consider(self.spent_tokens as f64, self.tokens as f64);
        worst
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum OutwardAction {
    Push,
    Merge,
    Comment,
    DeleteBranch,
}

/// There is only one kind of gate: a question with an answer. Waiting on a clock is a status
/// (`Sleeping` + `next_wake_at`), not a gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gate {
    pub id: String,
    pub slug: String,
    pub file: String,
    #[serde(rename = "askedAt")]
    pub asked_at: crate::clock::Timestamp,
    #[serde(rename = "answeredAt")]
    pub answered_at: Option<crate::clock::Timestamp>,
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<crate::clock::Timestamp>,
    /// The output version the question is about (`design@1`): its answer is kept on that version
    /// as the person's review of it (`outputs::record_review`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
}

/// A person's note to the run, outside any gate: guidance on how to do the work, handed to the
/// wakes verbatim (see `notes`). The text lives in `file`; this is the bookkeeping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub file: String,
    /// Given to every wake until dropped. A one-off note is given until a wake that carried it
    /// completes, then leaves `notes` (its file stays, and the journal says it was delivered).
    #[serde(default)]
    pub standing: bool,
    #[serde(rename = "addedAt")]
    pub added_at: crate::clock::Timestamp,
    /// The wake whose prompt last carried this note. Only that wake completing delivers a
    /// one-off note — one added while a wake runs was never in its prompt, and must not be.
    #[serde(rename = "givenToWake", default, skip_serializing_if = "Option::is_none")]
    pub given_to_wake: Option<u32>,
}

/// A person's answer to a gate that no wake has yet been given — and completed with.
///
/// The answer is in the gate file and the journal the moment it is given, but a wake only acts on
/// what its prompt says, and only a wake started with the answer had it there. One that lands while
/// the asking wake is still finishing, or with `--no-wake`, used to reach no prompt at all: the run
/// slept its period and the next wake never heard it. So it is kept here, given to every wake that
/// starts until one that carried it completes, the way a one-off note is (`notes`).
///
/// Only the person's answer (`core::record_answer`). A wake closing a gate with an answer it was
/// handed, by `otto state close-gate`, has heard it already.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnheardAnswer {
    /// The gate it answers, by id.
    pub gate: String,
    pub slug: String,
    /// Verbatim, as recorded in the gate file.
    pub answer: String,
    #[serde(rename = "answeredAt")]
    pub answered_at: crate::clock::Timestamp,
    /// The wake whose prompt last carried it. Only that wake completing means it was heard — one
    /// that was already running when the answer landed never saw it.
    #[serde(rename = "givenToWake", default, skip_serializing_if = "Option::is_none")]
    pub given_to_wake: Option<u32>,
}

impl UnheardAnswer {
    /// The same answer, whatever wake it has been given to since.
    pub fn is(&self, other: &UnheardAnswer) -> bool {
        self.gate == other.gate && self.answered_at == other.answered_at
    }
}

/// A message the run has sent a person and that is still waiting for them (see `messages`). The
/// message itself — its body, any draft, what it attached — lives in `messages/NNN-<slug>.*`; this
/// is the index the inbox reads, so listing what needs a person never scans a run's history. A
/// message leaves this list when the person marks it done, and its files stay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenMessage {
    pub id: String,
    pub kind: crate::messages::MessageKind,
    pub title: String,
    /// The message's metadata file, relative to the run directory (`messages/004-report.json`).
    pub file: String,
    #[serde(rename = "sentAt")]
    pub sent_at: crate::clock::Timestamp,
}

/// What the run is waiting on, when a wake said so with a limit (`arm-timer --waiting-for`): CI on
/// a commit, a review, a deploy. The stall guard bounds a run that has stopped moving at all; this
/// bounds one wait, the way paperclip's scheduled checks carry a timeout ("wait for CI up to 2h,
/// then escalate"). Re-arming the same wait keeps its start and its limit, so the limit can't drift
/// later one sleep at a time. Once `giveUpAt` passes, otto refuses another sleep for it and tells
/// the next wake to decide. A sleep armed for anything else ends it, and so does an answered gate:
/// a person saying "keep waiting" starts it afresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wait {
    #[serde(rename = "for")]
    pub what: String,
    pub since: crate::clock::Timestamp,
    #[serde(rename = "giveUpAt", default, skip_serializing_if = "Option::is_none")]
    pub give_up_at: Option<crate::clock::Timestamp>,
}

impl Wait {
    pub fn is_overdue(&self) -> bool {
        self.give_up_at.is_some_and(|at| at.is_past())
    }
}

/// A person has paused the run: poke starts no wake for it — not the period's, not a timer's, not
/// a check script's — until `until`, or until they resume it. A wake already running finishes, and
/// a person can still answer, note or wake it by hand; a pause holds back only what is scheduled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pause {
    pub at: crate::clock::Timestamp,
    /// `None`: until a person resumes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<crate::clock::Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What an opt-in check script (DESIGN.md §8) reported. `Changed` and `Error` are handled
/// identically by poke — both spawn a wake — but kept distinct in the journal so a person can
/// tell "the PR moved" from "the script broke" in `otto logs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckResult {
    NoChange,
    Changed,
    Error,
}

/// Wake anyway after this many "nothing new" results in a row, unless told otherwise — the
/// safety net under a check that is wrong without failing. A day of an hourly period.
pub const DEFAULT_CHECK_WAKE_AFTER: u32 = 24;

fn default_check_wake_after() -> u32 {
    DEFAULT_CHECK_WAKE_AFTER
}

/// A gate in front of a sleeping run's wake: a script poke runs directly, no LLM involved, when
/// the wake comes due. "Nothing new" (exit 0) skips the wake and sleeps again; anything else
/// wakes it. There is no separate cadence — the sleep's own `nextWakeAt` is when it runs.
/// Absent for every run that doesn't set one (`otto check`, or `arm-timer --check-script`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    /// Path relative to the run directory — same convention as `gates/NNN-*.md`.
    pub script: String,
    /// How long to sleep again after "nothing new". `None`: the run's period. A wake's own check
    /// sets it to the sleep that wake asked for, so a check armed for "CI in ten minutes" keeps
    /// asking every ten minutes rather than every period.
    #[serde(rename = "retrySeconds", default, skip_serializing_if = "Option::is_none")]
    pub retry_seconds: Option<i64>,
    /// Wake anyway after this many "nothing new" results in a row; 0 turns the safety net off.
    #[serde(rename = "wakeAfter", default = "default_check_wake_after")]
    pub wake_after: u32,
    /// "Nothing new" results since the last real wake. The safety net counts this.
    #[serde(rename = "consecutiveNoChange", default)]
    pub consecutive_no_change: u32,
    #[serde(rename = "lastResult", default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<CheckResult>,
    /// When poke last ran it, and the first line or so of what it printed — so `otto show` can say
    /// what the script saw without a no-change result ever reaching the journal.
    #[serde(rename = "lastAt", default, skip_serializing_if = "Option::is_none")]
    pub last_at: Option<crate::clock::Timestamp>,
    #[serde(rename = "lastNote", default, skip_serializing_if = "Option::is_none")]
    pub last_note: Option<String>,
    /// Every no-change result since the check was set: each one a wake that did not happen.
    #[serde(rename = "noChangeTotal", default)]
    pub no_change_total: u64,
    /// Standing: it belongs to the run rather than to one sleep, so it stands in front of every
    /// period wake, a wake's plain `arm-timer` keeps it, and a one-sleep `--check-script` does not
    /// replace it. A person's check (`otto check`) is always standing; so is one a wake set with
    /// `otto state set-check`. `false`: a wake armed it with one sleep, and the next wake ends it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// Set by a wake (`set-check`, or `arm-timer --check-script`) rather than by a person. A wake
    /// may replace or remove a check a wake set; a person's check is the person's to change.
    #[serde(rename = "setByWake", default, skip_serializing_if = "std::ops::Not::not")]
    pub set_by_wake: bool,
}


fn default_gate_stale_after_hours() -> u32 {
    48
}
fn default_max_ticks_without_progress() -> i64 {
    24
}
fn default_handoff_max_bytes() -> u64 {
    commands::HANDOFF_MAX_BYTES as u64
}
fn default_max_wake_minutes() -> u64 {
    45
}
fn default_max_incomplete_wakes() -> u32 {
    5
}
fn default_period_minutes() -> u64 {
    60
}
/// Whether a period is a whole number of days — the only kind a time of day (`periodAt`) fits.
pub fn whole_days(period_minutes: u64) -> bool {
    period_minutes > 0 && period_minutes % (24 * 60) == 0
}

/// Whether a period is one day — the only kind days of the week (`periodDays`) fit.
pub fn daily(period_minutes: u64) -> bool {
    period_minutes == 24 * 60
}

fn default_verify_done() -> bool {
    true
}
fn default_max_done_reopens() -> u32 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    #[serde(rename = "autoMergeWhenGreen", default)]
    pub auto_merge_when_green: bool,
    /// DESIGN.md §7: a gate open longer than this should escalate. Not enforced yet.
    #[serde(rename = "gateStaleAfterHours", default = "default_gate_stale_after_hours")]
    pub gate_stale_after_hours: u32,
    #[serde(rename = "maxTicksWithoutProgress", default = "default_max_ticks_without_progress")]
    pub max_ticks_without_progress: i64,
    #[serde(rename = "handoffMaxBytes", default = "default_handoff_max_bytes")]
    pub handoff_max_bytes: u64,
    #[serde(rename = "maxWakeMinutes", default = "default_max_wake_minutes")]
    pub max_wake_minutes: u64,
    #[serde(rename = "maxIncompleteWakes", default = "default_max_incomplete_wakes")]
    pub max_incomplete_wakes: u32,
    #[serde(default)]
    pub perpetual: bool,
    /// How long after a wake starts the next one is due, when the wake did not say otherwise.
    /// The parent fills `nextWakeAt` from this for a wake that finished cleanly with nothing
    /// pending, and `arm-timer` without `--in`/`--at` uses it too. A gate takes precedence;
    /// once it is answered, the period resumes.
    #[serde(rename = "periodMinutes", default = "default_period_minutes")]
    pub period_minutes: u64,
    /// With a period of whole days, the time on the local clock the period's wakes are due —
    /// `otto period 7 1d --at 3:00` — so heavy work runs while nobody is using the machine.
    /// `None`: whenever the period comes round. Only the period's own sleeps land on it; a timer a
    /// wake arms, a check's own retry gap and an answered gate still wake when they say.
    #[serde(rename = "periodAt", default, skip_serializing_if = "Option::is_none")]
    pub period_at: Option<crate::clock::TimeOfDay>,
    /// With a period of one day, the days of the week on the local clock its wakes fall on —
    /// `otto period 7 1d --on mon-fri`. A period wake due on another day moves on to the next one
    /// of these, at the same time. `None`: every day. Like `periodAt`, only the period's own sleeps.
    #[serde(rename = "periodDays", default, skip_serializing_if = "Option::is_none")]
    pub period_days: Option<crate::clock::Weekdays>,
    /// A wake's `set-status done` is a claim that a fresh wake checks before the run is done
    /// (`DoneClaim`). `false`: done is done, on the word of the wake that said it.
    #[serde(rename = "verifyDone", default = "default_verify_done")]
    pub verify_done: bool,
    /// How many times a check may send a done claim back before the run stops and asks the person.
    #[serde(rename = "maxDoneReopens", default = "default_max_done_reopens")]
    pub max_done_reopens: u32,
    /// Anything `--policy` set that isn't one of the fields above.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            auto_merge_when_green: false,
            gate_stale_after_hours: default_gate_stale_after_hours(),
            max_ticks_without_progress: default_max_ticks_without_progress(),
            handoff_max_bytes: default_handoff_max_bytes(),
            max_wake_minutes: default_max_wake_minutes(),
            max_incomplete_wakes: default_max_incomplete_wakes(),
            perpetual: false,
            period_minutes: default_period_minutes(),
            period_at: None,
            period_days: None,
            verify_done: default_verify_done(),
            max_done_reopens: default_max_done_reopens(),
            extra: Map::new(),
        }
    }
}

impl Policy {
    pub fn set(&mut self, key: String, value: Value) {
        match key.as_str() {
            "autoMergeWhenGreen" => match value.as_bool() {
                Some(b) => self.auto_merge_when_green = b,
                None => {
                    self.extra.insert(key, value);
                }
            },
            "gateStaleAfterHours" => match value.as_u64() {
                Some(n) => self.gate_stale_after_hours = n as u32,
                None => {
                    self.extra.insert(key, value);
                }
            },
            "maxTicksWithoutProgress" => match value.as_i64() {
                Some(n) => self.max_ticks_without_progress = n,
                None => {
                    self.extra.insert(key, value);
                }
            },
            "handoffMaxBytes" => match value.as_u64() {
                Some(n) => self.handoff_max_bytes = n,
                None => {
                    self.extra.insert(key, value);
                }
            },
            "maxWakeMinutes" => match value.as_u64() {
                Some(n) => self.max_wake_minutes = n,
                None => {
                    self.extra.insert(key, value);
                }
            },
            "maxIncompleteWakes" => match value.as_u64() {
                Some(n) => self.max_incomplete_wakes = n as u32,
                None => {
                    self.extra.insert(key, value);
                }
            },
            "perpetual" => match value.as_bool() {
                Some(b) => self.perpetual = b,
                None => {
                    self.extra.insert(key, value);
                }
            },
            "periodMinutes" => match value.as_u64() {
                Some(n) if n > 0 => self.period_minutes = n,
                _ => {
                    self.extra.insert(key, value);
                }
            },
            "periodAt" => match value.as_str().and_then(crate::clock::TimeOfDay::parse) {
                Some(at) => self.period_at = Some(at),
                None => {
                    self.extra.insert(key, value);
                }
            },
            "periodDays" => match serde_json::from_value::<crate::clock::Weekdays>(value.clone()) {
                Ok(days) => self.period_days = days.limiting(),
                Err(_) => {
                    self.extra.insert(key, value);
                }
            },
            "verifyDone" => match value.as_bool() {
                Some(b) => self.verify_done = b,
                None => {
                    self.extra.insert(key, value);
                }
            },
            "maxDoneReopens" => match value.as_u64() {
                Some(n) => self.max_done_reopens = n as u32,
                None => {
                    self.extra.insert(key, value);
                }
            },
            _ => {
                self.extra.insert(key, value);
            }
        }
    }

    /// When a period's sleep anchored on `anchor` ends: one period on, then onto `periodAt` when
    /// the run has one (`TimeOfDay::snap`), so a wake poke started a few minutes late, or a
    /// catch-up hours off, doesn't move every wake after it — and on past any day `periodDays`
    /// leaves out (`Weekdays::next_on`).
    pub fn period_due(&self, anchor: time::OffsetDateTime) -> time::OffsetDateTime {
        let offset = crate::clock::local_offset();
        let due = anchor + time::Duration::minutes(self.period_minutes as i64);
        let due = match self.period_at {
            Some(at) if whole_days(self.period_minutes) => at.snap(due, offset),
            _ => due,
        };
        match self.period_days {
            Some(days) if daily(self.period_minutes) => days.next_on(due, offset),
            _ => due,
        }
    }

    pub fn is_true(&self, key: &str) -> bool {
        match key {
            "autoMergeWhenGreen" => self.auto_merge_when_green,
            "perpetual" => self.perpetual,
            "verifyDone" => self.verify_done,
            _ => self.extra.get(key).and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// The run number (`7`) for every run created since numbering; older runs keep the
    /// `<date>-<slug>` id they were made with until `otto renumber`. Also the directory name.
    pub id: String,
    /// What a person calls the run — unique, typeable, and changeable with `otto rename`, since
    /// nothing on disk is keyed by it. Empty only on a run from before names.
    #[serde(default)]
    pub name: String,
    /// The `<date>-<slug>` id this run had before `otto renumber` gave it a number. Still
    /// resolves, so a command in an old handoff or a person's shell history keeps working.
    #[serde(rename = "formerId", default, skip_serializing_if = "Option::is_none")]
    pub former_id: Option<String>,
    pub wraps: Wraps,
    /// Verbatim from `--goal` — the only field a wake must never rewrite.
    pub goal: String,
    /// Always serialized even when unset, so `run.json` shows the question exists with no
    /// answer yet; skipping it made `otto state get --field doneCondition` fail with "no such
    /// field" on exactly the runs where you most want to ask.
    #[serde(rename = "doneCondition", default)]
    pub done_condition: Option<String>,
    pub status: Status,
    /// A free-form label, for humans. The engine does not validate it — an arbitrary wrapped
    /// skill has no phase table to validate against.
    pub phase: String,
    pub gate: Option<Gate>,
    #[serde(rename = "nextWakeAt")]
    pub next_wake_at: Option<crate::clock::Timestamp>,
    /// The time a wake last asked for with `arm-timer --in`/`--at`. While it still equals
    /// `nextWakeAt`, the sleep is the wake's own rather than the period's — so a person's check
    /// does not stand in front of it, and changing the period does not move it. Anything else
    /// that sets `nextWakeAt` leaves this behind, which is what makes it stop matching.
    #[serde(rename = "armedWakeAt", default, skip_serializing_if = "Option::is_none")]
    pub armed_wake_at: Option<crate::clock::Timestamp>,
    /// The wake time otto set when a wake ran into the account's usage limit: the reset, plus a
    /// minute. While it still equals `nextWakeAt` the run is waiting out the limit (`usage_limited`),
    /// and as with `armedWakeAt`, anything else that sets `nextWakeAt` leaves it behind.
    #[serde(rename = "limitedUntil", default, skip_serializing_if = "Option::is_none")]
    pub limited_until: Option<crate::clock::Timestamp>,
    /// The current or most recent wake. Liveness is the wake lock, never this — the `pid` here
    /// is for reporting only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<Wake>,
    /// Opt-in only — see `Check`. `arm-timer` clears this back to `None` whenever it's called
    /// without `--check-script`, so a stale check never survives a plain re-arm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
    #[serde(rename = "incompleteWakes", default)]
    pub incomplete_wakes: u32,
    /// Wakes in a row that never got going: claude exited straight away, having done nothing
    /// (`wake::couldnt_start`). Kept apart from `incompleteWakes` because the cause is the machine —
    /// an account out of credit, a login that lapsed — not the run, so these back off and retry for
    /// as long as it takes and never block the run on a question its owner cannot answer.
    #[serde(rename = "startFailures", default, skip_serializing_if = "is_zero")]
    pub start_failures: u32,
    /// What claude said the last time a wake couldn't start — "Failed to authenticate: OAuth
    /// session expired" — since that, not otto's account of it, is what tells a person what to fix.
    #[serde(rename = "startFailure", default, skip_serializing_if = "Option::is_none")]
    pub start_failure: Option<String>,
    /// Why `status` is `blocked`, when it is. Set through `block`; cleared by `transaction`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<Blocked>,
    /// A done claim waiting for a fresh wake to check it. Cleared by `transaction` once the run is
    /// over, however it got there.
    #[serde(rename = "doneClaim", default, skip_serializing_if = "Option::is_none")]
    pub done_claim: Option<DoneClaim>,
    /// Times a check has sent a done claim back since the run last stopped to ask the person.
    #[serde(rename = "doneReopens", default, skip_serializing_if = "is_zero")]
    pub done_reopens: u32,
    #[serde(rename = "lastVerdict", default, skip_serializing_if = "Option::is_none")]
    pub last_verdict: Option<Verdict>,
    /// Poke's own bookkeeping, kept out of `facts` deliberately: `facts` is the wake's scratch
    /// space, and a wake that re-records what it read has been observed clobbering this with
    /// otto's own `0`, silently corrupting the backoff.
    #[serde(rename = "spawnAttempts", default)]
    pub spawn_attempts: u32,
    #[serde(rename = "lastSpawnedAt", default, skip_serializing_if = "Option::is_none")]
    pub last_spawned_at: Option<crate::clock::Timestamp>,
    /// When the 80% budget warning was journaled — once per run. Top level for the same reason:
    /// it used to live in `facts`, where a wake replacing its facts wholesale would erase it and
    /// earn the run a second warning.
    #[serde(rename = "budgetWarnedAt", default, skip_serializing_if = "Option::is_none")]
    pub budget_warned_at: Option<crate::clock::Timestamp>,
    /// Wakes in a row that changed nothing, by otto's look or the wake's word (`progress`). The
    /// name is from when only `tick` calls counted.
    #[serde(rename = "ticksWithoutProgress")]
    pub ticks_without_progress: u32,
    /// otto's mark of what it can see of the run's work, taken after the last wake that met the
    /// contract (`progress::mark`). A wake that leaves it as it was changed nothing.
    #[serde(rename = "progressMark", default, skip_serializing_if = "Option::is_none")]
    pub progress_mark: Option<String>,
    /// The wake that last said `tick --progress`: what changed is somewhere otto can't see.
    #[serde(rename = "saidProgress", default, skip_serializing_if = "Option::is_none")]
    pub said_progress: Option<u32>,
    pub launcher: Launcher,
    pub permission: Permission,
    pub budget: Budget,
    #[serde(default)]
    pub policy: Policy,
    pub facts: Map<String, Value>,
    #[serde(rename = "createdAt")]
    pub created_at: crate::clock::Timestamp,
    #[serde(rename = "updatedAt")]
    pub updated_at: crate::clock::Timestamp,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub authorizations: Map<String, Value>,
    /// Which desktop notifications poke has already sent, keyed by what each was about
    /// (`gate:003`, `gate-stale:003`, …) — see `notify`. Engine state, so not in `facts`: a wake
    /// re-recording what it read would clear it and every notice would fire again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notified: BTreeMap<String, crate::clock::Timestamp>,
    /// Notes a person has left for the run that are still to be delivered, and standing ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// A person's answer no wake has been given and completed with yet (`UnheardAnswer`).
    #[serde(rename = "unheardAnswer", default, skip_serializing_if = "Option::is_none")]
    pub unheard_answer: Option<UnheardAnswer>,
    /// Messages the run has sent that the person has not yet marked done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<OpenMessage>,
    /// Set while a person has paused the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<Pause>,
    /// What a wake said the run is waiting on, with a limit (`Wait`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<Wait>,
    /// The part it plays when it is one of otto's system agents (`crate::agents`): `lander`,
    /// `babysitter`. otto made it, and it is listed apart from the person's own runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// `k=v` pairs. Values are JSON when parseable (so `prNumber=42` is a number), else the literal
/// string. A repeated key keeps its first position but takes the last value, matching Python
/// dict-assignment semantics.
pub fn parse_kv(pairs: &[String], what: &str) -> Result<Map<String, Value>, OttoError> {
    let mut out = Map::new();
    for pair in pairs {
        let (key, raw) = pair
            .split_once('=')
            .ok_or_else(|| OttoError::usage(format!("{what} must be key=value, got: {pair}")))?;
        let key = key.trim();
        if key.is_empty() {
            return Err(OttoError::usage(format!("{what} has an empty key: {pair}")));
        }
        let value = serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

/// Reads exactly one of an inline value, a file, or stdin.
pub fn read_text_arg(
    inline: Option<&str>,
    file: Option<&str>,
    use_stdin: bool,
    what: &str,
) -> Result<String, OttoError> {
    let given = [inline.is_some(), file.is_some(), use_stdin].iter().filter(|b| **b).count();
    if given != 1 {
        return Err(OttoError::usage(format!("give exactly one of --{what}, --{what}-file, --stdin")));
    }
    if let Some(text) = inline {
        return Ok(text.to_string());
    }
    if let Some(path) = file {
        return std::fs::read_to_string(path).map_err(|e| OttoError::usage(format!("cannot read {path}: {e}")));
    }
    let mut buf = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
    Ok(buf)
}

/// Reads a dotted path, e.g. `facts.branch` or `gate.file`, out of a generic JSON value.
pub fn dig(value: &Value, dotted: &str) -> Result<Value, OttoError> {
    let mut node = value.clone();
    for part in dotted.split('.') {
        match node {
            Value::Object(mut map) => {
                node = map
                    .remove(part)
                    .ok_or_else(|| OttoError::conflict(format!("no such field: {dotted}")))?;
            }
            _ => return Err(OttoError::conflict(format!("no such field: {dotted}"))),
        }
    }
    Ok(node)
}

/// Prints a bare string unquoted, everything else as pretty JSON — matches the Python
/// `emit()` so `get --field status` prints `running`, not `"running"`.
pub fn emit(value: &Value) {
    match value {
        Value::String(s) => println!("{s}"),
        other => println!("{}", serde_json::to_string_pretty(other).expect("Value always serializes")),
    }
}

/// Write via temp file in the same directory, then rename. A rename within a directory
/// is atomic, so a reader never observes a half-written file, whatever happens mid-write.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), OttoError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!(".{filename}."))
        .suffix(".tmp")
        .tempfile_in(dir)?;
    tmp.write_all(text.as_bytes())?;
    tmp.flush()?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| OttoError::usage(e.to_string()))?;
    // The rename is atomic but not durable until the directory entry itself is on disk: without
    // this, a crash just after can bring back the previous version of the file (never a torn one).
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// A blocking exclusive flock on `path` (created if missing), released on drop. The one helper
/// behind every lock on a shared file in otto: a run's `.lock` (`RunLock`, under `transaction`), a
/// repo lock's flock, `runs/.last-id.lock` and `config.json.lock`. flock is per open file, so
/// taking the same one twice in one process deadlocks — take each at most once.
///
/// Lock order — always acquire in this order, never the reverse, and release before going back up:
///
/// 1. `WakeLock` (`<run>/.wake.lock`, try-lock: refuses rather than waits)
/// 2. `RunLock` (`<run>/.lock`) — at most one run's at a time
/// 3. a repo lock's flock (`runs/.repo-flocks/repo-<slug>.flock`)
/// 4. leaves, held only around their own read-modify-write and never while taking anything else:
///    `runs/.last-id.lock` and `config.json.lock`
///
/// So nothing that holds a repo flock ever takes a run lock, and nothing holding a leaf takes
/// anything at all.
pub(crate) struct Flock {
    file: std::fs::File,
}

impl Flock {
    pub(crate) fn acquire(path: &Path) -> Result<Self, OttoError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        fs4::fs_std::FileExt::lock_exclusive(&file)?;
        Ok(Self { file })
    }
}

impl Drop for Flock {
    fn drop(&mut self) {
        let _ = fs4::fs_std::FileExt::unlock(&self.file);
    }
}

/// Serializes writers for one run (blocking exclusive flock on `<run>/.lock`), released
/// on drop. Mirrors `fcntl.flock(LOCK_EX)` exactly, including the footgun: flock is
/// per-open-fd, so acquiring it twice from nested calls in the same process deadlocks
/// just as it would in Python. Every mutation must go through `transaction()` exactly
/// once — no "reentrant-safe" cleverness here.
/// Built on `Flock`; see it for the lock order.
pub struct RunLock {
    _flock: Flock,
}

impl RunLock {
    pub fn acquire(run_path: &Path) -> Result<Self, OttoError> {
        std::fs::create_dir_all(run_path)?;
        Ok(Self { _flock: Flock::acquire(&run_path.join(".lock"))? })
    }
}

pub(crate) fn read_state(path: &Path) -> Result<RunState, OttoError> {
    let run_json = path.join("run.json");
    let text = match std::fs::read_to_string(&run_json) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(OttoError::not_found(format!("{}/run.json is missing", path.display())));
        }
        Err(e) => return Err(e.into()),
    };
    // Check the version before deserializing into RunState, so a run written by another
    // version says so plainly instead of failing with whichever field serde noticed first.
    // v1 wrote `schemaVersion` and nothing ever read it; that is the gap this closes.
    let probe: Value = serde_json::from_str(&text)
        .map_err(|e| OttoError::conflict(format!("{}/run.json is not valid JSON: {e}", path.display())))?;
    match probe.get("schemaVersion").and_then(Value::as_u64) {
        Some(found) if found == SCHEMA_VERSION as u64 => {}
        Some(found) => {
            return Err(OttoError::conflict(format!(
                "{}/run.json is schemaVersion {found}, this otto speaks {SCHEMA_VERSION} — \
                 not migrating it, because guessing at a shape is how a run gets corrupted three days later",
                path.display()
            )))
        }
        None => {
            return Err(OttoError::conflict(format!(
                "{}/run.json has no schemaVersion",
                path.display()
            )))
        }
    }
    serde_json::from_str(&text)
        .map_err(|e| OttoError::conflict(format!("{}/run.json is not valid JSON: {e}", path.display())))
}

fn write_state(path: &Path, state: &mut RunState) -> Result<(), OttoError> {
    state.updated_at = crate::clock::Timestamp::now();
    let text = serde_json::to_string_pretty(state)? + "\n";
    write_atomic(&path.join("run.json"), &text)
}

/// Lock, hand out `(path, state)` for mutation, then write state back atomically. On
/// error from the closure, the lock is still released (via `RunLock`'s `Drop`) but
/// nothing is written — matching the Python contextmanager, which only reaches
/// `write_state` if the caller's block completes without raising.
pub fn transaction<F, R>(run_id: &str, f: F) -> Result<R, OttoError>
where
    F: FnOnce(&Path, &mut RunState) -> Result<R, OttoError>,
{
    let path = crate::paths::run_dir(run_id)?;
    let _lock = RunLock::acquire(&path)?;
    let mut state = read_state(&path)?;
    let result = f(&path, &mut state)?;
    // A blocked-reason is a fact about the current status, not history. Every write passes
    // through here, so this is the one place that keeps the two in step — whichever of the many
    // status assignments moved the run on.
    if state.status != Status::Blocked {
        state.blocked = None;
    }
    // Likewise a done claim: once the run is over — confirmed, stopped, failed — there is nothing
    // left to check, and a claim left behind would make the next wake after a resume a checker.
    if state.status.is_terminal() {
        state.done_claim = None;
    }
    write_state(&path, &mut state)?;
    Ok(result)
}

impl RunState {
    /// The name to show: `name`, or the old id's slug for a run from before names.
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            crate::paths::slug_of(&self.id).to_string()
        } else {
            self.name.clone()
        }
    }

    /// `#7 legenda` — how the run reads to a person.
    pub fn label(&self) -> String {
        crate::paths::label(&self.id, &self.display_name())
    }

    /// When the next wake is due by `policy.periodMinutes` alone. Anchored on the start of the
    /// wake in progress, so an hourly run stays hourly instead of drifting by each wake's own
    /// length; with no wake in progress (a person arming it by hand), anchored on now. Never in
    /// the past: a wake that outlived its period is due immediately, not retroactively.
    pub fn wake_armed_timer(&self) -> bool {
        self.armed_wake_at.is_some() && self.armed_wake_at == self.next_wake_at
    }

    /// A wake got going, or a person started over: whatever kept wakes from starting is behind it.
    pub fn clear_start_failures(&mut self) {
        self.start_failures = 0;
        self.start_failure = None;
    }

    /// Whether a person's pause is holding back the run's scheduled wakes right now. A pause whose
    /// `until` has passed no longer holds; the next wake clears it.
    pub fn is_paused(&self) -> bool {
        self.paused_at(crate::clock::now())
    }

    pub fn paused_at(&self, now: time::OffsetDateTime) -> bool {
        match &self.paused {
            Some(pause) => pause.until.is_none_or(|until| until.dt() > now),
            None => false,
        }
    }

    /// Whether the run is asleep until the account's usage limit resets (`limitedUntil`).
    pub fn usage_limited(&self) -> bool {
        self.limited_until.is_some() && self.limited_until == self.next_wake_at
    }

    /// Whether poke runs the check script instead of waking the run when its wake comes due. A
    /// person's check stands in front of the period's wakes but not in front of a timer a wake
    /// armed for its own reason ("CI in ten minutes"), which that check knows nothing about; a
    /// wake's own check stands in front of the sleep it was armed with. Never with a gate open —
    /// an expired gate wants a wake, not a script's opinion.
    pub fn check_gates_wake(&self) -> bool {
        match &self.check {
            Some(check) => {
                // Never in front of the retry of a wake that did not finish — one that failed, or
                // one the usage limit stopped: that work is half done whatever the world outside
                // has been doing.
                self.status == Status::Sleeping
                    && self.gate.is_none()
                    // A claim waiting to be checked is checked by a wake, whatever the world says.
                    && self.done_claim.is_none()
                    && self.incomplete_wakes == 0
                    && self.start_failures == 0
                    && !self.usage_limited()
                    && !(check.pinned && self.wake_armed_timer())
            }
            None => false,
        }
    }

    pub fn period_wake_at(&self) -> crate::clock::Timestamp {
        let now = crate::clock::Timestamp::now();
        let anchor = match &self.wake {
            Some(wake) if wake.outcome.is_none() => wake.started_at,
            _ => now,
        };
        let due = crate::clock::Timestamp::at(self.policy.period_due(anchor.dt()));
        if due.dt() > now.dt() { due } else { now }
    }

    /// Enter `blocked`, saying why. The only way in, so a blocked run always carries its reason;
    /// the caller still owes it a gate (`wake::contract`: blocked without one is stranded).
    pub fn block(&mut self, cause: BlockedCause, detail: Option<String>) {
        self.status = Status::Blocked;
        self.next_wake_at = None;
        self.blocked = Some(Blocked { cause, detail, at: crate::clock::Timestamp::now() });
    }

    /// The cause `set-status --status blocked` means when the wake did not say: the stall guard,
    /// if its counter is what tripped; otherwise the instructions themselves decided.
    pub fn inferred_block_cause(&self) -> BlockedCause {
        // A perpetual run has no stall guard (`commands::tick`), so its counter never means one.
        let limit = if self.policy.perpetual { 0 } else { self.policy.max_ticks_without_progress };
        if limit > 0 && i64::from(self.ticks_without_progress) >= limit {
            BlockedCause::Stall
        } else {
            BlockedCause::Instructions
        }
    }
}

/// Reads `run.json` without locking — fine for read-only commands (`get`, `list`, `due`),
/// never for a mutation.
pub fn read_run(run_id: &str) -> Result<RunState, OttoError> {
    let path = crate::paths::run_dir(run_id)?;
    read_state(&path)
}

/// Appends one already-built JSON line to `journal.jsonl`. Shared by the untyped `journal()`
/// below and by `event::record`, so the file I/O — and the fsync a durable audit trail needs —
/// lives in exactly one place.
pub(crate) fn append_journal_line(path: &Path, value: &Value) -> Result<(), OttoError> {
    let line = serde_json::to_string(value)? + "\n";
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.join("journal.jsonl"))?;
    file.write_all(line.as_bytes())?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

/// Appends one journal line. `ts`/`event` in `fields` are dropped (the frame always
/// wins), and `null`-valued fields are omitted — matching the Python's reserved-field and
/// omit-null handling.
///
/// This is the free-form escape hatch: `otto state log` and anything a wrapped skill journals
/// under a name otto never declared. Otto's own control-flow events are typed — see `event::Event`
/// and `log_event`, called via `event::record` once the caller already holds the run's lock.
pub fn journal(path: &Path, event: &str, fields: Value) -> Result<(), OttoError> {
    let mut entry = Map::new();
    entry.insert("ts".to_string(), Value::String(crate::clock::now_iso()));
    entry.insert("event".to_string(), Value::String(event.to_string()));
    if let Value::Object(map) = fields {
        for (k, v) in map {
            if k == "ts" || k == "event" || v.is_null() {
                continue;
            }
            entry.insert(k, v);
        }
    }
    append_journal_line(path, &Value::Object(entry))
}

/// Journal one of otto's own typed events, taking the run's lock for the write — the typed
/// counterpart to `commands::log`, for callers (poke, mainly) that have no other reason to open
/// a transaction. `event::record` itself assumes the lock is already held.
pub fn log_event(run_id: &str, event: crate::event::Event) -> Result<(), OttoError> {
    let path = crate::paths::run_dir(run_id)?;
    let _lock = RunLock::acquire(&path)?;
    crate::event::record(&path, &event)
}

/// A run whose `run.json` cannot be understood — bad JSON, or the wrong `schemaVersion` — is a
/// real, distinct case (DESIGN.md §9: "a person needs to look"), so it stays a variant here
/// rather than being smuggled into a `RunState` with fabricated defaults.
#[derive(Debug, Clone)]
pub enum RunEntry {
    Readable(RunState),
    Unreadable { id: String },
}

impl RunEntry {
    pub fn id(&self) -> &str {
        match self {
            RunEntry::Readable(state) => &state.id,
            RunEntry::Unreadable { id } => id,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            RunEntry::Readable(state) => serde_json::to_value(state).unwrap_or(Value::Null),
            RunEntry::Unreadable { id } => serde_json::json!({"id": id, "unreadable": true}),
        }
    }
}

pub fn read_all_runs() -> Result<Vec<RunEntry>, OttoError> {
    let root = crate::paths::runs_dir();
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    let mut entries: Vec<_> = std::fs::read_dir(&root)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if !dir.join("run.json").is_file() {
            continue;
        }
        let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        match read_state(&dir) {
            Ok(state) => out.push(RunEntry::Readable(state)),
            Err(_) => out.push(RunEntry::Unreadable { id }),
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn test_run_state(id: &str) -> RunState {
    let now = crate::clock::Timestamp::now();
    RunState {
        schema_version: SCHEMA_VERSION,
        id: id.to_string(),
        name: id.to_string(),
        former_id: None,
        wraps: Wraps { kind: WrapKind::GoalOnly, reference: None },
        goal: "a goal".to_string(),
        done_condition: None,
        status: Status::Running,
        phase: "start".to_string(),
        gate: None,
        next_wake_at: None,
        armed_wake_at: None,
        limited_until: None,
        wake: None,
        check: None,
        incomplete_wakes: 0,
        start_failures: 0,
        start_failure: None,
        blocked: None,
        done_claim: None,
        done_reopens: 0,
        last_verdict: None,
        spawn_attempts: 0,
        last_spawned_at: None,
        budget_warned_at: None,
        ticks_without_progress: 0,
        progress_mark: None,
        said_progress: None,
        launcher: Launcher { kind: "claude".into(), detach: Detach::Tmux, repos: vec![], workdir: None },
        permission: Permission { mode: PermissionMode::AcceptEdits, allowed_tools: vec![], disallowed_tools: vec![] },
        budget: Budget { wakes: 0, hours: 0, usd: 0.0, spent_wakes: 0, tokens: 0, spent_tokens: 0, spent_usd: 0.0 },
        policy: Policy::default(),
        facts: Map::new(),
        created_at: now,
        updated_at: now,
        authorizations: Map::new(),
        notified: BTreeMap::new(),
        notes: Vec::new(),
        unheard_answer: None,
        messages: Vec::new(),
        paused: None,
        wait: None,
        role: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use std::sync::Arc;

    #[test]
    fn a_daily_period_kept_to_weekdays_skips_the_weekend_at_its_time_of_day() {
        let local = crate::clock::local_offset();
        let at = |text: &str| time::PrimitiveDateTime::parse(text, time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]")).unwrap().assume_offset(local);
        let mut policy = Policy { period_minutes: 24 * 60, ..Policy::default() };
        policy.set("periodAt".into(), Value::String("3:00".into()));
        policy.set("periodDays".into(), serde_json::json!(["mon", "tue", "wed", "thu", "fri"]));
        // 2026-10-23 is a Friday: a wake poke started at 3:05 is followed by Monday's.
        assert_eq!(policy.period_due(at("2026-10-23 03:05")), at("2026-10-26 03:00"));
        assert_eq!(policy.period_due(at("2026-10-22 03:05")), at("2026-10-23 03:00"));
        // One you ran by hand on a Sunday morning is followed by Monday's too.
        assert_eq!(policy.period_due(at("2026-10-25 10:00")), at("2026-10-26 03:00"));
        // Without a time of day, at the time the last one started.
        policy.period_at = None;
        assert_eq!(policy.period_due(at("2026-10-23 14:20")), at("2026-10-26 14:20"));
        // A period that isn't one day ignores them.
        policy.period_minutes = 2 * 24 * 60;
        assert_eq!(policy.period_due(at("2026-10-23 14:20")), at("2026-10-25 14:20"));

        // All seven keeps it to nothing, and a value that isn't days is kept as an extra key.
        policy.set("periodDays".into(), Value::String("mon-sun".into()));
        assert_eq!(policy.period_days, None);
        policy.set("periodDays".into(), Value::String("someday".into()));
        assert_eq!(policy.extra["periodDays"], Value::String("someday".into()));
    }

    #[test]
    fn read_all_runs_surfaces_a_malformed_timestamp_as_unreadable() {
        let _home = TempHome::new();
        commands::test_init("bad-ts", "a goal").unwrap();
        let run_json = crate::paths::run_dir("bad-ts").unwrap().join("run.json");
        let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&run_json).unwrap()).unwrap();
        value["nextWakeAt"] = Value::String("tomorrow-ish".to_string());
        write_atomic(&run_json, &serde_json::to_string_pretty(&value).unwrap()).unwrap();

        let entries = read_all_runs().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], RunEntry::Unreadable { id } if id == "bad-ts"));
    }

    #[test]
    fn write_atomic_leaves_no_stray_temp_files() {
        let home = TempHome::new();
        let dir = home.path().join("scratch");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("run.json");
        for i in 0..20 {
            write_atomic(&target, &format!("payload {i}")).unwrap();
        }
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "run.json")
            .collect();
        assert!(leftovers.is_empty(), "stray files left behind: {leftovers:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "payload 19");
    }

    #[test]
    fn concurrent_reads_never_see_a_partial_write() {
        let home = TempHome::new();
        let dir = Arc::new(home.path().join("scratch"));
        std::fs::create_dir_all(dir.as_ref()).unwrap();
        let target = dir.join("run.json");
        write_atomic(&target, &"x".repeat(500)).unwrap();

        let writer_target = target.clone();
        let writer = std::thread::spawn(move || {
            for i in 0..200 {
                let filler = if i % 2 == 0 { "a" } else { "b" };
                write_atomic(&writer_target, &filler.repeat(500)).unwrap();
            }
        });
        let reader_target = target.clone();
        let reader = std::thread::spawn(move || {
            for _ in 0..200 {
                let text = std::fs::read_to_string(&reader_target).unwrap();
                assert!(
                    text.chars().all(|c| c == 'a') || text.chars().all(|c| c == 'b') || text.chars().all(|c| c == 'x'),
                    "observed a torn write: {text:?}"
                );
            }
        });
        writer.join().unwrap();
        reader.join().unwrap();
    }

    #[test]
    fn run_lock_serializes_a_read_increment_write_race() {
        let home = TempHome::new();
        let run_path = home.path().join("runs").join("locked-run");
        std::fs::create_dir_all(&run_path).unwrap();
        let counter_path = run_path.join("counter.txt");
        std::fs::write(&counter_path, "0").unwrap();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let run_path = run_path.clone();
            let counter_path = counter_path.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..25 {
                    let _lock = RunLock::acquire(&run_path).unwrap();
                    let current: u32 = std::fs::read_to_string(&counter_path).unwrap().trim().parse().unwrap();
                    std::fs::write(&counter_path, (current + 1).to_string()).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let total: u32 = std::fs::read_to_string(&counter_path).unwrap().trim().parse().unwrap();
        assert_eq!(total, 8 * 25, "run_lock must serialize every increment — lost updates mean it didn't");
    }
}
