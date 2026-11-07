//! tests/wake_kill.rs — what `wake-killed` says, through the real binary: a real `otto wake` holding
//! the lock with a stub for the model, a real `otto poke`, and a real signal to a real process group.
//!
//! The run is `--detach none`, so nothing needs tmux and poke starts the wake as it would in life:
//! in a process group of its own, under no one here.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const OTTO: &str = env!("CARGO_BIN_EXE_otto");

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (home, repo) = (root.join("home"), root.join("repo"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        // Whatever a wake does this time is in `next`; it always leaves a handoff.
        let stub = root.join("stub");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nnext='{root}/next'\nif [ -f \"$next\" ]; then\n  mv \"$next\" \"$next.running\"\n  . \"$next.running\"\nfi\n\
                 printf '# h\\n## Goal\\ng\\n## Where this is\\nHere.\\n## Decided\\nn\\n## This wake did\\nn\\n## Next wake must\\nn\\n## Needs\\nnothing\\n## Don'\"'\"'t re-derive\\nn\\n' | '{OTTO}' state handoff \"$OTTO_RUN_ID\" --stdin >/dev/null\n",
                root = root.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        // Named `claude` too, so nothing here can ever reach the real one.
        std::fs::write(
            home.join("config.json"),
            serde_json::json!({"launchers": [{"name": "claude", "command": stub.display().to_string()}]}).to_string(),
        )
        .unwrap();
        Scratch { _dir: dir, root }
    }

    fn otto(&self, args: &[&str]) -> Output {
        Command::new(OTTO)
            .args(args)
            .current_dir(self.root.join("repo"))
            .env("OTTO_HOME", self.root.join("home"))
            .env("TMUX_TMPDIR", &self.root)
            .env("OTTO_BIN", OTTO)
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT")
            .output()
            .unwrap_or_else(|e| panic!("could not run otto {args:?}: {e}"))
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.otto(args);
        assert!(output.status.success(), "otto {args:?}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    /// A run with a first wake that does nothing but leave its handoff, returning its id.
    fn run(&self, name: &str) -> String {
        let repo = self.root.join("repo");
        let repo = repo.to_str().unwrap();
        self.ok(&["run", "--goal", "g", "--name", name, "--perpetual", "--period", "1h", "--detach", "none", "--workdir", repo]);
        let list: Value = serde_json::from_str(&self.ok(&["state", "list", "--json"])).unwrap();
        list.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap()["id"].as_str().unwrap().to_string()
    }

    fn next_wake_does(&self, script: &str) {
        std::fs::write(self.root.join("next"), script).unwrap();
    }

    fn run_dir(&self, id: &str) -> PathBuf {
        self.root.join("home/runs").join(id)
    }

    fn state(&self, id: &str) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.run_dir(id).join("run.json")).unwrap()).unwrap()
    }

    fn journal_line(&self, id: &str, event: &str) -> Option<Value> {
        let text = std::fs::read_to_string(self.run_dir(id).join("journal.jsonl")).unwrap();
        text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).find(|l| l["event"] == event)
    }
}

