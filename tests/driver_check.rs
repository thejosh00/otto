//! tests/driver_check.rs — the driver's check (`scripts/driver-check.sh`) and a blocked agent,
//! through the real script and the real binary: a wake for one nobody has looked at, and none for
//! one a wake already recorded the way `workflows/drive-flow.md` tells it to.

mod support;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch `OTTO_HOME`, and a scratch `HOME` whose `.local/bin/otto` is the otto under test,
/// which is where the script looks for it.
struct Scratch {
    _dir: tempfile::TempDir,
    home: PathBuf,
    user: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (home, user) = (root.join("home"), root.join("user"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(user.join(".local/bin")).unwrap();
        std::os::unix::fs::symlink(support::otto_bin(), user.join(".local/bin/otto")).unwrap();
        Self {
            _dir: dir,
            home,
            user,
        }
    }

    fn otto(&self, args: &[&str]) -> String {
        let mut command = support::otto_command();
        command
            .args(args)
            .env("OTTO_HOME", &self.home)
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("TMUX");
        let output = command
            .output()
            .unwrap_or_else(|e| panic!("could not run otto {args:?}: {e}"));
        assert!(
            output.status.success(),
            "otto {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn run(&self, name: &str) -> String {
        self.otto(&["state", "init", "--goal", "a goal", "--name", name]);
        let list: Value = serde_json::from_str(&self.otto(&["state", "list", "--json"])).unwrap();
        list.as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// A driver waiting on a person, so a blocked agent is the only thing that could wake it.
    fn driver(&self) -> String {
        let id = self.run("driver");
        std::fs::create_dir_all(self.artifacts(&id)).unwrap();
        std::fs::write(self.artifacts(&id).join("waiting-on"), "hold\n").unwrap();
        id
    }

    fn artifacts(&self, id: &str) -> PathBuf {
        self.home.join("runs").join(id).join("artifacts")
    }

    fn block(&self, id: &str, reason: &str) {
        self.otto(&[
            "state",
            "set-status",
            id,
            "--status",
            "blocked",
            "--because",
            "instructions",
            "--reason",
            reason,
        ]);
    }

    fn unblock(&self, id: &str) {
        self.otto(&[
            "state",
            "set-status",
            id,
            "--status",
            "running",
            "--reason",
            "let go",
        ]);
    }

    /// When otto says it became blocked.
    fn blocked_at(&self, id: &str) -> String {
        let list: Value =
            serde_json::from_str(&self.otto(&["state", "list", "--status", "blocked", "--json"]))
                .unwrap();
        list.as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap()["blocked"]["at"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Block it again, at a later moment than `before`: otto's timestamps are to the second.
    fn block_again(&self, id: &str, before: &str) {
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(1100));
            self.block(id, "stuck again");
            if self.blocked_at(id) != before {
                return;
            }
            self.unblock(id);
        }
        panic!("it never got a new blocked time after {before}");
    }

    fn command(&self, program: &str, driver: &str) -> Command {
        let mut command = Command::new(program);
        let path = format!(
            "{}:{}",
            self.user.join(".local/bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        command
            .env("HOME", &self.user)
            .env("OTTO_HOME", &self.home)
            .env("OTTO_RUN_ID", driver)
            .env("PATH", path);
        command
            .env("OTTO_NOTIFY", "off")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("TMUX");
        command
    }

    /// The check, as otto runs it for the driver: its exit code and what it said.
    fn check(&self, driver: &str) -> (i32, String) {
        let output = self
            .command("/bin/sh", driver)
            .arg(support::repo_path("scripts/driver-check.sh"))
            .output()
            .unwrap();
        (output.status.code().unwrap(), text(&output))
    }

    /// What a wake does once it has looked: drive-flow.md's own command, from the run directory.
    fn record_seen(&self, driver: &str) {
        let flow = std::fs::read_to_string(support::repo_path("workflows/drive-flow.md")).unwrap();
        let line = flow
            .lines()
            .find(|l| l.starts_with("otto state list") && l.ends_with("> artifacts/blocked-seen"));
        let line =
            line.expect("drive-flow.md gives the command that writes artifacts/blocked-seen");
        let run_dir = self.artifacts(driver).parent().unwrap().to_path_buf();
        let output = self
            .command("/bin/sh", driver)
            .args(["-c", line])
            .current_dir(run_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{line}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn seen(&self, driver: &str) -> String {
        std::fs::read_to_string(self.artifacts(driver).join("blocked-seen")).unwrap_or_default()
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_string()
}

fn script() -> &'static Path {
    Path::new("scripts/driver-check.sh")
}

#[test]
fn a_blocked_agent_nobody_has_looked_at_wakes_the_driver_and_is_named() {
    let scratch = Scratch::new();
    let driver = scratch.driver();
    let builder = scratch.run("builder");
    assert_eq!(
        scratch.check(&driver),
        (0, "waiting on a person".to_string()),
        "nothing is blocked yet"
    );

    scratch.block(&builder, "token budget spent");
    let (code, said) = scratch.check(&driver);
    assert_eq!(code, 1, "{said}");
    assert_eq!(
        said,
        "an agent is blocked: builder",
        "{} names it",
        script().display()
    );
    assert!(said.len() < 200, "short enough for otto to keep: {said}");
}

#[test]
fn one_a_wake_recorded_does_not_wake_it_again_while_it_stays_blocked() {
    let scratch = Scratch::new();
    let driver = scratch.driver();
    let builder = scratch.run("builder");
    scratch.block(&builder, "token budget spent");
    assert_eq!(scratch.check(&driver).0, 1);

    scratch.record_seen(&driver);
    assert_eq!(
        scratch.seen(&driver),
        format!("{builder} {}\n", scratch.blocked_at(&builder))
    );
    for period in 2..=4 {
        assert_eq!(
            scratch.check(&driver),
            (0, "waiting on a person".to_string()),
            "period {period}"
        );
    }
}

#[test]
fn one_that_was_let_go_and_blocks_again_wakes_it() {
    let scratch = Scratch::new();
    let driver = scratch.driver();
    let builder = scratch.run("builder");
    scratch.block(&builder, "token budget spent");
    let first = scratch.blocked_at(&builder);
    scratch.record_seen(&driver);
    assert_eq!(scratch.check(&driver).0, 0);

    scratch.unblock(&builder);
    assert_eq!(scratch.check(&driver).0, 0, "nothing is blocked");
    scratch.block_again(&builder, &first);
    let (code, said) = scratch.check(&driver);
    assert_eq!(
        (code, said.as_str()),
        (1, "an agent is blocked: builder"),
        "seen: {}",
        scratch.seen(&driver)
    );
}

#[test]
fn a_different_agent_blocking_wakes_it_and_is_the_one_named() {
    let scratch = Scratch::new();
    let driver = scratch.driver();
    let builder = scratch.run("builder");
    let curator = scratch.run("curator");
    scratch.block(&builder, "token budget spent");
    scratch.record_seen(&driver);
    assert_eq!(scratch.check(&driver).0, 0);

    scratch.block(&curator, "can't sign in");
    let (code, said) = scratch.check(&driver);
    assert_eq!((code, said.as_str()), (1, "an agent is blocked: curator"));

    scratch.record_seen(&driver);
    assert_eq!(
        scratch.seen(&driver).lines().count(),
        2,
        "{}",
        scratch.seen(&driver)
    );
    assert_eq!(
        scratch.check(&driver).0,
        0,
        "and both are seen once a wake has looked again"
    );
}

#[test]
fn lines_for_agents_no_longer_blocked_wake_nothing() {
    let scratch = Scratch::new();
    let driver = scratch.driver();
    let builder = scratch.run("builder");
    let curator = scratch.run("curator");
    scratch.block(&builder, "token budget spent");
    scratch.block(&curator, "can't sign in");
    scratch.record_seen(&driver);
    assert_eq!(scratch.seen(&driver).lines().count(), 2);

    scratch.unblock(&curator);
    assert_eq!(
        scratch.check(&driver),
        (0, "waiting on a person".to_string()),
        "one line is stale"
    );
    scratch.unblock(&builder);
    assert_eq!(
        scratch.check(&driver),
        (0, "waiting on a person".to_string()),
        "both are"
    );
    std::fs::write(
        scratch.artifacts(&driver).join("blocked-seen"),
        "999 2020-01-01T00:00:00Z\nnot a line\n\n",
    )
    .unwrap();
    assert_eq!(
        scratch.check(&driver),
        (0, "waiting on a person".to_string()),
        "and lines about nothing"
    );
}

/// The line is the whole key: a seen agent's id with another time, or its time alone, is not it.
#[test]
fn only_the_exact_line_counts_as_seen() {
    let scratch = Scratch::new();
    let driver = scratch.driver();
    let builder = scratch.run("builder");
    scratch.block(&builder, "token budget spent");
    let at = scratch.blocked_at(&builder);
    let seen = scratch.artifacts(&driver).join("blocked-seen");
    for wrong in [
        format!("{builder} 2020-01-01T00:00:00Z\n"),
        format!("{builder}\n"),
        format!("{at}\n"),
        format!("1{builder} {at}\n"),
        String::new(),
    ] {
        std::fs::write(&seen, &wrong).unwrap();
        assert_eq!(scratch.check(&driver).0, 1, "seen: {wrong:?}");
    }
    std::fs::write(&seen, format!("{builder} {at}\n")).unwrap();
    assert_eq!(scratch.check(&driver).0, 0);
}
