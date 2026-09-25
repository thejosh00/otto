//! Is a wake running for this run, right now?
//!
//! v1 inferred this. It read `heartbeatAt` to guess whether a conductor was working,
//! classified a tmux pane as `Live`/`Shell`/`Gone` to guess whether anybody was home, and
//! carried a whole vocabulary — `stuck-alive`, `stalled-busy` — for the cases where a
//! session was alive but useless. All of that existed because a session outlived the work
//! it was doing, so being alive told you almost nothing.
//!
//! A wake is a process. It either holds its lock or it does not. So liveness becomes an
//! observation instead of an inference: `otto wake` takes an exclusive lock on
//! `runs/<id>/wake.lock` and holds it for its entire life; anyone else non-blockingly
//! tries for the same lock. Acquired means nobody is home.
//!
//! Why a lock rather than `wake.pid` and `kill(pid, 0)`:
//!
//! - **Pid reuse.** A stale pid that some unrelated process has since been assigned reads
//!   as "a wake is running" forever, and the run never gets revived.
//! - **No `ps`.** Verifying a pid really belongs to a wake wants process metadata, and
//!   macOS Seatbelt blocks the setuid `/bin/ps` unconditionally — that is precisely what
//!   defeated background sessions inside a sandbox.
//! - **It works across the sandbox boundary.** The lock file lives on the host under
//!   `$OTTO_HOME`, while the model may be running inside nono. `otto poke` runs on the
//!   host and never needs to see into the sandbox to answer the question.
//! - **The kernel cleans up.** A wake that is `kill -9`ed, or whose machine loses power,
//!   releases its flock on process exit with no bookkeeping and nothing to time out.
//!
//! `wake.lock` is deliberately a *different* file from `state`'s `<run>/.lock`. A wake
//! holds this one for minutes while taking the state lock briefly on each write, so they
//! must never be the same flock or a wake would deadlock against its own first write.

use crate::error::OttoError;
use std::path::Path;

pub const WAKE_LOCK_FILE: &str = "wake.lock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeState {
    /// The lock was free: no wake is running for this run.
    Idle,
    /// The lock is held: a wake is working right now.
    Running,
    /// We could not find out. Treated as `Running` by every caller — starting a second
    /// wake on top of a live one is the outcome worth avoiding — but carries the reason so
    /// poke can report it rather than silently doing nothing.
    Unknown(String),
}

impl WakeState {
    /// The conservative reading. `Unknown` counts as busy on purpose.
    pub fn is_busy(&self) -> bool {
        !matches!(self, WakeState::Idle)
    }
}

/// Injected so poke and the repo locks can be tested without spawning processes.
pub trait Liveness {
    fn probe(&self, run_id: &str) -> WakeState;
}

pub struct LockLiveness;

impl Liveness for LockLiveness {
    fn probe(&self, run_id: &str) -> WakeState {
        let dir = match crate::paths::run_dir(run_id) {
            Ok(dir) => dir,
            // No run directory means no wake could be holding anything.
            Err(_) => return WakeState::Idle,
        };
        probe_path(&dir.join(WAKE_LOCK_FILE))
    }
}

fn open_lock_file(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path)
}

/// Try for the lock and immediately give it back. Acquiring it proves nobody held it at
/// that instant, which is all the question asks.
pub fn probe_path(path: &Path) -> WakeState {
    let file = match open_lock_file(path) {
        Ok(file) => file,
        Err(e) => return WakeState::Unknown(format!("cannot open {}: {e}", path.display())),
    };
    match fs4::fs_std::FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            let _ = fs4::fs_std::FileExt::unlock(&file);
            WakeState::Idle
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => WakeState::Running,
        Err(e) => WakeState::Unknown(format!("cannot probe {}: {e}", path.display())),
    }
}

/// Held by `otto wake` for the whole wake. Dropping it — including via a panic, a signal,
/// or the process being killed — releases it, because that is what flock does on close.
#[derive(Debug)]
pub struct WakeLock {
    file: std::fs::File,
}

