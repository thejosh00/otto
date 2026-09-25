//! What each person-facing command does, as data.
//!
//! `human.rs` (the terminal) and `server` (the browser) are two renderings of this module, and
//! neither decides anything the other doesn't. Nothing here prints or reads stdin: the one
//! interactive piece — `otto answer`'s numbered menu — stays in `human`, because only a terminal
//! has someone to type into it.
//!
//! Both front ends read and write the run directory directly. The server is a view over
//! `~/.otto`, not an owner of it: `otto ls` and the web page agree because they read the same
//! files, and the CLI never needs the server to be running.

use crate::clock::relative;
use crate::error::OttoError;
use crate::liveness::{Liveness, LockLiveness};
use crate::paths::{short_id, short_id_among};
use crate::spawner::Strategy;
use crate::state::commands::InitArgs;
use crate::state::{read_all_runs, read_run, Blocked, BlockedCause, Detach, RunEntry, RunState, Status};
use serde::Serialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Starting a wake
// ---------------------------------------------------------------------------

/// Who is asking for a wake, which decides what `Detach::None` means. A person in a terminal
/// asked to watch it there; anything else — poke, the web server — has no terminal to put it in
/// and must never block on a wake that can run for most of an hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Terminal,
    Background,
}

impl Caller {
    pub fn strategy(self, detach: Detach) -> Strategy {
        match (self, detach) {
            (Caller::Terminal, Detach::None) => Strategy::Foreground,
            (Caller::Background, detach) => Strategy::for_poke(detach),
            (Caller::Terminal, Detach::Tmux) => Strategy::Tmux,
        }
    }
}

/// What starting a wake produced. `note` is `None` for a foreground wake, which already said how
/// it went.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeStart {
    pub session: Option<String>,
    pub note: Option<String>,
}

/// Start a wake under `strategy` and, when it is backgrounded, confirm it actually started. A
/// detached wake that dies immediately — a bad PATH, a lost $OTTO_HOME, a missing binary — takes
/// its tmux session with it and writes nothing anywhere, so "wake started" would be a lie nobody
/// could check. Waiting for the wake number to move is definitive: `otto wake` records it before
/// spawning.
pub fn start_wake(id: &str, strategy: Strategy, answer: Option<&str>) -> Result<WakeStart, OttoError> {
    let mut exec = crate::exec::RealExec;
    if strategy == Strategy::Foreground {
        crate::spawner::start(id, strategy, answer, &mut exec)?;
        return Ok(WakeStart::default());
    }
    let before = crate::spawner::wake_number(id);
    let handle = crate::spawner::start(id, strategy, answer, &mut exec)?;
    if !crate::spawner::wait_for_hold(id, before) {
        let container = match &handle.session {
            Some(session) => format!("started tmux session {session}"),
            None => "started a detached wake".to_string(),
        };
        return Err(OttoError::usage(format!(
            "{container} but no wake took hold within {}s — the wake process exited immediately. \
             Run `otto wake {} --watch` in a terminal to see why.",
            crate::spawner::CONFIRM_SECONDS,
            short_id(id)
        )));
    }
    Ok(WakeStart { session: handle.session, note: handle.description })
}

/// Refuse to background a wake onto a run that already has one. The foreground path gets this
/// from the wake lock. The background path would not: the child would die on the lock while
/// `spawner::wait_for_hold` saw that very lock and reported success.
pub fn refuse_if_busy(id: &str) -> Result<(), OttoError> {
    if LockLiveness.probe(id).is_busy() {
        return Err(OttoError::conflict(format!(
            "a wake is already running for {} — `otto attach {}` to watch it, or wait for it to finish",
            id,
            short_id(id)
        )));
    }
    Ok(())
}

