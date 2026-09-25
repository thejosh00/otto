//! Backgrounding a wake, and nothing else.
//!
//! This is the whole of what otto needs from tmux now: start a detached session running a
//! command, ask whether one exists, kill one. No pane reading, no composer typing, no waiting
//! for a TUI to paint, no classifying a pane as `Live` or `Shell`.
//!
//! v1 needed all of that because a session *was* the run: a prompt had to be typed into a live
//! Claude composer, and liveness had to be inferred from what the pane appeared to be doing. A
//! wake is a process that exits on its own, so the session's only job is to hold it somewhere a
//! human can look. When the wake exits, the command exits, and the session goes away by itself —
//! reaping is not something anyone has to arrange.
//!
//! Which makes detachment **observability, not architecture**. `Detach::None` runs the wake in
//! the foreground and is exactly as correct; the run does not know or care which was used.
//!
//! There is deliberately no `bg` strategy. `claude --bg` is disabled under yolo
//! (`AGENT_VIEW_ENABLED = False`) because `claude attach` on a re-adopted worker needs a
//! process-identity probe that execs the setuid `/bin/ps`, which macOS Seatbelt blocks
//! unconditionally. A strategy that cannot work under the sandbox this is meant to run in is not
//! a strategy.

use crate::error::OttoError;
use crate::exec::{Exec, Output};
use std::time::Duration;

/// tmux calls are local and instant; anything slower than this is broken, not busy.
const TMUX_TIMEOUT: Duration = Duration::from_secs(20);

/// Tags that let a session manager list an otto wake beside every other Claude session. Kept
/// from v1, where they were the contract that replaced coupling to a particular manager's
/// config. Nothing in otto reads `@cc_*` — they exist for whatever tool a human watches — and
/// `@otto_run` is how a reaper with no memory maps a session back to a run.
const TAGS: [(&str, &str); 2] = [("@cc_session", "1"), ("@cc_owner", "otto")];

/// The session a run's wake lives in. Derived, never stored: a name that must be looked up is a
/// name that can go stale.
pub fn session_name(run_id: &str) -> String {
    format!("otto-{run_id}")
}

fn tmux(exec: &mut dyn Exec, argv: &[&str]) -> Output {
    exec.exec(argv, None, TMUX_TIMEOUT)
}

pub fn session_exists(exec: &mut dyn Exec, name: &str) -> bool {
    tmux(exec, &["tmux", "has-session", "-t", name]).code == 0
}

/// What a session's pane shows right now, colour escapes included — a read-only `attach` for a
/// caller with no terminal to attach. `None` when there is no such session.
pub fn capture_pane(exec: &mut dyn Exec, name: &str) -> Option<String> {
    let out = tmux(exec, &["tmux", "capture-pane", "-p", "-e", "-J", "-t", name]);
    (out.code == 0).then_some(out.stdout)
}

pub fn kill_session(exec: &mut dyn Exec, name: &str) -> bool {
    tmux(exec, &["tmux", "kill-session", "-t", name]).code == 0
}

/// Environment a detached wake cannot do without.
///
/// A tmux session inherits the environment of the **tmux server**, not of the client that ran
/// `new-session` — and that server may have been started hours ago from an unrelated shell. So a
/// wake detached without this loses `$OTTO_HOME` and goes looking for the run under `~/.otto`,
/// where it does not exist; the session then dies in under a second with nothing written
/// anywhere. Found exactly that way.
fn inherited_env() -> Vec<(String, String)> {
    let mut env = Vec::new();
    for key in ["OTTO_HOME", "PATH"] {
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                env.push((key.to_string(), value));
            }
        }
    }
    env
}

/// Start `argv` in a detached tmux session. The session ends when the command does, so a
/// finished wake leaves nothing behind to clean up.
pub fn spawn_detached(exec: &mut dyn Exec, run_id: &str, argv: &[&str], cwd: &str) -> Result<String, OttoError> {
    let name = session_name(run_id);
    if session_exists(exec, &name) {
        return Err(OttoError::conflict(format!(
            "tmux session {name} already exists — a wake may be running; `otto attach {}` to look",
            crate::paths::short_id(run_id)
        )));
    }
    let env = inherited_env();
    let pairs: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let mut call: Vec<&str> = vec!["tmux", "new-session", "-d", "-s", &name, "-c", cwd];
    for pair in &pairs {
        call.push("-e");
        call.push(pair);
    }
    call.extend_from_slice(argv);
    let out = tmux(exec, &call);
    if !out.ok() {
        return Err(OttoError::usage(format!(
            "could not start tmux session {name}: {}",
            crate::exec::truncate(out.merged().trim(), 300)
        )));
    }
    for (key, value) in TAGS {
        tmux(exec, &["tmux", "set-option", "-t", &name, key, value]);
    }
    tmux(exec, &["tmux", "set-option", "-t", &name, "@otto_run", run_id]);
    tmux(exec, &["tmux", "set-option", "-t", &name, "@cc_label", &format!("otto · {run_id}")]);
    Ok(name)
}

