//! The contract, and the only thing otto mechanically enforces.
//!
//! otto validates neither phases nor plans nor progress: with an arbitrary wrapped skill there
//! is no table to validate against, so any such check would only work on things written for
//! otto and the premise — wrap anything — would be false. What can be checked against
//! anything is whether the wake left the run in a state that can be picked up cold. Two
//! questions, asked once, at process exit:
//!
//! 1. Is the run in a state something will bring it back from?
//! 2. Did this wake write the handoff the next one will read?
//!
//! Deliberately two. A third — "did it make progress", "was the work correct" — is not
//! answerable from outside the work.
//!
//! **This runs whether or not the child exited cleanly**, because it is the parent's job. A
//! wake that segfaulted, was killed at its deadline, or simply stopped talking all fail check
//! 1 or 2 in the same way and become the same retry. That is what makes v1's worst failure —
//! a conductor ending its turn `running` with no gate and no wake time, stranded but still
//! reporting `running`, invisible until somebody noticed a day later — unrepresentable rather
//! than merely discouraged.

use crate::state::{RunState, Status};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    Incomplete(String),
}

// Read by the tests, and by anything that wants the verdict without matching on the enum.
#[cfg_attr(not(test), allow(dead_code))]
impl Outcome {
    pub fn is_complete(&self) -> bool {
        matches!(self, Outcome::Complete)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Outcome::Complete => None,
            Outcome::Incomplete(why) => Some(why),
        }
    }
}

/// Is this a state something will come back from? A gate means a person can answer; a wake time
/// means poke will return; terminal means the run is over and needs nothing. Anything else —
/// `running` with neither, or `blocked` with no gate to unblock it — is a stranded run.
pub fn stopping_state_is_legal(state: &RunState) -> Result<(), String> {
    match state.status {
        Status::Done | Status::Failed | Status::Stopped => Ok(()),
        Status::AwaitingHuman => {
            if state.gate.is_some() {
                Ok(())
            } else {
                Err("status is awaiting_human but no gate is open, so nobody has been asked anything".into())
            }
        }
        Status::Blocked => {
            if state.gate.is_some() {
                Ok(())
            } else {
                Err("status is blocked with no gate open, so nothing can unblock it".into())
            }
        }
        Status::Sleeping => match &state.next_wake_at {
            Some(_) => Ok(()),
            None => Err("status is sleeping but nextWakeAt is unset, so no wake will ever come".into()),
        },
        Status::Running => Err(
            "ended still running, with no gate open and no nextWakeAt — nothing would ever \
             bring this run back"
                .into(),
        ),
    }
}

/// Did *this* wake write the handoff? An untouched file from three wakes ago is worse than a
/// missing one: it reads as current and quietly sends the next wake back to stale instructions.
/// Compared by mtime against when the wake started, so rewriting identical content still counts.
fn handoff_is_fresh(run_path: &Path, started_at: std::time::SystemTime, cap: usize) -> Result<(), String> {
    let path = run_path.join(crate::state::commands::HANDOFF_FILE);
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(_) => return Err(format!("{} was not written", crate::state::commands::HANDOFF_FILE)),
    };
    let modified = meta.modified().map_err(|e| format!("cannot read handoff mtime: {e}"))?;
    // Second-granularity filesystems can report a wake's own write as marginally older than
    // its start, so allow a small tolerance rather than failing a wake that did the right thing.
    let tolerance = std::time::Duration::from_secs(2);
    if modified + tolerance < started_at {
        return Err(format!(
            "{} was not rewritten by this wake — it still dates from before the wake started",
            crate::state::commands::HANDOFF_FILE
        ));
    }
    let len = meta.len() as usize;
    if cap > 0 && len > cap {
        return Err(format!(
            "{} is {len} bytes, over the {cap}-byte cap",
            crate::state::commands::HANDOFF_FILE
        ));
    }
    if len == 0 {
        return Err(format!("{} is empty", crate::state::commands::HANDOFF_FILE));
    }
    Ok(())
}

