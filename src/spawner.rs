//! One seam for starting and killing a wake process.
//!
//! Before this module, "how does a wake get backgrounded" had two different answers depending
//! on who was asking, both spelled `Detach::None`: `human::start_wake` read it as "run the wake
//! in this process, blocking", while `wake::spawn_background` read the identical value as "spawn
//! a *detached* process group, logging to `wake.log`". That split had a real consequence: `otto
//! stop` only knew how to kill a tmux session, so a run created with `--detach none` whose wake
//! had been backgrounded by poke kept working — for up to `policy.maxWakeMinutes` — after being
//! retired. Poke's own kill path (by recorded pid and process group) never got reused because
//! nothing else went looking for it.
//!
//! [`Strategy`] gives the three outcomes of "start a wake" three different names, so a caller
//! chooses one on purpose instead of overloading a two-valued preference. [`kill`] is the one
//! path back, used by both `otto stop` and poke's deadline kill.

use crate::error::OttoError;
use crate::exec::Exec;
use crate::liveness::{Liveness, LockLiveness};
use crate::state::RunState;
use crate::wake::{wake, WakeArgs};
use std::time::Duration;

/// How to start one wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Run the wake in this process, blocking until it finishes. What a person gets when they
    /// ask to watch it, or when the run's own preference is `Detach::None` and a human — not
    /// poke — is the one starting it.
    Foreground,
    /// Spawn a detached process group, logging to `wake.log` in the run directory. What poke
    /// uses for a `Detach::None` run: poke runs from launchd every five minutes and can never
    /// block out a 45-minute wake waiting for it to finish.
    Detached,
    /// Spawn inside a named tmux session.
    Tmux,
}

impl Strategy {
    /// How poke starts a wake for a run's recorded `Detach` preference. Poke never watches, so
    /// `Foreground` is not a choice it has — `None` means "detached", not "here".
    pub fn for_poke(detach: crate::state::Detach) -> Self {
        match detach {
            crate::state::Detach::None => Strategy::Detached,
            crate::state::Detach::Tmux => Strategy::Tmux,
        }
    }
}

/// What starting a wake produced, for the caller to report. `description` is `None` for
/// `Foreground`: that path already printed its own "wake complete/incomplete" line, and there is
/// nothing else to say.
#[derive(Debug, Clone)]
pub struct Handle {
    pub description: Option<String>,
    pub session: Option<String>,
}

fn wake_argv(id: &str, answer: Option<&str>) -> Result<Vec<String>, OttoError> {
    let exe = crate::paths::current_exe()?;
    // `--foreground` is not optional. The child is the wake; the container it needs was created
    // here. Without it the child resolves the run's `detach` preference itself, decides it should
    // be backgrounded, and spawns another container — which spawns another. See `wake::WakeArgs`.
    let mut argv: Vec<String> = vec![exe.display().to_string(), "wake".to_string(), id.to_string(), "--foreground".to_string()];
    if let Some(answer) = answer {
        argv.push("--answer".to_string());
        argv.push(answer.to_string());
    }
    Ok(argv)
}

/// Where the wake process starts: the run's working directory, so a wake the reviver starts runs
/// in the same place as one started from a terminal. Without one (a run from before working
/// directories, on a machine with no default), wherever this process is.
fn cwd(id: &str) -> String {
    crate::state::read_run(id)
        .ok()
        .and_then(|state| crate::config::workdir_for(&state))
        .or_else(|| std::env::current_dir().ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| crate::paths::home_dir().display().to_string())
}

/// Start one wake under the given strategy.
pub fn start(id: &str, strategy: Strategy, answer: Option<&str>, exec: &mut dyn Exec) -> Result<Handle, OttoError> {
    match strategy {
        Strategy::Foreground => {
            wake(WakeArgs {
                id: id.to_string(),
                answer: answer.map(str::to_string),
                dry_run: false,
                watch: false,
                detach: None,
                // The caller *is* where this wake runs, so the destination resolution must not
                // happen again inside `wake::run_wake`.
                foreground: true,
            })?;
            Ok(Handle { description: None, session: None })
        }
        Strategy::Tmux => {
            let argv = wake_argv(id, answer)?;
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let session = crate::detach::spawn_detached(exec, id, &refs, &cwd(id))?;
            Ok(Handle {
                description: Some(format!(
                    "wake started in tmux session {session} — `otto attach {}` to watch",
                    crate::paths::short_id(id)
                )),
                session: Some(session),
            })
        }
        Strategy::Detached => {
            let argv = wake_argv(id, answer)?;
            // `process_group(0)` detaches it from this process group, so the wake survives poke
            // exiting seconds later. Output goes to the run directory rather than being discarded:
            // a wake that dies before it can journal anything leaves its reason only here.
            let log_path = crate::paths::run_dir(id)?.join("wake.log");
            let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path)?;
            let errlog = log.try_clone()?;
            use std::os::unix::process::CommandExt;
            std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .current_dir(cwd(id))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::from(log))
                .stderr(std::process::Stdio::from(errlog))
                .process_group(0)
                .spawn()
                .map_err(|e| OttoError::usage(format!("could not start a wake for {id}: {e}")))?;
            Ok(Handle {
                description: Some(format!("detached process (output in {})", log_path.display())),
                session: None,
            })
        }
    }
}

