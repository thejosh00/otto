//! `otto sandbox report` (plans/v3.md, 1.7), against the real binary in a scratch home: it is given log lines
//! in a file, so no test reads the machine's log. What it finds for a run and leaves out for another, what it
//! says of itself, what it refuses, and that it changes nothing on disk. The parsing and grouping are tested
//! on lines recorded from this machine's log, in `src/sandbox_report.rs`.

#![allow(dead_code)]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn write_exec(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn said(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

impl Scratch {
    fn new() -> Scratch {
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let root = dir.path().canonicalize().unwrap();
        let s = Scratch { _dir: dir, root };
        for sub in ["tmp", "bin", "h/.otto", "h/workspace/repo", "h/workspace/notes"] {
            std::fs::create_dir_all(s.root.join(sub)).unwrap();
        }
        // The stand-in is called claude, as the real one is: the wrapper goes in front of that word.
        write_exec(&s.claude(), "#!/bin/sh\nexit 0\n");
        let launchers = serde_json::json!({"launchers": [
            {"name": "claude", "command": s.claude().display().to_string()},
            {"name": "legenda-curator", "command": format!("env LEGENDA_DIR={} {}", s.root.join("legenda").display(), s.claude().display())},
        ]});
        std::fs::write(s.config(), launchers.to_string()).unwrap();
        s
    }

    fn home(&self) -> PathBuf {
        self.root.join("h")
    }

    fn otto_home(&self) -> PathBuf {
        self.home().join(".otto")
    }

    fn claude(&self) -> PathBuf {
        self.root.join("bin/claude")
    }

    fn wrapper(&self) -> String {
        self.otto_home().join("sandbox/wrapper").display().to_string()
    }

    fn config(&self) -> PathBuf {
        self.otto_home().join("config.json")
    }

    fn record(&self, id: &str) -> PathBuf {
        self.otto_home().join("sandbox/runs").join(format!("{id}.json"))
    }

    fn run_json(&self, id: &str) -> PathBuf {
        self.otto_home().join("runs").join(id).join("run.json")
    }

