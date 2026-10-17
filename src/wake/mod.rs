//! `otto wake <id>` — run one wake, then validate what it left behind.
//!
//! A wake is one process. It orients from the run directory, does a stretch of work, reaches a
//! stopping point and exits. Nothing survives it but the run directory, which is what makes
//! "the run directory holds the truth" structurally true rather than a rule someone has to
//! remember.
//!
//! This module is the parent of that process, and the ordering of its work is the point:
//!
//! 1. Take the wake lock, so a second wake cannot start on top of this one.
//! 2. Refuse to launch at all when launching would be pointless — the run is over, its budget
//!    is spent, or its launcher could never resolve what it wraps.
//! 3. Record the wake (number, start, deadline) **before** spawning, so a crash in the next
//!    instant still leaves evidence that a wake happened.
//! 4. Spawn, and capture stdout and stderr separately — stdout is the wake's own narration and
//!    the launcher may be writing progress to stderr, and only the first is worth keeping.
//! 5. Account for what it used, from the session transcript it left behind (`transcript`).
//! 6. **Validate the contract, whatever happened to the child** (`contract`).
//! 7. Record the outcome, and on failure arm a backoff so poke retries rather than hammering.
//!
//! Step 6 is why validation lives in the parent. A wake that crashed, was killed at its
//! deadline, or simply stopped talking cannot report on itself; the process that outlives it
//! can. That is the difference between v1's silent stranded run and a retry.

pub mod contract;
pub mod launcher;
pub mod prompt;
pub mod transcript;

use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::event::Event;
use crate::exec::{Exec, Output, RealExec};
use crate::liveness::WakeLock;
use crate::state::{transaction, RunState, Status, Wake, WakeOutcome};
use serde_json::Value;
use std::time::Duration;

#[derive(clap::Args, Debug, Clone)]
pub struct WakeArgs {
    /// The run: its number or name (a unique prefix of the name will do)
    pub id: String,
    /// A person's answer to the open gate, handed to the wake verbatim
    #[arg(long)]
    pub answer: Option<String>,
    /// Print the command that would run, and change nothing
    #[arg(long = "dry-run")]
    pub dry_run: bool,
    /// Run this wake in your terminal instead of backgrounding it
    #[arg(long, conflicts_with = "detach")]
    pub watch: bool,
    /// How to background this wake, overriding the run's own setting for this wake only
    #[arg(long, value_enum)]
    pub detach: Option<crate::state::Detach>,
    /// Run the wake in *this* process, whatever the run prefers.
    ///
    /// Internal, and the reason `otto wake` can default to backgrounding itself at all:
    /// `spawner::start` runs `otto wake <id>` as its child, and that child is already inside the
    /// tmux session or detached process that was just created for it. Without this it would
    /// read the run's `detach` preference, conclude it should background itself, and spawn another
    /// container — forever. Deliberately not spelled `--detach none`: that is a person choosing
    /// where a wake runs, while this is the harness telling a child it is already in place.
    #[arg(long, hide = true)]
    pub foreground: bool,
}

/// What a wake used, and whether that could be found out at all.
///
/// This used to be parsed from a JSON result document on stdout. Without `-p` there is no such
/// document — see `launcher` for why `-p` had to go and `transcript` for what replaced it. So
/// `usage` is `None` whenever the transcript could not be read, which is an ordinary outcome
/// rather than a failure: a wake that was killed at its deadline may have written nothing, and
/// a sandbox may not be able to see `~/.claude` at all.
#[derive(Debug, Default, Clone)]
pub struct WakeResult {
    pub usage: Option<transcript::Usage>,
    /// Set when the wake ended on the account's usage limit — see `record_usage_limited`.
    pub limit: Option<transcript::UsageLimit>,
}

impl WakeResult {
    /// Zeroes when there was no transcript, so a caller does not have to branch just to journal.
    fn tokens(&self) -> transcript::Usage {
        self.usage.clone().unwrap_or_default()
    }
}

/// Hours since the run was created, for the wall-clock budget dimension. `created_at` is a
/// `Timestamp`, validated when `run.json` was deserialized — so unlike the `String` it used to
/// be, there is no parse failure left to silently read as `0.0` and disable the hours budget.
pub(crate) fn elapsed_hours(state: &RunState) -> f64 {
    (crate::clock::now() - state.created_at.dt()).as_seconds_f64() / 3600.0
}

/// Which budget dimension is spent, if any. Checked before launching, because a wake that
/// cannot afford to finish is worse than one that never starts.
///
/// Dollars are deliberately not a dimension here. Nothing can measure them without `-p`, so
/// `budget.usd` is refused at create time (`state::commands`) and never read: checking a number
/// that can only ever be zero would be a ceiling that silently never fires.
fn budget_exceeded(state: &RunState) -> Option<String> {
    let budget = &state.budget;
    if budget.wakes > 0 && budget.spent_wakes >= budget.wakes {
        return Some(format!("wake budget spent: {} of {}", budget.spent_wakes, budget.wakes));
    }
    let hours = elapsed_hours(state);
    if budget.hours > 0 && hours >= budget.hours as f64 {
        return Some(format!("time budget spent: {:.1}h of {}h", hours, budget.hours));
    }
    None
}

/// The command a wake would run, for a person to read. The harness is printed as a length
/// rather than inline: it is several KB and would bury the part a human is actually checking.
pub fn dry_run_line(argv: &[String]) -> String {
    let shown: Vec<String> = argv
        .iter()
        .map(|a| {
            if a == prompt::HARNESS {
                format!("<harness: {} bytes>", a.len())
            } else {
                a.clone()
            }
        })
        .collect();
    shown.join(" ")
}

/// What `otto run --dry-run` prints: the first wake's command line, for a run that does not
/// exist yet. Same argv builder as the real thing, so what it shows is what would run.
pub fn first_wake_dry_run_line(state: &crate::state::RunState, run_dir: &std::path::Path) -> Result<String, OttoError> {
    let launcher = launcher::resolve(state)?;
    let prompt = prompt::user_prompt(&state.id, run_dir);
    let session = transcript::session_id(&state.id, 1, &Timestamp::now().to_string());
    Ok(dry_run_line(&launcher::argv(state, &launcher, &prompt, &session)))
}

pub fn wake(args: WakeArgs) -> Result<(), OttoError> {
    let mut exec = RealExec;
    run_wake(&args, &mut exec)
}

/// A wake, and then — when it claimed done, or was the check that sent a claim back — the wake
/// that follows from that, straight away and in the same place, rather than up to five minutes
/// later when poke notices. Poke is still the backstop: the next wake is due now either way, so if
/// this process dies in between, the next pass starts it.
///
/// Bounded twice over: a claim alternates checker and doer, and `maxDoneReopens` caps how often a
/// check can send it back; `MAX_FOLLOW_ONS` caps the chain should that ever be set to 0 (no limit).
pub fn run_wake(args: &WakeArgs, exec: &mut dyn Exec) -> Result<(), OttoError> {
    const MAX_FOLLOW_ONS: u32 = 6;
    run_one_wake(args, exec)?;
    if args.dry_run {
        return Ok(());
    }
    let next = WakeArgs { answer: None, ..args.clone() };
    for _ in 0..MAX_FOLLOW_ONS {
        if !follows_on(&crate::state::read_run(&args.id)?) {
            break;
        }
        run_one_wake(&next, exec)?;
    }
    Ok(())
}

/// Whether the wake that just finished leaves one that should start now: it completed, and either
/// claimed done (a check is next) or was a check that reopened the run (the fix is next). Not when
/// anything else holds the run — a gate, a pause, a block.
fn follows_on(state: &RunState) -> bool {
    let Some(wake) = &state.wake else { return false };
    if wake.outcome != Some(WakeOutcome::Complete) || state.gate.is_some() || state.is_paused() {
        return false;
    }
    if state.status != Status::Sleeping {
        return false;
    }
    let claimed = state.done_claim.as_ref().is_some_and(|c| c.by_wake == wake.n);
    let reopened = state.last_verdict.as_ref().is_some_and(|v| !v.confirmed && v.wake == wake.n);
    claimed || reopened
}