/// Wait for an in-flight wake to finish, bounded.
///
/// A wake writes its gate to disk and then spends a few more seconds on its handoff and exit, so
/// `otto ls` can show a question as open while the wake that asked it is still running. Answering
/// in that window used to fail: `close_gate` succeeded, then the spawn hit the wake lock and
/// reported "a wake is already running", which looks like the answer was rejected when it was
/// safely recorded. Waiting is the honest fix — the work is nearly done, and the alternative
/// (spawning anyway) is two wakes on one run.
pub fn wait_for_wake_to_finish(id: &str) -> bool {
    for _ in 0..(crate::spawner::CONFIRM_SECONDS * 4) {
        if !LockLiveness.probe(id).is_busy() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    !LockLiveness.probe(id).is_busy()
}

/// How this run's wakes are backgrounded, as configured when the run was created.
pub fn detach_of(id: &str) -> Detach {
    read_run(id).ok().map(|s| s.launcher.detach).unwrap_or(Detach::Tmux)
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Planned {
    pub id: String,
    pub first_wake: String,
}

/// Everything `init_run` would decide, and the same refusals, with nothing on disk — so a flag
/// can be checked before it costs a run directory and a wake.
pub fn plan_run(init: &InitArgs) -> Result<Planned, OttoError> {
    let planned = crate::state::commands::plan_run(init)?;
    crate::wake::launcher::check_resolvable(&planned.state)?;
    Ok(Planned {
        id: planned.state.id.clone(),
        first_wake: crate::wake::first_wake_dry_run_line(&planned.state, &planned.path),
    })
}

/// Create the run. The first wake is the caller's to start, so the terminal can print the id
/// before a foreground wake takes over the screen.
pub fn create_run(init: InitArgs) -> Result<String, OttoError> {
    let id = crate::state::commands::init_run(init)?;
    // Fail before the first wake rather than after it: a launcher that cannot resolve what the
    // run wraps produces a wake that spends money and achieves nothing.
    crate::wake::launcher::check_resolvable(&read_run(&id)?)?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// ls
// ---------------------------------------------------------------------------

/// Whatever the run is waiting on, in a few words. This is the column that answers "which of my
/// runs needs me?", so a gate names its question rather than just its id.
pub fn blocking(state: &RunState, wake_running: bool) -> String {
    if let Some(gate) = &state.gate {
        // The wake that opened this gate does not always exit right away — a model that keeps
        // working past "stop there" leaves the lock held. Without this, `otto ls` reads as
        // "waiting on you" when a wake is in fact still alive and could race an answer.
        return if wake_running {
            format!("you: gate {} {} (wake {} still running)", gate.id, gate.slug, state.wake.as_ref().map(|w| w.n).unwrap_or(0))
        } else {
            format!("you: gate {} {}", gate.id, gate.slug)
        };
    }
    if wake_running {
        let n = state.wake.as_ref().map(|w| w.n).unwrap_or(0);
        return format!("working (wake {n})");
    }
    match state.status {
        // When is its own column (`next_wake`); this one only says what the run waits on.
        Status::Sleeping => match state.next_wake_at {
            Some(_) => "timer".to_string(),
            None => "nothing — sleeping with no wake time".to_string(),
        },
        Status::Done | Status::Failed | Status::Stopped => "—".to_string(),
        // Not running and nothing pending. The validator turns this into a retry, so seeing it
        // here means a wake is between attempts, or something went wrong outside a wake.
        _ => "nothing scheduled".to_string(),
    }
}

/// When the next wake is due, for a list: relative first (what a person scans for), then the
/// local clock time. A running wake has cleared its timer, so nothing is due until it decides.
pub fn next_wake(state: &RunState, wake_running: bool) -> String {
    match state.next_wake_at {
        Some(at) if !wake_running && !state.status.is_terminal() => {
            format!("{} ({})", crate::clock::due(at), crate::clock::local_clock(at))
        }
        _ => "—".to_string(),
    }
}

/// The status as a person should read it. `blocked` on its own sends them to the logs; the
/// cause says whether that is even the right place to look.
pub fn status_label(state: &RunState) -> String {
    if state.status == Status::Blocked {
        format!("blocked ({})", blocked_record(state).0.cause.label())
    } else {
        crate::state::commands::status_str(state.status).to_string()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LsRow {
    pub id: String,
    pub short: String,
    pub status: String,
    pub phase: String,
    pub blocking: String,
    pub wakes: String,
    /// `1h`, `30m`; `—` for a finished run, which will never wake again.
    pub period: String,
    /// For the web page, which renders it relative to the viewer's clock on each refresh.
    pub next_wake_at: Option<crate::clock::Timestamp>,
    /// For `otto ls`: `in 43m (14:05)`, or `—` with nothing scheduled.
    pub next_wake: String,
    pub running: bool,
    pub terminal: bool,
}

/// A gate a person can act on right now.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeedsYou {
    pub id: String,
    pub short: String,
    pub label: String,
    /// The default leads, so the first one shown is the one the wake recommended.
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LsView {
    pub rows: Vec<LsRow>,
    pub needs_you: Vec<NeedsYou>,
    /// How many runs are sleeping on a timer while the reviver is known not to be loaded — the
    /// runs that will silently never come back.
    pub stranded_sleepers: Option<usize>,
}

pub fn list_runs(all: bool) -> Result<LsView, OttoError> {
    let liveness = LockLiveness;
    let all_ids = crate::paths::all_run_ids();
    let mut rows: Vec<LsRow> = Vec::new();
    let mut needs_you: Vec<NeedsYou> = Vec::new();
    let mut sleeping_count = 0usize;
    for entry in read_all_runs()? {
        let state = match entry {
            RunEntry::Readable(state) => state,
            RunEntry::Unreadable { id } => {
                if all {
                    let short = short_id_among(&id, &all_ids);
                    rows.push(LsRow {
                        id,
                        short,
                        status: "unreadable".to_string(),
                        phase: "?".to_string(),
                        blocking: "a person needs to look".to_string(),
                        wakes: "?".to_string(),
                        period: "?".to_string(),
                        next_wake_at: None,
                        next_wake: "?".to_string(),
                        running: false,
                        terminal: false,
                    });
                }
                continue;
            }
        };
        if state.status.is_terminal() && !all {
            continue;
        }
        let running = liveness.probe(&state.id).is_busy();
        if let Some(gate) = &state.gate {
            let dir = crate::paths::run_dir(&state.id).ok();
            let question = dir.and_then(|d| crate::gate::read_question(&d, gate).ok()).unwrap_or_default();
            needs_you.push(NeedsYou {
                id: state.id.clone(),
                short: short_id_among(&state.id, &all_ids),
                label: format!("gate {} {}", gate.id, gate.slug),
                options: crate::gate::options_default_first(&question),
            });
        } else if !running && state.status == Status::Sleeping {
            sleeping_count += 1;
        }
        // Wakes, not dollars. There is no per-wake cost to show since otto stopped passing `-p`
        // (see `wake::launcher`), and a `$0.00` column that can never change is worse than no
        // column — it reads as a run that has cost nothing. Wakes against their budget is the
        // number that still means something at a glance.
        let spent = state.budget.spent_wakes;
        let limit = state.budget.wakes;
        let wakes = if limit > 0 { format!("{spent}/{limit}") } else { spent.to_string() };
        rows.push(LsRow {
            id: state.id.clone(),
            short: short_id_among(&state.id, &all_ids),
            status: status_label(&state),
            phase: state.phase.clone(),
            blocking: blocking(&state, running),
            wakes,
            period: if state.status.is_terminal() {
                "—".to_string()
            } else {
                crate::clock::format_minutes(state.policy.period_minutes)
            },
            next_wake_at: state.next_wake_at,
            next_wake: next_wake(&state, running),
            running,
            terminal: state.status.is_terminal(),
        });
    }
    let stranded_sleepers = if sleeping_count > 0 && crate::launchd::is_loaded() == Some(false) {
        Some(sleeping_count)
    } else {
        None
    };
    Ok(LsView { rows, needs_you, stranded_sleepers })
}

// ---------------------------------------------------------------------------
// show
// ---------------------------------------------------------------------------

/// The run a command means when none is named. With one gate open anywhere, it is that run —
/// `otto ls` said "needs you", and this is the reply. Otherwise, only when `or_only_live` and
/// exactly one run is live, that one. Anything else is a question back, naming what to type.
pub fn implied_run(verb: &str, or_only_live: bool) -> Result<String, OttoError> {
    let mut waiting: Vec<String> = Vec::new();
    let mut live: Vec<String> = Vec::new();
    for entry in read_all_runs()? {
        let RunEntry::Readable(state) = entry else { continue };
        if state.status.is_terminal() {
            continue;
        }
        if state.gate.is_some() {
            waiting.push(state.id.clone());
        }
        live.push(state.id);
    }
    let ids = crate::paths::all_run_ids();
    let name_each = |runs: &[String]| -> String {
        runs.iter().map(|id| format!("`otto {verb} {}`", short_id_among(id, &ids))).collect::<Vec<_>>().join(", ")
    };
    match (waiting.as_slice(), live.as_slice()) {
        ([only], _) => Ok(only.clone()),
        ([], [only]) if or_only_live => Ok(only.clone()),
        ([], []) => Err(OttoError::conflict("no run is live — `otto ls --all` to see finished ones")),
        ([], _) => Err(OttoError::conflict(format!("no run is waiting on you — say which: {}", name_each(&live)))),
        (several, _) => Err(OttoError::usage(format!(
            "{} runs are waiting on you — say which: {}",
            several.len(),
            name_each(several)
        ))),
    }
}

/// An open gate, as a person answers it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateView {
    pub id: String,
    pub slug: String,
    pub file: String,
    /// The whole gate file; `None` when it is missing.
    pub text: Option<String>,
    /// Just the question, for a caller that shows it without the file's header.
    pub question: String,
    /// In the order the question lists them.
    pub options: Vec<String>,
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub state: RunState,
    pub short: String,
    pub status_label: String,
    pub wraps_kind: &'static str,
    pub running: bool,
    pub blocked_explanation: Option<String>,
    pub gate: Option<GateView>,
    /// Shown when no gate is open: what the last wake left for the next one.
    pub handoff: Option<String>,
    /// Standing notes, and one-off notes not yet delivered.
    pub notes: Vec<NoteView>,
}

/// Everything needed to answer a gate cold, in one screen: where the run stands, what it last
/// did, and the question in full. Someone arriving eight hours later has no transcript, because
/// the session that asked is gone.
pub fn run_detail(id: Option<&str>) -> Result<RunDetail, OttoError> {
    let id = match id {
        Some(id) => crate::paths::resolve_run_id(id)?,
        None => implied_run("show", true)?,
    };
    let state = read_run(&id)?;
    let dir = crate::paths::run_dir(&id)?;
    let running = LockLiveness.probe(&id).is_busy();
    let short = short_id(&id);
    let blocked_explanation = (state.status == Status::Blocked).then(|| blocked_explanation(&state, &short));
    let gate = state.gate.as_ref().map(|gate| {
        let question = crate::gate::read_question(&dir, gate).unwrap_or_default();
        let options = crate::gate::parse_options(&question);
        let default = crate::gate::parse_default(&question, &options);
        GateView {
            id: gate.id.clone(),
            slug: gate.slug.clone(),
            file: gate.file.clone(),
            text: std::fs::read_to_string(dir.join(&gate.file)).ok(),
            question,
            options,
            default,
        }
    });
    let handoff = match gate {
        Some(_) => None,
        None => std::fs::read_to_string(dir.join(crate::state::commands::HANDOFF_FILE)).ok(),
    };
    let notes = note_views(&dir, &state);
    Ok(RunDetail {
        short,
        notes,
        status_label: status_label(&state),
        wraps_kind: crate::state::commands::kind_str(state.wraps.kind),
        running,
        blocked_explanation,
        gate,
        handoff,
        state,
    })
}

/// Why the run is blocked and what to do about it, in one line, matched to the cause. This used
/// to say "blocked after repeated wake failures — see the logs, then retry the wake" for every
/// blocked run, and was seen saying it to a run whose wakes had all completed: it had stalled,
/// nothing had failed, and retrying was the one thing that could not help.
pub fn blocked_explanation(state: &RunState, short: &str) -> String {
    let gate = match &state.gate {
        Some(gate) => format!("answer gate {} below", gate.id),
        // The contract forbids this state; if it is seen anyway, say what to do rather than nothing.
        None => format!("no gate is open, so nothing can unblock it — `otto wake {short} --watch` or `otto stop {short}`"),
    };
    let (blocked, inferred) = blocked_record(state);
    let detail = blocked.detail.as_deref();
    let line = match blocked.cause {
        BlockedCause::WakeFailures => format!(
            "blocked: {} wake(s) in a row did not finish{} — `otto logs {short}` for what happened, \
             `otto wake {short} --watch` to retry one in this terminal, or {gate}",
            state.incomplete_wakes,
            detail.map(|d| format!(" (last: {d})")).unwrap_or_default()
        ),
        BlockedCause::Budget => format!(
            "blocked: {} — nothing is wrong with the work; {gate}",
            detail.unwrap_or("a budget ceiling was reached")
        ),
        BlockedCause::Stall => format!(
            "blocked: {} tick(s) in a row changed nothing (policy maxTicksWithoutProgress = {}) — nothing \
             failed, the run is asking whether to keep going; {gate}",
            state.ticks_without_progress, state.policy.max_ticks_without_progress
        ),
        BlockedCause::Instructions => format!(
            "blocked by its instructions{} — {gate}",
            detail.map(|d| format!(": {d}")).unwrap_or_default()
        ),
    };
    if inferred {
        format!("{line}\n(cause inferred from the run's counters: it was blocked before otto recorded reasons)")
    } else {
        line
    }
}

/// The blocked record, or — for a run blocked before otto kept one — the same inference
/// `set-status` makes today, flagged as such. The `bool` is "inferred".
pub fn blocked_record(state: &RunState) -> (Blocked, bool) {
    match &state.blocked {
        Some(blocked) => (blocked.clone(), false),
        None => (Blocked { cause: state.inferred_block_cause(), detail: None, at: state.updated_at }, true),
    }
}

// ---------------------------------------------------------------------------
// answer
// ---------------------------------------------------------------------------

/// The gate an answer is for, read once so that what is validated, prompted for and recorded
/// all come from the same question.
#[derive(Debug, Clone)]
pub struct PendingGate {
    pub id: String,
    pub short: String,
    pub gate_id: String,
    pub slug: String,
    pub question: String,
    pub options: Vec<String>,
}

pub fn pending_gate(id: Option<&str>) -> Result<PendingGate, OttoError> {
    let id = match id {
        Some(id) => crate::paths::resolve_run_id(id)?,
        None => implied_run("answer", false)?,
    };
    let short = short_id(&id);
    let state = read_run(&id)?;
    let gate = state.gate.as_ref().ok_or_else(|| {
        OttoError::conflict(format!("{id} has no gate open — nothing is being asked. `otto show {short}` for where it stands"))
    })?;
    let dir = crate::paths::run_dir(&id)?;
    let question = crate::gate::read_question(&dir, gate).unwrap_or_default();
    let options = crate::gate::parse_options(&question);
    Ok(PendingGate { gate_id: gate.id.clone(), slug: gate.slug.clone(), id, short, question, options })
}

/// Match `choice` against the gate's own options, case-insensitively, and record the gate's
/// casing rather than the user's — so the journal always shows the option exactly as named in
/// the question, whichever case someone typed. An empty `options` (a question with no parseable
/// list) skips validation entirely: most gates are free-form prose from an arbitrary wrapped
/// skill, and refusing those would break far more than it catches.
pub fn resolve_choice(choice: &str, options: &[String]) -> Result<String, OttoError> {
    if options.is_empty() {
        return Ok(choice.to_string());
    }
    match options.iter().find(|o| o.eq_ignore_ascii_case(choice.trim())) {
        Some(matched) => Ok(matched.clone()),
        None => Err(OttoError::usage(format!(
            "\"{choice}\" is not one of this gate's options: {}",
            options.join(", ")
        ))),
    }
}

/// Record `answer` against the gate. `ops::close_gate` records it verbatim and returns
/// `running`, so the wake that follows sees an answered gate. It takes the run lock inside its
/// own transaction and releases it on return, so a wake started afterwards is sequential rather
/// than nested — `RunLock` is not reentrant.
pub fn record_answer(pending: &PendingGate, answer: &str) -> Result<(), OttoError> {
    crate::state::ops::close_gate(&pending.id, Some(answer), Status::Running)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerOutcome {
    pub id: String,
    pub gate_id: String,
    pub slug: String,
    pub answer: String,
    pub wake: Option<WakeStart>,
    /// Why no wake was started, when none was.
    pub note: Option<String>,
}

/// Record an answer and continue the run in the background — the whole of `otto answer` for a
/// caller with no terminal. `choice` is checked against the gate's options; `text` is not.
pub fn answer_in_background(id: &str, choice: Option<&str>, text: Option<&str>, no_wake: bool) -> Result<AnswerOutcome, OttoError> {
    let pending = pending_gate(Some(id))?;
    let answer = match (choice, text) {
        (Some(choice), _) => resolve_choice(choice, &pending.options)?,
        (None, Some(text)) if !text.trim().is_empty() => text.to_string(),
        _ => return Err(OttoError::usage("give the answer: a choice or some text")),
    };
    record_answer(&pending, &answer)?;
    let mut outcome = AnswerOutcome {
        id: pending.id.clone(),
        gate_id: pending.gate_id.clone(),
        slug: pending.slug.clone(),
        answer: answer.clone(),
        wake: None,
        note: None,
    };
    if no_wake {
        outcome.note = Some("recorded; not waking it yet".to_string());
        return Ok(outcome);
    }
    if !wait_for_wake_to_finish(&pending.id) {
        outcome.note = Some(format!(
            "a wake is still running after {}s — your answer is recorded, and the run will act on it",
            crate::spawner::CONFIRM_SECONDS
        ));
        return Ok(outcome);
    }
    // Hand the answer to the wake as well as recording it: the wake must see the person's own
    // words, not a summary of them.
    let strategy = Caller::Background.strategy(detach_of(&pending.id));
    outcome.wake = Some(start_wake(&pending.id, strategy, Some(&answer))?);
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// notes
// ---------------------------------------------------------------------------

/// A note as a person reads it back: its text, and whether a wake has seen it yet.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteView {
    pub id: String,
    pub standing: bool,
    pub added_at: crate::clock::Timestamp,
    pub text: String,
    /// The wake whose prompt last carried it. For a one-off note still listed, that wake has not
    /// completed — it is running now, or it failed and the note will be given again.
    pub given_to_wake: Option<u32>,
}

pub fn note_views(dir: &std::path::Path, state: &RunState) -> Vec<NoteView> {
    state
        .notes
        .iter()
        .map(|note| NoteView {
            id: note.id.clone(),
            standing: note.standing,
            added_at: note.added_at,
            text: crate::notes::read_text(dir, note),
            given_to_wake: note.given_to_wake,
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteOutcome {
    pub id: String,
    pub short: String,
    pub note: NoteView,
    /// When a wake will read it, in words.
    pub delivery: String,
    pub wake: Option<WakeStart>,
    /// Why `now` did not start a wake, when it was asked to and didn't.
    pub not_woken: Option<String>,
}

/// Why a wake cannot be started for a note right now, or `None` when one can.
pub fn why_not_wake_for_note(state: &RunState, running: bool) -> Option<String> {
    if running {
        return Some("a wake is already running".to_string());
    }
    if let Some(gate) = &state.gate {
        return Some(format!("gate {} is open — answer it and the wake that follows reads the note", gate.id));
    }
    None
}

/// When the wakes will see a note just added, said plainly — a note is easy to mistake for an
/// answer, and one sitting unread behind an open gate should not be a surprise.
pub fn note_delivery(state: &RunState, running: bool, standing: bool) -> String {
    let next = if running {
        "the wake running now won't see it; the next one will".to_string()
    } else if let Some(gate) = &state.gate {
        format!("gate {} is open, so it reaches the wake after you answer it — a note is not an answer", gate.id)
    } else {
        match state.next_wake_at {
            Some(at) if at.is_past() => "the next wake reads it, at the next poke".to_string(),
            Some(at) => format!("the next wake reads it, {}", relative(at)),
            None => "the next wake reads it".to_string(),
        }
    };
    if standing {
        format!("standing, until you drop it; {next}")
    } else {
        next
    }
}

/// Record a note, and say when it will be read. Waking for it is the caller's business, because
/// a terminal and the web page start wakes differently.
pub fn add_note(id: &str, text: &str, standing: bool) -> Result<NoteOutcome, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let note = crate::notes::add(&id, text, standing)?;
    let state = read_run(&id)?;
    let dir = crate::paths::run_dir(&id)?;
    let running = LockLiveness.probe(&id).is_busy();
    Ok(NoteOutcome {
        short: short_id(&id),
        delivery: note_delivery(&state, running, standing),
        note: NoteView {
            text: crate::notes::read_text(&dir, &note),
            id: note.id,
            standing,
            added_at: note.added_at,
            given_to_wake: None,
        },
        id,
        wake: None,
        not_woken: None,
    })
}

/// `add_note`, then — when `now` — a backgrounded wake to read it.
pub fn note_in_background(id: &str, text: &str, standing: bool, now: bool) -> Result<NoteOutcome, OttoError> {
    let mut outcome = add_note(id, text, standing)?;
    if !now {
        return Ok(outcome);
    }
    let state = read_run(&outcome.id)?;
    if let Some(why) = why_not_wake_for_note(&state, LockLiveness.probe(&outcome.id).is_busy()) {
        outcome.not_woken = Some(why);
        return Ok(outcome);
    }
    match start_wake(&outcome.id, Caller::Background.strategy(state.launcher.detach), None) {
        Ok(wake) => outcome.wake = Some(wake),
        // The note stands either way; it is on disk for whichever wake comes next.
        Err(err) => outcome.not_woken = Some(format!("the wake did not start: {}", err.message)),
    }
    Ok(outcome)
}

pub fn drop_note(id: &str, note: &str) -> Result<NoteView, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let dir = crate::paths::run_dir(&id)?;
    let note = crate::notes::drop_note(&id, note)?;
    Ok(NoteView {
        text: crate::notes::read_text(&dir, &note),
        id: note.id,
        standing: note.standing,
        added_at: note.added_at,
        given_to_wake: note.given_to_wake,
    })
}

// ---------------------------------------------------------------------------
// wake
// ---------------------------------------------------------------------------

/// Force one wake now, in the background, wherever the run's wakes go — or wherever `detach`
/// says, for this wake only.
pub fn wake_in_background(id: &str, detach: Option<Detach>) -> Result<WakeStart, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let state = read_run(&id)?;
    if state.status.is_terminal() {
        return Err(OttoError::conflict(format!(
            "{id} is {} — there is nothing left to wake",
            crate::state::commands::status_str(state.status)
        )));
    }
    refuse_if_busy(&id)?;
    let strategy = Caller::Background.strategy(detach.unwrap_or(state.launcher.detach));
    start_wake(&id, strategy, None)
}

// ---------------------------------------------------------------------------
// stop
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopOutcome {
    pub id: String,
    pub status: Status,
    pub reason: String,
    pub killed_wake: bool,
}

/// Retiring a healthy run is `stopped`, not `failed` — `failed` in the audit trail would be a
/// lie about a run that worked and simply is not wanted any more.
pub fn stop_run(id: &str, reason: Option<String>, failed: bool, exec: &mut dyn crate::exec::Exec) -> Result<StopOutcome, OttoError> {
    // Resolve before anything else: `spawner::kill` names a tmux session directly from the id,
    // never through `paths::run_dir`, so an unresolved prefix would go looking for a session that
    // was never named that.
    let id = crate::paths::resolve_run_id(id)?;
    let reason = reason.unwrap_or_else(|| "stopped by hand".to_string());
    let status = if failed { Status::Failed } else { Status::Stopped };
    crate::state::commands::set_status(crate::state::commands::SetStatusArgs {
        id: id.clone(),
        status,
        reason: Some(reason.clone()),
        because: None,
    })?;
    // Release locks on the way out, or the next run against that repo waits on a corpse.
    let _ = crate::state::locks::unlock(crate::state::locks::UnlockArgs { id: id.clone(), repo: None });
    // A wake still running would keep working on a run nobody wants — kill it however it was
    // started, by pid as well as by tmux session.
    let killed_wake = match read_run(&id) {
        Ok(state) => crate::spawner::kill(&state, &id, exec).did_anything(),
        Err(_) => false,
    };
    Ok(StopOutcome { id, status, reason, killed_wake })
}

// ---------------------------------------------------------------------------
// resume
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeOutcome {
    pub id: String,
    pub status: Status,
    pub reason: String,
    pub wake: Option<WakeStart>,
    /// Why no wake was started, when none was.
    pub note: Option<String>,
}

/// Undo `stop`: bring a stopped or failed run back. `done` stays done — its goal was met, and
/// more work is a new run with a goal of its own.
///
/// The run comes back `sleeping` and due, never `running`: that is a state poke already knows how
/// to revive, so a wake that fails to start here still gets taken on the next poke rather than
/// leaving the run stranded. A gate that was open when it stopped is still the question, so the
/// run goes back to waiting on it instead.
pub fn resume_run(id: &str, reason: Option<String>, no_wake: bool) -> Result<ResumeOutcome, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let reason = reason.unwrap_or_else(|| "resumed by hand".to_string());
    // A wake that `stop` failed to kill must not get a second one alongside it.
    refuse_if_busy(&id)?;
    let status = crate::state::transaction(&id, |path, state| {
        let previous = crate::state::commands::status_str(state.status);
        match state.status {
            Status::Stopped | Status::Failed => {}
            Status::Done => {
                return Err(OttoError::conflict(format!(
                    "{id} is done — its goal was met; start a new run for more work"
                )))
            }
            _ => {
                return Err(OttoError::conflict(format!(
                    "{id} is {previous} — only a stopped or failed run can be resumed"
                )))
            }
        }
        // A fresh start: whatever counted towards giving up last time should not count again.
        state.incomplete_wakes = 0;
        state.spawn_attempts = 0;
        state.ticks_without_progress = 0;
        if state.gate.is_some() {
            state.status = Status::AwaitingHuman;
            state.next_wake_at = None;
        } else {
            state.status = Status::Sleeping;
            state.next_wake_at = Some(if no_wake { state.period_wake_at() } else { crate::clock::Timestamp::now() });
        }
        crate::event::record(
            path,
            &crate::event::Event::StatusChanged {
                from: previous.to_string(),
                to: crate::state::commands::status_str(state.status).to_string(),
                reason: Some(reason.clone()),
                because: None,
            },
        )?;
        Ok(state.status)
    })?;
    let mut outcome = ResumeOutcome { id: id.clone(), status, reason, wake: None, note: None };
    let state = read_run(&id)?;
    if let Some(gate) = &state.gate {
        outcome.note = Some(format!("waiting on its open gate {} ({}) — answer it to continue", gate.id, gate.slug));
        return Ok(outcome);
    }
    if no_wake {
        let at = state.next_wake_at.map(relative).unwrap_or_default();
        outcome.note = Some(format!("not waking it now — next wake {at}"));
        return Ok(outcome);
    }
    match start_wake(&id, Caller::Background.strategy(state.launcher.detach), None) {
        Ok(wake) => outcome.wake = Some(wake),
        // The resume itself stands: the run is due, so the next poke tries again.
        Err(err) => outcome.note = Some(format!("resumed, but the wake did not start: {} — the next poke will retry it", err.message)),
    }
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// logs
// ---------------------------------------------------------------------------

/// What `--decisions` keeps: the lines that changed what the run is doing, as opposed to the
/// wake-started/spent/complete rhythm that surrounds every one of them.
pub const DECISION_EVENTS: &[&str] = &[
    "run-created",
    "phase-changed",
    "status-changed",
    "gate-opened",
    "gate-closed",
    "gate-expired",
    "note-added",
    "note-dropped",
    "budget-warning",
    "budget-exhausted",
    "wake-incomplete",
    "wake-killed",
    "spawn-abandoned",
    "authorized",
    "lock-broken",
    "lock-lost",
];

/// The `wake-spent` counters. Six-digit cache reads are the norm and nobody compares them to
/// the token; `359k` is what a person actually reads off the line.
const TOKEN_KEYS: &[&str] = &["inputTokens", "outputTokens", "cacheRead", "cacheCreation"];

pub fn humanise(n: u64) -> String {
    match n {
        n if n < 10_000 => n.to_string(),
        n if n < 1_000_000 => format!("{}k", (n as f64 / 1_000.0).round() as u64),
        n => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// `--since`: an age (`45m`, `2h`, `3d`), a date (midnight, local), or a full timestamp.
pub fn parse_since(text: &str, offset: time::UtcOffset) -> Result<time::OffsetDateTime, OttoError> {
    let text = text.trim();
    if let Some(ago) = crate::clock::parse_age(text) {
        return Ok(crate::clock::now() - ago);
    }
    if let Ok(dt) = crate::clock::parse_iso(text) {
        return Ok(dt);
    }
    let day = time::macros::format_description!("[year]-[month]-[day]");
    if let Ok(date) = time::Date::parse(text, &day) {
        return Ok(date.midnight().assume_offset(offset));
    }
    Err(OttoError::usage(format!(
        "--since takes an age (45m, 2h, 3d), a date (2026-09-15) or a timestamp, not {text:?}"
    )))
}

/// One parsed journal line: the JSON, and its instant, when it has one.
pub struct Line {
    pub value: Value,
    pub at: Option<time::OffsetDateTime>,
}

pub fn parse_line(raw: &str) -> Option<Line> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let at = value.get("ts").and_then(Value::as_str).and_then(|ts| crate::clock::parse_iso(ts).ok());
    Some(Line { value, at })
}

/// What `logs` keeps, from `--since`, `--event` and `--decisions`. Filters compose as "and".
pub struct Filter {
    pub since: Option<time::OffsetDateTime>,
    pub events: Vec<String>,
}

impl Filter {
    pub fn keeps(&self, line: &Line) -> bool {
        if let Some(since) = self.since {
            match line.at {
                Some(at) if at >= since => {}
                _ => return false,
            }
        }
        if !self.events.is_empty() {
            let event = line.value.get("event").and_then(Value::as_str).unwrap_or("");
            if !self.events.iter().any(|e| e == event) {
                return false;
            }
        }
        true
    }
}

/// Turns journal lines into what a person reads: local times, a rule wherever the date changes
/// (forty lines can span days, and a bare `05:51` on each does not say which), timestamps inside
/// a line shown in the same clock, and token counts rounded to what the eye takes in. The journal
/// is JSON because a wake writes it; this is for reading.
pub struct Renderer {
    offset: time::UtcOffset,
    last_day: Option<time::Date>,
}

impl Renderer {
    pub fn new(offset: time::UtcOffset) -> Self {
        Renderer { offset, last_day: None }
    }

    pub fn render(&mut self, line: &Line) -> String {
        let mut out = String::new();
        let local = line.at.map(|at| at.to_offset(self.offset));
        let day = local.map(|dt| dt.date());
        if let Some(d) = day.filter(|_| day != self.last_day) {
            let weekday = d.weekday().to_string();
            out.push_str(&format!("── {} {d} ──\n", &weekday[..3]));
            self.last_day = day;
        }
        let hms = time::macros::format_description!("[hour]:[minute]:[second]");
        let time = match local {
            Some(dt) => dt.format(&hms).unwrap_or_default(),
            None => line.value.get("ts").and_then(Value::as_str).unwrap_or("").to_string(),
        };
        let event = line.value.get("event").and_then(Value::as_str).unwrap_or("?");
        let mut rest: Vec<String> = Vec::new();
        if let Value::Object(map) = &line.value {
            for (key, val) in map {
                if key == "ts" || key == "event" {
                    continue;
                }
                rest.push(format!("{key}={}", self.shown(key, val, day)));
            }
        }
        out.push_str(&format!("{time}  {event:<18}  {}", rest.join(" ")));
        out
    }

    /// One field's value. `day` is the line's own date, so a timestamp on the same day is just a
    /// time and one on another day says which.
    fn shown(&self, key: &str, val: &Value, day: Option<time::Date>) -> String {
        match val {
            Value::String(s) => {
                if let Ok(at) = crate::clock::parse_iso(s) {
                    let local = at.to_offset(self.offset);
                    let fmt = if Some(local.date()) == day {
                        time::macros::format_description!("[hour]:[minute]:[second]")
                    } else {
                        time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]")
                    };
                    return local.format(&fmt).unwrap_or_else(|_| s.clone());
                }
                // Long prose (a goal, a verbatim answer) makes the log unreadable as a sequence;
                // `otto show` is where full text belongs.
                if s.chars().count() > 80 {
                    format!("{}…", s.chars().take(77).collect::<String>())
                } else {
                    s.clone()
                }
            }
            Value::Number(n) if TOKEN_KEYS.contains(&key) => n.as_u64().map(humanise).unwrap_or_else(|| n.to_string()),
            other => other.to_string(),
        }
    }
}

/// The line above the log: where the run stands, so the tail below it has a frame. Someone
/// arriving cold otherwise reads forty lines of wake rhythm and still has to ask `otto show`.
pub fn logs_header(state: &RunState, short: &str, offset: time::UtcOffset) -> String {
    let since = state.created_at.dt().to_offset(offset).date();
    let days = (crate::clock::now() - state.created_at.dt()).whole_days();
    let age = if days == 0 { "today".to_string() } else { format!("{days}d") };
    let last = match &state.wake {
        Some(wake) => format!("last wake {}", relative(wake.started_at)),
        None => "no wake yet".to_string(),
    };
    let gate = match &state.gate {
        Some(gate) => format!(", gate {} {} open", gate.id, gate.slug),
        None => String::new(),
    };
    format!(
        "# {short} · {} wake(s) since {since} ({age}) · {last} · {}{gate} · times {}",
        state.budget.spent_wakes,
        status_label(state),
        crate::clock::offset_label(offset)
    )
}

/// Which journal lines to show.
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Trailing lines [default: 40, or everything with `since`]
    pub lines: Option<usize>,
    pub since: Option<String>,
    pub events: Vec<String>,
    pub decisions: bool,
}

/// One rendered journal line. `text` may start with a day rule on a line of its own.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub text: String,
    pub event: String,
    pub raw: Value,
}

/// A journal being read, and where to pick it up again. The renderer is kept so that following
/// on only prints a day rule when the day actually changes.
pub struct LogCursor {
    path: std::path::PathBuf,
    filter: Filter,
    renderer: Renderer,
    seen: usize,
}

impl LogCursor {
    /// Lines written since the last read.
    pub fn poll(&mut self) -> Vec<LogLine> {
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        if text.len() <= self.seen {
            return Vec::new();
        }
        let fresh: Vec<Line> = text[self.seen..].lines().filter_map(parse_line).filter(|l| self.filter.keeps(l)).collect();
        self.seen = text.len();
        fresh.iter().map(|l| self.log_line(l)).collect()
    }

    fn log_line(&mut self, line: &Line) -> LogLine {
        LogLine {
            text: self.renderer.render(line),
            event: line.value.get("event").and_then(Value::as_str).unwrap_or("?").to_string(),
            raw: line.value.clone(),
        }
    }
}

pub struct LogPage {
    pub header: String,
    pub lines: Vec<LogLine>,
    /// Continue from here to follow.
    pub cursor: LogCursor,
}

/// The tail of a run's journal, readably. `offset` is the clock times are shown in; read it
/// with `clock::local_offset` before any thread starts (see there).
pub fn read_logs(id: &str, query: &LogQuery, offset: time::UtcOffset) -> Result<LogPage, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let state = read_run(&id)?;
    let path = crate::paths::run_dir(&id)?.join("journal.jsonl");

    let mut events: Vec<String> = query.events.iter().map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).collect();
    if query.decisions {
        events.extend(DECISION_EVENTS.iter().map(|e| e.to_string()));
    }
    let filter = Filter { since: query.since.as_deref().map(|s| parse_since(s, offset)).transpose()?, events };
    // `since` says where to start, so it shows everything from there unless `lines` says otherwise.
    let limit = query.lines.unwrap_or(if filter.since.is_some() { usize::MAX } else { 40 });

    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let kept: Vec<Line> = text.lines().filter_map(parse_line).filter(|l| filter.keeps(l)).collect();
    let start = kept.len().saturating_sub(limit);
    let mut cursor = LogCursor { path, filter, renderer: Renderer::new(offset), seen: text.len() };
    let lines = kept[start..].iter().map(|l| cursor.log_line(l)).collect();
    Ok(LogPage { header: logs_header(&state, &short_id(&id), offset), lines, cursor })
}

// ---------------------------------------------------------------------------
// Watching a live wake
// ---------------------------------------------------------------------------

/// What a running wake looks like right now, for a caller that cannot `tmux attach`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveView {
    /// The tmux pane's contents, with its colour escapes.
    Pane(String),
    /// The tail of `wake.log`, for a wake backgrounded without tmux.
    Log(String),
    /// No wake is running; the message says where to look instead.
    Ended(String),
}

