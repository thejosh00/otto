//! tests/logs.rs — the two launchd logs through the real binary, with stdout and stderr appended to
//! the log the way launchd does it: the file opened and handed over, not a pipe the test reads.
//! `--detach none` and a stub for the model, so nothing needs tmux or can reach the real one.

mod support;

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// The bound in `src/logs.rs`, here too so that changing it is a decision.
const BOUND: usize = 1024 * 1024;

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn iso(dt: OffsetDateTime) -> String {
    dt.replace_nanosecond(0).unwrap().format(&Rfc3339).unwrap()
}

fn ago(minutes: i64) -> String {
    iso(OffsetDateTime::now_utc() - time::Duration::minutes(minutes))
}

fn stamped(line: &str) -> bool {
    line.split_whitespace().next().is_some_and(|word| OffsetDateTime::parse(word, &Rfc3339).is_ok())
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(home.join("logs")).unwrap();
        let stub = root.join("stub");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nprintf '# h\\n## Goal\\ng\\n## Where this is\\nHere.\\n## Decided\\nn\\n## This wake did\\nn\\n## Next wake must\\nn\\n## Needs\\nnothing\\n## Don'\"'\"'t re-derive\\nn\\n' | '{}' state handoff \"$OTTO_RUN_ID\" --stdin >/dev/null\n",
                support::OTTO,
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, PermissionsExt::from_mode(0o755)).unwrap();
        // Named `claude` too, so nothing here can ever reach the real one.
        std::fs::write(
            home.join("config.json"),
            serde_json::json!({"launchers": [{"name": "claude", "command": stub.display().to_string()}]}).to_string(),
        )
        .unwrap();
        Scratch { _dir: dir, root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = support::otto_command();
        command
            .args(args)
            .current_dir(&self.root)
            .env("OTTO_HOME", self.root.join("home"))
            .env("TMUX_TMPDIR", &self.root)
            .env("OTTO_BIN", support::OTTO)
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT");
        command
    }

    fn ok(&self, args: &[&str]) -> String {
        let output: Output = self.command(args).output().unwrap();
        assert!(output.status.success(), "otto {args:?}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn log(&self, name: &str) -> PathBuf {
        self.root.join("home/logs").join(name)
    }

    fn read(&self, name: &str) -> String {
        String::from_utf8_lossy(&std::fs::read(self.log(name)).unwrap_or_default()).to_string()
    }

    /// Standard output and error appended to a log, both the one file, as launchd hands them over.
    fn handed(&self, command: &mut Command, name: &str) {
        let file = File::options().create(true).append(true).open(self.log(name)).unwrap();
        command.stdin(Stdio::null()).stdout(file.try_clone().unwrap()).stderr(file);
    }

    /// One pass as the reviver runs it.
    fn reviver(&self, args: &[&str]) {
        let mut command = self.command(&[&["poke"], args].concat());
        self.handed(&mut command, "poke.log");
        command.status().unwrap();
    }

    fn logs(&self) -> BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(self.root.join("home/logs"))
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap())
            })
            .collect()
    }

    /// A sleeping run whose first wake is over. Its id.
    fn run(&self) -> String {
        let root = self.root.to_str().unwrap();
        self.ok(&["run", "--goal", "g", "--name", "due", "--perpetual", "--period", "1h", "--detach", "none", "--workdir", root]);
        let list: Value = serde_json::from_str(&self.ok(&["state", "list", "--json"])).unwrap();
        let id = list.as_array().unwrap().iter().find(|r| r["name"] == "due").unwrap()["id"].as_str().unwrap().to_string();
        self.wait_for(&id, 1);
        id
    }

    fn run_json(&self, id: &str) -> PathBuf {
        self.root.join("home/runs").join(id).join("run.json")
    }

    fn journal(&self, id: &str) -> Vec<Value> {
        std::fs::read_to_string(self.root.join("home/runs").join(id).join("journal.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn wait_for(&self, id: &str, wakes: usize) {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let finished = self.journal(id).iter().filter(|l| l["event"] == "wake-complete" || l["event"] == "wake-incomplete").count();
            let state: Value = serde_json::from_str(&std::fs::read_to_string(self.run_json(id)).unwrap()).unwrap();
            if finished >= wakes && state["status"] == "sleeping" {
                return;
            }
            assert!(Instant::now() < until, "{id} never got to {wakes} wakes: {:#?}", self.journal(id));
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// A reviver's pass over the run, made due, must start its wake: whatever the log is like.
    fn wakes_anyway(&self, id: &str) {
        let wakes = self.journal(id).iter().filter(|l| l["event"] == "wake-started").count();
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let mut state: Value = serde_json::from_str(&std::fs::read_to_string(self.run_json(id)).unwrap()).unwrap();
            state["nextWakeAt"] = "2020-01-01T00:00:00Z".into();
            std::fs::write(self.run_json(id), serde_json::to_string_pretty(&state).unwrap()).unwrap();
            self.reviver(&[]);
            if self.journal(id).iter().filter(|l| l["event"] == "wake-started").count() > wakes {
                self.wait_for(id, wakes + 1);
                return;
            }
            assert!(Instant::now() < until, "no wake: {}", self.read("poke.log"));
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// What `otto agent status` says the last poke was.
    fn last_poke(&self) -> String {
        let said = self.ok(&["agent", "status"]);
        said.lines().find(|l| l.contains("last poke:")).unwrap_or_else(|| panic!("{said}")).to_string()
    }
}

/// A log of "nothing due" lines stamped `minutes` ago, just over `size` bytes of it.
fn planted(size: usize, minutes: i64) -> String {
    let line = format!("{} nothing due (3 run(s) checked)\n", ago(minutes));
    line.repeat(size / line.len() + 1)
}

#[test]
fn every_line_the_server_writes_has_a_stamp() {
    let s = Scratch::new();
    // Old lines stay as they are.
    std::fs::write(s.log("serve.log"), "otto: serving the old way\n").unwrap();
    // In its way, so that the eprint on the way up has something to say.
    std::fs::write(s.root.join("home/agents"), "not a directory").unwrap();
    let mut command = s.command(&["serve", "--port", "0"]);
    s.handed(&mut command, "serve.log");
    let mut server = command.spawn().unwrap();

    let until = Instant::now() + Duration::from_secs(20);
    while !(s.read("serve.log").contains("Ctrl-C to stop") && s.read("serve.log").contains("couldn't check on the system agents")) {
        assert!(Instant::now() < until, "{}", s.read("serve.log"));
        std::thread::sleep(Duration::from_millis(50));
    }
    server.kill().unwrap();
    server.wait().unwrap();

    let log = s.read("serve.log");
    let (old, new) = log.split_once('\n').unwrap();
    assert_eq!(old, "otto: serving the old way");
    assert!(new.lines().count() >= 3, "{log}");
    for line in new.lines() {
        assert!(stamped(line), "no stamp: {line:?} in\n{log}");
    }
}

#[test]
fn why_the_server_would_not_start_is_stamped_and_not_lost() {
    let s = Scratch::new();
    let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port().to_string();
    let mut command = s.command(&["serve", "--port", &port]);
    s.handed(&mut command, "serve.log");
    assert!(!command.status().unwrap().success());

    let log = s.read("serve.log");
    let line = log.lines().find(|l| l.contains("cannot listen on")).unwrap_or_else(|| panic!("lost: {log}"));
    assert!(stamped(line), "{log}");
}

#[test]
fn a_log_over_the_bound_rolls_and_the_pass_is_in_the_new_one() {
    let s = Scratch::new();
    let old = planted(BOUND, 5);
    assert!(old.len() > BOUND);
    std::fs::write(s.log("poke.log"), &old).unwrap();
    for (n, says) in [(1, "the first"), (2, "the second"), (3, "the third")] {
        std::fs::write(s.log(&format!("poke.log.{n}")), format!("{says}\n")).unwrap();
    }

    s.reviver(&[]);

    assert_eq!(s.read("poke.log.1"), old, "every old line, in order, and nothing of this pass");
    assert_eq!(s.read("poke.log.2"), "the first\n");
    assert_eq!(s.read("poke.log.3"), "the second\n");
    assert!(!s.log("poke.log.4").exists(), "the fourth is what goes");
    let new = s.read("poke.log");
    let last = new.lines().rev().find(|l| stamped(l)).unwrap_or_else(|| panic!("this pass isn't in poke.log: {new:?}"));
    assert!(new.len() < 10_000, "{new}");
    let stamp = last.split_whitespace().next().unwrap();
    assert!(s.last_poke().contains(stamp), "{} against {last}", s.last_poke());

    // The next pass carries on in the same file, with nothing to roll.
    let before = s.logs();
    s.reviver(&[]);
    assert_eq!(s.logs()["poke.log.1"], before["poke.log.1"]);
    assert!(s.read("poke.log").starts_with(&new), "{}", s.read("poke.log"));
}

#[test]
fn under_the_bound_or_by_hand_nothing_rolls() {
    let s = Scratch::new();
    let line = format!("{} nothing due (3 run(s) checked)\n", ago(5));
    let under = line.repeat((BOUND - 1) / line.len());
    assert!(under.len() < BOUND && under.len() + line.len() >= BOUND);
    std::fs::write(s.log("poke.log"), &under).unwrap();
    s.reviver(&[]);
    assert!(!s.log("poke.log.1").exists());

    // A poke by hand writes elsewhere, so the log isn't its to move, however big or how long ago.
    let over = planted(BOUND, 300);
    std::fs::write(s.log("poke.log"), &over).unwrap();
    s.ok(&["poke"]);
    assert_eq!(s.read("poke.log"), over);
    assert!(!s.log("poke.log.1").exists());
}

#[test]
fn a_long_gap_is_said_once() {
    let s = Scratch::new();
    let last = ago(180);
    std::fs::write(s.log("poke.log"), format!("{last} nothing due (1 run(s) checked)\notto poke: unstamped, and not a pass\n")).unwrap();

    s.reviver(&[]);
    let log = s.read("poke.log");
    let notes: Vec<&str> = log.lines().filter(|l| l.contains("since the last pass")).collect();
    assert_eq!(notes.len(), 1, "{log}");
    assert!(stamped(notes[0]), "{log}");
    assert!(notes[0].contains(" 3h0m since the last pass, at ") && notes[0].ends_with(&last), "{log}");

    s.reviver(&[]);
    assert_eq!(s.read("poke.log").matches("since the last pass").count(), 1, "the next pass says nothing: {}", s.read("poke.log"));
}

#[test]
fn five_minutes_or_a_first_pass_says_nothing() {
    let s = Scratch::new();
    s.reviver(&[]);
    assert!(!s.read("poke.log").contains("since the last pass"), "a first pass: {}", s.read("poke.log"));

    std::fs::write(s.log("poke.log"), format!("{} nothing due (1 run(s) checked)\n", ago(5))).unwrap();
    s.reviver(&[]);
    assert!(!s.read("poke.log").contains("since the last pass"), "{}", s.read("poke.log"));
}

#[test]
fn the_gap_is_found_in_a_log_rotated_a_moment_ago() {
    let s = Scratch::new();
    std::fs::write(s.log("poke.log"), "").unwrap();
    std::fs::write(s.log("poke.log.1"), format!("{} nothing due (1 run(s) checked)\n", ago(120))).unwrap();
    assert!(s.last_poke().contains("2h ago"), "{}", s.last_poke());
    s.reviver(&[]);
    assert!(s.read("poke.log").contains(" 2h0m since the last pass"), "{}", s.read("poke.log"));
}

#[test]
fn a_log_it_cannot_rotate_or_read_changes_nothing_else() {
    let s = Scratch::new();
    let id = s.run();

    // Garbled: nothing to compare with, and it says so.
    std::fs::write(s.log("poke.log"), b"\xff\xfe\x00 not a line\nneither is this\n").unwrap();
    s.wakes_anyway(&id);
    let log = s.read("poke.log");
    assert!(log.contains("couldn't tell when the last pass was"), "{log}");
    assert!(!log.contains("since the last pass, at"), "{log}");

    // A directory nothing can be renamed in.
    let over = planted(BOUND, 5);
    std::fs::write(s.log("poke.log"), &over).unwrap();
    let logs = s.root.join("home/logs");
    std::fs::set_permissions(&logs, PermissionsExt::from_mode(0o555)).unwrap();
    s.wakes_anyway(&id);
    std::fs::set_permissions(&logs, PermissionsExt::from_mode(0o755)).unwrap();
    let log = s.read("poke.log");
    assert!(log.starts_with(&over), "nothing lost");
    assert!(log.contains("couldn't rotate the log"), "{}", &log[over.len()..]);
    assert!(!s.log("poke.log.1").exists());
}

#[test]
fn a_dry_run_leaves_the_logs_alone_and_is_not_a_pass() {
    let s = Scratch::new();
    s.ok(&["state", "init", "--name", "a", "--perpetual", "--workdir", s.root.to_str().unwrap(), "--goal", "g"]);
    let over = planted(BOUND, 180);
    std::fs::write(s.log("poke.log"), &over).unwrap();
    std::fs::write(s.log("poke.log.1"), "the first\n").unwrap();
    let before = s.logs();

    s.ok(&["poke", "--dry-run"]);
    assert_eq!(s.logs(), before);

    // Even handed the log itself, it moves nothing and notes nothing: all it adds is what it printed.
    s.reviver(&["--dry-run"]);
    assert_eq!(s.logs()["poke.log.1"], before["poke.log.1"]);
    assert!(!s.log("poke.log.2").exists());
    let log = s.read("poke.log");
    assert!(log.starts_with(&over) && !log.contains("since the last pass"), "{}", &log[over.len()..]);
}
