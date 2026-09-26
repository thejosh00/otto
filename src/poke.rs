//! The poke pass: decides, for each run, whether it's due for a wake, stuck, stranded, or
//! done, and drives spawning/killing/reaping accordingly.

use crate::clock::Timestamp;
use serde::Serialize;
use crate::event::Event;
use crate::exec::{Exec, RealExec};
use crate::liveness::{Liveness, LockLiveness};
use crate::state::{CheckResult, RunEntry, RunState};
use time::{Duration, OffsetDateTime};

// How long past its `nextWakeAt` a run waits before poke spawns it. Zero: poke is the only thing
// that wakes a sleeping run, so any grace is pure lateness — at 15 it made every wake of an hourly
// run start ~19 minutes late, and since the period is anchored on the wake's start, "hourly" meant
// every ~80 minutes. The spawn-to-lock gap it once covered is already handled twice: the wake lock
// refuses a second wake outright, and a spawn that has not yet taken hold backs poke off 5 minutes.
// (A v1 leftover: an in-session timer was the primary waker then, and poke had to let it go first.)
// `otto poke --grace N` still overrides it.
pub const GRACE_MINUTES: i64 = 0;
pub const MAX_STARTS: u32 = 2;
// A spawn that produces no wake leaves the run exactly as it was; this is how many retries it
// gets before poke gives up and leaves a journal line for a person.
pub const MAX_SPAWN_ATTEMPTS: u32 = 5;
pub const BACKOFF_CAP_MINUTES: i64 = 60;
// The wake's own Exec already kills its child at deadline; this covers the rarer case of
// `otto wake` itself being wedged.
pub const DEADLINE_GRACE_MINUTES: i64 = 5;
// Generous for `gh pr view`/`git fetch --dry-run`, short enough that a hung check script can't
// block a poke pass — DESIGN.md §8.
pub const CHECK_TIMEOUT_SECONDS: u64 = 10;
// What a person reads cold in `otto show`/`otto logs`, not a place for a script to accumulate.
pub const CHECK_NOTE_MAX_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Skip,
    Defer,
    Spawn,
    Kill,
    Reap,
    Recover,
    Check,
    Error,
}

fn action_str(action: Action) -> &'static str {
    match action {
        Action::Skip => "skip",
        Action::Defer => "defer",
        Action::Spawn => "spawn",
        Action::Kill => "kill",
        Action::Reap => "reap",
        Action::Recover => "recover",
        Action::Check => "check",
        Action::Error => "error",
    }
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub run: String,
    pub action: Action,
    pub reason: String,
    pub attempts: u32,
    pub escalate: bool,
}

impl Decision {
    fn base(run: &str, action: Action, reason: String) -> Self {
        Self {
            run: run.to_string(),
            action,
            reason,
            attempts: 1,
            escalate: false,
        }
    }

    pub fn line(&self) -> String {
        format!("{:<6} {:<34} {}", action_str(self.action), self.run, self.reason)
    }
}

// 5, 10, 20, 40, then capped — doubling turns a storm into a trickle while still recovering
// within the hour once whatever broke is fixed.
pub fn backoff_minutes(attempts: u32) -> i64 {
    // Capped well below BACKOFF_CAP_MINUTES's own ceiling — just enough to keep `1i64 << exponent`
    // from ever overflowing on a run with an absurd attempt count.
    let exponent = attempts.saturating_sub(1).min(20);
    (5i64.saturating_mul(1i64 << exponent)).min(BACKOFF_CAP_MINUTES)
}

pub fn decide(
    entry: &RunEntry,
    now: OffsetDateTime,
    liveness: &dyn Liveness,
    grace_minutes: i64,
    max_attempts: u32,
    deadline_grace: i64,
) -> Decision {
    let run_id = entry.id().to_string();

    let state = match entry {
        RunEntry::Unreadable { .. } => {
            return Decision::base(
                &run_id,
                Action::Skip,
                "run.json is unreadable — a person needs to look".to_string(),
            );
        }
        RunEntry::Readable(state) => state,
    };

    if state.status.is_terminal() {
        // Normally there is nothing to reap: a tmux session ends when its wake exits. `Reap` is
        // for the leftovers — a session whose wake was killed in a way that left the shell up.
        return Decision::base(&run_id, Action::Reap, format!("status={}", crate::state::commands::status_str(state.status)));
    }

    if liveness.probe(&run_id).is_busy() {
        if let Some(wake) = &state.wake {
            let over = now - wake.deadline_at.dt();
            // `deadline_grace` covers a wake that is still wrapping up just past its deadline;
            // beyond that it's stuck, not finishing.
            if over > Duration::minutes(deadline_grace) {
                return Decision::base(
                    &run_id,
                    Action::Kill,
                    format!("wake is {}m past its deadline", over.whole_minutes()),
                );
            }
        }
        return Decision::base(&run_id, Action::Skip, "a wake is running".to_string());
    }

    if let Some(gate) = &state.gate {
        // A gate is nobody else's business: a person answers it, and `otto answer` starts the
        // next wake. An expired gate falls through to the due-time path below instead.
        let expired = gate.expires_at.map(|e| now >= e.dt()).unwrap_or(false);
        if !expired {
            return Decision::base(&run_id, Action::Skip, format!("awaiting a person on gate {}", gate.id));
        }
    }

    let attempts_now = state.spawn_attempts;
    let last_at = state.last_spawned_at.map(Timestamp::dt);

    match state.next_wake_at {
        Some(due) if due.dt() > now => return Decision::base(&run_id, Action::Skip, format!("due at {due}")),
        Some(due) if now - due.dt() < Duration::minutes(grace_minutes) => {
            return Decision::base(
                &run_id,
                Action::Defer,
                format!("due {due}, inside the {grace_minutes}m grace"),
            )
        }
        // Due, and a check script stands in front of the wake (§8.1): poke runs it directly, no
        // LLM involved, and only a real change, an error, or the safety net escalates to the
        // ordinary spawn path below.
        Some(due) if state.check_gates_wake() => {
            return Decision::base(&run_id, Action::Check, format!("due at {due}; checking first"))
        }
        Some(_) => {}
        None => {
            // n > 0 with no outcome is the shape a SIGKILL leaves: a wake began but never got to
            // record how it ended. That's a crash to recover from, not a run that's merely new.
            let started_and_died = state.wake.as_ref().map(|w| w.n > 0 && w.outcome.is_none()).unwrap_or(false);
            if started_and_died {
                let n = state.wake.as_ref().map(|w| w.n).unwrap_or(0);
                return Decision::base(
                    &run_id,
                    Action::Recover,
                    format!("wake {n} started and died without finishing"),
                );
            }
            return spawn_or_back_off(
                &run_id,
                "stranded: nothing running, no gate, no wake time".to_string(),
                attempts_now,
                last_at,
                now,
                max_attempts,
            );
        }
    }

    spawn_or_back_off(
        &run_id,
        format!("missed its wake at {}", state.next_wake_at.expect("Some checked above")),
        attempts_now,
        last_at,
        now,
        max_attempts,
    )
}