/// How many trailing lines of `wake.log` stand in for a pane.
const LIVE_LOG_LINES: usize = 200;

pub fn live_view(id: &str, exec: &mut dyn crate::exec::Exec) -> LiveView {
    let session = crate::detach::session_name(id);
    if let Some(pane) = crate::detach::capture_pane(exec, &session) {
        return LiveView::Pane(pane);
    }
    if LockLiveness.probe(id).is_busy() {
        if let Ok(text) = crate::paths::run_dir(id).and_then(|d| Ok(std::fs::read_to_string(d.join("wake.log"))?)) {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(LIVE_LOG_LINES);
            return LiveView::Log(lines[start..].join("\n"));
        }
    }
    let short = short_id(id);
    LiveView::Ended(format!(
        "no wake is running for {id} right now — `otto show {short}` for where it stands, `otto logs {short}` for what it has done"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::{open_gate, test_init, OpenGateArgs};

    fn gate_open(id: &str) {
        open_gate(OpenGateArgs {
            id: id.to_string(),
            slug: "plan-review".to_string(),
            question: Some("Approve the plan?\n\n- Approve\n- Revise *(default)*".to_string()),
            question_file: None,
            stdin: false,
            expires_at: None,
            expires_in: None,
        })
        .unwrap();
    }

    #[test]
    fn a_gate_is_listed_as_needing_you_with_its_default_first() {
        let _h = TempHome::new();
        test_init("2026-09-20-gated", "a goal").unwrap();
        test_init("2026-09-20-quiet", "a goal").unwrap();
        gate_open("2026-09-20-gated");
        let view = list_runs(false).unwrap();
        assert_eq!(view.rows.len(), 2);
        assert_eq!(view.needs_you.len(), 1);
        let needs = &view.needs_you[0];
        assert_eq!(needs.id, "2026-09-20-gated");
        assert_eq!(needs.short, "gated");
        assert_eq!(needs.options, vec!["Revise".to_string(), "Approve".to_string()]);
    }

    #[test]
    fn run_detail_carries_the_question_and_its_parsed_options() {
        let _h = TempHome::new();
        test_init("2026-09-20-detail", "a goal").unwrap();
        gate_open("2026-09-20-detail");
        let detail = run_detail(Some("detail")).unwrap();
        let gate = detail.gate.expect("a gate is open");
        assert_eq!(gate.options, vec!["Approve".to_string(), "Revise".to_string()]);
        assert_eq!(gate.default.as_deref(), Some("Revise"));
        assert!(gate.text.unwrap().contains("Approve the plan?"));
        assert!(detail.handoff.is_none(), "an open gate is shown instead of the handoff");
        // And it serialises, which is what the web page reads.
        let json = serde_json::to_value(run_detail(Some("detail")).unwrap()).unwrap();
        assert_eq!(json["gate"]["default"], "Revise");
        assert_eq!(json["state"]["id"], "2026-09-20-detail");
    }

    #[test]
    fn a_background_answer_with_a_bad_choice_leaves_the_gate_open() {
        let _h = TempHome::new();
        test_init("2026-09-20-bad", "a goal").unwrap();
        gate_open("2026-09-20-bad");
        let err = answer_in_background("bad", Some("aprove"), None, true).expect_err("must refuse");
        assert_eq!(err.code, 1);
        assert!(read_run("2026-09-20-bad").unwrap().gate.is_some());

        let ok = answer_in_background("bad", Some("approve"), None, true).unwrap();
        assert_eq!(ok.answer, "Approve");
        assert!(ok.wake.is_none());
        assert!(read_run("2026-09-20-bad").unwrap().gate.is_none());
    }

    fn stopped(id: &str) {
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() });
        stop_run(id, None, false, &mut exec).unwrap();
    }

    #[test]
    fn a_stopped_run_resumes_sleeping_on_a_clean_slate() {
        let _h = TempHome::new();
        test_init("2026-09-25-back", "a goal").unwrap();
        crate::state::transaction("2026-09-25-back", |_p, state| {
            state.incomplete_wakes = 3;
            state.ticks_without_progress = 7;
            Ok(())
        })
        .unwrap();
        stopped("back");

        let outcome = resume_run("back", None, true).unwrap();
        assert_eq!(outcome.status, Status::Sleeping);
        assert!(outcome.wake.is_none());
        let state = read_run("2026-09-25-back").unwrap();
        assert_eq!(state.status, Status::Sleeping);
        assert!(state.next_wake_at.is_some(), "a resumed run must be revivable by poke");
        assert_eq!((state.incomplete_wakes, state.ticks_without_progress), (0, 0));
        let journal = std::fs::read_to_string(crate::paths::run_dir("2026-09-25-back").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("resumed by hand"), "the resume must be in the audit trail");
    }

    #[test]
    fn a_resumed_run_with_an_open_gate_goes_back_to_waiting_on_it() {
        let _h = TempHome::new();
        test_init("2026-09-25-asked", "a goal").unwrap();
        gate_open("2026-09-25-asked");
        stopped("asked");

        let outcome = resume_run("asked", None, false).unwrap();
        assert_eq!(outcome.status, Status::AwaitingHuman);
        assert!(outcome.wake.is_none(), "the question is still unanswered — nothing to wake for");
        assert!(read_run("2026-09-25-asked").unwrap().next_wake_at.is_none());
    }

    #[test]
    fn only_a_stopped_or_failed_run_can_be_resumed() {
        let _h = TempHome::new();
        test_init("2026-09-25-live", "a goal").unwrap();
        assert_eq!(resume_run("live", None, true).expect_err("still live").code, 2);

        test_init("2026-09-25-met", "a goal").unwrap();
        crate::state::transaction("2026-09-25-met", |_p, state| {
            state.status = Status::Done;
            Ok(())
        })
        .unwrap();
        let err = resume_run("met", None, true).expect_err("done stays done");
        assert!(err.message.contains("new run"), "got: {}", err.message);

        test_init("2026-09-25-broke", "a goal").unwrap();
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() });
        stop_run("broke", None, true, &mut exec).unwrap();
        assert_eq!(resume_run("broke", None, true).unwrap().status, Status::Sleeping);
    }

    #[test]
    fn a_background_wake_of_a_finished_run_is_refused() {
        let _h = TempHome::new();
        test_init("2026-09-20-done", "a goal").unwrap();
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() });
        stop_run("done", None, false, &mut exec).unwrap();
        assert_eq!(wake_in_background("done", None).expect_err("finished").code, 2);
    }

    /// The web page never runs a wake inside the server's request: `Detach::None` from anything
    /// but a terminal is a detached process, as it is for poke.
    #[test]
    fn only_a_terminal_ever_gets_a_foreground_wake() {
        assert_eq!(Caller::Terminal.strategy(Detach::None), Strategy::Foreground);
        assert_eq!(Caller::Terminal.strategy(Detach::Tmux), Strategy::Tmux);
        assert_eq!(Caller::Background.strategy(Detach::None), Strategy::Detached);
        assert_eq!(Caller::Background.strategy(Detach::Tmux), Strategy::Tmux);
    }

    #[test]
    fn a_log_cursor_picks_up_only_what_was_written_since() {
        let _h = TempHome::new();
        test_init("2026-09-20-logs", "a goal").unwrap();
        let mut page = read_logs("logs", &LogQuery::default(), time::UtcOffset::UTC).unwrap();
        assert!(page.lines.iter().any(|l| l.event == "run-created"));
        assert!(page.cursor.poll().is_empty());
        gate_open("2026-09-20-logs");
        let fresh = page.cursor.poll();
        assert!(fresh.iter().any(|l| l.event == "gate-opened"), "got {:?}", fresh.iter().map(|l| &l.event).collect::<Vec<_>>());
    }
}
