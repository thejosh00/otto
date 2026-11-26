//! `otto sandbox observe`, `enforce` and `off` (plans/v3.md, 1.7), against the real binary in a scratch home,
//! with a stand-in for claude and no model: what each writes, that the record and `run.json` agree afterwards,
//! and that none of them runs from a wake or a chat. That a process inside an enforced profile can't write the
//! record even with the variable cleared is in `tests/sandbox.rs` (`the_sandbox_commands_can_not_be_run_from_inside`).

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

    /// Neither `otto wake` nor `otto poke`, asked what they would do, refuses the run for disagreeing.
    fn agrees(&self, id: &str) {
        let wake = self.otto().args(["wake", id, "--dry-run"]).output().unwrap();
        assert!(wake.status.success(), "a dry wake is not refused: {}", said(&wake));
        let poke = said(&self.otto().args(["poke", "--dry-run", "--verbose"]).output().unwrap());
        assert!(!poke.contains("disagrees") && !poke.contains("sandbox record"), "{poke}");
        assert_ne!(self.json(&self.run_json(id))["status"], "blocked");
        assert!(!self.journal(id).contains("sandbox-refused"), "{}", self.journal(id));
    }
}

#[test]
fn observe_enforce_and_off_move_a_run_in_and_out() {
    let s = Scratch::new();
    let id = s.run("r", "claude");
    let repo = s.home().join("workspace/repo").display().to_string();
    let config_before = std::fs::read_to_string(s.config()).unwrap();

    let out = s.ok(&["sandbox", "observe", &id]);
    let record = s.json(&s.record(&id));
    assert_eq!(record["mode"], "report");
    assert_eq!(record["launcher"], "sandbox-claude");
    assert_eq!(record["workdir"], s.home().join("workspace").display().to_string());
    assert_eq!(record["write"], serde_json::json!([repo]), "only the run's own repos are open");
    assert_eq!(record["read"], serde_json::json!([]));
    assert_eq!(s.launcher_of(&id), "sandbox-claude");
    assert_eq!(s.launcher_command("sandbox-claude").unwrap(), format!("{} -- {}", s.wrapper(), s.claude().display()));
    for word in [s.record(&id).display().to_string(), "report".to_string(), "sandbox-claude".to_string(), s.wrapper(), repo.clone(), format!("otto sandbox off")] {
        assert!(out.contains(&word), "it says {word}: {out}");
    }
    assert!(s.journal(&id).contains("\"sandbox-observed\""), "{}", s.journal(&id));
    s.agrees(&id);

    // A path added by hand after reading what was denied survives being enforced, and one given here joins it.
    let notes = s.home().join("workspace/notes").display().to_string();
    let mut edited = record.clone();
    edited["read"] = serde_json::json!([notes]);
    std::fs::write(s.record(&id), edited.to_string()).unwrap();
    let out = s.ok(&["sandbox", "enforce", &id, "--write", &notes, "--write", &repo]);
    let record = s.json(&s.record(&id));
    assert_eq!(record["mode"], "enforce");
    assert_eq!(record["read"], serde_json::json!([notes]), "the hand edit is kept");
    assert_eq!(record["write"], serde_json::json!([repo, notes]), "added once, and what was there kept");
    assert_eq!(record["launcher"], "sandbox-claude");
    assert_eq!(s.launcher_of(&id), "sandbox-claude");
    assert!(out.contains("enforce") && out.contains(&notes), "{out}");
    assert!(s.journal(&id).contains("\"sandbox-enforced\""));
    s.agrees(&id);

    let out = s.ok(&["sandbox", "off", &id]);
    assert!(!s.record(&id).exists(), "no record is left");
    assert_eq!(s.launcher_of(&id), "claude", "the launcher it had before");
    assert!(out.contains("removed") && out.contains("next wake"), "{out}");
    assert!(s.journal(&id).contains("\"sandbox-off\""));
    assert!(s.launcher_command("sandbox-claude").is_some(), "the sandboxed launcher stays defined for other runs");
    s.agrees(&id);

    // Off again changes nothing and says so; and everything config.json had before is still there.
    let out = s.ok(&["sandbox", "off", &id]);
    assert!(out.contains("isn't sandboxed"), "{out}");
    let before: serde_json::Value = serde_json::from_str(&config_before).unwrap();
    assert_eq!(s.json(&s.config())["launchers"].as_array().unwrap()[..2], before["launchers"].as_array().unwrap()[..]);
}