/// Is anything left in this process group?
fn group_alive(pgid: i64) -> bool {
    Command::new("kill").args(["-0", "--", &format!("-{pgid}")]).stderr(std::process::Stdio::null()).status().unwrap().success()
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < until, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whatever goes wrong below, the sleeping stub doesn't outlive the test.
struct KillGroup(i64);

impl Drop for KillGroup {
    fn drop(&mut self) {
        let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", self.0)]).stderr(std::process::Stdio::null()).status();
    }
}

#[test]
fn a_kill_names_the_wake_it_hit_and_that_the_signal_landed() {
    let s = Scratch::new();
    let id = s.run("stuck");

    // A wake poke will start: it says it's up, then hangs, in the model's place.
    s.next_wake_does(&format!("'{OTTO}' state arm-timer \"$OTTO_RUN_ID\" --in 1\n"));
    s.ok(&["wake", &id]);
    let up = s.root.join("up");
    s.next_wake_does(&format!("touch '{}'\nsleep 300\n", up.display()));
    std::thread::sleep(Duration::from_secs(2));
    let said = s.ok(&["poke"]);
    wait_until(&format!("the stub starting (poke said: {said})"), || up.exists());

    // While it runs, the record beside the lock names it as run.json does.
    let wake = s.state(&id)["wake"].clone();
    let (n, pid) = (wake["n"].as_i64().unwrap(), wake["pid"].as_i64().unwrap());
    let _cleanup = KillGroup(pid);
    assert!(group_alive(pid), "the wake is its own process group");
    let holder: Value = serde_json::from_str(&std::fs::read_to_string(s.run_dir(&id).join("wake-holder.json")).unwrap()).unwrap();
    assert_eq!(holder["pid"], pid, "{holder}");
    assert_eq!(holder["wake"], n, "{holder}");
    assert!(holder["at"].is_string(), "{holder}");

    // Its deadline passes, as a wake stuck for an hour would see it.
    let past = "2020-01-01T00:00:00Z";
    let mut state = s.state(&id);
    state["wake"]["deadlineAt"] = past.into();
    std::fs::write(s.run_dir(&id).join("run.json"), serde_json::to_string_pretty(&state).unwrap()).unwrap();

    let said = s.ok(&["poke"]);
    assert!(said.contains("kill"), "{said}");

    let line = s.journal_line(&id, "wake-killed").unwrap_or_else(|| panic!("no wake-killed line; poke said: {said}"));
    assert_eq!(line["wake"], n, "{line}");
    assert_eq!(line["pid"], pid, "{line}");
    assert_eq!(line["deadlineAt"], past, "{line}");
    assert_eq!(line["outcome"], Value::Null, "{line}");
    assert!(line.as_object().unwrap().contains_key("outcome"), "no outcome is said, not left out: {line}");
    assert_eq!(line["holder"], "this-wake", "{line}");
    assert_eq!(line["holderRecord"], holder, "{line}");
    assert_eq!(line["signalLanded"], true, "{line}");
    assert_eq!(line["tmuxKilled"], false, "{line}");

    wait_until("the stub's process group going", || !group_alive(pid));
}

/// A run with a kill from before they recorded their evidence: it reads, and says so, and what a
/// person sees doesn't guess. A new line that has its evidence doesn't say "not recorded".
#[test]
fn an_old_kill_shows_its_evidence_as_not_recorded() {
    let s = Scratch::new();
    let id = s.run("old");
    let journal = s.run_dir(&id).join("journal.jsonl");
    let lines = std::fs::read_to_string(&journal).unwrap();
    let old = "{\"event\":\"wake-killed\",\"reason\":\"wake is 16m past its deadline\",\"ts\":\"2026-09-28T11:00:00Z\"}\n";
    std::fs::write(&journal, format!("{old}{lines}")).unwrap();

    let logs = s.ok(&["logs", &id, "--event", "wake-killed"]);
    for field in ["wake", "deadlineAt", "outcome", "holder", "signalLanded", "tmuxKilled"] {
        assert!(logs.contains(&format!("{field}=not recorded")), "{field}: {logs}");
    }

    let new = "{\"event\":\"wake-killed\",\"reason\":\"r\",\"wake\":5,\"pid\":9,\"deadlineAt\":\"2026-09-28T10:00:00Z\",\"outcome\":null,\"holder\":\"not-known\",\"signalLanded\":false,\"tmuxKilled\":false,\"ts\":\"2026-09-28T11:00:00Z\"}\n";
    std::fs::write(&journal, new).unwrap();
    let logs = s.ok(&["logs", &id, "--event", "wake-killed"]);
    assert!(!logs.contains("not recorded"), "{logs}");
    assert!(logs.contains("outcome=none") && logs.contains("holder=not-known"), "{logs}");
}