/// Spawn, unless our own last spawn produced nothing and it's too soon to try again.
/// "Produced nothing" is judged by `spawn_attempts` vs. whether a wake has run since: `otto wake`
/// clears the counter when it starts, so a non-zero count means the last spawn never took hold.
fn spawn_or_back_off(
    run_id: &str,
    reason: String,
    attempts: u32,
    last_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
    max_attempts: u32,
) -> Decision {
    let Some(last_at) = last_at else {
        return Decision {
            attempts: 1,
            ..Decision::base(run_id, Action::Spawn, reason)
        };
    };
    if attempts == 0 {
        // A wake ran since the last spawn, so the count was cleared: start over.
        return Decision {
            attempts: 1,
            ..Decision::base(run_id, Action::Spawn, reason)
        };
    }
    if attempts > max_attempts {
        return Decision::base(run_id, Action::Defer, format!("gave up after {max_attempts} spawns"));
    }
    if attempts >= max_attempts {
        return Decision {
            attempts: attempts + 1,
            escalate: true,
            ..Decision::base(
                run_id,
                Action::Defer,
                format!("{attempts} spawns produced no wake — giving up, a person needs to look"),
            )
        };
    }
    let wait = backoff_minutes(attempts);
    if now - last_at < Duration::minutes(wait) {
        return Decision::base(
            run_id,
            Action::Defer,
            format!("backing off {wait}m after {attempts} spawn(s) that produced no wake"),
        );
    }
    Decision {
        attempts: attempts + 1,
        ..Decision::base(run_id, Action::Spawn, reason)
    }
}

/// What one run of a check script said, and what it printed.
pub struct CheckRun {
    pub result: CheckResult,
    pub note: Option<String>,
    pub code: i32,
    pub timed_out: bool,
}

/// Run a check script the way poke does — same environment, same timeout, same reading of the
/// exit code — without deciding anything. Shared with `otto check`, which runs a new script once
/// on the spot so a broken one shows now rather than when the run is next due.
pub fn execute_check(run_id: &str, state: &RunState, script: &str, exec: &mut dyn Exec) -> CheckRun {
    let mut env = std::collections::HashMap::new();
    env.insert("OTTO_RUN_ID".to_string(), run_id.to_string());
    if let Some(repo) = state.facts.get("repo").and_then(serde_json::Value::as_str) {
        env.insert("OTTO_REPO".to_string(), repo.to_string());
    }

    // Isolated: a script killed at its timeout takes whatever it started down with it, so a
    // hung `curl` under it can't stall this pass — and every other run's — behind it.
    let output = exec.exec_isolated(&[script], Some(&env), std::time::Duration::from_secs(CHECK_TIMEOUT_SECONDS));
    let note = crate::exec::truncate(output.stdout.trim(), CHECK_NOTE_MAX_CHARS);
    let note = (!note.is_empty()).then_some(note);

    let result = if output.timed_out {
        CheckResult::Error
    } else {
        match output.code {
            0 => CheckResult::NoChange,
            1 => CheckResult::Changed,
            _ => CheckResult::Error,
        }
    };
    CheckRun { result, note, code: output.code, timed_out: output.timed_out }
}

/// Runs a run's check script (§8.1) in front of its due wake and turns the result into an
/// ordinary decision: no change resolves to `Skip` — silent by default, same as `Reap` when
/// there's nothing to reap — and the run sleeps again. Anything else (a real change, a script that
/// exits with something other than 0/1, or a timeout) falls into exactly the `spawn_or_back_off`
/// path a due wake without a check takes: fail open to a real wake rather than trust a script
/// that might be wrong. So does the safety net — `wakeAfter` no-change results in a row wake the
/// run anyway, which is what catches a script that is wrong without failing.
fn run_check(
    entry: &RunEntry,
    decision: Decision,
    now: OffsetDateTime,
    exec: &mut dyn Exec,
    dry_run: bool,
    max_attempts: u32,
) -> Decision {
    let RunEntry::Readable(state) = entry else {
        unreachable!("decide() never returns Check for an unreadable run")
    };
    let run_id = decision.run.clone();
    let Some(check) = &state.check else {
        unreachable!("decide() never returns Check when state.check is None")
    };

    if dry_run {
        return Decision {
            reason: format!("[dry-run] would run check script {}", check.script),
            ..decision
        };
    }

    let script = match crate::paths::run_dir(&run_id) {
        Ok(dir) => dir.join(&check.script),
        Err(e) => return Decision::base(&run_id, Action::Error, format!("cannot resolve check script: {e}")),
    };
    let Some(script) = script.to_str() else {
        return Decision::base(&run_id, Action::Error, "check script path is not valid UTF-8".to_string());
    };

    let CheckRun { result, note, code, timed_out } = execute_check(&run_id, state, script, exec);
    let streak = check.consecutive_no_change + 1;
    let safety_net = result == CheckResult::NoChange && check.wake_after > 0 && streak >= check.wake_after;
    let _ = crate::state::commands::record_check(&run_id, result, note, !safety_net);

    match result {
        CheckResult::NoChange if safety_net => spawn_or_back_off(
            &run_id,
            format!("check found nothing {streak} times in a row — waking anyway, in case the check is wrong"),
            state.spawn_attempts,
            state.last_spawned_at.map(Timestamp::dt),
            now,
            max_attempts,
        ),
        CheckResult::NoChange => Decision::base(&run_id, Action::Skip, "checked — no change; sleeping again".to_string()),
        CheckResult::Changed => spawn_or_back_off(
            &run_id,
            "check script reported a change".to_string(),
            state.spawn_attempts,
            state.last_spawned_at.map(Timestamp::dt),
            now,
            max_attempts,
        ),
        CheckResult::Error => spawn_or_back_off(
            &run_id,
            format!(
                "check script errored (exit {}{}) — spawning a wake to look",
                code,
                if timed_out { ", timed out" } else { "" }
            ),
            state.spawn_attempts,
            state.last_spawned_at.map(Timestamp::dt),
            now,
            max_attempts,
        ),
    }
}

