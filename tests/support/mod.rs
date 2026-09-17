//! tests/support/mod.rs — shared harness for the live integration tests.
//!
//! Everything a live test got wrong at least once lives here in exactly one copy,
//! so `tests/live_gate.rs`, `tests/live_devflow.rs` and `tests/live_revive.rs` don't each grow
//! their own slightly-different polling loop.
//!
//! Every helper treats `otto` as an external binary — the same thing a person on the command
//! line runs — and reads `run.json`/`journal.jsonl` back generically via `serde_json::Value`,
//! the same way the bash scripts shelled out to `python3 -c 'import json...'`. There is no
//! `otto` library crate to depend on, and even if there were, these tests exist to check the
//! CLI contract, not the internals behind it.

#![allow(dead_code)]

use serde_json::Value;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

// --- running the binary -----------------------------------------------------

pub fn otto_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_otto"))
}

/// A path inside this checkout, e.g. `repo_path("test/fixtures/hello-flow.md")` — resolved
/// from `CARGO_MANIFEST_DIR` rather than the process's cwd, so it doesn't matter how the test
/// binary was invoked.
pub fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Where a devflow-style test builds its scratch repos: `$OTTO_WORKSPACE`, else `$HOME`.
pub fn workspace_dir() -> PathBuf {
    std::env::var_os("OTTO_WORKSPACE").map(PathBuf::from).unwrap_or_else(home_dir)
}