/// One wake, with the spawner injected so the whole path is testable without spending money.
fn run_one_wake(args: &WakeArgs, exec: &mut dyn Exec) -> Result<(), OttoError> {
    let run_path = crate::paths::run_dir(&args.id)?;
    let state = crate::state::read_run(&args.id)?;

    // A late wake firing into a closed run is normal, and doing nothing is correct.
    if state.status.is_terminal() {
        println!("{} is {:?} — nothing to do", args.id, state.status);
        return Ok(());
    }
    let launcher = launcher::resolve(&state)?;
    if let Some(why) = budget_exceeded(&state) {
        return block_on_budget(&args.id, &why);
    }

    let base = prompt::for_wake(&args.id, &run_path, &state);
    let mut prompt = match &args.answer {
        Some(answer) => prompt::answer_prompt(&base, answer),
        None => base,
    };
    // Read from the same `state` the prompt is built from, so the notes marked as given below are
    // exactly the ones this wake was told — never one added a moment later.
    let notes = crate::notes::for_wake(&run_path, &state);
    if let Some(given) = &notes {
        prompt = prompt::with_notes(&prompt, &given.block);
    }

    // Ahead of the argv rather than after the lock, because the session id is part of the command
    // line: it is what makes the wake's usage findable afterwards, so `--dry-run` has to be able
    // to show it. Capturing `started_at` early is harmless and if anything safer — the handoff
    // freshness check compares against it, and an earlier mark is the more permissive one.
    let deadline_minutes = state.policy.max_wake_minutes;
    let started_at = std::time::SystemTime::now();
    let started = Timestamp::now();
    let deadline = Timestamp::in_minutes(deadline_minutes as i64);
    let wake_number = state.wake.as_ref().map(|w| w.n).unwrap_or(0) + 1;
    let session = transcript::session_id(&args.id, wake_number, &started.to_string());

    let argv = launcher::argv(&state, &launcher, &prompt, &session);

    if args.dry_run {
        println!("{}", dry_run_line(&argv));
        return Ok(());
    }

    // Hold the wake lock for the whole wake. This is what liveness observes, and taking it
    // here means a second `otto wake` refuses rather than racing.
    let _wake_lock = WakeLock::acquire(&args.id)?;

    // Recorded before the spawn, so a crash in the next instant still leaves evidence.
    let id = args.id.clone();
    let launcher_name = launcher.name.clone();
    transaction(&id, |path, state| {
        state.wake = Some(Wake {
            n: wake_number,
            started_at: started,
            deadline_at: deadline,
            launcher: launcher_name.clone(),
            pid: Some(std::process::id()),
            // Written before the spawn so the transcript is findable even if this wake never
            // comes back to record what it used.
            session: Some(session.clone()),
            outcome: None,
        });
        if state.status == Status::Sleeping {
            // The timer fired; the run is working again until it says otherwise.
            state.status = Status::Running;
            state.next_wake_at = None;
        }
        // Getting this far *is* the proof a spawn took hold, so clear poke's attempt counter.
        // Without this, poke's backoff reads every spawn as having failed and defers a run whose
        // wakes are starting perfectly well — which is what happened the first time a real wake
        // was killed and poke then refused to restart it for five minutes.
        state.spawn_attempts = 0;
        if let Some(given) = &notes {
            crate::notes::mark_given(state, &given.ids, wake_number);
        }
        // A check a wake armed belonged to the sleep that just ended; left in place it would stand
        // in front of the next period sleep too, still asking at the old wake's gap. A person's
        // check belongs to the run, and a wake is a real look at the world, so the safety net's
        // count of "nothing new" results starts again from here.
        if state.check.as_ref().is_some_and(|c| !c.pinned) {
            state.check = None;
        }
        if let Some(check) = state.check.as_mut() {
            check.consecutive_no_change = 0;
        }
        // A pause whose time has come is over; one still holding stays — a person started this
        // wake by hand, and the pause still holds back the scheduled ones after it.
        if state.paused.is_some() && !state.is_paused() {
            state.paused = None;
            crate::event::record(path, &Event::Unpaused { by: "time".to_string() })?;
        }
        crate::event::record(
            path,
            &Event::WakeStarted {
                wake: wake_number,
                deadline_at: deadline,
                launcher: launcher_name.clone(),
            },
        )
    })?;

    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let workdir = crate::config::workdir_for(&state);
    let output = exec.exec_in(&argv_refs, workdir.as_deref(), Duration::from_secs(deadline_minutes * 60));
    // The child has exited, so its transcript is finished being written.
    let result = WakeResult {
        usage: transcript::usage_for(&session),
        // A limit ends claude with a non-zero exit, so a clean one never needs the second read.
        limit: if output.code != 0 { transcript::limit_for(&session) } else { None },
    };

    finish_wake(&id, &run_path, started_at, &output, &result)
}

/// Account, validate, record. Runs on every path out of a spawn, including a kill.
fn finish_wake(
    id: &str,
    run_path: &std::path::Path,
    started_at: std::time::SystemTime,
    output: &Output,
    result: &WakeResult,
) -> Result<(), OttoError> {
    // Usage is recorded before validation, because it was used either way.
    let used = result.tokens();
    let exit_code = output.code;
    let timed_out = output.timed_out;
    let usage_found = result.usage.is_some();
    transaction(id, |path, state| {
        state.budget.spent_wakes += 1;
        crate::event::record(
            path,
            &Event::WakeSpent {
                turns: used.turns,
                spent_wakes: state.budget.spent_wakes,
                input_tokens: used.input_tokens,
                output_tokens: used.output_tokens,
                cache_read: used.cache_read,
                cache_creation: used.cache_creation,
                tool_errors: used.tool_errors,
            },
        )?;
        if !usage_found {
            // Not a failure: the wake is validated against its contract either way. But blind
            // accounting has to be visible, or a run of zero-token wakes reads as a run that did
            // nothing. Expected under a sandbox that cannot see `~/.claude`.
            crate::event::record(
                path,
                &Event::UsageUnavailable {
                    note: "no readable session transcript; tokens unknown for this wake".to_string(),
                },
            )?;
        }
        if exit_code != 0 {
            // The wake's own process failed. That is not the same as failing the contract — a
            // wake can exit non-zero and still stop somewhere resumable — so it gets its own line.
            // Without a result document the exit code is the only thing the child says about
            // itself, which is why `timedOut` rides along: 124 is otherwise just a number.
            crate::event::record(path, &Event::WakeReportedError { exit_code, timed_out, turns: used.turns })?;
        }
        // Warn once as a budget nears its limit, so the first sign is not the run stopping dead.
        // `budget_exceeded` handles the ceiling itself, before the next wake launches.
        if let Some(fraction) = state.budget.worst_fraction(elapsed_hours(state)) {
            // A run warned before the marker moved out of `facts` still carries it there; adopt
            // it rather than warn a second time.
            if state.budget_warned_at.is_none() {
                if let Some(Value::String(at)) = state.facts.remove("budgetWarnedAt") {
                    state.budget_warned_at = Some(Timestamp::parse(&at).unwrap_or_else(|_| Timestamp::now()));
                }
            }
            if (0.8..1.0).contains(&fraction) && state.budget_warned_at.is_none() {
                state.budget_warned_at = Some(Timestamp::now());
                crate::event::record(
                    path,
                    &Event::BudgetWarning {
                        fraction_spent: (fraction * 100.0).round() / 100.0,
                        budget: state.budget.clone(),
                    },
                )?;
            }
        }
        Ok(())
    })?;

    // A wake that finished cleanly and left nothing pending sleeps for the run's period. Only a
    // clean exit that rewrote the handoff: a crash, a kill mid-work, or a wake that did nothing
    // at all is exactly the stranded run the contract exists to catch, and it gets the backoff,
    // not a full period. A wake that armed its own timer (the start of this wake cleared the last
    // one) or opened a gate has already said what comes next.
    if output.code == 0 && !output.timed_out {
        transaction(id, |path, state| {
            let cap = state.policy.handoff_max_bytes as usize;
            if state.status == Status::Running
                && state.gate.is_none()
                && state.next_wake_at.is_none()
                && contract::handoff_is_fresh(path, started_at, cap).is_ok()
            {
                let next = state.period_wake_at();
                state.status = Status::Sleeping;
                state.next_wake_at = Some(next);
                crate::event::record(
                    path,
                    &Event::TimerArmed { next_wake_at: next, note: Some("period".to_string()) },
                )?;
            }
            Ok(())
        })?;
    }

    // Re-read: the wake wrote to run.json while it ran, and that is what gets validated.
    let state = crate::state::read_run(id)?;
    let mut outcome = contract::validate(run_path, &state, started_at);

    // A timed-out or crashed child usually fails the contract anyway, but say which it was —
    // "killed at its deadline" and "ended without a gate" want different fixes.
    if let contract::Outcome::Incomplete(why) = &outcome {
        let detail = if output.timed_out {
            format!("killed at its deadline; {why}")
        } else if output.code != 0 {
            format!("exited {}; {why}", output.code)
        } else {
            why.clone()
        };
        outcome = contract::Outcome::Incomplete(detail);
    }

    match &outcome {
        contract::Outcome::Complete => {
            // Taken outside the lock: it runs git in the run's repos.
            let mark = crate::progress::mark(run_path, &state);
            let mut stalled = None;
            transaction(id, |path, state| {
                if let Some(wake) = state.wake.as_mut() {
                    wake.outcome = Some(WakeOutcome::Complete);
                }
                state.incomplete_wakes = 0;
                state.clear_start_failures();
                let n = state.wake.as_ref().map(|w| w.n).unwrap_or(0);
                // Only a wake that met the contract delivers its notes; a crashed one leaves them
                // for the next wake to be given again.
                if n > 0 {
                    crate::notes::deliver(path, state, n)?;
                }
                let progress = crate::progress::account(state, mark.clone(), n);
                if crate::progress::stalled(state) {
                    stalled = Some(state.ticks_without_progress);
                    let from = crate::state::commands::status_str(state.status).to_string();
                    let reason = format!("{} wakes in a row changed nothing", state.ticks_without_progress);
                    state.block(crate::state::BlockedCause::Stall, Some(reason.clone()));
                    crate::event::record(
                        path,
                        &Event::StatusChanged {
                            from,
                            to: "blocked".into(),
                            reason: Some(reason),
                            because: Some(crate::state::BlockedCause::Stall.label().into()),
                            left_open: Vec::new(),
                        },
                    )?;
                }
                crate::event::record(
                    path,
                    &Event::WakeComplete { status: crate::state::commands::status_str(state.status).to_string(), progress },
                )
            })?;
            if let Some(wakes) = stalled {
                open_stall_gate(id, run_path, wakes)?;
            }
            // Tokens rather than dollars: there is no per-wake dollar figure to report without
            // `-p`, and an estimate dressed up as a measurement is worse than neither.
            println!(
                "wake complete — {} ({} turns, {} in / {} out tokens{})",
                describe_stop(&state),
                used.turns,
                used.input_tokens,
                used.output_tokens,
                if usage_found { "" } else { " — usage unavailable" },
            );
            Ok(())
        }
        contract::Outcome::Incomplete(why) => {
            // Keep what the wake said, but only when it failed. A wake's narration is otherwise
            // discarded on purpose — the journal is the audit trail, and a transcript per wake
            // would bury it. But a wake that did not finish is precisely the case where the
            // narration is the only clue, and there is no session left to scroll back through.
            let dump = run_path.join(format!("wake-{}-output.txt", state.wake.as_ref().map(|w| w.n).unwrap_or(0)));
            let body = format!(
                "wake failed the contract: {why}\n\nexit code: {}{}\n\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
                output.code,
                if output.timed_out { " (killed at its deadline)" } else { "" },
                output.stdout.trim(),
                output.stderr.trim(),
            );
            let _ = crate::state::write_atomic(&dump, &body);
            match &result.limit {
                Some(limit) => {
                    let next = record_usage_limited(id, limit)?;
                    println!("wake stopped by the usage limit — sleeping until {next}\n  what it said: {}", dump.display());
                }
                None if couldnt_start(output, result, started_at.elapsed().unwrap_or_default())
                    && contract::handoff_is_fresh(run_path, started_at, 0).is_err() =>
                {
                    let next = record_start_failure(id, &claude_said(output).unwrap_or_else(|| why.clone()))?;
                    println!("the wake never got going — retrying at {next}\n  what it said: {}", dump.display());
                }
                None => {
                    record_incomplete(id, why)?;
                    println!("wake incomplete — {why}\n  what it said: {}", dump.display());
                }
            }
            Ok(())
        }
    }
}