/// A killed wake cannot run its own validator, so poke records the failure on its behalf.
/// `spawner::kill` takes the process group, not just the wrapper — otherwise the model
/// underneath would be left running.
fn kill_wake(state: &RunState, run_id: &str, reason: &str, exec: &mut dyn Exec) -> Decision {
    crate::spawner::kill(state, run_id, exec);
    let _ = crate::state::log_event(run_id, Event::WakeKilled { reason: reason.to_string() });
    match crate::wake::record_incomplete(run_id, &format!("killed by poke: {reason}")) {
        Ok(()) => Decision::base(run_id, Action::Kill, format!("killed — {reason}")),
        Err(e) => Decision::base(run_id, Action::Error, format!("killed but could not record it: {e}")),
    }
}

/// One sweep. Overdue by an hour or by three days makes no difference: each run gets at most
/// one wake, which then does exactly one reconcile — there is no backlog to replay.
#[allow(clippy::too_many_arguments)]
pub fn pass_once(
    runs: &[RunEntry],
    now: OffsetDateTime,
    liveness: &dyn Liveness,
    exec: &mut dyn Exec,
    max_starts: u32,
    grace_minutes: i64,
    dry_run: bool,
    max_attempts: u32,
    deadline_grace: i64,
) -> Vec<Decision> {
    let mut decisions = Vec::new();
    let mut started = 0u32;
    for entry in runs {
        let mut decision = decide(entry, now, liveness, grace_minutes, max_attempts, deadline_grace);
        let run_id = decision.run.clone();

        // Resolves to `Skip` (no change), or to `Spawn`/`Defer` exactly as a missed `nextWakeAt`
        // would — the existing match below handles those the same way either way arrived.
        if let Action::Check = decision.action {
            decision = run_check(entry, decision, now, exec, dry_run, max_attempts);
        }

        if decision.escalate && !dry_run {
            // Say it once, in the run's own history, and write the marker that keeps a
            // five-minute poller from repeating it every pass.
            let _ = crate::state::log_event(&run_id, Event::SpawnAbandoned { reason: decision.reason.clone() });
            let _ = crate::state::commands::record_spawn(&run_id, decision.attempts, false);
        }

        match decision.action {
            Action::Reap => {
                // Silent unless there was actually something to remove, so a directory full of
                // finished runs does not print a line each every five minutes.
                let session = crate::detach::session_name(&run_id);
                if dry_run {
                    decision.action = Action::Skip;
                } else if crate::detach::session_exists(exec, &session) {
                    crate::detach::kill_session(exec, &session);
                    decision.reason = format!("{}, reaped leftover session {session}", decision.reason);
                } else {
                    decision.action = Action::Skip;
                }
            }
            Action::Kill => {
                if dry_run {
                    decision.reason = format!("[dry-run] {}", decision.reason);
                } else {
                    // `Kill` is only ever decided for a run whose state we could read — see `decide`.
                    let RunEntry::Readable(state) = entry else {
                        unreachable!("decide() never returns Kill for an unreadable run")
                    };
                    decision = kill_wake(state, &run_id, &decision.reason.clone(), exec);
                }
            }
            Action::Recover => {
                if dry_run {
                    decision.reason = format!("[dry-run] {}", decision.reason);
                } else if let Err(e) = crate::wake::record_incomplete(&run_id, &decision.reason.clone()) {
                    decision = Decision {
                        action: Action::Error,
                        reason: format!("could not record the failed wake: {e}"),
                        ..decision
                    };
                }
                // No spawn this pass: `record_incomplete` armed the retry itself, so the next
                // pass will find a due wake time and honour the backoff.
            }
            Action::Spawn => {
                if started >= max_starts {
                    decision = Decision {
                        action: Action::Defer,
                        reason: format!("start limit {max_starts} reached this pass"),
                        ..decision
                    };
                } else if dry_run {
                    decision.reason = format!("[dry-run] {}", decision.reason);
                    started += 1;
                } else {
                    // Record the attempt *before* spawning, so a spawn that fails to produce a
                    // wake still counts against the backoff. Otherwise, a broken launcher would be
                    // retried every five minutes forever.
                    let _ = crate::state::commands::record_spawn(&run_id, decision.attempts, true);
                    // `Spawn` is only ever decided for a run whose state we could read — see `decide`.
                    let RunEntry::Readable(state) = entry else {
                        unreachable!("decide() never returns Spawn for an unreadable run")
                    };
                    let strategy = crate::spawner::Strategy::for_poke(state.launcher.detach);
                    match crate::spawner::start(&run_id, strategy, None, exec) {
                        Ok(handle) => {
                            decision.reason =
                                format!("{} → {}", decision.reason, handle.description.unwrap_or_default())
                        }
                        Err(e) => {
                            decision = Decision {
                                action: Action::Error,
                                reason: format!("could not start a wake: {e}"),
                                ..decision
                            }
                        }
                    }
                    started += 1;
                }
            }
            _ => {}
        }
        decisions.push(decision);
    }
    decisions
}