/// The whole contract. `started_at` is when this wake began, for the freshness comparison.
pub fn validate(run_path: &Path, state: &RunState, started_at: std::time::SystemTime) -> Outcome {
    if let Err(why) = stopping_state_is_legal(state) {
        return Outcome::Incomplete(why);
    }
    let cap = state.policy.handoff_max_bytes as usize;
    if let Err(why) = handoff_is_fresh(run_path, started_at, cap) {
        return Outcome::Incomplete(why);
    }
    Outcome::Complete
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::{test_init, HANDOFF_FILE};
    use crate::state::read_run;
    use std::time::{Duration, SystemTime};

    fn run(id: &str) -> (std::path::PathBuf, RunState) {
        test_init(id, "a goal").unwrap();
        let path = crate::paths::run_dir(id).unwrap();
        let state = read_run(id).unwrap();
        (path, state)
    }

    fn write_handoff(path: &Path, body: &str) {
        std::fs::write(path.join(HANDOFF_FILE), body).unwrap();
    }

    fn just_now() -> SystemTime {
        SystemTime::now() - Duration::from_secs(1)
    }

    // --- check 1: the stopping state ---

    #[test]
    fn running_with_nothing_pending_is_the_stranded_run() {
        let (_p, state) = { let _h = TempHome::new(); run("c-running") };
        let err = stopping_state_is_legal(&state).expect_err("running must be illegal");
        assert!(err.contains("nothing would ever bring this run back"));
    }

    #[test]
    fn sleeping_needs_a_wake_time() {
        let _h = TempHome::new();
        let (_p, mut state) = run("c-sleep");
        state.status = Status::Sleeping;
        assert!(stopping_state_is_legal(&state).is_err(), "sleeping with no nextWakeAt is stranded");
        state.next_wake_at = Some(crate::clock::Timestamp::parse("2026-09-13T00:00:00Z").unwrap());
        assert!(stopping_state_is_legal(&state).is_ok());
    }

    #[test]
    fn awaiting_human_needs_an_open_gate() {
        let _h = TempHome::new();
        let (_p, mut state) = run("c-await");
        state.status = Status::AwaitingHuman;
        assert!(
            stopping_state_is_legal(&state).is_err(),
            "claiming to await a human without asking anything is stranded"
        );
    }

    /// `blocked` is the subtle one: it sounds terminal but nothing clears it on its own.
    #[test]
    fn blocked_without_a_gate_is_stranded() {
        let _h = TempHome::new();
        let (_p, mut state) = run("c-blocked");
        state.status = Status::Blocked;
        let err = stopping_state_is_legal(&state).expect_err("blocked with no gate must be illegal");
        assert!(err.contains("nothing can unblock it"));
    }

    #[test]
    fn terminal_states_need_nothing_pending() {
        let _h = TempHome::new();
        let (_p, mut state) = run("c-term");
        for status in [Status::Done, Status::Failed, Status::Stopped] {
            state.status = status;
            state.gate = None;
            state.next_wake_at = None;
            assert!(stopping_state_is_legal(&state).is_ok(), "{status:?} is a legal end");
        }
    }

    // --- check 2: the handoff, and the pair together ---

    #[test]
    fn a_wake_that_stopped_legally_and_wrote_a_handoff_is_complete() {
        let _h = TempHome::new();
        let (path, mut state) = run("c-ok");
        state.status = Status::Sleeping;
        state.next_wake_at = Some(crate::clock::Timestamp::parse("2026-09-13T00:00:00Z").unwrap());
        write_handoff(&path, "# c-ok\n## Next wake must\nCarry on.\n");
        assert_eq!(validate(&path, &state, just_now()), Outcome::Complete);
    }

    #[test]
    fn a_missing_handoff_is_incomplete_even_when_the_state_is_legal() {
        let _h = TempHome::new();
        let (path, mut state) = run("c-nohandoff");
        state.status = Status::Done;
        std::fs::remove_file(path.join(HANDOFF_FILE)).unwrap();
        let outcome = validate(&path, &state, just_now());
        assert!(outcome.reason().unwrap().contains("was not written"));
    }

    /// The case worth having a test for: `init` wrote a handoff, so the file exists, but this
    /// wake never touched it. Stale-but-present is more dangerous than absent.
    #[test]
    fn a_handoff_left_over_from_a_previous_wake_does_not_count() {
        let _h = TempHome::new();
        let (path, mut state) = run("c-stale");
        state.status = Status::Done;
        // The wake started well after init wrote its handoff.
        let started = SystemTime::now() + Duration::from_secs(600);
        let outcome = validate(&path, &state, started);
        assert!(
            outcome.reason().unwrap().contains("not rewritten by this wake"),
            "got: {:?}",
            outcome.reason()
        );
    }

    #[test]
    fn an_oversized_handoff_is_incomplete() {
        let _h = TempHome::new();
        let (path, mut state) = run("c-fat");
        state.status = Status::Done;
        state.policy.handoff_max_bytes = 64;
        write_handoff(&path, &"x".repeat(200));
        assert!(validate(&path, &state, just_now()).reason().unwrap().contains("over the 64-byte cap"));
    }

    #[test]
    fn an_empty_handoff_is_incomplete() {
        let _h = TempHome::new();
        let (path, mut state) = run("c-empty");
        state.status = Status::Done;
        write_handoff(&path, "");
        assert!(validate(&path, &state, just_now()).reason().unwrap().contains("is empty"));
    }

    /// A killed or crashed wake reaches the validator by the same path as any other, because
    /// validating is the parent's job — so it fails on whichever check it actually failed,
    /// with no special casing for how the child died.
    #[test]
    fn a_killed_wake_fails_on_the_state_it_left_behind() {
        let _h = TempHome::new();
        let (path, state) = run("c-killed");
        // Killed mid-work: still `running`, handoff never rewritten.
        let outcome = validate(&path, &state, just_now());
        assert!(!outcome.is_complete());
        assert!(outcome.reason().unwrap().contains("still running"));
    }

    #[test]
    fn the_state_check_runs_before_the_handoff_check() {
        let _h = TempHome::new();
        let (path, state) = run("c-order");
        std::fs::remove_file(path.join(HANDOFF_FILE)).unwrap();
        // Both checks fail; the more fundamental one should be reported.
        let outcome = validate(&path, &state, just_now());
        assert!(outcome.reason().unwrap().contains("still running"));
    }
}