/// Record a wake the account's usage limit stopped, and sleep the run until the limit resets.
///
/// Not `record_incomplete`. Every wake until the reset fails the same way, a few seconds in, so
/// the backoff's retries were five quick failures that blocked a run which only had to wait —
/// observed on a live `dev-flow` run, blocked at 22:23 by a limit that lifted at 23:40. So the wake
/// is not counted toward `maxIncompleteWakes`, and the run sleeps to a minute past the reset (for
/// clock skew). With no reset time in the transcript, or one already gone, it sleeps an hour or five
/// minutes and the next wake finds out. `limitedUntil` marks the sleep, so a check script does not
/// stand in front of the resume — the wake was likely stopped mid-work — and the list says why.
pub fn record_usage_limited(id: &str, limit: &transcript::UsageLimit) -> Result<Timestamp, OttoError> {
    let next = match limit.resets_at {
        Some(at) if !at.is_past() => Timestamp::at(at.dt() + time::Duration::minutes(1)),
        Some(_) => Timestamp::in_minutes(5),
        None => Timestamp::in_minutes(60),
    };
    transaction(id, |path, state| {
        if let Some(wake) = state.wake.as_mut() {
            wake.outcome = Some(WakeOutcome::Incomplete);
        }
        state.status = Status::Sleeping;
        state.next_wake_at = Some(next);
        state.limited_until = Some(next);
        crate::event::record(path, &Event::UsageLimited { limit: limit.kind.clone(), resets_at: limit.resets_at })?;
        crate::event::record(path, &Event::TimerArmed { next_wake_at: next, note: Some("usage limit".to_string()) })
    })?;
    Ok(next)
}

/// How long a wake with no readable transcript may have run and still count as never having got
/// going. A real wake spends longer than this just orienting.
const COULDNT_START_SECONDS: u64 = 30;

/// Whether a failed wake never got going at all: claude exited non-zero having produced nothing —
/// no output tokens, or, with no transcript to read, within seconds. (The caller also requires that
/// it never rewrote the handoff: a wake that did that got going, whatever else happened.) Seen as
/// five wakes in a row of two live runs, one turn and zero tokens each, every one printing "Failed
/// to authenticate: OAuth session expired and could not be refreshed": the runs were fine, the
/// login was not, and blocking each run on a gate asked its owner a question about their account
/// dressed up as a question about the work. They sat blocked for five days.
pub fn couldnt_start(output: &Output, result: &WakeResult, ran: Duration) -> bool {
    if output.code == 0 || output.timed_out {
        return false;
    }
    match &result.usage {
        Some(usage) => usage.output_tokens == 0,
        None => ran < Duration::from_secs(COULDNT_START_SECONDS),
    }
}

/// The first thing claude printed, when it printed anything — for a wake that couldn't start, that
/// is the whole story ("Failed to authenticate: …").
fn claude_said(output: &Output) -> Option<String> {
    [output.stdout.as_str(), output.stderr.as_str()]
        .iter()
        .flat_map(|text| text.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| crate::exec::truncate(line, 200))
}

/// Record a wake that never got going, and try again later — for as long as it takes. Not counted
/// toward `maxIncompleteWakes`: nothing about the run is wrong, so nothing about it should block.
/// Poke tells the person after a few in a row (`notify`), and the list says so; the backoff caps at
/// an hour, so a run comes back within the hour of whatever broke being fixed.
pub fn record_start_failure(id: &str, why: &str) -> Result<Timestamp, OttoError> {
    let mut next = Timestamp::now();
    transaction(id, |path, state| {
        if let Some(wake) = state.wake.as_mut() {
            wake.outcome = Some(WakeOutcome::Incomplete);
        }
        state.start_failures += 1;
        state.start_failure = Some(why.to_string());
        let consecutive = state.start_failures;
        crate::event::record(path, &Event::StartFailed { reason: why.to_string(), consecutive })?;
        next = Timestamp::in_minutes(crate::poke::backoff_minutes(consecutive));
        // With a question open the run waits on the person, as it did before this wake.
        if state.gate.is_some() {
            if state.status != Status::Blocked {
                state.status = Status::AwaitingHuman;
            }
            return Ok(());
        }
        state.status = Status::Sleeping;
        state.next_wake_at = Some(next);
        crate::event::record(
            path,
            &Event::TimerArmed { next_wake_at: next, note: Some("couldn't start; retrying".to_string()) },
        )
    })?;
    Ok(next)
}

/// Record a wake that did not finish, and arrange for another attempt.
///
/// Shared with `poke`, which reaches the same conclusion by a different route: it kills a wake
/// that has outlived its deadline, and a killed wake cannot record its own failure. Both paths
/// must leave the run in a state something will come back to, or the failure to finish becomes a
/// failure to ever run again.
pub fn record_incomplete(id: &str, why: &str) -> Result<(), OttoError> {
    let state = crate::state::read_run(id)?;
    let limit = state.policy.max_incomplete_wakes;
    let mut consecutive = 0u32;
    let mut gave_up = false;
    let why_owned = why.to_string();
    transaction(id, |path, state| {
        if let Some(wake) = state.wake.as_mut() {
            wake.outcome = Some(WakeOutcome::Incomplete);
        }
        state.incomplete_wakes += 1;
        // This one got going, so whatever kept wakes from starting is over.
        state.clear_start_failures();
        consecutive = state.incomplete_wakes;
        crate::event::record(path, &Event::WakeIncomplete { reason: why_owned.clone(), consecutive })?;
        if limit > 0 && consecutive >= limit {
            // Retrying forever is the expensive failure. Hand it to a person, in a state a
            // person can actually act on.
            gave_up = true;
            state.block(crate::state::BlockedCause::WakeFailures, Some(why_owned.clone()));
        } else {
            // Back off rather than hammer: whatever broke may need a moment.
            let minutes = crate::poke::backoff_minutes(consecutive);
            let next = Timestamp::in_minutes(minutes);
            state.status = Status::Sleeping;
            state.next_wake_at = Some(next);
            crate::event::record(
                path,
                &Event::TimerArmed { next_wake_at: next, note: Some("wake-incomplete backoff".to_string()) },
            )?;
        }
        Ok(())
    })?;
    if gave_up {
        open_stuck_gate(id, &why_owned, consecutive)?;
    }
    Ok(())
}


fn describe_stop(state: &RunState) -> String {
    match state.status {
        Status::AwaitingHuman => match &state.gate {
            Some(gate) => format!("waiting on gate {} ({})", gate.id, gate.slug),
            None => "awaiting a human".to_string(),
        },
        Status::Sleeping => match &state.next_wake_at {
            Some(at) => format!("sleeping until {at}"),
            None => "sleeping until ?".to_string(),
        },
        other => format!("{other:?}").to_lowercase(),
    }
}

/// `blocked` on its own would strand the run, so a gate goes with it — the contract this
/// module enforces applies to otto's own writes too.
fn open_stuck_gate(id: &str, why: &str, consecutive: u32) -> Result<(), OttoError> {
    let short = crate::paths::short_id(id);
    let question = format!(
        "# I keep stopping before I finish\n\n\
         I started work {consecutive} times in a row and each time stopped without leaving things where \
         I could pick them up again, so I've stopped trying until you say.\n\n\
         The last time: {}\n\n\
         That usually means my instructions ask for something I can't do on my own — most often waiting \
         for a reply in the session, which nobody is there to give — or the work is stuck on something \
         outside me. What each attempt said is in its log (`otto logs {short}`).\n\n\
         ## Options\n\n\
         - **Retry** — try again; right once the cause is fixed.\n\
         - **Stop** — retire this run.\n\n\
         Nothing happens until you answer.",
        plain_failure(why)
    );
    crate::state::ops::open_gate(id, "keeps-stopping", &question, None)?;
    Ok(())
}