#[test]
fn enforce_with_no_record_writes_a_first_one_and_says_so() {
    let s = Scratch::new();
    let id = s.run("r", "claude");
    let out = s.ok(&["sandbox", "enforce", &id]);
    assert!(out.contains("had no sandbox record"), "{out}");
    assert_eq!(s.json(&s.record(&id))["mode"], "enforce");
    s.agrees(&id);
}

#[test]
fn words_in_front_of_claude_are_kept_all_the_way_through() {
    let s = Scratch::new();
    let id = s.run("curate", "legenda-curator");
    let original = s.launcher_command("legenda-curator").unwrap();
    let variable = format!("env LEGENDA_DIR={}", s.root.join("legenda").display());
    let wanted = format!("{variable} {} -- {}", s.wrapper(), s.claude().display());

    s.ok(&["sandbox", "observe", &id]);
    assert_eq!(s.launcher_of(&id), "sandbox-legenda-curator");
    assert_eq!(s.launcher_command("sandbox-legenda-curator").unwrap(), wanted);
    s.agrees(&id);

    s.ok(&["sandbox", "enforce", &id]);
    assert_eq!(s.launcher_of(&id), "sandbox-legenda-curator");
    assert_eq!(s.launcher_command("sandbox-legenda-curator").unwrap(), wanted);
    assert_eq!(s.json(&s.record(&id))["launcher"], "sandbox-legenda-curator");
    s.agrees(&id);

    s.ok(&["sandbox", "off", &id]);
    assert_eq!(s.launcher_of(&id), "legenda-curator");
    assert_eq!(s.launcher_command("legenda-curator").unwrap(), original, "the launcher it had is untouched");
    assert!(!s.record(&id).exists());
}

#[test]
fn a_run_that_is_over_is_refused() {
    let s = Scratch::new();
    let id = s.run("r", "claude");
    s.ok(&["stop", &id]);
    let run_before = std::fs::read(s.run_json(&id)).unwrap();
    let config_before = std::fs::read(s.config()).unwrap();
    for what in ["observe", "enforce", "off"] {
        let out = s.otto().args(["sandbox", what, &id]).output().unwrap();
        assert!(!out.status.success(), "{what} on a stopped run: {}", said(&out));
        assert!(said(&out).contains("stopped") && said(&out).contains("nothing was changed"), "{}", said(&out));
    }
    assert!(!s.record(&id).exists());
    assert_eq!(std::fs::read(s.run_json(&id)).unwrap(), run_before);
    assert_eq!(std::fs::read(s.config()).unwrap(), config_before);
}

#[test]
fn off_refuses_when_the_launcher_it_had_is_gone() {
    let s = Scratch::new();
    let id = s.run("curate", "legenda-curator");
    s.ok(&["sandbox", "observe", &id]);
    let mut config = s.json(&s.config());
    config["launchers"].as_array_mut().unwrap().retain(|l| l["name"] != "legenda-curator");
    std::fs::write(s.config(), config.to_string()).unwrap();
    let record_before = std::fs::read(s.record(&id)).unwrap();
    let out = s.otto().args(["sandbox", "off", &id]).output().unwrap();
    assert!(!out.status.success());
    assert!(said(&out).contains("legenda-curator") && said(&out).contains("nothing was changed"), "{}", said(&out));
    assert_eq!(std::fs::read(s.record(&id)).unwrap(), record_before);
    assert_eq!(s.launcher_of(&id), "sandbox-legenda-curator");
}