    fn json(&self, path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn launcher_of(&self, id: &str) -> String {
        self.json(&self.run_json(id))["launcher"]["kind"].as_str().unwrap().to_string()
    }

    fn launcher_command(&self, name: &str) -> Option<String> {
        self.json(&self.config())["launchers"].as_array().unwrap().iter().find(|l| l["name"] == name).map(|l| l["command"].as_str().unwrap().to_string())
    }

    fn journal(&self, id: &str) -> String {
        std::fs::read_to_string(self.otto_home().join("runs").join(id).join("journal.jsonl")).unwrap()
    }

    fn otto(&self) -> Command {
        let mut c = support::otto_command();
        c.env("HOME", self.home())
            .env("OTTO_HOME", self.otto_home())
            .env("TMUX_TMPDIR", &self.root)
            .env("TMPDIR", self.root.join("tmp"))
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT")
            .env_remove("OTTO_SESSION_ID")
            .current_dir(&self.root);
        c
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.otto().args(args).output().unwrap();
        assert!(out.status.success(), "otto {args:?}: {}", said(&out));
        said(&out)
    }

    fn run(&self, name: &str, launcher: &str) -> String {
        let workdir = self.home().join("workspace");
        let repo = self.home().join("workspace/repo");
        self.ok(&["run", "--goal", "g", "--name", name, "--perpetual", "--period", "1h", "--detach", "none", "--launcher", launcher, "--workdir", workdir.to_str().unwrap(), "--repo", repo.to_str().unwrap()]);
        let list: serde_json::Value = serde_json::from_slice(&self.otto().args(["state", "list", "--json"]).output().unwrap().stdout).unwrap();
        list.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap()["id"].as_str().unwrap().to_string()
    }
}

/// Lines as `/usr/bin/log show --style compact` prints them: the message on the line after its record.
fn log_lines(id: &str) -> String {
    let other = format!("{id}7");
    let mut log = String::from("Timestamp               Ty Process[PID:TID]\n");
    for (second, process, verdict, rest, run) in [
        (1, "ls", "allow", "file-read-data /Users/someone/.ssh", id),
        (2, "ls", "allow", "file-read-data /Users/someone/.ssh", id),
        (3, "bash", "deny(1)", "file-write-create /private/tmp/elsewhere/made", id),
        (4, "cat", "allow", "file-read-data /Users/someone/.aws/credentials", other.as_str()),
    ] {
        log += &format!("2026-11-15 10:00:{second:02}.000 Df kernel[0:1] (Sandbox) Sandbox: {process}(1) {verdict} {rest}\notto-run-{run}\n");
    }
    log
}

/// Every file under the scratch home, with its bytes.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(dir) = todo.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = path.symlink_metadata().unwrap();
            if meta.is_dir() {
                todo.push(path);
            } else if meta.is_file() && !path.to_string_lossy().ends_with(".lock") {
                out.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn the_report_names_what_was_logged_for_the_run_and_changes_nothing() {
    let s = Scratch::new();
    let id = s.run("watched", "claude");
    s.ok(&["sandbox", "observe", &id]);
    let log = s.root.join("log.txt");
    std::fs::write(&log, log_lines(&id)).unwrap();
    let (record, run_json, journal) = (std::fs::read(s.record(&id)).unwrap(), std::fs::read(s.run_json(&id)).unwrap(), s.journal(&id));
    let before = snapshot(&s.otto_home());

    let text = s.ok(&["sandbox", "report", &id, "--log-file", log.to_str().unwrap()]);
    assert!(text.contains("2×") && text.contains("file-read-data") && text.contains("/Users/someone/.ssh"), "{text}");
    assert!(text.contains("Would be denied (allowed and logged") && text.contains("Denied (blocked"), "{text}");
    assert!(text.contains("/private/tmp/elsewhere/made"), "{text}");
    assert!(!text.contains(".aws"), "another run's line, whose id starts with this one's: {text}");
    assert!(text.contains("guides a decision and proves nothing"), "{text}");
    assert!(text.contains("window") && text.contains("when the run was last put under observation or enforced"), "{text}");
    assert!(text.contains("docs/guides/sandboxing.md") && text.contains("This report changes nothing"), "{text}");
    assert!(text.lines().count() < 20, "{text}");

    let json = s.otto().args(["sandbox", "report", &id, "--json", "--since", "2h", "--log-file", log.to_str().unwrap()]).output().unwrap();
    assert!(json.status.success(), "{}", said(&json));
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["findings"].as_array().unwrap().len(), 2, "{value}");
    assert_eq!(value["findings"][0]["count"], 2);
    assert_eq!(value["findings"][0]["verdict"], "would-deny");
    assert_eq!(value["findings"][1]["verdict"], "denied");
    assert!(value["caveat"].as_str().unwrap().contains("proves nothing"));

    assert_eq!(std::fs::read(s.record(&id)).unwrap(), record, "the record is as it was");
    assert_eq!(std::fs::read(s.run_json(&id)).unwrap(), run_json, "run.json is as it was");
    assert_eq!(s.journal(&id), journal, "nothing was journalled");
    assert!(snapshot(&s.otto_home()) == before, "nothing under the scratch home changed");
}

#[test]
fn a_run_with_no_record_is_said_not_to_be_sandboxed_and_none_is_made() {
    let s = Scratch::new();
    let id = s.run("plain", "claude");
    let log = s.root.join("log.txt");
    std::fs::write(&log, "").unwrap();
    let text = s.ok(&["sandbox", "report", &id, "--log-file", log.to_str().unwrap()]);
    assert!(text.contains("has no sandbox record") && text.contains("--since"), "{text}");
    assert!(text.contains("Nothing in the log for this run") && text.contains("proves nothing"), "{text}");
    assert!(!s.record(&id).exists());
}

#[test]
fn a_run_that_does_not_exist_and_a_time_that_is_not_one_are_refused() {
    let s = Scratch::new();
    let id = s.run("there", "claude");
    let none = s.otto().args(["sandbox", "report", "999"]).output().unwrap();
    assert!(!none.status.success());
    assert!(said(&none).contains("no such run"), "{}", said(&none));
    let when = s.otto().args(["sandbox", "report", &id, "--since", "whenever"]).output().unwrap();
    assert!(!when.status.success());
    assert!(said(&when).contains("--since takes"), "{}", said(&when));
    let until = s.otto().args(["sandbox", "report", &id, "--until", "whenever"]).output().unwrap();
    assert!(!until.status.success());
    assert!(said(&until).contains("--until takes"), "{}", said(&until));
}

/// An enforced run told to use `observe --read` would be put back under observation: the advice names both.
#[test]
fn the_advice_on_opening_a_path_names_enforce_as_well_as_observe() {
    let s = Scratch::new();
    let id = s.run("enforced", "claude");
    s.ok(&["sandbox", "enforce", &id]);
    let log = s.root.join("log.txt");
    std::fs::write(&log, log_lines(&id)).unwrap();
    let text = s.ok(&["sandbox", "report", &id, "--log-file", log.to_str().unwrap()]);
    assert!(text.contains("otto sandbox enforce"), "{text}");
    assert!(text.contains("whichever the run is under now"), "{text}");
}
