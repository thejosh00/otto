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

#[derive(clap::Args, Debug)]
pub struct WakeArgs {
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
/// under yolo the sandbox cannot see `~/.claude` at all.
#[derive(Debug, Default, Clone)]
pub struct WakeResult {
    pub usage: Option<transcript::Usage>,
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
fn elapsed_hours(state: &RunState) -> f64 {
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

pub fn wake(args: WakeArgs) -> Result<(), OttoError> {
    let mut exec = RealExec;
    run_wake(&args, &mut exec)
}

/// The body, with the spawner injected so the whole path is testable without spending money.
pub fn run_wake(args: &WakeArgs, exec: &mut dyn Exec) -> Result<(), OttoError> {
    let run_path = crate::paths::run_dir(&args.id)?;
    let state = crate::state::read_run(&args.id)?;

    // A late wake firing into a closed run is normal, and doing nothing is correct.
    if state.status.is_terminal() {
        println!("{} is {:?} — nothing to do", args.id, state.status);
        return Ok(());
    }
    launcher::check_resolvable(&state)?;
    if let Some(why) = budget_exceeded(&state) {
        return block_on_budget(&args.id, &why);
    }

    let prompt = match &args.answer {
        Some(answer) => prompt::answer_prompt(&args.id, &run_path, answer),
        None => prompt::user_prompt(&args.id, &run_path),
    };

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

    let argv = launcher::argv(&state, &prompt, &session);

    if args.dry_run {
        // Print the harness as a length rather than inline: it is several KB and would bury
        // the part a human is actually checking.
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
        println!("{}", shown.join(" "));
        return Ok(());
    }

    // Hold the wake lock for the whole wake. This is what liveness observes, and taking it
    // here means a second `otto wake` refuses rather than racing.
    let _wake_lock = WakeLock::acquire(&args.id)?;

    // Recorded before the spawn, so a crash in the next instant still leaves evidence.
    let id = args.id.clone();
    let launcher_kind = state.launcher.kind;
    transaction(&id, |path, state| {
        state.wake = Some(Wake {
            n: wake_number,
            started_at: started,
            deadline_at: deadline,
            launcher: launcher_kind,
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
        crate::event::record(
            path,
            &Event::WakeStarted {
                wake: wake_number,
                deadline_at: deadline,
                launcher: format!("{launcher_kind:?}").to_lowercase(),
            },
        )
    })?;

    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let output = exec.exec(&argv_refs, None, Duration::from_secs(deadline_minutes * 60));
    // The child has exited, so its transcript is finished being written.
    let result = WakeResult {
        usage: transcript::usage_for(&session),
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
            // nothing. Expected under yolo, where the sandbox cannot see `~/.claude`.
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
            let already = state.facts.get("budgetWarnedAt").and_then(Value::as_str).is_some();
            if fraction >= 0.8 && fraction < 1.0 && !already {
                state
                    .facts
                    .insert("budgetWarnedAt".to_string(), Value::String(crate::clock::now_iso()));
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
            transaction(id, |path, state| {
                if let Some(wake) = state.wake.as_mut() {
                    wake.outcome = Some(WakeOutcome::Complete);
                }
                state.incomplete_wakes = 0;
                crate::event::record(
                    path,
                    &Event::WakeComplete { status: crate::state::commands::status_str(state.status).to_string() },
                )
            })?;
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
            record_incomplete(id, why)?;
            println!("wake incomplete — {why}\n  what it said: {}", dump.display());
            Ok(())
        }
    }
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
        consecutive = state.incomplete_wakes;
        crate::event::record(path, &Event::WakeIncomplete { reason: why_owned.clone(), consecutive })?;
        if limit > 0 && consecutive >= limit {
            // Retrying forever is the expensive failure. Hand it to a person, in a state a
            // person can actually act on.
            gave_up = true;
            state.status = Status::Blocked;
            state.next_wake_at = None;
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
    let question = format!(
        "Run `{id}` has failed to finish a wake {consecutive} times in a row and has stopped \
         retrying.\n\nThe last failure was: {why}\n\nA wake \"fails\" when it exits without \
         leaving the run somewhere anything could resume it — no gate open, no wake time set — \
         or without rewriting `handoff.md`. That usually means the wrapped instructions are \
         asking for something a wake cannot do (waiting for an interactive answer, most often), \
         or the work genuinely cannot proceed.\n\n\
         - **Retry** — clear the counter and wake it again; right if you have fixed the cause.\n\
         - **Stop the run** — retire it (`otto stop {id}`).\n\
         - **Investigate** — read `otto logs {id}` and the journal first; the run stays blocked \
         meanwhile.\n\nDefault: investigate. Nothing further happens on its own."
    );
    crate::state::ops::open_gate(id, "wake-stuck", &question, None)?;
    Ok(())
}

fn block_on_budget(id: &str, why: &str) -> Result<(), OttoError> {
    let already_gated = crate::state::read_run(id)?.gate.is_some();
    transaction(id, |path, state| {
        state.status = Status::Blocked;
        state.next_wake_at = None;
        crate::event::record(path, &Event::BudgetExhausted { reason: why.to_string() })
    })?;
    if !already_gated {
        let question = format!(
            "Run `{id}` has stopped because its {why}.\n\nNothing is wrong with the work; it has \
             reached a ceiling set when the run was created.\n\n\
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
        assert_eq!(state.gate.as_ref().unwrap().slug, "wake-stuck");
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

    /// A wake whose transcript cannot be read at all. Expected under yolo, where the sandbox
    /// cannot see `~/.claude` — so blind accounting must be an ordinary outcome that still counts
    /// the wake, still validates the contract, and says out loud that the tokens are unknown.
    /// Accounting is never allowed to be what fails a wake.
    #[test]
    fn a_wake_with_no_readable_transcript_still_accounts_and_validates() {
        let _h = TempHome::new();
        test_init("w-blind", "a goal").unwrap();
        let mut exec = FakeExec::new();
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