/// Run `otto <args...>`, inheriting this process's environment (so `OTTO_HOME`, `PATH`, etc.
/// pass through exactly as they would from a person's shell).
pub fn otto(args: &[&str]) -> Output {
    Command::new(otto_bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("could not run otto {args:?}: {e}"))
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// otto (and `claude`) exist on PATH — checked first, so a missing prerequisite reads as a
/// clear message rather than a wall of unrelated timeouts.
pub fn require_launcher(checks: &mut Checks) {
    let claude_ok = Command::new("claude").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    checks.check(claude_ok, "claude is reachable", "claude is not on PATH");
}

// --- soft assertions ---------------------------------------------------------

/// Replaces the bash `pass`/`fail`/`ok`/`bad`/`skip`/`note`/`step` globals. Accumulates
/// failures instead of panicking on the first one: a live run costs real money and wall-clock
/// time, so a test should report *everything* that went wrong in one run, not just the first
/// thing.
pub struct Checks {
    pass: u32,
    fail: u32,
    failures: Vec<String>,
    started_at: Instant,
}

impl Checks {
    pub fn new() -> Self {
        Self { pass: 0, fail: 0, failures: Vec::new(), started_at: Instant::now() }
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// How many checks have failed so far — for a caller deciding whether to keep a scratch
    /// repo around for inspection before `finish()` consumes `self`.
    pub fn fail_count(&self) -> u32 {
        self.fail
    }

    fn elapsed_min(&self) -> u64 {
        self.elapsed().as_secs() / 60
    }

    pub fn step(&self, title: &str) {
        println!("\n{title}");
    }

    pub fn ok(&mut self, msg: impl AsRef<str>) {
        self.pass += 1;
        println!("  ok   {}", msg.as_ref());
    }

    pub fn bad(&mut self, msg: impl AsRef<str>) {
        self.fail += 1;
        let msg = msg.as_ref().to_string();
        eprintln!("  FAIL {msg}");
        self.failures.push(msg);
    }

    pub fn skip(&mut self, msg: impl AsRef<str>) {
        println!("  skip {}", msg.as_ref());
    }

    pub fn note(&mut self, msg: impl AsRef<str>) {
        println!("       {}", msg.as_ref());
    }

    pub fn check(&mut self, cond: bool, pass_msg: impl AsRef<str>, fail_msg: impl AsRef<str>) {
        if cond {
            self.ok(pass_msg);
        } else {
            self.bad(fail_msg);
        }
    }

    /// Stop early on a run that is still working — everything is kept, exactly like bash's
    /// `bail`. Panics, so the test fails and prints how to pick the run back up by hand.
    pub fn bail(&mut self, run: &Run, why: &str) -> ! {
        self.bad(why);
        self.step(&format!("stopped early after {}min", self.elapsed_min()));
        self.note(format!("phase {}, status {}", run.field_str("phase"), run.field_str("status")));
        self.note(format!("Resume:  otto wake {}   (or wait for otto poke)", run.id));
        self.note(format!(
            "Release: otto state unlock {} && otto state set-status {} --status failed --reason abandoned",
            run.id, run.id
        ));
        self.note(format!("Run dir: {}", run.dir.display()));
        panic!("{why}");
    }

    /// Report, and panic (listing every failure) if any check failed. Call this at the end of
    /// every live test in place of bash's `finish`/`exit $((fail > 0))`.
    pub fn finish(self) {
        println!("\n{} passed, {} failed  ({}min)", self.pass, self.fail, self.elapsed_min());
        if !self.failures.is_empty() {
            panic!("{} check(s) failed:\n  - {}", self.failures.len(), self.failures.join("\n  - "));
        }
    }
}

// --- a run's state on disk ----------------------------------------------------

/// `$OTTO_HOME` if set, else `~/.otto` — mirrors `paths::otto_home()`.
pub fn otto_data_dir() -> PathBuf {
    if let Ok(value) = std::env::var("OTTO_HOME") {
        if !value.is_empty() {
            return shellexpand_home(&value);
        }
    }
    home_dir().join(".otto")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn shellexpand_home(value: &str) -> PathBuf {
    match value.strip_prefix('~') {
        Some(rest) => home_dir().join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    }
}

pub struct Run {
    pub id: String,
    pub dir: PathBuf,
}

impl Run {
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        let dir = otto_data_dir().join("runs").join(&id);
        Self { id, dir }
    }

    /// `run.json`, parsed fresh every call — a run's state changes under the test's feet.
    fn run_json(&self) -> Option<Value> {
        let text = std::fs::read_to_string(self.dir.join("run.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// A dotted field out of run.json — `jf`/`f` in the old bash. `None` on a missing file or
    /// field, never an error: a run that has not written a field yet is an ordinary state, not
    /// a bug in the test.
    pub fn field(&self, dotted: &str) -> Option<Value> {
        let mut node = self.run_json()?;
        for part in dotted.split('.') {
            node = node.get(part)?.clone();
        }
        Some(node)
    }

    /// The field as a bare string: a JSON string unwrapped, anything else via its JSON text,
    /// or "" when absent/null — matches bash's `f`, which always prints *something* to
    /// compare against.
    pub fn field_str(&self, dotted: &str) -> String {
        match self.field(dotted) {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s,
            Some(other) => other.to_string(),
        }
    }

    pub fn status(&self) -> String {
        self.field_str("status")
    }

    pub fn is_over(&self) -> bool {
        matches!(self.status().as_str(), "done" | "failed" | "stopped")
    }

    /// How many times an event appears in the journal. Counting these prove something
    /// *happened*, rather than that something else stopped being true.
    pub fn events(&self, event: &str) -> usize {
        let Ok(text) = std::fs::read_to_string(self.dir.join("journal.jsonl")) else {
            return 0;
        };
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|v| v.get("event").and_then(Value::as_str) == Some(event))
            .count()
    }

    pub fn artifact(&self, rel: &str) -> PathBuf {
        self.dir.join("artifacts").join(rel)
    }

    pub fn gate_files_matching(&self, suffix: &str) -> Vec<PathBuf> {
        let gates_dir = self.dir.join("gates");
        let Ok(entries) = std::fs::read_dir(&gates_dir) else {
            return Vec::new();
        };
        entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(suffix)))
            .collect()
    }

    pub fn handoff_path(&self) -> PathBuf {
        self.dir.join("handoff.md")
    }

    pub fn wake_log(&self, lines: usize) -> String {
        let Ok(text) = std::fs::read_to_string(self.dir.join("wake.log")) else {
            return String::new();
        };
        let all: Vec<&str> = text.lines().collect();
        let start = all.len().saturating_sub(lines);
        all[start..].join("\n")
    }

    pub fn journal_dump(&self) {
        println!("\njournal");
        let Ok(text) = std::fs::read_to_string(self.dir.join("journal.jsonl")) else {
            return;
        };
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let ts = v.get("ts").and_then(Value::as_str).unwrap_or("?");
            let event = v.get("event").and_then(Value::as_str).unwrap_or("?");
            let mut rest = v.clone();
            if let Some(obj) = rest.as_object_mut() {
                obj.remove("ts");
                obj.remove("event");
            }
            let mut rest_str = rest.to_string();
            rest_str.truncate(rest_str.len().min(105));
            println!("  {ts}  {event:<16} {rest_str}");
        }
    }
}

// --- waiting ------------------------------------------------------------------

const POLL: Duration = Duration::from_secs(1);

/// Wait for a field to equal a value. Returns `false` on timeout, or as soon as the run goes
/// terminal or blocked while waiting on something else — a run that ended is not a run about
/// to satisfy you.
pub fn await_field(run: &Run, dotted: &str, want: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut ticks = 0u32;
    loop {
        if run.field_str(dotted) == want {
            return true;
        }
        if dotted != "status" && matches!(run.status().as_str(), "failed" | "blocked" | "done" | "stopped") {
            return false;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
        ticks += 1;
        if ticks.is_multiple_of(120) {
            println!(
                "         {}min — phase {}, status {}",
                ticks / 60,
                run.field_str("phase"),
                run.status()
            );
        }
    }
}

pub fn await_gate(run: &Run, slug: &str, timeout: Duration) -> bool {
    await_field(run, "gate.slug", slug, timeout)
}

/// Wait for a wake to stop holding the run, so a poll does not race a wake mid-write.
pub fn await_idle(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let out = otto(&["ls"]);
        if !stdout(&out).contains("working (wake") {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

pub fn locks_held() -> usize {
    let out = otto(&["state", "locks"]);
    stdout(&out).lines().filter(|l| !l.trim().is_empty()).count()
}

pub fn repo_is_locked(repo: &Path) -> bool {
    let out = otto(&["state", "locks"]);
    stdout(&out).contains(&repo.display().to_string())
}

// --- starting and answering ---------------------------------------------------

/// Start a run and take its first wake. Detached by default (the run's own default), so the
/// test can poll while the wake works.
pub fn start_run(checks: &mut Checks, run_id: &str, goal: &str, extra_args: &[&str]) -> Run {
    let mut args: Vec<&str> = vec!["run", "--id", run_id, "--goal", goal];
    args.extend_from_slice(extra_args);
    let out = otto(&args);
    checks.check(out.status.success(), format!("started {run_id}"), "otto run could not start the run");
    if !out.status.success() {
        panic!("otto run failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Run::new(run_id)
}

/// Answer the open gate through the CLI, then PROVE it closed by counting journal events
/// rather than watching the slug change — a new gate can reuse a slug, so the count is the
/// only honest signal.
pub fn answer_gate(checks: &mut Checks, run: &Run, expected_slug: &str, answer: &str, timeout: Duration) -> bool {
    let actual = run.field_str("gate.slug");
    if actual.is_empty() {
        checks.bad("no gate is open to answer");
        return false;
    }
    let slug = if actual == expected_slug {
        expected_slug.to_string()
    } else {
        checks.note(format!("gate slug is '{actual}', expected '{expected_slug}'"));
        actual
    };
    let before = run.events("gate-closed");
    // No --no-wake: answering must actually continue the run, or the test would wait for a
    // gate nothing is working toward.
    let out = otto(&["answer", &run.id, "--text", answer]);
    if !out.status.success() {
        checks.bad(format!("otto answer failed on the {slug} gate"));
        return false;
    }
    let deadline = Instant::now() + timeout;
    loop {
        if run.events("gate-closed") > before {
            break;
        }
        if Instant::now() >= deadline {
            checks.bad(format!("the {slug} gate never closed"));
            return false;
        }
        std::thread::sleep(POLL);
    }
    let unanswered_left = run
        .gate_files_matching(&format!("{slug}.md"))
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .any(|text| text.contains("_unanswered_"));
    if unanswered_left {
        checks.bad(format!("the {slug} answer never reached the gate file"));
        return false;
    }
    checks.ok(format!("{slug} answered, verbatim on disk"));
    true
}

// --- scratch repos (devflow) ---------------------------------------------------

/// RAII guard for the throwaway git repo + bare origin `devflow` builds against. Removed on
/// drop unless `OTTO_TEST_KEEP=1` is set (mirrors the old `--keep`), or `keep()` was called —
/// a live test that failed wants the scratch repo left for inspection.
pub struct ScratchRepo {
    root: PathBuf,
    keep: bool,
}

impl ScratchRepo {
    pub fn new(workspace: &Path, label: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let root = workspace.join(".work-trees").join(format!("otto-scratch-{label}-{stamp}"));
        std::fs::create_dir_all(&root).expect("create scratch dir");
        Self { root, keep: std::env::var("OTTO_TEST_KEEP").as_deref() == Ok("1") }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn origin_path(&self) -> PathBuf {
        self.root.join("origin.git")
    }

    pub fn repo_path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Force the scratch repo to survive the drop — for a failed run worth inspecting by hand.
    pub fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for ScratchRepo {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.root);
        } else {
            eprintln!("kept scratch repo at {}", self.root.display());
        }
    }
}

pub fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("could not run git {args:?} in {}: {e}", dir.display()))
}

pub fn git_ok(dir: &Path, args: &[&str]) {
    let out = git(dir, args);
    assert!(out.status.success(), "git {args:?} in {} failed: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
}

pub fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
}

/// Final report and, on an expected finish, best-effort cleanup: stop the run, reap the
/// worktree, and drop the scratch repo. Mirrors bash's `finish`, minus the `exit` — the test
/// itself decides pass/fail via `checks.finish()`.
pub fn finish_run(checks: Checks, run: &Run, want_status: &str, worktree: Option<&Path>, repo: Option<&Path>) {
    run.journal_dump();
    let final_status = run.status();
    if checks.fail == 0 && final_status == want_status {
        let _ = otto(&["stop", &run.id, "--reason", "live test finished"]);
        if let (Some(wt), Some(repo)) = (worktree, repo) {
            if wt.is_dir() {
                let _ = git(repo, &["worktree", "remove", "--force", &wt.display().to_string()]);
            }
        }
    } else {
        println!("kept everything: the run is {final_status}");
        if final_status != want_status {
            println!(
                "release: otto state unlock {} && otto state set-status {} --status failed --reason abandoned",
                run.id, run.id
            );
        }
    }
    if checks.fail > 0 {
        println!("wake output tail:\n{}", run.wake_log(40));
    }
    checks.finish();
}