/// What killing a wake actually touched — for the caller to decide whether to say anything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KillOutcome {
    pub killed_process: bool,
    pub killed_session: bool,
}

impl KillOutcome {
    pub fn did_anything(self) -> bool {
        self.killed_process || self.killed_session
    }
}

/// Kill whatever is running this run's wake — by its recorded pid, treated as a process group,
/// and by its tmux session if one exists — regardless of which [`Strategy`] started it. This is
/// the seam that was missing: `otto stop` used to know only the tmux half, so a `Detach::none`
/// run's detached process kept running past retirement. Both `otto stop` and poke's deadline
/// kill go through this now; they differ only in the bookkeeping layered on top (poke also
/// records the wake incomplete, `otto stop` does not — a deliberate stop is not a failure).
pub fn kill(state: &RunState, run_id: &str, exec: &mut dyn Exec) -> KillOutcome {
    let killed_process = if let Some(pid) = state.wake.as_ref().and_then(|w| w.pid) {
        // Negative pid means "the whole process group", which is how the launcher and the model
        // under it go too. A wake is spawned into its own group for exactly this reason.
        let _ = exec.exec(&["kill", "-TERM", &format!("-{pid}")], None, Duration::from_secs(10));
        true
    } else {
        false
    };
    let session = crate::detach::session_name(run_id);
    let killed_session = crate::detach::session_exists(exec, &session);
    if killed_session {
        crate::detach::kill_session(exec, &session);
    }
    KillOutcome { killed_process, killed_session }
}

/// How long to wait for a detached or tmux wake to prove it exists. Generous enough for a cold
/// process start, short enough that a failure is reported while a person is still watching.
pub const CONFIRM_SECONDS: u64 = 15;

pub fn wake_number(id: &str) -> u32 {
    crate::state::read_run(id).ok().and_then(|s| s.wake.map(|w| w.n)).unwrap_or(0)
}

/// Did a wake actually begin, after being backgrounded? `otto wake` records the wake number
/// before spawning the model, and holds the wake lock for its lifetime, so either signal proves
/// a process got that far.
pub fn wait_for_hold(id: &str, before: u32) -> bool {
    for _ in 0..(CONFIRM_SECONDS * 4) {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if wake_number(id) > before || LockLiveness.probe(id).is_busy() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::fake::FakeExec;
    use crate::exec::Output;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;

    /// The recursion guard, and the reason `otto wake` can default to backgrounding itself.
    ///
    /// The child a `Tmux` start spawns resolves the run's `detach` preference for itself, so
    /// without `--foreground` a tmux run would spawn a session whose wake spawns a session whose
    /// wake spawns a session. Silent, unbounded, and it would look like otto working — hence a
    /// test rather than a comment.
    #[test]
    fn a_backgrounded_wake_tells_its_child_to_run_in_place() {
        let _h = TempHome::new();
        test_init("sp-bg", "a goal").unwrap();
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 1, ..Default::default() }); // has-session: nothing there yet
        let handle = start("sp-bg", Strategy::Tmux, Some("go ahead"), &mut exec).unwrap();
        assert!(handle.session.is_some());

        let call = exec
            .calls
            .borrow()
            .iter()
            .find(|c| c.contains(&"new-session".to_string()))
            .cloned()
            .expect("a session must be created");
        assert!(
            call.contains(&"--foreground".to_string()),
            "the child must be told it is already in place, or it backgrounds itself again: {call:?}"
        );
        let wake = call.iter().position(|a| a == "wake").expect("it runs `otto wake`");
        assert_eq!(call[wake + 1], "sp-bg");
        let answer = call.iter().position(|a| a == "--answer").expect("the answer must survive");
        assert_eq!(call[answer + 1], "go ahead");
    }

    /// `otto stop` used to only know how to kill a tmux session — a run backgrounded without
    /// tmux (`Strategy::Detached`, what poke uses for a `Detach::None` run) kept its wake running
    /// past retirement. `kill` has to reach it by pid too.
    #[test]
    fn kill_reaches_a_detached_wake_by_pid_even_with_no_tmux_session() {
        let _h = TempHome::new();
        let mut state = crate::state::test_run_state("sp-kill");
        state.wake = Some(crate::state::Wake {
            n: 1,
            started_at: crate::clock::Timestamp::now(),
            deadline_at: crate::clock::Timestamp::now(),
            launcher: "claude".into(),
            pid: Some(424242),
            session: None,
            outcome: None,
        });
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 0, ..Default::default() }); // the `kill -TERM` call itself
        exec.queue(Output { code: 1, ..Default::default() }); // has-session: no tmux session at all
        let outcome = kill(&state, "sp-kill", &mut exec);
        assert!(outcome.killed_process, "a recorded pid must be killed even without a tmux session");
        assert!(!outcome.killed_session);
        let calls: Vec<String> = exec.calls.borrow().iter().map(|c| c.join(" ")).collect();
        assert!(calls.iter().any(|c| c.contains("kill -TERM -424242")), "got {calls:?}");
    }

    #[test]
    fn kill_with_nothing_recorded_does_nothing() {
        let state = crate::state::test_run_state("sp-idle");
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 1, ..Default::default() }); // has-session: no
        let outcome = kill(&state, "sp-idle", &mut exec);
        assert!(!outcome.did_anything());
    }
}