/// The stall guard, enforced by otto rather than left to a wake to notice: `wakes` in a row met the
/// contract and changed nothing it can see (`progress`). Nothing failed, so the question is only
/// whether to keep looking.
fn open_stall_gate(id: &str, run_dir: &std::path::Path, wakes: u32) -> Result<(), OttoError> {
    let handoff = std::fs::read_to_string(run_dir.join(crate::state::commands::HANDOFF_FILE)).unwrap_or_default();
    let stands = crate::core::where_this_is(&handoff)
        .map(|said| format!("\n\nWhere it stands, as I last put it: {said}"))
        .unwrap_or_default();
    let question = format!(
        "# Nothing has changed in a while\n\n\
         I've looked {wakes} times in a row and found nothing to do: no commits, nothing published or \
         sent, nothing ticked off or recorded. Nothing has failed — what I'm waiting on may just be \
         slow — but each look costs, so I've stopped until you say.{stands}\n\n\
         ## Options\n\n\
         - **Keep going** — carry on as before; I'll ask again if another {wakes} go by the same way.\n\
         - **Stop** — retire this run.\n\n\
         Nothing happens until you answer."
    );
    crate::state::ops::open_gate(id, "nothing-changing", &question, None)?;
    Ok(())
}

/// A contract failure (`contract::stopping_state_is_legal`, `handoff_is_fresh`), said for a person.
/// The original rides along in brackets, because it is what the logs will show.
fn plain_failure(why: &str) -> String {
    let said = if why.contains("killed at its deadline") {
        "it ran past its time limit and was stopped"
    } else if why.contains("nothing would ever bring this run back") || why.contains("nextWakeAt is unset") {
        "it stopped without asking anything or saying when to come back"
    } else if why.contains("was not written") || why.contains("was not rewritten") || why.contains("is empty") {
        "it stopped without writing down where it got to"
    } else if why.contains("over the") && why.contains("cap") {
        "its notes for next time grew past their size limit"
    } else if why.contains("no gate open") {
        "it said it was waiting on you without asking a question"
    } else if why.contains("without a verdict") {
        "it was checking whether the work was finished, and stopped without saying"
    } else {
        "it stopped part-way"
    };
    format!("{said} ({why}).")
}