#[derive(clap::Args, Debug)]
pub struct PokeArgs {
    /// Decide, print what it would do, and change nothing — no wake, no check script, no kill
    #[arg(long = "dry-run")]
    pub dry_run: bool,
    /// Say what it decided for every run, not only the ones where it did something
    #[arg(long)]
    pub verbose: bool,
    /// Start at most this many wakes in one pass; the rest wait for the next pass
    #[arg(long = "max-starts", default_value_t = MAX_STARTS)]
    pub max_starts: u32,
    /// Minutes past a run's wake time before poke starts it. 0: as soon as it is due
    #[arg(long, default_value_t = GRACE_MINUTES, value_name = "MINUTES")]
    pub grace: i64,
    /// Spawn attempts that produce no wake before poke gives up on a run and says so once
    #[arg(long = "max-attempts", default_value_t = MAX_SPAWN_ATTEMPTS)]
    pub max_attempts: u32,
    /// Minutes past a wake's deadline before poke kills it as stuck rather than finishing
    #[arg(long = "deadline-grace", default_value_t = DEADLINE_GRACE_MINUTES, value_name = "MINUTES")]
    pub deadline_grace: i64,
}

/// What one poke pass said, and the exit code it ends with (0 fine, 1 a decision errored, 2 poke
/// itself could not run).
#[derive(Debug, Clone, Default, Serialize)]
pub struct PokeReport {
    pub lines: Vec<String>,
    pub errors: Vec<String>,
    pub code: i32,
}

impl Default for PokeArgs {
    fn default() -> Self {
        PokeArgs {
            dry_run: false,
            verbose: false,
            max_starts: MAX_STARTS,
            grace: GRACE_MINUTES,
            max_attempts: MAX_SPAWN_ATTEMPTS,
            deadline_grace: DEADLINE_GRACE_MINUTES,
        }
    }
}

/// One pass: start the wakes that are due and clean up after the ones that are over. Shared by
/// `otto poke` (launchd, or by hand) and the web page's "poke now".
pub fn poke_pass(args: &PokeArgs) -> PokeReport {
    let mut report = PokeReport::default();
    let runs_dir = crate::paths::runs_dir();
    if !runs_dir.is_dir() {
        report.lines.push("otto poke: no runs directory — nothing to do".to_string());
        return report;
    }

    let lock_path = match crate::paths::poke_lock_path() {
        Ok(p) => p,
        Err(e) => {
            report.errors.push(format!("otto poke: cannot create the poke lock directory: {e}"));
            report.code = 2;
            return report;
        }
    };
    let lock_file = match std::fs::OpenOptions::new().create(true).append(true).open(&lock_path) {
        Ok(f) => f,
        Err(e) => {
            report.errors.push(format!("otto poke: cannot open {}: {e}", lock_path.display()));
            report.code = 2;
            return report;
        }
    };
    // Poke is typically invoked on a timer; the lock keeps overlapping invocations from double-
    // spawning the same stranded run instead of one just waiting its turn.
    if fs4::fs_std::FileExt::try_lock_exclusive(&lock_file).is_err() {
        report.lines.push("otto poke: another pass is still running — skipping this one".to_string());
        return report;
    }

    let runs = crate::state::read_all_runs().unwrap_or_default();
    let mut exec = RealExec;
    let decisions = pass_once(
        &runs,
        crate::clock::now(),
        &LockLiveness,
        &mut exec,
        args.max_starts,
        args.grace,
        args.dry_run,
        args.max_attempts,
        args.deadline_grace,
    );
    // Read again: this pass may itself have just blocked a run (a kill, a recovered crash), and
    // that is worth saying now rather than five minutes from now. Still under the poke lock, so
    // two overlapping passes cannot both send the same notice.
    let runs = crate::state::read_all_runs().unwrap_or_default();
    let notices = crate::notify::pass(&runs, crate::clock::now(), &mut exec, args.dry_run);
    let _ = fs4::fs_std::FileExt::unlock(&lock_file);

    let stamp = crate::clock::format_iso(crate::clock::now());
    let shown: Vec<&Decision> = decisions.iter().filter(|d| args.verbose || d.action != Action::Skip).collect();
    for decision in &shown {
        report.lines.push(format!("{stamp} {}", decision.line()));
    }
    for line in &notices {
        report.lines.push(format!("{stamp} {line}"));
    }
    if shown.is_empty() && notices.is_empty() {
        report.lines.push(format!("{stamp} nothing due ({} run(s) checked)", decisions.len()));
    }
    if decisions.iter().any(|d| d.action == Action::Error) {
        report.code = 1;
    }
    report
}

