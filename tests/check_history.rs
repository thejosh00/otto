//! tests/check_history.rs — `check-history.jsonl` through the real binary: a real `otto poke` running
//! a real script on a due run, with a stub for the model. Every run is `--detach none`, so nothing
//! needs tmux, and poke's wakes run as detached processes, which `wait_for` waits out.

mod support;

use serde_json::Value;
use std::path::PathBuf;
use std::process::Output;
use std::time::{Duration, Instant};

/// The bound in `src/check_history.rs`, here too so that changing it is a decision.
const CAP: usize = 700;

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let stub = root.join("stub");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nprintf '# h\\n## Goal\\ng\\n## Where this is\\nHere.\\n## Decided\\nn\\n## This wake did\\nn\\n## Next wake must\\nn\\n## Needs\\nnothing\\n## Don'\"'\"'t re-derive\\nn\\n' | '{}' state handoff \"$OTTO_RUN_ID\" --stdin >/dev/null\n",
                support::OTTO,
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
        support::otto_command()
            .args(args)
            .current_dir(&self.root)
            .env("OTTO_HOME", self.root.join("home"))
            .env("TMUX_TMPDIR", &self.root)
            .env("OTTO_BIN", support::OTTO)
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

    /// A sleeping run whose first wake is over, with a standing check whose behaviour `says` decides:
    /// it sources that file, which starts out saying "exit 0". Returns the run's id. Setting a check
    /// runs it once on the spot: a trial, which must leave no record.
    fn run_with_check(&self) -> String {
        let root = self.root.to_str().unwrap();
        let name = "checked";
        self.ok(&["run", "--goal", "g", "--name", name, "--perpetual", "--period", "1h", "--detach", "none", "--workdir", root]);
        let list: Value = serde_json::from_str(&self.ok(&["state", "list", "--json"])).unwrap();
        let id = list.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap()["id"].as_str().unwrap().to_string();

        self.says("exit 0");
        let script = self.root.join("check-source.sh");
        std::fs::write(&script, format!("#!/bin/sh\n. '{root}/says'\n")).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        self.ok(&["check", &id, "--script", script.to_str().unwrap()]);
        id
    }

    fn says(&self, script: &str) {
        std::fs::write(self.root.join("says"), format!("{script}\n")).unwrap();
    }

    fn run_dir(&self, id: &str) -> PathBuf {
        self.root.join("home/runs").join(id)
    }

    fn history_path(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("check-history.jsonl")
    }

    /// Every line, each of which must parse.
    fn history(&self, id: &str) -> Vec<Value> {
        std::fs::read_to_string(self.history_path(id))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
            .collect()
    }

    fn state(&self, id: &str) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.run_dir(id).join("run.json")).unwrap()).unwrap()
    }

    /// Bring the run's sleep to its end, as the period would.
    fn make_due(&self, id: &str) {
        let path = self.run_dir(id).join("run.json");
        let mut state = self.state(id);
        assert_eq!(state["status"], "sleeping", "{state:#}");
        state["nextWakeAt"] = "2020-01-01T00:00:00Z".into();
        std::fs::write(path, serde_json::to_string_pretty(&state).unwrap()).unwrap();
    }

    fn journal(&self, id: &str) -> Vec<Value> {
        std::fs::read_to_string(self.run_dir(id).join("journal.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn wakes_started(&self, id: &str) -> usize {
        self.journal(id).iter().filter(|l| l["event"] == "wake-started").count()
    }

    /// Wait for a wake started in another process to have finished, and the run to be asleep.
    fn wait_for(&self, id: &str, wakes: usize) {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let finished = self.journal(id).iter().filter(|l| l["event"] == "wake-complete" || l["event"] == "wake-incomplete").count();
            if finished >= wakes && self.state(id)["status"] == "sleeping" {
                return;
            }
            assert!(Instant::now() < until, "{id} never got to {wakes} wakes: {:#?}", self.journal(id));
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// One pass of poke over the due run. Returns what it did, and whether it started a wake.
    fn pass(&self, id: &str) -> (String, bool) {
        let before = self.wakes_started(id);
        // The wake before this one can have one last write of `run.json` in hand when it looks
        // finished, and this helper's edit takes no lock: if that undid it, do it again.
        let until = Instant::now() + Duration::from_secs(30);
        let said = loop {
            self.make_due(id);
            let said = self.ok(&["poke", "--verbose"]);
            if !said.lines().any(|l| l.contains(" skip ") && l.contains(&format!(" {id} ")) && l.contains("due at")) {
                break said;
            }
            assert!(Instant::now() < until, "{id} never came due: {said}");
            std::thread::sleep(Duration::from_millis(100));
        };
        let woke = said.lines().any(|l| l.contains(" spawn ") && l.contains(&format!(" {id} ")));
        if woke {
            self.wait_for(id, before + 1);
        }
        (said, woke)
    }
}

/// The record says what the script did, and is what `run.json` says about the same pass.
fn assert_latest(s: &Scratch, id: &str, count: usize, code: i64, timed_out: bool, result: &str, note: Option<&str>) {
    let history = s.history(id);
    assert_eq!(history.len(), count, "one record a pass: {history:#?}");
    let latest = history.last().unwrap();
    assert_eq!(latest["code"], code, "{latest}");
    assert_eq!(latest["timedOut"], timed_out, "{latest}");
    assert_eq!(latest["result"], result, "{latest}");
    assert_eq!(latest["note"].as_str(), note, "{latest}");

    let check = &s.state(id)["check"];
    assert_eq!(latest["at"], check["lastAt"], "{latest} {check}");
    assert_eq!(latest["result"], check["lastResult"], "{latest} {check}");
    assert_eq!(latest["note"].as_str(), check["lastNote"].as_str(), "{latest} {check}");
}

#[test]
fn every_pass_poke_runs_leaves_one_record() {
    let s = Scratch::new();
    let id = s.run_with_check();

    s.says("echo quiet; exit 0");
    let (said, woke) = s.pass(&id);
    assert!(!woke, "{said}");
    assert_latest(&s, &id, 1, 0, false, "no-change", Some("quiet"));
    assert!(!s.journal(&id).iter().any(|l| l["event"] == "check-ran"), "nothing new stays out of the journal: {:#?}", s.journal(&id));

    s.says("exit 0");
    let (said, woke) = s.pass(&id);
    assert!(!woke, "{said}");
    assert_latest(&s, &id, 2, 0, false, "no-change", None);

    s.says("echo 2 new; exit 1");
    let (said, woke) = s.pass(&id);
    assert!(woke, "{said}");
    assert_latest(&s, &id, 3, 1, false, "changed", Some("2 new"));
    let ran: Vec<_> = s.journal(&id).into_iter().filter(|l| l["event"] == "check-ran").collect();
    assert_eq!(ran.len(), 1, "check-ran is as it was: {ran:#?}");

    s.says("echo broken; exit 7");
    let (said, woke) = s.pass(&id);
    assert!(woke, "{said}");
    assert_latest(&s, &id, 4, 7, false, "error", Some("broken"));

    // Poke's real limit, so this takes as long as that.
    s.says("sleep 60");
    let (said, woke) = s.pass(&id);
    assert!(woke, "{said}");
    assert_latest(&s, &id, 5, 124, true, "error", None);

    let times: Vec<_> = s.history(&id).iter().map(|r| r["at"].as_str().unwrap().to_string()).collect();
    assert!(times.windows(2).all(|t| t[0] <= t[1]), "oldest first: {times:?}");
    assert!(times[0].as_str() > "2026-", "the time it ran: {times:?}");
}

#[test]
fn the_file_keeps_the_newest_records_at_its_cap() {
    let s = Scratch::new();
    let id = s.run_with_check();
    let old: String = (0..CAP + 5).map(|n| format!("{{\"n\":{n}}}\n")).collect();
    std::fs::write(s.history_path(&id), old).unwrap();

    s.says("echo newest; exit 0");
    s.pass(&id);

    let history = s.history(&id);
    assert_eq!(history.len(), CAP);
    assert_eq!(history[0]["n"], 6, "the oldest went first: {}", history[0]);
    assert_eq!(history[CAP - 2]["n"], (CAP + 4) as u64, "in order, up to the newest of the old");
    assert_eq!(history[CAP - 1]["note"], "newest", "and the record just made is never what goes");

    let strays: Vec<_> = std::fs::read_dir(s.run_dir(&id))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(strays.is_empty(), "{strays:?}");
}

/// A directory where the file should be: it can be neither read nor replaced.
#[test]
fn a_write_that_cannot_happen_changes_nothing_else() {
    let s = Scratch::new();
    let id = s.run_with_check();
    std::fs::create_dir(s.history_path(&id)).unwrap();

    s.says("echo quiet; exit 0");
    let (said, woke) = s.pass(&id);
    assert!(!woke, "nothing new still sleeps: {said}");
    let state = s.state(&id);
    assert_eq!(state["check"]["lastResult"], "no-change");
    assert_eq!(state["check"]["lastNote"], "quiet");
    assert_eq!(state["check"]["consecutiveNoChange"], 1);
    assert!(state["nextWakeAt"].as_str().unwrap() > "2026-", "and sleeps again: {state:#}");

    s.says("echo 2 new; exit 1");
    let (said, woke) = s.pass(&id);
    assert!(woke, "a change still wakes it: {said}");
    assert_eq!(s.state(&id)["check"]["lastResult"], "changed");
    assert!(s.history_path(&id).is_dir(), "and nothing took the path over");
}

#[test]
fn a_dry_run_and_the_trial_run_write_nothing() {
    let s = Scratch::new();
    let id = s.run_with_check();
    assert!(!s.history_path(&id).exists(), "setting a check runs it once, as a trial");

    s.ok(&["check", &id]);
    assert!(!s.history_path(&id).exists(), "looking at a check");

    s.make_due(&id);
    let said = s.ok(&["poke", "--dry-run", "--verbose"]);
    assert!(said.contains("would run check script"), "{said}");
    assert!(!s.history_path(&id).exists(), "a dry run");

    s.pass(&id);
    assert_eq!(s.history(&id).len(), 1, "and the real pass after them does");
}

/// A wake that did nothing, with the file having grown since the wake before: otto's own look at the
/// run still finds it hasn't moved.
#[test]
fn records_added_between_wakes_are_not_the_run_moving() {
    let s = Scratch::new();
    let id = s.run_with_check();
    s.ok(&["wake", &id]);
    s.wait_for(&id, 2);
    let before = s.state(&id)["ticksWithoutProgress"].as_i64().unwrap();

    s.says("echo quiet; exit 0");
    s.pass(&id);
    s.pass(&id);
    assert_eq!(s.history(&id).len(), 2);

    s.ok(&["wake", &id]);
    s.wait_for(&id, 3);
    let journal = s.journal(&id);
    let last = journal.iter().rev().find(|l| l["event"] == "wake-complete").unwrap();
    assert_eq!(last["progress"], false, "{last}");
    assert_eq!(s.state(&id)["ticksWithoutProgress"], before + 1);
}