fn block_on_budget(id: &str, why: &str) -> Result<(), OttoError> {
    let already_gated = crate::state::read_run(id)?.gate.is_some();
    transaction(id, |path, state| {
        state.block(crate::state::BlockedCause::Budget, Some(why.to_string()));
        crate::event::record(path, &Event::BudgetExhausted { reason: why.to_string() })
    })?;
    if !already_gated {
        let question = format!(
            "Run `{id}` has stopped because its {why}.\n\nNothing is wrong with the work; it has \
             reached a ceiling set when the run was created.\n\n## Options\n\n\
             - **Raise the budget** — `otto state get {id} --field budget` shows the current one.\n\
             - **Stop the run** — `otto stop {id}`.\n\nDefault: stop. No further wakes will run."
        );
        crate::state::ops::open_gate(id, "budget", &question, None)?;
    }
    Err(OttoError::conflict(format!("{id}: {why}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::fake::FakeExec;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::{test_init, HANDOFF_FILE};
    use crate::state::read_run;
    use serde_json::json;

    /// Stand in for the session transcript claude leaves behind, written *during* the spawn.
    ///
    /// The session id is read back out of `run.json` rather than hardcoded, because that is the
    /// real mechanism: otto records the id before spawning and finds the transcript by it
    /// afterwards. A test that invented its own id would exercise neither half.
    fn leave_transcript(id: &str, turns: u64, input: u64, output: u64, tool_errors: u64) {
        let state = read_run(id).unwrap();
        let session = state
            .wake
            .as_ref()
            .and_then(|w| w.session.clone())
            .expect("the session id is recorded before the spawn");
        let dir = crate::paths::claude_projects_dir().join("-w-test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut lines: Vec<String> = (0..turns)
            .map(|_| {
                json!({"type": "assistant", "message": {"usage": {
                    "input_tokens": input, "output_tokens": output,
                    "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0,
                }}})
                .to_string()
            })
            .collect();
        for _ in 0..tool_errors {
            lines.push(
                json!({"type": "user", "message": {"content": [
                    {"type": "tool_result", "tool_use_id": "t", "is_error": true, "content": "denied"}
                ]}})
                .to_string(),
            );
        }
        std::fs::write(dir.join(format!("{session}.jsonl")), lines.join("\n")).unwrap();
    }

    /// Stand in for what a well-behaved wake does to the run directory: leave a legal stopping
    /// state and a fresh handoff. Wired through `FakeExec::on_exec` so it happens *during* the
    /// spawn, as it would for real — the executor clears a fired timer at wake start, so a
    /// test that wrote this beforehand would have it overwritten and misread the result.
    fn behave_well(id: &str) {
        let path = crate::paths::run_dir(id).unwrap();
        std::fs::write(path.join(HANDOFF_FILE), "# handoff\n## Next wake must\nCarry on.\n").unwrap();
        transaction(id, |_p, state| {
            state.status = Status::Sleeping;
            state.next_wake_at = Some(Timestamp::in_minutes(30));
            Ok(())
        })
        .unwrap();
    }

    fn args(id: &str) -> WakeArgs {
        WakeArgs {
            id: id.to_string(),
            answer: None,
            dry_run: false,
            watch: false,
            detach: None,
            foreground: true,
        }
    }

    #[test]
    fn a_well_behaved_wake_is_complete_and_records_what_it_used() {
        let _h = TempHome::new();
        test_init("w-ok", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| {
            behave_well("w-ok");
            leave_transcript("w-ok", 7, 100, 10, 0);
        });
        run_wake(&args("w-ok"), &mut exec).unwrap();

        let state = read_run("w-ok").unwrap();
        assert_eq!(state.wake.as_ref().unwrap().outcome, Some(WakeOutcome::Complete));
        assert_eq!(state.wake.as_ref().unwrap().n, 1);
        assert_eq!(state.budget.spent_wakes, 1);
        assert_eq!(state.incomplete_wakes, 0);
        // What replaced `total_cost_usd`: real tokens, read back from the transcript by the
        // session id otto chose. Dollars are gone on purpose — see `Budget`.
        let journal = std::fs::read_to_string(crate::paths::run_dir("w-ok").unwrap().join("journal.jsonl")).unwrap();
        let spent = journal
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find(|l| l.get("event").and_then(Value::as_str) == Some("wake-spent"))
            .expect("a wake must journal what it used");
        assert_eq!(spent["turns"], 7);
        assert_eq!(spent["inputTokens"], 700);
        assert_eq!(spent["outputTokens"], 70);
        assert!(spent.get("costUsd").is_none(), "a cost otto cannot measure must not be reported");
        assert!(!journal.contains("usage-unavailable"), "the transcript was right there");
        assert_eq!(state.budget.spent_usd, 0.0, "dollars are never accumulated");
    }

    /// A failed wake's narration is the only clue to why, and there is no session left to read.
    #[test]
    fn a_failed_wake_keeps_what_it_said() {
        let _h = TempHome::new();
        test_init("w-dump", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.queue(Output {
            code: 1,
            stdout: "I could not find the repo".into(),
            stderr: "boom".into(),
            timed_out: false,
        });
        run_wake(&args("w-dump"), &mut exec).unwrap();
        let dump = std::fs::read_to_string(crate::paths::run_dir("w-dump").unwrap().join("wake-1-output.txt"))
            .expect("a failed wake must leave its output behind");
        assert!(dump.contains("I could not find the repo"));
        assert!(dump.contains("boom"));
        assert!(dump.contains("failed the contract"));
    }

    /// ...and a wake that succeeded leaves none, so the run directory does not fill with output
    /// dumps nobody will read. (Distinct from the *session* transcript, which is claude's and
    /// lives under `~/.claude/projects` — see `transcript`.)
    #[test]
    fn a_successful_wake_keeps_no_output_dump() {
        let _h = TempHome::new();
        test_init("w-quiet", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-quiet"));
        run_wake(&args("w-quiet"), &mut exec).unwrap();
        assert!(!crate::paths::run_dir("w-quiet").unwrap().join("wake-1-output.txt").exists());
    }

    /// The stranded run v1 could not detect. Here it is a retry.
    #[test]
    fn a_wake_that_leaves_the_run_running_is_incomplete_and_backs_off() {
        let _h = TempHome::new();
        test_init("w-strand", "a goal").unwrap();
        age_handoff("w-strand");
        let mut exec = FakeExec::new();
        exec.on_exec(|| leave_transcript("w-strand", 3, 50, 5, 0));
        run_wake(&args("w-strand"), &mut exec).unwrap();

        let state = read_run("w-strand").unwrap();
        assert_eq!(state.incomplete_wakes, 1);
        assert_eq!(state.wake.as_ref().unwrap().outcome, Some(WakeOutcome::Incomplete));
        assert_eq!(state.status, Status::Sleeping, "a failed wake must still be revivable");
        assert!(state.next_wake_at.is_some(), "backoff must arm a retry");
        // It still consumed a wake and real tokens, and both must be recorded.
        assert_eq!(state.budget.spent_wakes, 1);
        let journal = std::fs::read_to_string(crate::paths::run_dir("w-strand").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("\"inputTokens\":150"), "usage is accounted even when the contract fails");
    }

    /// What claude leaves when the account's usage limit stops a wake: one assistant turn of real
    /// work, then the refusal, with the reset as epoch seconds.
    fn leave_limit_transcript(id: &str, resets_at: Timestamp) {
        let state = read_run(id).unwrap();
        let session = state.wake.as_ref().and_then(|w| w.session.clone()).expect("session recorded before the spawn");
        let dir = crate::paths::claude_projects_dir().join("-w-test");
        std::fs::create_dir_all(&dir).unwrap();
        let lines = [
            json!({"type": "assistant", "message": {"usage": {"input_tokens": 10, "output_tokens": 500}}}).to_string(),
            json!({
                "type": "assistant", "isApiErrorMessage": true, "error": "rate_limit", "apiError": "usage_limit_reached",
                "apiErrorParams": {"rate_limit_info": {"resetsAt": resets_at.dt().unix_timestamp(), "rateLimitType": "five_hour"}},
                "message": {"content": [{"type": "text", "text": "You've hit your session limit · resets 6:40pm"}]},
            })
            .to_string(),
        ];
        std::fs::write(dir.join(format!("{session}.jsonl")), lines.join("\n")).unwrap();
    }

    fn limited_exit() -> Output {
        Output { code: 1, stdout: "You've hit your session limit · resets 6:40pm".into(), stderr: String::new(), timed_out: false }
    }

    /// The live `dev-flow` run that hit the five-hour limit spent its five retries in an hour and was
    /// blocked eighty minutes before the limit lifted. A limit is waited out, not retried.
    #[test]
    fn a_wake_stopped_by_the_usage_limit_sleeps_until_it_resets_and_is_not_counted() {
        let _h = TempHome::new();
        test_init("w-limit", "a goal").unwrap();
        age_handoff("w-limit");
        let resets_at = Timestamp::in_minutes(90);
        let mut exec = FakeExec::new();
        exec.queue(limited_exit());
        exec.on_exec(move || leave_limit_transcript("w-limit", resets_at));
        run_wake(&args("w-limit"), &mut exec).unwrap();

        let state = read_run("w-limit").unwrap();
        assert_eq!(state.incomplete_wakes, 0, "the limit is not the run's failure");
        assert_eq!(state.wake.as_ref().unwrap().outcome, Some(WakeOutcome::Incomplete), "but the wake did not finish");
        assert_eq!(state.status, Status::Sleeping);
        assert_eq!(state.next_wake_at.unwrap().dt(), resets_at.dt() + time::Duration::minutes(1), "a minute past the reset");
        assert!(state.usage_limited());
        assert_eq!(crate::core::blocking(&state, false), "usage limit");
        let journal = std::fs::read_to_string(crate::paths::run_dir("w-limit").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains(r#""event":"usage-limited","limit":"five_hour""#), "got: {journal}");
        assert!(!journal.contains("wake-incomplete"));
        assert!(journal.contains(r#""outputTokens":500"#), "the work it did before the limit is still accounted");
    }

    #[test]
    fn hitting_the_limit_over_and_over_never_blocks_the_run() {
        let _h = TempHome::new();
        test_init("w-limits", "a goal").unwrap();
        age_handoff("w-limits");
        for _ in 0..6 {
            let mut exec = FakeExec::new();
            exec.queue(limited_exit());
            exec.on_exec(|| leave_limit_transcript("w-limits", Timestamp::in_minutes(30)));
            run_wake(&args("w-limits"), &mut exec).unwrap();
        }
        let state = read_run("w-limits").unwrap();
        assert_eq!(state.status, Status::Sleeping, "six limited wakes, still waiting rather than blocked");
        assert_eq!(state.incomplete_wakes, 0);
        assert!(state.gate.is_none());
    }

    /// A reset already behind it means the limit has lifted, or is about to: look again soon.
    #[test]
    fn a_reset_already_past_is_retried_in_minutes() {
        let _h = TempHome::new();
        test_init("w-lifted", "a goal").unwrap();
        age_handoff("w-lifted");
        let mut exec = FakeExec::new();
        exec.queue(limited_exit());
        exec.on_exec(|| leave_limit_transcript("w-lifted", Timestamp::in_minutes(-2)));
        run_wake(&args("w-lifted"), &mut exec).unwrap();
        let state = read_run("w-lifted").unwrap();
        assert!((4..=5).contains(&minutes_until(state.next_wake_at.unwrap())));
        assert_eq!(state.incomplete_wakes, 0);
    }

    /// An ordinary failure is still an ordinary failure: a non-zero exit with no limit in the
    /// transcript takes the backoff and counts.
    #[test]
    fn a_failed_wake_without_a_limit_still_counts() {
        let _h = TempHome::new();
        test_init("w-plain", "a goal").unwrap();
        age_handoff("w-plain");
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 1, stdout: "boom".into(), stderr: String::new(), timed_out: false });
        exec.on_exec(|| leave_transcript("w-plain", 1, 10, 5, 0));
        run_wake(&args("w-plain"), &mut exec).unwrap();
        let state = read_run("w-plain").unwrap();
        assert_eq!(state.incomplete_wakes, 1);
        assert!(!state.usage_limited());
    }

    /// `init` writes a handoff moments before a test's wake starts, which the freshness
    /// tolerance would count as this wake's. Backdate it so a wake that writes nothing is seen
    /// to have written nothing.
    fn age_handoff(id: &str) {
        let path = crate::paths::run_dir(id).unwrap().join(HANDOFF_FILE);
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600)).unwrap();
    }

    fn write_handoff_only(id: &str) {
        let path = crate::paths::run_dir(id).unwrap();
        std::fs::write(path.join(HANDOFF_FILE), "# handoff\n## Next wake must\nCarry on.\n").unwrap();
    }

    fn minutes_until(at: Timestamp) -> i64 {
        (at.dt() - crate::clock::now()).whole_minutes()
    }

    /// What two live runs' wakes did five times in a row each while the login had lapsed: one turn,
    /// no tokens, exit 1, and one line on stdout.
    fn couldnt_authenticate(id: &'static str, exec: &mut FakeExec) {
        exec.queue(Output {
            code: 1,
            stdout: "Failed to authenticate: OAuth session expired and could not be refreshed\n".into(),
            stderr: String::new(),
            timed_out: false,
        });
        exec.on_exec(move || leave_transcript(id, 1, 0, 0, 0));
    }

    #[test]
    fn a_wake_that_never_got_going_retries_without_ever_blocking() {
        let _h = TempHome::new();
        test_init("w-auth", "a goal").unwrap();
        age_handoff("w-auth");
        for n in 1..=7u32 {
            let mut exec = FakeExec::new();
            couldnt_authenticate("w-auth", &mut exec);
            run_wake(&args("w-auth"), &mut exec).unwrap();
            let state = read_run("w-auth").unwrap();
            assert_eq!(state.start_failures, n);
            assert_eq!(state.incomplete_wakes, 0, "nothing about the run failed");
            assert_eq!(state.status, Status::Sleeping, "never blocked, never a question: attempt {n}");
            assert!(state.gate.is_none());
            assert!(minutes_until(state.next_wake_at.unwrap()) <= crate::poke::backoff_minutes(n));
        }
        let state = read_run("w-auth").unwrap();
        assert_eq!(
            state.start_failure.as_deref(),
            Some("Failed to authenticate: OAuth session expired and could not be refreshed"),
            "what claude said is what tells a person what to fix"
        );
        assert!(!state.check_gates_wake(), "the retry is a real wake, not a check's opinion");
        let glance = crate::story::glance(&state, false, None, None);
        assert!(glance.line.contains("Log in again"), "{}", glance.line);
        let journal = std::fs::read_to_string(crate::paths::run_dir("w-auth").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains(r#""event":"start-failed""#));
        assert!(!journal.contains("wake-incomplete"));

        // The login is fixed: the next wake gets going, and the streak is over.
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-auth"));
        run_wake(&args("w-auth"), &mut exec).unwrap();
        let state = read_run("w-auth").unwrap();
        assert_eq!((state.start_failures, state.start_failure.as_deref()), (0, None));
    }

    /// A wake that wrote its handoff got going, whatever else happened to it.
    #[test]
    fn a_wake_that_wrote_its_handoff_and_then_crashed_is_a_failure_not_a_non_start() {
        let _h = TempHome::new();
        test_init("w-wrote", "a goal").unwrap();
        age_handoff("w-wrote");
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 1, stdout: "Failed to authenticate".into(), stderr: String::new(), timed_out: false });
        exec.on_exec(|| {
            write_handoff_only("w-wrote");
            leave_transcript("w-wrote", 1, 0, 0, 0);
        });
        run_wake(&args("w-wrote"), &mut exec).unwrap();
        let state = read_run("w-wrote").unwrap();
        assert_eq!((state.incomplete_wakes, state.start_failures), (1, 0));
    }

    /// A pause whose time has passed is cleared by the next wake; one still holding stays, since a
    /// wake a person starts by hand does not lift it.
    #[test]
    fn a_wake_clears_a_pause_that_has_run_out_and_keeps_one_that_has_not() {
        let _h = TempHome::new();
        test_init("w-pause", "a goal").unwrap();
        crate::core::pause_run("w-pause", Some(Timestamp::in_minutes(60)), None).unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-pause"));
        run_wake(&args("w-pause"), &mut exec).unwrap();
        assert!(read_run("w-pause").unwrap().is_paused(), "a wake by hand leaves the pause in place");

        transaction("w-pause", |_p, s| {
            s.paused.as_mut().unwrap().until = Some(Timestamp::in_minutes(-1));
            Ok(())
        })
        .unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-pause"));
        run_wake(&args("w-pause"), &mut exec).unwrap();
        assert!(read_run("w-pause").unwrap().paused.is_none());
        let journal = std::fs::read_to_string(crate::paths::run_dir("w-pause").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains(r#""event":"unpaused","by":"time""#), "got: {journal}");
    }

    /// The period: a wake that did its work and said nothing about when to come back sleeps
    /// for the run's own period, measured from when it started.
    #[test]
    fn a_clean_wake_that_arms_nothing_sleeps_for_the_period() {
        let _h = TempHome::new();
        test_init("w-period", "a goal").unwrap();
        transaction("w-period", |_p, state| {
            state.policy.period_minutes = 90;
            Ok(())
        })
        .unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| write_handoff_only("w-period"));
        run_wake(&args("w-period"), &mut exec).unwrap();

        let state = read_run("w-period").unwrap();
        assert_eq!(state.wake.as_ref().unwrap().outcome, Some(WakeOutcome::Complete));
        assert_eq!(state.status, Status::Sleeping);
        let expected = state.wake.as_ref().unwrap().started_at.dt() + time::Duration::minutes(90);
        assert_eq!(state.next_wake_at.unwrap().dt(), expected, "anchored on the wake's start");
    }

    /// `arm-timer --in` is a one-off override: the wake's own choice stands for this sleep.
    #[test]
    fn a_wake_that_armed_its_own_timer_keeps_it() {
        let _h = TempHome::new();
        test_init("w-override", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-override"));
        run_wake(&args("w-override"), &mut exec).unwrap();

        let state = read_run("w-override").unwrap();
        let minutes = minutes_until(state.next_wake_at.unwrap());
        assert!((28..=30).contains(&minutes), "the wake asked for 30m, not the 60m period; got {minutes}m");
    }

    /// A crash is not a clean finish: it takes the short backoff, never a whole period, and it
    /// still counts as incomplete.
    #[test]
    fn a_crashed_wake_backs_off_instead_of_sleeping_for_the_period() {
        let _h = TempHome::new();
        test_init("w-crash", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| write_handoff_only("w-crash"));
        exec.queue(Output { code: 1, stdout: String::new(), stderr: "boom".into(), timed_out: false });
        run_wake(&args("w-crash"), &mut exec).unwrap();

        let state = read_run("w-crash").unwrap();
        assert_eq!(state.incomplete_wakes, 1);
        let minutes = minutes_until(state.next_wake_at.unwrap());
        assert!(minutes <= crate::poke::backoff_minutes(1), "got {minutes}m");
    }

    /// A gate takes precedence over the period; the period resumes after it is answered.
    #[test]
    fn a_wake_that_opens_a_gate_does_not_also_sleep() {
        let _h = TempHome::new();
        test_init("w-gated", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| {
            write_handoff_only("w-gated");
            crate::state::ops::open_gate("w-gated", "approve", "Ship it?", None).unwrap();
        });
        run_wake(&args("w-gated"), &mut exec).unwrap();

        let state = read_run("w-gated").unwrap();
        assert_eq!(state.status, Status::AwaitingHuman);
        assert!(state.next_wake_at.is_none(), "a gate waits on a person, not a clock");
    }

    /// The signal poke's backoff depends on. A wake starting is the only proof its spawn worked,
    /// so it clears the counter; otherwise poke defers a healthy run for five minutes after every
    /// crash, which is precisely what it must not do.
    #[test]
    fn starting_a_wake_clears_pokes_spawn_attempt_counter() {
        let _h = TempHome::new();
        test_init("w-attempts", "a goal").unwrap();
        crate::state::commands::record_spawn("w-attempts", 3, true).unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-attempts"));
        run_wake(&args("w-attempts"), &mut exec).unwrap();
        assert_eq!(read_run("w-attempts").unwrap().spawn_attempts, 0);
    }

    #[test]
    fn a_killed_wake_says_it_was_killed() {
        let _h = TempHome::new();
        test_init("w-killed", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.queue(Output {
            code: 124,
            stdout: String::new(),
            stderr: "…".into(),
            timed_out: true,
        });
        run_wake(&args("w-killed"), &mut exec).unwrap();
        let journal = std::fs::read_to_string(crate::paths::run_dir("w-killed").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("wake-incomplete"));
        assert!(journal.contains("killed at its deadline"), "got: {journal}");
    }

    #[test]
    fn repeated_failures_stop_retrying_and_open_a_gate() {
        let _h = TempHome::new();
        test_init("w-stuck", "a goal").unwrap();
        age_handoff("w-stuck");
        transaction("w-stuck", |_p, state| {
            state.policy.max_incomplete_wakes = 2;
            Ok(())
        })
        .unwrap();
        for _ in 0..2 {
            let mut exec = FakeExec::new();
            // Clear the sleep the previous failure armed, so the next wake is a fresh attempt.
            transaction("w-stuck", |_p, state| {
                state.status = Status::Running;
                state.next_wake_at = None;
                Ok(())
            })
            .unwrap();
            run_wake(&args("w-stuck"), &mut exec).unwrap();
        }
        let state = read_run("w-stuck").unwrap();
        assert_eq!(state.status, Status::Blocked);
        assert!(state.gate.is_some(), "blocked without a gate would strand the run");
        assert_eq!(state.gate.as_ref().unwrap().slug, "keeps-stopping");
        let blocked = state.blocked.as_ref().expect("a blocked run says why");
        assert_eq!(blocked.cause, crate::state::BlockedCause::WakeFailures);
        assert!(blocked.detail.as_deref().unwrap_or("").contains("nothing would ever bring this run back"));
    }

    #[test]
    fn a_terminal_run_is_left_alone() {
        let _h = TempHome::new();
        test_init("w-done", "a goal").unwrap();
        transaction("w-done", |_p, state| {
            state.status = Status::Done;
            Ok(())
        })
        .unwrap();
        let mut exec = FakeExec::new();
        run_wake(&args("w-done"), &mut exec).unwrap();
        assert!(exec.calls.borrow().is_empty(), "a finished run must not spawn anything");
    }

    #[test]
    fn a_spent_budget_blocks_with_a_gate_instead_of_launching() {
        let _h = TempHome::new();
        test_init("w-broke", "a goal").unwrap();
        transaction("w-broke", |_p, state| {
            state.budget.wakes = 2;
            state.budget.spent_wakes = 2;
            Ok(())
        })
        .unwrap();
        let mut exec = FakeExec::new();
        let err = run_wake(&args("w-broke"), &mut exec).expect_err("must refuse");
        assert_eq!(err.code, 2);
        assert!(exec.calls.borrow().is_empty(), "nothing may be spawned once the budget is gone");
        let state = read_run("w-broke").unwrap();
        assert_eq!(state.status, Status::Blocked);
        assert!(state.gate.is_some());
        let blocked = state.blocked.as_ref().expect("a blocked run says why");
        assert_eq!(blocked.cause, crate::state::BlockedCause::Budget);
        assert_eq!(blocked.detail.as_deref(), Some("wake budget spent: 2 of 2"));
    }

    fn budget_warnings(id: &str) -> usize {
        let journal = std::fs::read_to_string(crate::paths::run_dir(id).unwrap().join("journal.jsonl")).unwrap();
        journal.matches("\"event\":\"budget-warning\"").count()
    }

    #[test]
    fn the_budget_warning_survives_a_wake_that_rewrites_its_facts() {
        let _h = TempHome::new();
        test_init("w-warn", "a goal").unwrap();
        transaction("w-warn", |_p, state| {
            state.budget.wakes = 10;
            state.budget.spent_wakes = 7;
            Ok(())
        })
        .unwrap();

        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-warn"));
        run_wake(&args("w-warn"), &mut exec).unwrap();
        assert_eq!(budget_warnings("w-warn"), 1, "8 of 10 is past 80%");
        assert!(read_run("w-warn").unwrap().budget_warned_at.is_some());

        // A wake that replaces its facts wholesale must not earn the run a second warning.
        let mut exec = FakeExec::new();
        exec.on_exec(|| {
            behave_well("w-warn");
            transaction("w-warn", |_p, state| {
                state.facts = serde_json::Map::new();
                Ok(())
            })
            .unwrap();
        });
        run_wake(&args("w-warn"), &mut exec).unwrap();
        assert_eq!(budget_warnings("w-warn"), 1);
    }

    #[test]
    fn a_run_warned_under_the_old_facts_key_is_not_warned_again() {
        let _h = TempHome::new();
        test_init("w-legacy", "a goal").unwrap();
        transaction("w-legacy", |_p, state| {
            state.budget.wakes = 10;
            state.budget.spent_wakes = 8;
            state.facts.insert("budgetWarnedAt".to_string(), Value::String("2026-09-01T00:00:00Z".to_string()));
            Ok(())
        })
        .unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-legacy"));
        run_wake(&args("w-legacy"), &mut exec).unwrap();
        assert_eq!(budget_warnings("w-legacy"), 0);
        let state = read_run("w-legacy").unwrap();
        assert_eq!(state.budget_warned_at.unwrap().to_string(), "2026-09-01T00:00:00Z");
        assert!(state.facts.get("budgetWarnedAt").is_none(), "the old key is moved, not copied");
    }

    #[test]
    fn a_second_wake_refuses_while_one_is_running() {
        let _h = TempHome::new();
        test_init("w-solo", "a goal").unwrap();
        let _held = WakeLock::acquire("w-solo").unwrap();
        let mut exec = FakeExec::new();
        let err = run_wake(&args("w-solo"), &mut exec).expect_err("must refuse");
        assert_eq!(err.code, 2);
        assert!(exec.calls.borrow().is_empty());
    }

    #[test]
    fn the_timer_that_fired_is_cleared_so_the_run_is_not_double_woken() {
        let _h = TempHome::new();
        test_init("w-timer", "a goal").unwrap();
        let stale = Timestamp::parse("2020-01-01T00:00:00Z").unwrap();
        transaction("w-timer", |_p, state| {
            state.status = Status::Sleeping;
            state.next_wake_at = Some(stale);
            Ok(())
        })
        .unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-timer"));
        run_wake(&args("w-timer"), &mut exec).unwrap();
        // behave_well armed a fresh future wake; the stale one must not have survived.
        let state = read_run("w-timer").unwrap();
        assert_ne!(state.next_wake_at, Some(stale));
    }

    /// What is left of the old `permission-denied` line. The result document used to name the tool
    /// a blocked wake wanted; nothing in a transcript says "denied" in a stable form, so otto
    /// counts failed tool calls instead and does not guess at the reason. A wake failing every
    /// tool call still has to be visible from the journal alone.
    #[test]
    fn failed_tool_calls_are_counted_so_a_blocked_posture_is_still_visible() {
        let _h = TempHome::new();
        test_init("w-denied", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| {
            behave_well("w-denied");
            leave_transcript("w-denied", 2, 10, 1, 3);
        });
        run_wake(&args("w-denied"), &mut exec).unwrap();
        let journal = std::fs::read_to_string(crate::paths::run_dir("w-denied").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("\"toolErrors\":3"), "three failed tool calls must show up");
    }

    /// A wake whose transcript cannot be read at all. Expected under a sandbox that
    /// cannot see `~/.claude` — so blind accounting must be an ordinary outcome that still counts
    /// the wake, still validates the contract, and says out loud that the tokens are unknown.
    /// Accounting is never allowed to be what fails a wake.
    #[test]
    fn a_wake_with_no_readable_transcript_still_accounts_and_validates() {
        let _h = TempHome::new();
        test_init("w-blind", "a goal").unwrap();
        let mut exec = FakeExec::new();
        // It got going — it rewrote the handoff — so this is a failed wake, not one that never started.
        exec.on_exec(|| write_handoff_only("w-blind"));
        exec.queue(Output {
            code: 1,
            stdout: "some narration".into(),
            stderr: "boom".into(),
            timed_out: false,
        });
        run_wake(&args("w-blind"), &mut exec).unwrap();
        let state = read_run("w-blind").unwrap();
        assert_eq!(state.budget.spent_wakes, 1, "a wake that ran counts even if it said nothing useful");
        assert_eq!(state.incomplete_wakes, 1);
        let journal = std::fs::read_to_string(crate::paths::run_dir("w-blind").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("usage-unavailable"), "blind accounting must not be silent");
        assert!(journal.contains("wake-reported-error"), "a non-zero exit is the only thing the child said");
    }

    /// The session id has to be on the command line *and* in the run directory, or the transcript
    /// written by one cannot be found by the other.
    #[test]
    fn the_session_id_is_both_passed_to_claude_and_recorded() {
        let _h = TempHome::new();
        test_init("w-session", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-session"));
        run_wake(&args("w-session"), &mut exec).unwrap();

        let recorded = read_run("w-session").unwrap().wake.as_ref().unwrap().session.clone().unwrap();
        let argv = exec.last_call();
        let idx = argv.iter().position(|a| a == "--session-id").expect("claude must be told the id");
        assert_eq!(argv[idx + 1], recorded, "the id claude used is the id otto reads back");
        assert!(!argv.iter().any(|a| a == "-p"), "print mode bills the wrong budget");
    }

    #[test]
    fn dry_run_spawns_nothing_and_writes_nothing() {
        let _h = TempHome::new();
        test_init("w-dry", "a goal").unwrap();
        let before = std::fs::read_to_string(crate::paths::run_dir("w-dry").unwrap().join("run.json")).unwrap();
        let mut exec = FakeExec::new();
        run_wake(
            &WakeArgs {
                id: "w-dry".into(),
                answer: None,
                dry_run: true,
                watch: false,
                detach: None,
                foreground: true,
            },
            &mut exec,
        )
        .unwrap();
        assert!(exec.calls.borrow().is_empty());
        let after = std::fs::read_to_string(crate::paths::run_dir("w-dry").unwrap().join("run.json")).unwrap();
        assert_eq!(before, after, "a dry run must not touch state");
    }

    #[test]
    fn an_answer_reaches_the_wake_verbatim() {
        let _h = TempHome::new();
        test_init("w-answer", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-answer"));
        run_wake(
            &WakeArgs {
                id: "w-answer".into(),
                answer: Some("Approve, but rename the flag first.".into()),
                dry_run: false,
                watch: false,
                detach: None,
                foreground: true,
            },
            &mut exec,
        )
        .unwrap();
        let call = exec.last_call().join(" ");
        assert!(call.contains("Approve, but rename the flag first."));
    }

    /// A scripted run of wakes: the `n`th spawn writes a handoff and then does `step(n)`. Comes with
    /// a count of the wakes that ran.
    fn script(id: &'static str, step: impl Fn(u32) + 'static) -> (FakeExec, std::rc::Rc<std::cell::Cell<u32>>) {
        let count = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let seen = count.clone();
        let mut exec = FakeExec::new();
        exec.on_exec(move || {
            seen.set(seen.get() + 1);
            let path = crate::paths::run_dir(id).unwrap();
            std::fs::write(path.join(HANDOFF_FILE), "# handoff\n## Next wake must\nCarry on.\n").unwrap();
            step(seen.get());
        });
        (exec, count)
    }

    fn claim_done(id: &str) {
        crate::state::commands::set_status(crate::state::commands::SetStatusArgs {
            id: id.into(),
            status: Status::Done,
            reason: Some("all of it".into()),
            because: None,
        })
        .unwrap();
    }

    fn verdict(id: &str, confirm: bool, reason: &str) {
        crate::state::commands::verify(crate::state::commands::VerifyArgs {
            id: id.into(),
            confirm,
            reopen: !confirm,
            reason: reason.into(),
        })
        .unwrap();
    }

    /// A claim is checked by the next wake straight away, in the same place — not up to five
    /// minutes later when poke notices — and that wake is told it is the checker.
    #[test]
    fn a_done_claim_is_checked_by_the_next_wake_straight_away() {
        let _h = TempHome::new();
        test_init("w-claim", "a goal").unwrap();
        let (mut exec, count) = script("w-claim", |n| match n {
            1 => claim_done("w-claim"),
            _ => verdict("w-claim", true, "checked the file is there"),
        });
        run_wake(&args("w-claim"), &mut exec).unwrap();

        assert_eq!(count.get(), 2, "the claim, then its check");
        let state = read_run("w-claim").unwrap();
        assert_eq!(state.status, Status::Done);
        assert_eq!(state.wake.unwrap().n, 2);
        let calls = exec.calls.borrow();
        assert!(!calls[0].join(" ").contains("to check a claim"));
        assert!(calls[1].join(" ").contains("to check a claim"), "the checker is told what it is");
    }

    /// Reopened, the fix comes next — told what was missing — and its claim is checked again.
    #[test]
    fn a_reopened_claim_goes_back_to_work_and_is_checked_again() {
        let _h = TempHome::new();
        test_init("w-again", "a goal").unwrap();
        let (mut exec, count) = script("w-again", |n| match n {
            1 | 3 => claim_done("w-again"),
            2 => verdict("w-again", false, "the worktree is still there"),
            _ => verdict("w-again", true, "it's gone now"),
        });
        run_wake(&args("w-again"), &mut exec).unwrap();

        assert_eq!(count.get(), 4);
        assert_eq!(read_run("w-again").unwrap().status, Status::Done);
        let calls = exec.calls.borrow();
        assert!(calls[2].join(" ").contains("the worktree is still there"), "the fix is told what was missing");
    }

    /// A note a person leaves while a wake works was never in its prompt. Its done is refused once,
    /// with the note, so the wake doing the work reads it — not the checker, which does no work
    /// and would deliver it unread.
    #[test]
    fn a_note_left_mid_wake_reaches_it_before_its_done() {
        let _h = TempHome::new();
        test_init("w-late", "a goal").unwrap();
        let refused = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let said = refused.clone();
        let (mut exec, count) = script("w-late", move |n| match n {
            1 => {
                crate::notes::add("w-late", "Use the staging db, not prod.", false).unwrap();
                let done = crate::state::commands::SetStatusArgs {
                    id: "w-late".into(),
                    status: Status::Done,
                    reason: Some("all of it".into()),
                    because: None,
                };
                *said.borrow_mut() = crate::state::commands::set_status(done).unwrap_err().message;
                claim_done("w-late");
            }
            _ => verdict("w-late", true, "checked"),
        });
        run_wake(&args("w-late"), &mut exec).unwrap();

        assert!(refused.borrow().contains("Use the staging db, not prod."), "{}", refused.borrow());
        assert_eq!(count.get(), 2);
        assert_eq!(read_run("w-late").unwrap().status, Status::Done);
        assert!(!exec.calls.borrow()[1].join(" ").contains("staging db"), "delivered by the wake that read it");
        let journal = std::fs::read_to_string(crate::paths::run_dir("w-late").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("\"event\":\"notes-delivered\",\"notes\":[\"001\"],\"wake\":1"), "{journal}");
    }

    /// A checker that ends without a verdict has not done its one job: incomplete, and nothing more
    /// is started on top of it.
    #[test]
    fn a_checker_that_never_decides_is_an_incomplete_wake() {
        let _h = TempHome::new();
        test_init("w-undecided", "a goal").unwrap();
        let (mut exec, count) = script("w-undecided", |n| {
            if n == 1 {
                claim_done("w-undecided");
            }
        });
        run_wake(&args("w-undecided"), &mut exec).unwrap();

        assert_eq!(count.get(), 2);
        let state = read_run("w-undecided").unwrap();
        assert_eq!(state.incomplete_wakes, 1);
        assert!(state.done_claim.is_some(), "the claim still waits, for the retry");
        assert_eq!(state.wake.unwrap().outcome, Some(WakeOutcome::Incomplete));
    }

    /// With checking off there is nothing to follow on: one wake, and the run is done.
    #[test]
    fn with_checking_off_a_done_wake_is_the_last() {
        let _h = TempHome::new();
        test_init("w-trust", "a goal").unwrap();
        transaction("w-trust", |_p, state| {
            state.policy.verify_done = false;
            Ok(())
        })
        .unwrap();
        let (mut exec, count) = script("w-trust", |_| claim_done("w-trust"));
        run_wake(&args("w-trust"), &mut exec).unwrap();
        assert_eq!(count.get(), 1);
        assert_eq!(read_run("w-trust").unwrap().status, Status::Done);
    }

    #[test]
    fn a_one_off_note_is_given_until_a_wake_that_carried_it_completes() {
        let _h = TempHome::new();
        test_init("w-note", "a goal").unwrap();
        crate::notes::add("w-note", "Skip the e2e suite, it's broken on main.", false).unwrap();
        crate::notes::add("w-note", "Never touch legacy/.", true).unwrap();

        // A wake that crashes carried the notes but did not complete, so nothing is delivered.
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 1, stdout: String::new(), stderr: "boom".into(), timed_out: false });
        run_wake(&args("w-note"), &mut exec).unwrap();
        let call = exec.last_call().join(" ");
        assert!(call.contains("Skip the e2e suite, it's broken on main."), "the note reaches the prompt verbatim");
        assert!(call.contains("Never touch legacy/."));
        let state = read_run("w-note").unwrap();
        assert_eq!(state.notes.len(), 2, "a failed wake must not swallow a note");
        assert_eq!(state.notes[0].given_to_wake, Some(1));

        // The next wake is given it again, completes, and so delivers it. The standing one stays.
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-note"));
        run_wake(&args("w-note"), &mut exec).unwrap();
        assert!(exec.last_call().join(" ").contains("Skip the e2e suite"));
        let left: Vec<String> = read_run("w-note").unwrap().notes.iter().map(|n| n.id.clone()).collect();
        assert_eq!(left, vec!["002"]);

        // And the wake after that carries only the standing note.
        transaction("w-note", |_p, s| {
            s.next_wake_at = None;
            Ok(())
        })
        .unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-note"));
        run_wake(&args("w-note"), &mut exec).unwrap();
        let call = exec.last_call().join(" ");
        assert!(!call.contains("Skip the e2e suite") && call.contains("Never touch legacy/."));
    }

    /// Wakes that meet the contract and change nothing otto can see are counted by otto — a plain
    /// tick doesn't save one — and at the limit the run stops and asks, rather than leaving it to a
    /// wake to notice. The first wake has nothing to compare with.
    #[test]
    fn wakes_that_change_nothing_stop_the_run_and_ask() {
        let _h = TempHome::new();
        test_init("w-stall", "a goal").unwrap();
        transaction("w-stall", |_p, s| {
            s.policy.max_ticks_without_progress = 3;
            Ok(())
        })
        .unwrap();
        for _ in 0..4 {
            let mut exec = FakeExec::new();
            exec.on_exec(|| {
                let look = crate::state::commands::TickArgs { id: "w-stall".into(), progress: false, note: Some("queue empty".into()) };
                crate::state::commands::tick(look).unwrap();
                behave_well("w-stall");
            });
            run_wake(&args("w-stall"), &mut exec).unwrap();
        }
        let state = read_run("w-stall").unwrap();
        assert_eq!(state.status, Status::Blocked);
        assert!(matches!(state.blocked.as_ref().map(|b| b.cause), Some(crate::state::BlockedCause::Stall)));
        let gate = state.gate.expect("otto asks");
        assert_eq!(gate.slug, "nothing-changing");
        let dir = crate::paths::run_dir("w-stall").unwrap();
        let question = std::fs::read_to_string(dir.join(&gate.file)).unwrap();
        assert!(question.contains("3 times in a row"), "{question}");
        let journal = std::fs::read_to_string(dir.join("journal.jsonl")).unwrap();
        assert_eq!(journal.matches("\"progress\":false").count(), 3, "{journal}");
    }

    /// Work otto can see keeps the run going, and so does a wake's word for work it can't.
    #[test]
    fn work_or_a_wake_s_word_for_it_keeps_the_run_going() {
        let _h = TempHome::new();
        test_init("w-busy", "a goal").unwrap();
        transaction("w-busy", |_p, s| {
            s.policy.max_ticks_without_progress = 2;
            Ok(())
        })
        .unwrap();
        for n in 1..=6u32 {
            let mut exec = FakeExec::new();
            exec.on_exec(move || {
                if n % 3 == 0 {
                    let dir = crate::paths::run_dir("w-busy").unwrap();
                    std::fs::create_dir_all(dir.join("artifacts")).unwrap();
                    std::fs::write(dir.join("artifacts/seen.md"), format!("look {n}\n")).unwrap();
                } else {
                    let said = crate::state::commands::TickArgs { id: "w-busy".into(), progress: true, note: Some("replied on the PR".into()) };
                    crate::state::commands::tick(said).unwrap();
                }
                behave_well("w-busy");
            });
            run_wake(&args("w-busy"), &mut exec).unwrap();
        }
        let state = read_run("w-busy").unwrap();
        assert_eq!((state.status, state.ticks_without_progress), (Status::Sleeping, 0));
        assert!(state.gate.is_none());
    }

    /// The safety net counts "nothing new" results since the run was last actually looked at, so
    /// a real wake starts the count again.
    #[test]
    fn a_wake_restarts_the_safety_net_s_count() {
        let _h = TempHome::new();
        test_init("w-check", "a goal").unwrap();
        crate::core::set_check("w-check", "#!/bin/sh\nexit 0\n", 24).unwrap();
        for _ in 0..3 {
            crate::state::commands::record_check("w-check", crate::state::CheckResult::NoChange, None, true).unwrap();
        }
        assert_eq!(read_run("w-check").unwrap().check.unwrap().consecutive_no_change, 3);
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-check"));
        run_wake(&args("w-check"), &mut exec).unwrap();
        let check = read_run("w-check").unwrap().check.unwrap();
        assert_eq!(check.consecutive_no_change, 0);
        assert_eq!(check.no_change_total, 3, "the total is history, not a count to reset");
    }

    /// Every wake starts in the run's working directory, whoever started it — the reviver runs
    /// from `$OTTO_HOME`, and without this its wakes would start there instead.
    #[test]
    fn a_wake_runs_in_the_run_s_workdir() {
        let h = TempHome::new();
        test_init("w-dir", "a goal").unwrap();
        crate::state::transaction("w-dir", |_, s| {
            s.launcher.workdir = Some(h.path().display().to_string());
            Ok(())
        })
        .unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-dir"));
        run_wake(&args("w-dir"), &mut exec).unwrap();
        assert_eq!(exec.cwds.borrow().as_slice(), [Some(h.path().to_path_buf())]);
    }

    /// A standing check the run set itself stays with the run, like a person's.
    #[test]
    fn a_wake_keeps_a_standing_check_the_run_set() {
        let _h = TempHome::new();
        test_init("w-standing", "a goal").unwrap();
        let script = crate::paths::run_dir("w-standing").unwrap().join("artifacts/pr.sh");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        crate::state::commands::set_check(crate::state::commands::SetCheckArgs {
            id: "w-standing".into(),
            script: Some("artifacts/pr.sh".into()),
            wake_after: 24,
            off: false,
        })
        .unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-standing"));
        run_wake(&args("w-standing"), &mut exec).unwrap();
        assert!(read_run("w-standing").unwrap().check.is_some());
    }

    /// A wake's own check was for the sleep that just ended, so the wake it let through ends it.
    #[test]
    fn a_wake_ends_the_check_the_last_wake_armed() {
        let _h = TempHome::new();
        test_init("w-own", "a goal").unwrap();
        crate::state::commands::arm_timer(crate::state::commands::ArmTimerArgs {
            id: "w-own".into(),
            at: None,
            seconds: Some(600),
            status: Status::Sleeping,
            note: None,
            check_script: Some("artifacts/ci.sh".into()),
        })
        .unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-own"));
        run_wake(&args("w-own"), &mut exec).unwrap();
        assert!(read_run("w-own").unwrap().check.is_none());
    }

    #[test]
    fn a_wake_with_no_notes_gets_no_notes_section() {
        let _h = TempHome::new();
        test_init("w-quiet", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-quiet"));
        run_wake(&args("w-quiet"), &mut exec).unwrap();
        assert!(!exec.last_call().join(" ").contains("left notes"));
    }

    #[test]
    fn the_deadline_is_what_bounds_the_spawn() {
        let _h = TempHome::new();
        test_init("w-deadline", "a goal").unwrap();
        transaction("w-deadline", |_p, state| {
            state.policy.max_wake_minutes = 3;
            Ok(())
        })
        .unwrap();
        let mut exec = FakeExec::new();
        exec.on_exec(|| behave_well("w-deadline"));
        run_wake(&args("w-deadline"), &mut exec).unwrap();
        assert_eq!(exec.timeouts.borrow()[0], Duration::from_secs(180));
        let state = read_run("w-deadline").unwrap();
        assert!(state.wake.as_ref().unwrap().deadline_at > state.wake.as_ref().unwrap().started_at);
    }
}
