//! Spawning a child process and getting its output back, with a deadline.
//!
//! This exists separately from `tmux::Runner` for one reason that matters: `Runner`
//! returns stdout and stderr **merged** into a single string, which is fine for `tmux
//! list-sessions` and wrong for a wake. A wake's stdout is its own narration, kept for the
//! failure dump a wake that broke its contract leaves behind, while its launcher may write
//! progress to stderr — a sandbox wrapper typically does, on every invocation. Merging them means every such dump
//! opens with the sandbox's chatter interleaved through the only account of what went wrong.
//! Here they stay apart.
//!
//! Piping stdout is also what keeps a wake non-interactive now that otto does not pass `-p`:
//! with no TTY and stdin at `/dev/null`, claude runs its prompt and exits by itself.
//!
//! The other difference is the deadline. `Runner` hard-codes 60 seconds, which is right
//! for a tmux call and absurd for a wake that may legitimately work for 45 minutes. Here
//! the timeout is per-call, because `wake.deadlineAt` is per-run policy.
//!
//! `tmux::Runner` is deliberately left alone for now; it goes away with the pane code it
//! serves, and the two surviving tmux calls move onto this trait then.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// What a child process left behind. `timed_out` is separate from a non-zero `code`
/// because they mean different things to a caller: a process that exceeded its deadline
/// was killed mid-work and its output is a fragment, while one that exited non-zero
/// finished and had something to say about it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl Output {
    /// For callers that genuinely don't care which stream said what — a tmux probe, a
    /// `launchctl` error message. Never use this on a wake.
    pub fn merged(&self) -> String {
        let mut out = self.stdout.clone();
        out.push_str(&self.stderr);
        out
    }

    pub fn ok(&self) -> bool {
        self.code == 0 && !self.timed_out
    }
}

/// Cut a string to `n` characters, for putting a subprocess's complaint into an error message
/// without pasting a screenful. Char-wise, not byte-wise, so it cannot split a UTF-8 sequence.
pub fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Injected so the wake executor can be tested without spawning anything. The `env`
/// argument adds to the child's environment rather than replacing it.
pub trait Exec {
    fn exec(&mut self, argv: &[&str], env: Option<&HashMap<String, String>>, timeout: Duration) -> Output;

    /// `exec`, with the child in a process group of its own, so a timeout kills everything it
    /// started and not just the child. For untrusted, short-lived commands — a check script
    /// whose `curl` hangs — where a survivor holding the pipe would stall the caller.
    ///
    /// Not the default, and never for a wake: `otto stop` and poke's deadline kill signal the
    /// `otto wake` process group (`spawner::kill`), and that only reaches the model because it
    /// shares the group. Isolating it would orphan the model on every stop.
    fn exec_isolated(&mut self, argv: &[&str], env: Option<&HashMap<String, String>>, timeout: Duration) -> Output {
        self.exec(argv, env, timeout)
    }
}

pub struct RealExec;

impl RealExec {
    fn command(argv: &[&str], env: Option<&HashMap<String, String>>) -> std::process::Command {
        let mut cmd = std::process::Command::new(argv[0]);
        cmd.args(&argv[1..]);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        if let Some(vars) = env {
            for (key, value) in vars {
                cmd.env(key, value);
            }
        }
        cmd
    }
}

fn empty_argv() -> Output {
    Output {
        code: 127,
        stderr: "exec: empty argv".to_string(),
        ..Default::default()
    }
}

impl Exec for RealExec {
    fn exec(&mut self, argv: &[&str], env: Option<&HashMap<String, String>>, timeout: Duration) -> Output {
        if argv.is_empty() {
            return empty_argv();
        }
        spawn_with_deadline(Self::command(argv, env), timeout, false)
    }

    fn exec_isolated(&mut self, argv: &[&str], env: Option<&HashMap<String, String>>, timeout: Duration) -> Output {
        if argv.is_empty() {
            return empty_argv();
        }
        use std::os::unix::process::CommandExt;
        let mut cmd = Self::command(argv, env);
        cmd.process_group(0);
        spawn_with_deadline(cmd, timeout, true)
    }
}

/// How long to keep reading after the child is gone. EOF normally arrives at once; when it
/// doesn't, something the child started is still holding the pipe, and the caller should not
/// wait on a process it never ran.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

enum Chunk {
    Out(Vec<u8>),
    Err(Vec<u8>),
    Eof,
}

/// Read one pipe to EOF on its own thread, sending what it reads as it goes, so that whatever
/// arrived is kept even if the reader is abandoned mid-pipe.
fn pump(pipe: Option<impl std::io::Read + Send + 'static>, tx: std::sync::mpsc::Sender<Chunk>, wrap: fn(Vec<u8>) -> Chunk) {
    std::thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            let mut buf = [0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(wrap(buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                }
            }
        }
        let _ = tx.send(Chunk::Eof);
    });
}