/// The `otto poke` entrypoint. Returns the exit code directly (0/1/2) rather than routing
/// through `OttoError` — there's no single "otto: <message>" to print, because the decisions
/// themselves are the output.
pub fn poke_run(args: PokeArgs) -> i32 {
    let report = poke_pass(&args);
    for line in &report.lines {
        println!("{line}");
    }
    for line in &report.errors {
        eprintln!("{line}");
    }
    report.code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::fake::FakeExec;
    use crate::exec::Output;
    use crate::liveness::fake::FakeLiveness;
    use crate::state::{test_run_state, Check, Gate, Status, Wake};
    use time::macros::datetime;

    fn now() -> OffsetDateTime {
        datetime!(2026-09-12 12:00:00 UTC)
    }

    fn at(dt: OffsetDateTime) -> Timestamp {
        Timestamp::at(dt)
    }

    fn base_state(id: &str) -> RunState {
        let mut state = test_run_state(id);
        state.status = Status::Sleeping;
        state
    }

    fn idle() -> FakeLiveness {
        FakeLiveness::new()
    }

    fn decide_with(state: &RunState, liveness: &dyn Liveness) -> Decision {
        decide(&RunEntry::Readable(state.clone()), now(), liveness, GRACE_MINUTES, MAX_SPAWN_ATTEMPTS, DEADLINE_GRACE_MINUTES)
    }

    fn wake_with(n: u32, deadline: Timestamp, pid: u32) -> Wake {
        Wake { n, started_at: deadline, deadline_at: deadline, launcher: "claude".into(), pid: Some(pid), session: None, outcome: None }
    }

    fn a_check(wake_after: u32) -> Check {
        Check {
            script: "artifacts/check.sh".to_string(),
            retry_seconds: None,
            wake_after,
            consecutive_no_change: 0,
            last_result: None,
            last_at: None,
            last_note: None,
            no_change_total: 0,
            pinned: true,
        }
    }

    /// A sleeping run whose wake came due a minute ago (by this module's fixed `now()`), with a
    /// check standing in front of it.
    fn due_with_check(id: &str, check: Check) -> RunState {
        let mut state = base_state(id);
        state.next_wake_at = Some(at(now() - Duration::minutes(1)));
        state.check = Some(check);
        state
    }

    /// `record_check` reads and writes the *real* `run.json` on disk (same as `record_spawn`),
    /// so a test that wants to see its effect needs a run whose disk state already has the
    /// check armed — a `RunEntry` built only in memory, as `base_state` gives every other test
    /// here, would let `record_check` see `check: None` on disk and no-op. This persists it,
    /// then hands back the same `RunEntry` poke would get from `read_all_runs()`.
    fn init_with_check(id: &str, check: Check) -> RunEntry {
        crate::state::commands::test_init(id, "a goal").unwrap();
        crate::state::transaction(id, |_path, state| {
            state.status = Status::Sleeping;
            state.next_wake_at = Some(at(now() - Duration::minutes(1)));
            state.check = Some(check.clone());
            Ok(())
        })
        .unwrap();
        RunEntry::Readable(crate::state::read_run(id).unwrap())
    }

    fn pass(entry: RunEntry, exec: &mut FakeExec) -> Vec<Decision> {
        pass_once(&[entry], now(), &idle(), exec, MAX_STARTS, GRACE_MINUTES, false, MAX_SPAWN_ATTEMPTS, DEADLINE_GRACE_MINUTES)
    }

    // --- the cases that need no inference at all ---

    #[test]
    fn an_unreadable_run_is_left_for_a_person() {
        let entry = RunEntry::Unreadable { id: "r".to_string() };
        let decision = decide(&entry, now(), &idle(), GRACE_MINUTES, MAX_SPAWN_ATTEMPTS, DEADLINE_GRACE_MINUTES);
        assert_eq!(decision.action, Action::Skip);
    }

    #[test]
    fn a_terminal_run_is_reaped_never_spawned() {
        for status in [Status::Done, Status::Failed, Status::Stopped] {
            let mut state = base_state("r");
            state.status = status;
            // Even with a wake time long past — a retired run must never be restarted.
            state.next_wake_at = Some(at(now() - Duration::hours(30)));
            assert_eq!(decide_with(&state, &idle()).action, Action::Reap, "{status:?}");
        }
    }

    #[test]
    fn a_future_wake_is_left_alone() {
        let mut state = base_state("r");
        state.next_wake_at = Some(at(now() + Duration::minutes(30)));
        assert_eq!(decide_with(&state, &idle()).action, Action::Skip);
    }

    // --- liveness ---

    #[test]
    fn a_running_wake_is_left_to_work() {
        let mut state = base_state("r");
        state.next_wake_at = Some(at(now() - Duration::hours(2)));
        let decision = decide_with(&state, &FakeLiveness::with_running(&["r"]));
        assert_eq!(decision.action, Action::Skip);
        assert!(decision.reason.contains("a wake is running"));
    }

    #[test]
    fn liveness_we_cannot_determine_is_treated_as_busy() {
        let mut state = base_state("r");
        state.next_wake_at = Some(at(now() - Duration::hours(2)));
        let mut liveness = FakeLiveness::new();
        liveness.set_unknown("r");
        assert_eq!(decide_with(&state, &liveness).action, Action::Skip);
    }

    #[test]
    fn a_wake_past_its_deadline_is_killed() {
        let mut state = base_state("r");
        state.wake = Some(wake_with(3, at(now() - Duration::minutes(30)), 4242));
        let decision = decide_with(&state, &FakeLiveness::with_running(&["r"]));
        assert_eq!(decision.action, Action::Kill);
        assert!(decision.reason.contains("past its deadline"));
    }

    /// A wake finishing up must not be killed out from under itself.
    #[test]
    fn a_wake_just_past_its_deadline_is_inside_the_grace() {
        let mut state = base_state("r");
        state.wake = Some(wake_with(3, at(now() - Duration::minutes(2)), 4242));
        assert_eq!(decide_with(&state, &FakeLiveness::with_running(&["r"])).action, Action::Skip);
    }

    // --- gates ---

    fn gate(id: &str, slug: &str, expires_at: Option<Timestamp>) -> Gate {
        Gate {
            id: id.to_string(),
            slug: slug.to_string(),
            file: format!("gates/{id}-{slug}.md"),
            asked_at: Timestamp::now(),
            answered_at: None,
            expires_at,
        }
    }

    #[test]
    fn an_open_gate_belongs_to_a_person() {
        let mut state = base_state("r");
        state.status = Status::AwaitingHuman;
        state.gate = Some(gate("001", "plan-review", None));
        let decision = decide_with(&state, &idle());
        assert_eq!(decision.action, Action::Skip);
        assert!(decision.reason.contains("gate 001"));
    }

    /// An expired gate is the one case where a gate needs a wake rather than a person: something
    /// has to record the silence and carry on.
    #[test]
    fn an_expired_gate_gets_a_wake() {
        let mut state = base_state("r");
        state.status = Status::AwaitingHuman;
        state.gate = Some(gate("001", "proposal", Some(at(now() - Duration::hours(1)))));
        state.next_wake_at = Some(at(now() - Duration::hours(1)));
        assert_eq!(decide_with(&state, &idle()).action, Action::Spawn);
    }

    #[test]
    fn a_gate_still_within_its_expiry_waits() {
        let mut state = base_state("r");
        state.status = Status::AwaitingHuman;
        state.gate = Some(gate("001", "proposal", Some(at(now() + Duration::hours(4)))));
        state.next_wake_at = Some(at(now() + Duration::hours(4)));
        assert_eq!(decide_with(&state, &idle()).action, Action::Skip);
    }

    // --- the stranded run ---

    #[test]
    fn a_run_with_nothing_at_all_pending_is_stranded_and_spawned() {
        let state = base_state("r");
        let decision = decide_with(&state, &idle());
        assert_eq!(decision.action, Action::Spawn);
        assert!(decision.reason.contains("stranded"));
    }

    // --- grace and backoff, inherited ---

    #[test]
    fn a_just_missed_wake_is_spawned_on_the_next_pass() {
        let mut state = base_state("r");
        state.next_wake_at = Some(at(now() - Duration::minutes(1)));
        let decision = decide_with(&state, &idle());
        assert_eq!(decision.action, Action::Spawn, "{}", decision.reason);
    }

    #[test]
    fn an_explicit_grace_still_defers_inside_it() {
        let mut state = base_state("r");
        state.next_wake_at = Some(at(now() - Duration::minutes(5)));
        let entry = RunEntry::Readable(state);
        let decision = decide(&entry, now(), &idle(), 15, MAX_SPAWN_ATTEMPTS, DEADLINE_GRACE_MINUTES);
        assert_eq!(decision.action, Action::Defer);
        assert!(decision.reason.contains("grace"));
    }

    #[test]
    fn a_missed_wake_past_the_grace_is_spawned_as_attempt_one() {
        let mut state = base_state("r");
        state.next_wake_at = Some(at(now() - Duration::hours(2)));
        let decision = decide_with(&state, &idle());
        assert_eq!(decision.action, Action::Spawn);
        assert_eq!(decision.attempts, 1);
    }

    #[test]
    fn a_recent_failed_spawn_backs_off_then_retries_once_elapsed() {
        let mut state = base_state("r");
        state.next_wake_at = Some(at(now() - Duration::hours(2)));
        state.spawn_attempts = 1;
        state.last_spawned_at = Some(at(now() - Duration::minutes(2)));
        assert_eq!(decide_with(&state, &idle()).action, Action::Defer);

        state.spawn_attempts = 1;
        state.last_spawned_at = Some(at(now() - Duration::minutes(20)));
        let decision = decide_with(&state, &idle());
        assert_eq!(decision.action, Action::Spawn);
        assert_eq!(decision.attempts, 2);
    }

    #[test]
    fn a_cleared_attempt_count_means_the_last_spawn_worked() {
        let mut state = base_state("r");
        state.next_wake_at = Some(at(now() - Duration::hours(2)));
        state.spawn_attempts = 0;
        state.last_spawned_at = Some(at(now() - Duration::minutes(1)));
        let decision = decide_with(&state, &idle());
        assert_eq!(decision.action, Action::Spawn);
        assert_eq!(decision.attempts, 1);
    }

    #[test]
    fn repeated_failed_spawns_escalate_once_then_give_up_quietly() {
        let mut state = base_state("r");
        state.next_wake_at = Some(at(now() - Duration::hours(9)));
        state.spawn_attempts = MAX_SPAWN_ATTEMPTS;
        state.last_spawned_at = Some(at(now() - Duration::hours(5)));
        let decision = decide_with(&state, &idle());
        assert_eq!(decision.action, Action::Defer);
        assert!(decision.escalate);

        state.spawn_attempts = MAX_SPAWN_ATTEMPTS + 1;
        let decision = decide_with(&state, &idle());
        assert_eq!(decision.action, Action::Defer);
        assert!(!decision.escalate, "it must complain only once");
    }

    #[test]
    fn the_backoff_schedule_doubles_then_caps() {
        assert_eq!(backoff_minutes(1), 5);
        assert_eq!(backoff_minutes(2), 10);
        assert_eq!(backoff_minutes(3), 20);
        assert_eq!(backoff_minutes(4), 40);
        assert_eq!(backoff_minutes(5), BACKOFF_CAP_MINUTES);
        assert_eq!(backoff_minutes(50), BACKOFF_CAP_MINUTES);
    }

    // --- opt-in check scripts (§8) ---

    #[test]
    fn a_due_wake_with_a_check_is_checked_first() {
        let decision = decide_with(&due_with_check("r", a_check(24)), &idle());
        assert_eq!(decision.action, Action::Check);
    }

    /// There is no separate cadence: nothing runs the check before the wake is due.
    #[test]
    fn a_check_waits_for_the_wake_to_come_due() {
        let mut state = due_with_check("r", a_check(24));
        state.next_wake_at = Some(at(now() + Duration::minutes(10)));
        assert_eq!(decide_with(&state, &idle()).action, Action::Skip);
    }

    /// A person's check knows nothing about why a wake armed its own timer ("CI in ten minutes"),
    /// so that wake goes ahead without it.
    #[test]
    fn a_person_s_check_does_not_stand_in_front_of_a_timer_a_wake_armed() {
        let mut state = due_with_check("r", a_check(24));
        state.armed_wake_at = state.next_wake_at;
        assert_eq!(decide_with(&state, &idle()).action, Action::Spawn);
    }

    /// A wake that did not finish is retried whatever the check says — the work is half done.
    #[test]
    fn a_check_never_stands_in_front_of_a_failed_wake_s_retry() {
        let mut state = due_with_check("r", a_check(24));
        state.incomplete_wakes = 1;
        assert_eq!(decide_with(&state, &idle()).action, Action::Spawn);
    }

    /// A wake's own check was armed for exactly that sleep, so it does stand in front of it.
    #[test]
    fn a_wake_s_own_check_stands_in_front_of_the_timer_it_came_with() {
        let mut check = a_check(24);
        check.pinned = false;
        let mut state = due_with_check("r", check);
        state.armed_wake_at = state.next_wake_at;
        assert_eq!(decide_with(&state, &idle()).action, Action::Check);
    }

    #[test]
    fn a_check_script_reporting_no_change_sleeps_the_run_again() {
        let _h = crate::paths::test_support::TempHome::new();
        let entry = init_with_check("p-check-none", a_check(24));
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 0, stdout: "nothing new".to_string(), ..Default::default() });
        let decisions = pass(entry, &mut exec);
        assert_eq!(decisions[0].action, Action::Skip, "nothing to reap is not worth a line, same as `Reap`");
        let after = crate::state::read_run("p-check-none").unwrap();
        // Put off by a period from the real wall clock, as `arm_timer` does — so from now, not
        // from this test's fixed `now()`.
        assert!(after.next_wake_at.unwrap().dt() > crate::clock::now() + Duration::minutes(59));
        let check = after.check.expect("check must survive a no-change result");
        assert_eq!(check.consecutive_no_change, 1);
        assert_eq!(check.last_result, Some(CheckResult::NoChange));
        assert_eq!(check.no_change_total, 1, "each no-change result is a wake not spent");
        assert_eq!(check.last_note.as_deref(), Some("nothing new"), "what it saw is kept for `otto show`");
        assert!(check.last_at.is_some());
        let journal = std::fs::read_to_string(crate::paths::run_dir("p-check-none").unwrap().join("journal.jsonl")).unwrap();
        assert!(!journal.contains("check-ran"), "a no-change result must not spam the journal");
    }

    #[test]
    fn a_check_script_reporting_a_change_falls_through_to_spawn() {
        let _h = crate::paths::test_support::TempHome::new();
        let entry = init_with_check("p-check-changed", a_check(24));
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 1, ..Default::default() });
        exec.queue(Output { code: 1, ..Default::default() }); // has-session: no, for the spawn itself
        let decisions = pass(entry, &mut exec);
        assert_eq!(decisions[0].action, Action::Spawn);
        let after = crate::state::read_run("p-check-changed").unwrap();
        assert_eq!(after.check.unwrap().last_result, Some(CheckResult::Changed));
        assert_eq!(after.spawn_attempts, 1, "the check-triggered spawn goes through the same bookkeeping");
        let journal = std::fs::read_to_string(crate::paths::run_dir("p-check-changed").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("check-ran"), "a real change must be journaled");
    }

    #[test]
    fn a_check_script_error_also_spawns_rather_than_staying_silent() {
        let _h = crate::paths::test_support::TempHome::new();
        let entry = init_with_check("p-check-error", a_check(24));
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 17, ..Default::default() });
        exec.queue(Output { code: 1, ..Default::default() });
        let decisions = pass(entry, &mut exec);
        assert_eq!(decisions[0].action, Action::Spawn, "fail open to a real wake, never fail closed into silence");
        let after = crate::state::read_run("p-check-error").unwrap();
        assert_eq!(after.check.unwrap().last_result, Some(CheckResult::Error));
    }

    /// The safety net: a script that is wrong without failing would otherwise keep a run asleep
    /// forever. After `wakeAfter` no-change results in a row, the run wakes anyway.
    #[test]
    fn the_safety_net_wakes_the_run_after_enough_no_change_results_in_a_row() {
        let _h = crate::paths::test_support::TempHome::new();
        let mut check = a_check(3);
        check.consecutive_no_change = 2;
        let entry = init_with_check("p-check-net", check);
        let due = crate::state::read_run("p-check-net").unwrap().next_wake_at;
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 0, ..Default::default() });
        exec.queue(Output { code: 1, ..Default::default() });
        let decisions = pass(entry, &mut exec);
        assert_eq!(decisions[0].action, Action::Spawn);
        assert!(decisions[0].reason.contains("3 times in a row"), "{}", decisions[0].reason);
        let after = crate::state::read_run("p-check-net").unwrap();
        assert_eq!(after.next_wake_at, due, "the wake stays due rather than being put off again");
        assert_eq!(after.check.unwrap().no_change_total, 1);
    }

    #[test]
    fn a_safety_net_of_zero_never_fires() {
        let _h = crate::paths::test_support::TempHome::new();
        let mut check = a_check(0);
        check.consecutive_no_change = 500;
        let entry = init_with_check("p-check-off", check);
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 0, ..Default::default() });
        assert_eq!(pass(entry, &mut exec)[0].action, Action::Skip);
    }

    #[test]
    fn a_dry_run_check_never_executes_the_script() {
        let state = due_with_check("r", a_check(24));
        let mut exec = FakeExec::new();
        let decisions = pass_once(
            &[RunEntry::Readable(state)],
            now(),
            &idle(),
            &mut exec,
            MAX_STARTS,
            GRACE_MINUTES,
            true,
            MAX_SPAWN_ATTEMPTS,
            DEADLINE_GRACE_MINUTES,
        );
        assert_eq!(decisions[0].action, Action::Check);
        assert!(decisions[0].reason.starts_with("[dry-run]"));
        assert!(exec.calls.borrow().is_empty(), "a dry run must not shell out, same as every other action");
    }

    // --- the pass ---

    #[test]
    fn a_pass_starts_no_more_than_its_limit() {
        let _h = crate::paths::test_support::TempHome::new();
        let mut runs = Vec::new();
        for n in 0..4 {
            let id = format!("p{n}");
            crate::state::commands::test_init(&id, "a goal").unwrap();
            let mut state = base_state(&id);
            state.next_wake_at = Some(at(now() - Duration::hours(2)));
            runs.push(RunEntry::Readable(state));
        }
        let mut exec = FakeExec::new();
        let decisions = pass_once(
            &runs,
            now(),
            &idle(),
            &mut exec,
            2,
            GRACE_MINUTES,
            true,
            MAX_SPAWN_ATTEMPTS,
            DEADLINE_GRACE_MINUTES,
        );
        let spawned = decisions.iter().filter(|d| d.action == Action::Spawn).count();
        let deferred = decisions.iter().filter(|d| d.action == Action::Defer).count();
        assert_eq!(spawned, 2, "two per pass");
        assert_eq!(deferred, 2, "the rest wait for the next one");
    }

    #[test]
    fn a_dry_run_changes_nothing() {
        let _h = crate::paths::test_support::TempHome::new();
        crate::state::commands::test_init("p-dry", "a goal").unwrap();
        let mut state = base_state("p-dry");
        state.next_wake_at = Some(at(now() - Duration::hours(2)));
        let before = std::fs::read_to_string(crate::paths::run_dir("p-dry").unwrap().join("run.json")).unwrap();
        let mut exec = FakeExec::new();
        let decisions = pass_once(
            &[RunEntry::Readable(state)],
            now(),
            &idle(),
            &mut exec,
            MAX_STARTS,
            GRACE_MINUTES,
            true,
            MAX_SPAWN_ATTEMPTS,
            DEADLINE_GRACE_MINUTES,
        );
        assert!(decisions[0].reason.starts_with("[dry-run]"));
        assert!(exec.calls.borrow().is_empty(), "a dry run must not shell out");
        let after = std::fs::read_to_string(crate::paths::run_dir("p-dry").unwrap().join("run.json")).unwrap();
        assert_eq!(before, after);
    }

    /// The attempt has to be recorded before the spawn, or a launcher that always fails would be
    /// retried every five minutes forever.
    #[test]
    fn a_spawn_records_its_attempt_before_trying() {
        let _h = crate::paths::test_support::TempHome::new();
        crate::state::commands::test_init("p-att", "a goal").unwrap();
        let mut state = base_state("p-att");
        state.next_wake_at = Some(at(now() - Duration::hours(2)));
        let mut exec = FakeExec::new();
        // has-session says "no", then new-session succeeds.
        exec.queue(crate::exec::Output { code: 1, ..Default::default() });
        pass_once(
            &[RunEntry::Readable(state)],
            now(),
            &idle(),
            &mut exec,
            MAX_STARTS,
            GRACE_MINUTES,
            false,
            MAX_SPAWN_ATTEMPTS,
            DEADLINE_GRACE_MINUTES,
        );
        let after = crate::state::read_run("p-att").unwrap();
        assert_eq!(after.spawn_attempts, 1);
        assert!(after.last_spawned_at.is_some());
        // And it must NOT be in `facts`, which the wake rewrites freely.
        assert!(!after.facts.contains_key("spawnAttempts"), "control state must be out of the wake's reach");
    }

    /// A killed wake cannot record its own failure, so poke does it — and must leave the run
    /// somewhere something will come back to.
    #[test]
    fn killing_a_wake_records_it_as_incomplete_and_arms_a_retry() {
        let _h = crate::paths::test_support::TempHome::new();
        crate::state::commands::test_init("p-kill", "a goal").unwrap();
        let mut state = base_state("p-kill");
        state.status = Status::Running;
        state.wake = Some(wake_with(1, at(now() - Duration::hours(1)), 999999));
        let mut exec = FakeExec::new();
        let decisions = pass_once(
            &[RunEntry::Readable(state)],
            now(),
            &FakeLiveness::with_running(&["p-kill"]),
            &mut exec,
            MAX_STARTS,
            GRACE_MINUTES,
            false,
            MAX_SPAWN_ATTEMPTS,
            DEADLINE_GRACE_MINUTES,
        );
        assert_eq!(decisions[0].action, Action::Kill);
        let after = crate::state::read_run("p-kill").unwrap();
        assert_eq!(after.incomplete_wakes, 1);
        assert_eq!(after.status, crate::state::Status::Sleeping);
        assert!(after.next_wake_at.is_some(), "a killed wake must still leave a way back");
        let journal = std::fs::read_to_string(crate::paths::run_dir("p-kill").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("wake-killed"));
        // It must have tried to kill the process group, not just the wrapper.
        let calls: Vec<String> = exec.calls.borrow().iter().map(|c| c.join(" ")).collect();
        assert!(calls.iter().any(|c| c.contains("kill -TERM -999999")), "got {calls:?}");
    }

    /// Without this, a wake that reliably crashes after starting would be respawned every
    /// five minutes forever: starting clears poke's attempt counter, so the backoff never bit.
    #[test]
    fn a_wake_that_started_and_died_is_recorded_rather_than_instantly_respawned() {
        let _h = crate::paths::test_support::TempHome::new();
        crate::state::commands::test_init("p-crash", "a goal").unwrap();
        let mut state = base_state("p-crash");
        state.status = Status::Running;
        // A wake began (n=1) and never recorded an outcome — the shape a SIGKILL leaves.
        state.wake = Some(wake_with(1, at(now() + Duration::hours(1)), 4242));
        let decision = decide_with(&state, &idle());
        assert_eq!(decision.action, Action::Recover);

        let mut exec = FakeExec::new();
        pass_once(
            &[RunEntry::Readable(state)],
            now(),
            &idle(),
            &mut exec,
            MAX_STARTS,
            GRACE_MINUTES,
            false,
            MAX_SPAWN_ATTEMPTS,
            DEADLINE_GRACE_MINUTES,
        );
        let after = crate::state::read_run("p-crash").unwrap();
        assert_eq!(after.incomplete_wakes, 1, "the crash must count against the retry budget");
        assert_eq!(after.status, Status::Sleeping);
        assert!(after.next_wake_at.is_some(), "the retry is armed, not taken immediately");
        assert!(exec.calls.borrow().is_empty(), "nothing is spawned on the pass that records it");
    }

    /// A run that has simply never been woken is not a crash, and must start straight away.
    #[test]
    fn a_run_that_never_had_a_wake_is_spawned_not_recovered() {
        let state = base_state("p-fresh");
        let decision = decide_with(&state, &idle());
        assert_eq!(decision.action, Action::Spawn);
        assert!(decision.reason.contains("stranded"));
    }

    #[test]
    fn a_terminal_run_with_no_session_prints_nothing() {
        let _h = crate::paths::test_support::TempHome::new();
        let mut state = base_state("p-done");
        state.status = Status::Done;
        let mut exec = FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() }); // has-session: no
        let decisions = pass_once(
            &[RunEntry::Readable(state)],
            now(),
            &idle(),
            &mut exec,
            MAX_STARTS,
            GRACE_MINUTES,
            false,
            MAX_SPAWN_ATTEMPTS,
            DEADLINE_GRACE_MINUTES,
        );
        assert_eq!(decisions[0].action, Action::Skip, "nothing to reap is not worth a line");
    }
}