/// A run blocked for disagreeing with its record comes out of it once a command has made the two agree.
#[test]
fn a_run_blocked_by_its_record_is_unblocked_once_they_agree() {
    let s = Scratch::new();
    let id = s.run("r", "claude");
    s.ok(&["sandbox", "observe", &id]);
    // As a wake might: back to the unsandboxed launcher in run.json, which it can write.
    let mut run = s.json(&s.run_json(&id));
    run["launcher"]["kind"] = "claude".into();
    std::fs::write(s.run_json(&id), run.to_string()).unwrap();
    assert!(!s.otto().args(["wake", &id]).output().unwrap().status.success());
    assert_eq!(s.json(&s.run_json(&id))["status"], "blocked");
    assert_eq!(s.json(&s.run_json(&id))["gate"]["slug"], "sandbox-record");

    let out = s.ok(&["sandbox", "observe", &id]);
    assert!(out.contains("no longer blocked"), "{out}");
    let run = s.json(&s.run_json(&id));
    assert_ne!(run["status"], "blocked");
    assert!(run["gate"].is_null(), "{run}");
    assert_eq!(s.launcher_of(&id), "sandbox-claude");
}

/// From a wake (`OTTO_RUN_ID`) or a chat (`OTTO_CHAT`), all three are refused and nothing on disk changes.
#[test]
fn none_of_them_runs_from_a_wake_or_a_chat() {
    let s = Scratch::new();
    let id = s.run("r", "claude");
    let other = s.run("o", "claude");
    s.ok(&["sandbox", "observe", &id]);
    let files = [s.record(&id), s.run_json(&id), s.run_json(&other), s.config()];
    let before: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();
    for variable in ["OTTO_RUN_ID", "OTTO_CHAT"] {
        // Whichever run the variable names: the run's own, or another's.
        for from in [&id, &other] {
            for args in [vec!["observe", &id], vec!["enforce", &id, "--write", "/"], vec!["off", &id], vec!["observe", &other], vec!["enforce", &other]] {
                let out = s.otto().env(variable, from).arg("sandbox").args(&args).output().unwrap();
                assert!(!out.status.success(), "{variable}={from} otto sandbox {args:?} should be refused: {}", said(&out));
                let err = String::from_utf8_lossy(&out.stderr).to_string();
                assert!(err.contains("the person's to run") && err.contains("nothing was changed"), "{err}");
            }
        }
    }
    let after: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap()).collect();
    assert_eq!(before, after, "record, run.json and config.json are byte for byte what they were");
    assert!(!s.record(&other).exists(), "and no record was made for the other run");
}

/// Inside a sandbox the records' directory is closed to writing. The same from outside, by its permissions.
#[test]
fn observe_and_enforce_that_cannot_write_the_record_say_so_and_change_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new();
    let id = s.run("closed", "claude");
    s.ok(&["sandbox", "observe", &id]);
    let dir = s.record(&id).parent().unwrap().to_path_buf();
    let (record, run_json, config, journal) = (std::fs::read(s.record(&id)).unwrap(), std::fs::read(s.run_json(&id)).unwrap(), std::fs::read(s.config()).unwrap(), s.journal(&id));
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let outs: Vec<Output> = ["enforce", "observe"].iter().map(|what| s.otto().args(["sandbox", what, &id]).output().unwrap()).collect();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    for (what, out) in ["enforce", "observe"].iter().zip(&outs) {
        let text = said(out);
        assert!(!out.status.success(), "{text}");
        assert!(text.contains("couldn't be written") && text.contains(s.record(&id).to_str().unwrap()), "{text}");
        assert!(text.contains("Nothing was changed") && text.contains(&format!("run `otto sandbox {what} ")) && text.contains("outside a sandbox"), "{text}");
    }
    assert_eq!(std::fs::read(s.record(&id)).unwrap(), record, "the record is as it was");
    assert_eq!(std::fs::read(s.run_json(&id)).unwrap(), run_json, "run.json is as it was");
    assert_eq!(std::fs::read(s.config()).unwrap(), config, "config.json is as it was");
    assert_eq!(s.journal(&id), journal, "nothing was journalled");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
}