/// Hand the terminal to tmux. This **replaces the current process** rather than spawning a
/// child: attaching needs the real TTY, and a child holding an inherited one behaves badly on
/// detach. Only returns on failure.
pub fn attach(run_id: &str) -> Result<(), OttoError> {
    use std::os::unix::process::CommandExt;
    let name = session_name(run_id);
    let mut exec = crate::exec::RealExec;
    if !session_exists(&mut exec, &name) {
        let short = crate::paths::short_id(run_id);
        return Err(OttoError::not_found(format!(
            "no live session for {run_id} — a wake is not running right now. \
             `otto show {short}` for where it stands, `otto logs {short}` for what it has done"
        )));
    }
    let err = std::process::Command::new("tmux").args(["attach", "-t", &name]).exec();
    Err(OttoError::usage(format!("could not attach to {name}: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::fake::FakeExec;

    #[test]
    fn the_session_name_is_derived_from_the_run_id() {
        assert_eq!(session_name("2026-09-12-thing"), "otto-2026-09-12-thing");
    }

    #[test]
    fn spawning_runs_the_command_detached_and_tags_the_session() {
        let mut exec = FakeExec::new();
        // has-session must report "no session" first, so queue a non-zero exit for it.
        exec.queue(Output { code: 1, ..Default::default() });
        let name = spawn_detached(&mut exec, "r1", &["otto", "wake", "r1"], "/tmp").unwrap();
        assert_eq!(name, "otto-r1");
        let calls = exec.calls.borrow().clone();
        let new_session = calls.iter().find(|c| c.contains(&"new-session".to_string())).expect("must create");
        assert!(new_session.contains(&"-d".to_string()), "the session must be detached");
        assert_eq!(new_session.last().unwrap(), "r1");
        assert!(new_session.windows(3).any(|w| w == ["otto", "wake", "r1"]));
        let joined: Vec<String> = calls.iter().map(|c| c.join(" ")).collect();
        let all = joined.join("\n");
        assert!(all.contains("@otto_run r1"), "a reaper needs to map a session to its run");
        assert!(all.contains("@cc_session 1"), "a session manager needs to see it");
    }

    /// Regression: a tmux session inherits the *server's* environment, so `$OTTO_HOME` has to be
    /// handed over explicitly. Without this the wake looked for the run under `~/.otto`, found
    /// nothing, exited in under a second and took its session with it — reporting success.
    #[test]
    fn the_session_is_given_otto_home_explicitly() {
        let _home = crate::paths::test_support::TempHome::new();
        let expected = std::env::var("OTTO_HOME").expect("TempHome sets it");
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 1, ..Default::default() });
        spawn_detached(&mut exec, "r1", &["otto", "wake", "r1"], "/tmp").unwrap();
        let new_session = exec
            .calls
            .borrow()
            .iter()
            .find(|c| c.contains(&"new-session".to_string()))
            .cloned()
            .expect("must create");
        let passed: Vec<&String> = new_session
            .iter()
            .zip(new_session.iter().skip(1))
            .filter(|(flag, _)| *flag == "-e")
            .map(|(_, value)| value)
            .collect();
        assert!(
            passed.iter().any(|v| *v == &format!("OTTO_HOME={expected}")),
            "OTTO_HOME must be handed to the session; got {passed:?}"
        );
        assert!(
            passed.iter().any(|v| v.starts_with("PATH=")),
            "PATH too, or `claude` may not resolve inside the session"
        );
        // The env must be set up before the command, not after it.
        let cmd = new_session.iter().position(|a| a == "otto").unwrap();
        let last_e = new_session.iter().rposition(|a| a == "-e").unwrap();
        assert!(last_e < cmd, "-e pairs belong before the command");
    }

    #[test]
    fn spawning_refuses_when_a_session_is_already_there() {
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 0, ..Default::default() }); // has-session says yes
        let err = spawn_detached(&mut exec, "r1", &["otto", "wake", "r1"], "/tmp").expect_err("must refuse");
        assert_eq!(err.code, 2);
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn a_tmux_failure_is_reported_not_swallowed() {
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 1, ..Default::default() });
        exec.queue(Output {
            code: 1,
            stderr: "no server running".into(),
            ..Default::default()
        });
        let err = spawn_detached(&mut exec, "r1", &["otto", "wake", "r1"], "/tmp").expect_err("must fail");
        assert!(err.to_string().contains("no server running"));
    }

    #[test]
    fn existence_and_kill_map_onto_the_obvious_tmux_calls() {
        let mut exec = FakeExec::new();
        exec.queue(Output { code: 0, ..Default::default() });
        assert!(session_exists(&mut exec, "otto-r1"));
        assert_eq!(exec.last_call(), vec!["tmux", "has-session", "-t", "otto-r1"]);
        exec.queue(Output { code: 0, ..Default::default() });
        assert!(kill_session(&mut exec, "otto-r1"));
        assert_eq!(exec.last_call(), vec!["tmux", "kill-session", "-t", "otto-r1"]);
    }
}
