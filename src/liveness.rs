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

use crate::clock::Timestamp;
use crate::error::OttoError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const WAKE_LOCK_FILE: &str = "wake.lock";

/// Beside the lock: who took it last. flock can't say who holds it, and a kill has to.
pub const HOLDER_FILE: &str = "wake-holder.json";

/// What whoever took the wake lock wrote when it did. It outlives them: nothing removes it, a
/// `kill -9` couldn't anyway, and the next holder overwrites it. So it says who took the lock
/// *last*, which is not always who holds it now, and a reader must compare it with the wake it
/// cares about (`event::KillHolder::of`). A file of its own, not a field in `run.json`, which an
/// older binary would drop on its next write; an older binary simply never opens this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    /// The wake's number, once it has read the state. Absent in the moment before, and for good
    /// when what took the lock isn't a wake, or stopped short of being one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake: Option<u32>,
    /// When the lock was taken.
    pub at: Timestamp,
}

/// `None` when there is no record, or none that can be read: both mean nobody can say.
pub fn read_holder(run_id: &str) -> Option<Holder> {
    let path = crate::paths::run_dir(run_id).ok()?.join(HOLDER_FILE);
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Evidence only, so failing to write it never stops a wake: a kill then finds an older record, or
/// none, and says so.
fn write_holder(path: &Path, holder: &Holder) {
    if let Ok(text) = serde_json::to_string(holder) {
        let _ = crate::state::write_atomic(path, &text);
    }
}

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
/// that instant, which is all the question asks. Shared, because only a wake holds it
/// exclusively: two probes at once — the web UI asks for several lists together — must not
/// each read the other as a wake, which showed idle runs as working for a moment.
pub fn probe_path(path: &Path) -> WakeState {
    let file = match open_lock_file(path) {
        Ok(file) => file,
        Err(e) => return WakeState::Unknown(format!("cannot open {}: {e}", path.display())),
    };
    match fs4::fs_std::FileExt::try_lock_shared(&file) {
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
    holder_path: PathBuf,
    holder: Holder,
}

impl WakeLock {
    /// Exit 2 (a state conflict) when another wake already holds it. Two wakes on one run
    /// would duplicate work and race each other's writes, so this refuses rather than
    /// queues: whatever wanted to start can come back when the current wake has finished.
    /// A probe holds it for an instant, so a few quick retries tell a probe from a wake.
    ///
    /// Taking it says so on disk at once, in the record beside it. Only once it is really held, so
    /// a refused wake leaves the holder's record alone. The state hasn't been read yet, so a wake's
    /// number comes after, from [`Self::record_wake`].
    pub fn acquire(run_id: &str) -> Result<Self, OttoError> {
        let dir = crate::paths::run_dir(run_id)?;
        let path = dir.join(WAKE_LOCK_FILE);
        let file = open_lock_file(&path)?;
        let mut tries = 0;
        loop {
            match fs4::fs_std::FileExt::try_lock_exclusive(&file) {
                Ok(()) => {
                    let holder = Holder { pid: std::process::id(), wake: None, at: Timestamp::now() };
                    let holder_path = dir.join(HOLDER_FILE);
                    write_holder(&holder_path, &holder);
                    return Ok(Self { file, holder_path, holder });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && tries < 5 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(OttoError::conflict(format!(
                        "a wake is already running for {run_id} (holding {})",
                        path.display()
                    )))
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

impl WakeLock {
    /// The holder is this wake, now its number is known. Before `run.json` names the wake, so that
    /// a kill acting on that wake finds the number on its record.
    pub fn record_wake(&mut self, n: u32) {
        self.holder.wake = Some(n);
        write_holder(&self.holder_path, &self.holder);
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

    /// The pid and time are there the moment the lock is, before any state is read: the gap until
    /// the number arrives is the one a kill most needs explaining. The time stays the lock's.
    #[test]
    fn taking_the_lock_records_the_holder_at_once_and_its_number_after() {
        let _home = TempHome::new();
        test_init("run-holder", "hello").unwrap();
        assert_eq!(read_holder("run-holder"), None);

        let mut held = WakeLock::acquire("run-holder").unwrap();
        let first = read_holder("run-holder").expect("on disk as soon as the lock is taken");
        assert_eq!(first.pid, std::process::id());
        assert_eq!(first.wake, None);

        held.record_wake(7);
        let second = read_holder("run-holder").unwrap();
        assert_eq!(second, Holder { wake: Some(7), ..first });
    }

    /// It outlives its wake, and the next holder replaces it. Being refused replaces nothing.
    #[test]
    fn the_record_outlives_the_holder_and_only_a_holder_replaces_it() {
        let _home = TempHome::new();
        test_init("run-after", "hello").unwrap();
        let mut held = WakeLock::acquire("run-after").unwrap();
        held.record_wake(3);
        assert!(WakeLock::acquire("run-after").is_err());
        assert_eq!(read_holder("run-after").unwrap().wake, Some(3), "refused, so it isn't the holder");
        drop(held);
        assert_eq!(read_holder("run-after").unwrap().wake, Some(3), "left behind, as a kill -9 would");

        let _next = WakeLock::acquire("run-after").unwrap();
        assert_eq!(read_holder("run-after").unwrap().wake, None);
    }

    /// Half a file, or a future shape, is not a holder anyone can name.
    #[test]
    fn a_record_that_cannot_be_read_is_no_record() {
        let _home = TempHome::new();
        test_init("run-garbled", "hello").unwrap();
        std::fs::write(crate::paths::run_dir("run-garbled").unwrap().join(HOLDER_FILE), "{\"pid\":").unwrap();
        assert_eq!(read_holder("run-garbled"), None);
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

    /// Several lists asked for at once probe the same run at the same instant; a probe that
    /// read another probe as a wake showed an idle run as working.
    #[test]
    fn a_probe_mid_flight_is_not_a_wake() {
        let _home = TempHome::new();
        test_init("run-probes", "hello").unwrap();
        let path = crate::paths::run_dir("run-probes").unwrap().join(WAKE_LOCK_FILE);
        let other_probe = open_lock_file(&path).unwrap();
        fs4::fs_std::FileExt::try_lock_shared(&other_probe).unwrap();
        assert_eq!(LockLiveness.probe("run-probes"), WakeState::Idle);
        // Nor does a wake starting in that instant give up.
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(15));
            drop(other_probe);
        });
        let held = WakeLock::acquire("run-probes").expect("a probe is not a wake");
        release.join().unwrap();
        assert_eq!(LockLiveness.probe("run-probes"), WakeState::Running);
        drop(held);
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