impl WakeLock {
    /// Exit 2 (a state conflict) when another wake already holds it. Two wakes on one run
    /// would duplicate work and race each other's writes, so this refuses rather than
    /// queues: whatever wanted to start can come back when the current wake has finished.
    pub fn acquire(run_id: &str) -> Result<Self, OttoError> {
        let path = crate::paths::run_dir(run_id)?.join(WAKE_LOCK_FILE);
        let file = open_lock_file(&path)?;
        match fs4::fs_std::FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Self { file }),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Err(OttoError::conflict(format!(
                "a wake is already running for {run_id} (holding {})",
                path.display()
            ))),
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for WakeLock {
    fn drop(&mut self) {
        let _ = fs4::fs_std::FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::HashSet;

    /// Liveness by fiat, for testing poke's decisions and lock breakability.
    pub struct FakeLiveness {
        running: HashSet<String>,
        unknown: HashSet<String>,
    }

    impl FakeLiveness {
        pub fn new() -> Self {
            Self {
                running: HashSet::new(),
                unknown: HashSet::new(),
            }
        }

        pub fn with_running(runs: &[&str]) -> Self {
            let mut it = Self::new();
            for run in runs {
                it.running.insert(run.to_string());
            }
            it
        }

        pub fn set_unknown(&mut self, run_id: &str) -> &mut Self {
            self.unknown.insert(run_id.to_string());
            self
        }
    }

    impl Liveness for FakeLiveness {
        fn probe(&self, run_id: &str) -> WakeState {
            if self.unknown.contains(run_id) {
                return WakeState::Unknown("fake".to_string());
            }
            if self.running.contains(run_id) {
                return WakeState::Running;
            }
            WakeState::Idle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;

    #[test]
    fn an_unlocked_run_reads_as_idle() {
        let _home = TempHome::new();
        test_init("run-idle", "hello").unwrap();
        assert_eq!(LockLiveness.probe("run-idle"), WakeState::Idle);
    }

    #[test]
    fn a_held_lock_reads_as_running() {
        let _home = TempHome::new();
        test_init("run-busy", "hello").unwrap();
        let held = WakeLock::acquire("run-busy").expect("first acquire");
        assert_eq!(LockLiveness.probe("run-busy"), WakeState::Running);
        drop(held);
        assert_eq!(LockLiveness.probe("run-busy"), WakeState::Idle);
    }

    /// The property that makes this worth using: releasing needs no bookkeeping, so a
    /// wake that dies without cleaning up does not strand the run.
    #[test]
    fn dropping_the_lock_releases_it_with_no_cleanup_step() {
        let _home = TempHome::new();
        test_init("run-drop", "hello").unwrap();
        {
            let _held = WakeLock::acquire("run-drop").unwrap();
            assert!(LockLiveness.probe("run-drop").is_busy());
        }
        assert_eq!(LockLiveness.probe("run-drop"), WakeState::Idle);
    }

    #[test]
    fn a_second_wake_is_refused_with_a_state_conflict() {
        let _home = TempHome::new();
        test_init("run-double", "hello").unwrap();
        let _held = WakeLock::acquire("run-double").unwrap();
        let err = WakeLock::acquire("run-double").expect_err("second acquire must be refused");
        assert_eq!(err.code, 2, "a contended wake lock is a state conflict");
        assert!(err.to_string().contains("already running"));
    }

    #[test]
    fn a_missing_run_reads_as_idle_rather_than_erroring() {
        let _home = TempHome::new();
        assert_eq!(LockLiveness.probe("no-such-run"), WakeState::Idle);
    }

    #[test]
    fn unknown_counts_as_busy_so_we_never_double_start() {
        assert!(WakeState::Unknown("whatever".to_string()).is_busy());
        assert!(WakeState::Running.is_busy());
        assert!(!WakeState::Idle.is_busy());
    }

    /// The wake lock and the state lock must be separate files, or a wake holding the
    /// former would deadlock against its own first state write.
    #[test]
    fn the_wake_lock_is_not_the_state_lock() {
        let _home = TempHome::new();
        test_init("run-sep", "hello").unwrap();
        let _held = WakeLock::acquire("run-sep").unwrap();
        // A state write must still succeed while the wake lock is held.
        crate::state::commands::record_fact(crate::state::commands::RecordFactArgs {
            id: "run-sep".to_string(),
            pairs: vec!["probe=ok".to_string()],
        })
        .expect("state writes must not block on the wake lock");
        assert_ne!(WAKE_LOCK_FILE, ".lock");
    }
}