/// Read both pipes on their own threads, because a child that fills one pipe's buffer
/// while the parent is blocked reading the other deadlocks. A wake writing a large JSON
/// result to stdout and a launcher chattering on stderr is exactly that shape.
///
/// Reading stops at EOF on both pipes or `DRAIN_GRACE` after the child is gone, whichever is
/// first — a grandchild that inherited a pipe would otherwise hold the caller until *it* exits.
/// With `own_group`, a timeout kills the whole group, so that grandchild dies too; without it,
/// the reader is abandoned, and it ends with the process.
fn spawn_with_deadline(mut cmd: std::process::Command, timeout: Duration, own_group: bool) -> Output {
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return Output {
                code: 127,
                stderr: e.to_string(),
                ..Default::default()
            }
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    pump(child.stdout.take(), tx.clone(), Chunk::Out);
    pump(child.stderr.take(), tx, Chunk::Err);

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    if own_group {
                        // The child leads its group, so its pid is the group id. Negative means
                        // the whole group — the same convention `spawner::kill` uses.
                        let _ = std::process::Command::new("kill")
                            .args(["-KILL", "--", &format!("-{}", child.id())])
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status();
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };

    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let drain_until = Instant::now() + DRAIN_GRACE;
    let mut open = 2;
    while open > 0 {
        let left = drain_until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Chunk::Out(bytes)) => stdout.extend(bytes),
            Ok(Chunk::Err(bytes)) => stderr.extend(bytes),
            Ok(Chunk::Eof) => open -= 1,
            Err(_) => break,
        }
    }
    Output {
        // 124 is what `timeout(1)` uses, and a killed wake needs a code that cannot be
        // confused with the child's own.
        code: match status {
            Some(status) => status.code().unwrap_or(1),
            None if timed_out => 124,
            None => 1,
        },
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        timed_out,
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::cell::RefCell;

    /// Records every argv it was handed and returns queued outputs in order. Once the
    /// queue is empty it returns a clean zero-exit with empty streams, so a test only has
    /// to describe the calls it cares about.
    pub struct FakeExec {
        pub calls: RefCell<Vec<Vec<String>>>,
        pub timeouts: RefCell<Vec<Duration>>,
        queued: RefCell<std::collections::VecDeque<Output>>,
        /// Runs while the fake "child" is running. A real wake writes to the run directory
        /// during its spawn, not before it, and tests that pretend otherwise miss anything
        /// the executor legitimately does at wake start.
        side_effect: Option<Box<dyn Fn()>>,
    }

    impl FakeExec {
        pub fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                timeouts: RefCell::new(Vec::new()),
                queued: RefCell::new(std::collections::VecDeque::new()),
                side_effect: None,
            }
        }

        pub fn queue(&mut self, output: Output) -> &mut Self {
            self.queued.borrow_mut().push_back(output);
            self
        }

        /// What the "wake" does to disk while it runs.
        pub fn on_exec(&mut self, effect: impl Fn() + 'static) -> &mut Self {
            self.side_effect = Some(Box::new(effect));
            self
        }

        pub fn last_call(&self) -> Vec<String> {
            self.calls.borrow().last().cloned().unwrap_or_default()
        }
    }

    impl Exec for FakeExec {
        fn exec(&mut self, argv: &[&str], _env: Option<&HashMap<String, String>>, timeout: Duration) -> Output {
            self.calls.borrow_mut().push(argv.iter().map(|a| a.to_string()).collect());
            self.timeouts.borrow_mut().push(timeout);
            if let Some(effect) = self.side_effect.as_ref() {
                effect();
            }
            self.queued.borrow_mut().pop_front().unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_are_kept_apart() {
        let mut exec = RealExec;
        let out = exec.exec(
            &["sh", "-c", "printf out; printf err 1>&2"],
            None,
            Duration::from_secs(10),
        );
        assert_eq!(out.stdout, "out");
        assert_eq!(out.stderr, "err");
        assert!(out.ok());
    }

    /// The whole reason this module exists: a JSON document on stdout must survive a
    /// launcher writing to stderr, because a sandbox wrapper may write to stderr on every invocation.
    #[test]
    fn stdout_json_survives_stderr_chatter() {
        let mut exec = RealExec;
        let out = exec.exec(
            &["sh", "-c", r#"printf 'launching sandboxed claude' 1>&2; printf '{"ok":true}'"#],
            None,
            Duration::from_secs(10),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out.stdout).expect("stdout is clean JSON");
        assert_eq!(parsed["ok"], true);
        assert!(out.merged().contains("launching sandboxed claude"));
    }

    #[test]
    fn nonzero_exit_is_reported_without_timing_out() {
        let mut exec = RealExec;
        let out = exec.exec(&["sh", "-c", "exit 3"], None, Duration::from_secs(10));
        assert_eq!(out.code, 3);
        assert!(!out.timed_out);
        assert!(!out.ok());
    }

    #[test]
    fn a_child_past_its_deadline_is_killed_and_marked() {
        let mut exec = RealExec;
        let out = exec.exec(&["sh", "-c", "sleep 30"], None, Duration::from_millis(200));
        assert!(out.timed_out, "a child past its deadline must say so");
        assert_eq!(out.code, 124);
        assert!(!out.ok());
    }

    /// Output written before the deadline is still returned, so a killed wake's partial
    /// output remains available for the journal.
    #[test]
    fn output_written_before_the_kill_is_kept() {
        let mut exec = RealExec;
        let out = exec.exec(
            &["sh", "-c", "printf partial; sleep 30"],
            None,
            Duration::from_millis(300),
        );
        assert!(out.timed_out);
        assert_eq!(out.stdout, "partial");
    }

    /// Is anything still running with this marker on its command line?
    fn survivor(marker: &str) -> bool {
        std::process::Command::new("pgrep")
            .args(["-f", marker])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// The hang this guards against: the child is killed at its deadline, but something it
    /// started still holds the pipe open, so waiting for EOF waits on the grandchild instead.
    /// Isolated, the grandchild dies with the group, so there is nothing left to wait for.
    #[test]
    fn an_isolated_timeout_kills_what_the_child_started() {
        let mut exec = RealExec;
        let started = Instant::now();
        let out = exec.exec_isolated(&["sh", "-c", "sleep 8.1 & echo started; sleep 8.1"], None, Duration::from_millis(300));
        assert!(out.timed_out);
        assert_eq!(out.stdout, "started\n", "output before the kill is kept");
        assert!(started.elapsed() < Duration::from_secs(1), "took {:?}", started.elapsed());
        assert!(!survivor("sleep 8.1"), "the grandchild must die with its group");
    }

    /// Not isolated — a wake, which must stay in otto's group — the grandchild survives, but the
    /// caller still gets its answer a bounded time after the deadline.
    #[test]
    fn a_grandchild_holding_the_pipe_does_not_hold_the_caller() {
        let mut exec = RealExec;
        let started = Instant::now();
        let out = exec.exec(&["sh", "-c", "sleep 8.2 & sleep 8.2"], None, Duration::from_millis(200));
        assert!(out.timed_out);
        assert!(started.elapsed() < DRAIN_GRACE + Duration::from_secs(1), "took {:?}", started.elapsed());
        let _ = std::process::Command::new("pkill").args(["-f", "sleep 8.2"]).status();
    }

    /// The same survivor after a clean exit: a child that backgrounds something and exits 0
    /// is finished, whatever its leftovers do with the pipe.
    #[test]
    fn a_clean_exit_is_not_held_open_by_a_background_process() {
        let mut exec = RealExec;
        let started = Instant::now();
        let out = exec.exec(&["sh", "-c", "sleep 8.3 & printf done"], None, Duration::from_secs(10));
        assert!(out.ok());
        assert_eq!(out.stdout, "done");
        assert!(started.elapsed() < DRAIN_GRACE + Duration::from_secs(1), "took {:?}", started.elapsed());
        let _ = std::process::Command::new("pkill").args(["-f", "sleep 8.3"]).status();
    }

    #[test]
    fn invalid_utf8_is_kept_lossily_rather_than_dropped() {
        let mut exec = RealExec;
        let out = exec.exec(&["sh", "-c", r"printf 'ok\377ok'"], None, Duration::from_secs(10));
        assert_eq!(out.stdout, "ok\u{fffd}ok");
    }

    #[test]
    fn a_missing_binary_is_an_error_not_a_panic() {
        let mut exec = RealExec;
        let out = exec.exec(&["otto-no-such-binary-anywhere"], None, Duration::from_secs(5));
        assert_eq!(out.code, 127);
        assert!(!out.stderr.is_empty());
    }

    #[test]
    fn empty_argv_is_an_error_not_a_panic() {
        let mut exec = RealExec;
        let out = exec.exec(&[], None, Duration::from_secs(5));
        assert_eq!(out.code, 127);
    }

    #[test]
    fn env_adds_to_the_child_environment() {
        let mut exec = RealExec;
        let mut env = HashMap::new();
        env.insert("OTTO_TEST_VAR".to_string(), "present".to_string());
        let out = exec.exec(&["sh", "-c", "printf %s \"$OTTO_TEST_VAR\""], Some(&env), Duration::from_secs(10));
        assert_eq!(out.stdout, "present");
    }
}
