//! tests/install_script.rs — what `install.sh` does about the web UI service and the rollback copy.
//!
//! A copy of the script runs against stand-ins for cargo, launchctl and otto, under a throwaway
//! `HOME`, so nothing here builds, installs or touches launchd. Each stand-in logs which binary it is and how
//! it was called, which says whether the service was restarted and from which binary.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLIST: &str = "Library/LaunchAgents/com.joshuahill.otto-serve.plist";

/// Where the web UI service stands when the script runs: `otto service stop` leaves the plist
/// behind, so a stopped service and a running one differ only in what launchd has loaded.
#[derive(Clone, Copy)]
enum Service {
    NeverInstalled,
    Stopped,
    Running,
}

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn stand_in(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// What an otto stand-in does: log the call, run `extra` (what makes a build good or bad), and answer
/// `state list --json` with the runs under OTTO_HOME, as the real one does.
fn otto_body(which: &str, extra: &str) -> String {
    format!(
        "echo \"{which} $*\" >> \"$STUB_LOG\"\n{extra}\n\
         if [ \"$*\" = \"state list --json\" ]; then printf '['; sep=; \
         for d in \"$OTTO_HOME\"/runs/*/; do [ -d \"$d\" ] || continue; \
         printf '%s{{\"id\":\"%s\"}}' \"$sep\" \"$(basename \"$d\")\"; sep=,; done; echo ']'; fi"
    )
}

fn otto_stand_in(path: &Path, which: &str) {
    stand_in(path, &otto_body(which, ""));
}

/// A stand-in whose behaviour beyond logging is `extra`, shell run before it answers.
fn otto_stand_in_with(path: &Path, which: &str, extra: &str) {
    stand_in(path, &otto_body(which, extra));
}

fn which(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

impl Sandbox {
    fn new(service: Service) -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("repo")).unwrap();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        fs::copy(manifest.join("install.sh"), root.join("repo/install.sh")).unwrap();
        fs::create_dir_all(root.join("repo/scripts")).unwrap();
        for script in ["rehearse.sh", "migration-check.sh"] {
            let to = root.join("repo/scripts").join(script);
            // main's install.sh (for the evidence run) has no rehearse.sh beside it in the tree.
            if manifest.join("scripts").join(script).exists() {
                fs::copy(manifest.join("scripts").join(script), &to).unwrap();
                fs::set_permissions(&to, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        fs::create_dir_all(root.join("tmp")).unwrap();
        stand_in(&root.join("stubs/cargo"), "exit 0");
        // jq merges the config on a rollback; the stubs directory is all of PATH beyond /usr/bin:/bin.
        if let Some(jq) = which("jq") {
            std::os::unix::fs::symlink(jq, root.join("stubs/jq")).unwrap();
        }
        // 113 is what launchctl print really exits with for a job that isn't loaded.
        let loaded = if matches!(service, Service::Running) { 0 } else { 113 };
        stand_in(&root.join("stubs/launchctl"), &format!("exit {loaded}"));
        otto_stand_in(&root.join("repo/target/release/otto"), "new");
        otto_stand_in(&root.join("bin/otto"), "old");
        otto_stand_in(&root.join("bin/otto.previous"), "previous");
        fs::create_dir_all(root.join("home")).unwrap();
        if !matches!(service, Service::NeverInstalled) {
            let plist = root.join("home").join(PLIST);
            fs::create_dir_all(plist.parent().unwrap()).unwrap();
            fs::write(plist, "").unwrap();
        }
        Sandbox { _dir: dir, root }
    }

    /// Run the script and return what the otto stand-ins were asked to do, one call a line.
    fn install(&self, args: &[&str]) -> String {
        let output = Command::new("/bin/bash")
            .arg(self.root.join("repo/install.sh"))
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.root.join("stubs").display()))
            .env("OTTO_BIN_DIR", self.root.join("bin"))
            .env("OTTO_HOME", self.root.join("otto-home"))
            .env("STUB_LOG", self.root.join("calls.log"))
            .env("TMPDIR", self.root.join("tmp"))
            .output()
            .unwrap();
        assert!(output.status.success(), "install.sh failed: {}", String::from_utf8_lossy(&output.stderr));
        fs::read_to_string(self.root.join("calls.log")).unwrap()
    }

    /// Run the script and return its exit status and stdout+stderr, whatever it exited with.
    fn run(&self, args: &[&str]) -> (bool, String) {
        let output = Command::new("/bin/bash")
            .arg(self.root.join("repo/install.sh"))
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.root.join("stubs").display()))
            .env("OTTO_BIN_DIR", self.root.join("bin"))
            .env("OTTO_HOME", self.root.join("otto-home"))
            .env("STUB_LOG", self.root.join("calls.log"))
            .env("TMPDIR", self.root.join("tmp"))
            .output()
            .unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        (output.status.success(), text)
    }

    fn otto_home(&self) -> PathBuf {
        self.root.join("otto-home")
    }

    /// A small otto home: two runs with a run.json and a journal, a config.
    fn with_runs(&self) -> &Sandbox {
        let home = self.otto_home();
        for (id, goal) in [("7", "Build things"), ("8", "Watch things")] {
            let run = home.join("runs").join(id);
            fs::create_dir_all(&run).unwrap();
            fs::write(
                run.join("run.json"),
                format!(
                    r#"{{"schemaVersion": 2, "id": "{id}", "name": "run-{id}", "goal": "{goal}", "doneCondition": null,
 "status": "sleeping", "nextWakeAt": null, "gate": null, "policy": {{"periodMinutes": 15}}}}"#
                ),
            )
            .unwrap();
            fs::write(run.join("journal.jsonl"), "{\"n\":1,\"kind\":\"wake-start\"}\n{\"n\":2,\"kind\":\"wake-end\"}\n").unwrap();
        }
        fs::write(home.join("runs/.last-id"), "8\n").unwrap();
        self.write_config(&serde_json::json!({"workdir": "/w", "launchers": [{"name": "plain", "command": ["claude"]}]}));
        self
    }

    /// Every file in the otto home but logs/, with its bytes.
    fn home_files(&self) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path.strip_prefix(base).unwrap().to_path_buf();
                if rel.starts_with("logs") {
                    continue;
                }
                if path.is_dir() {
                    walk(&path, base, out);
                } else {
                    out.push((rel, fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.otto_home(), &self.otto_home(), &mut out);
        out.sort();
        out
    }

    fn tmp_is_empty(&self) -> bool {
        fs::read_dir(self.root.join("tmp")).unwrap().next().is_none()
    }

    fn new_build(&self, extra: &str) {
        otto_stand_in_with(&self.root.join("repo/target/release/otto"), "new", extra);
    }

    fn rehearsal_log(&self) -> String {
        fs::read_to_string(self.otto_home().join("logs/rehearsal.log")).unwrap_or_default()
    }

    /// The install was refused: the old otto is in place, no rollback copy made, config untouched.
    fn assert_refused(&self, ok: bool, out: &str, config_before: &[u8]) {
        assert!(!ok, "install should have been refused: {out}");
        assert_eq!(self.installed("otto"), self.installed_before(), "{out}");
        assert!(!self.root.join("bin/otto.previous").exists() || self.installed("otto.previous").contains("previous"));
        assert_eq!(fs::read(self.config()).unwrap(), config_before);
        assert!(out.contains("install stopped"), "{out}");
        assert!(out.contains("logs/rehearsal.log"), "{out}");
        assert!(!self.rehearsal_log().is_empty(), "no rehearsal log");
        assert!(self.tmp_is_empty(), "the copy of the home was left in TMPDIR");
    }

    fn config(&self) -> PathBuf {
        self.root.join("otto-home/config.json")
    }

    fn write_config(&self, value: &serde_json::Value) {
        fs::create_dir_all(self.root.join("otto-home")).unwrap();
        fs::write(self.config(), serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    fn read_json(&self, path: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn installed(&self, name: &str) -> String {
        fs::read_to_string(self.root.join("bin").join(name)).unwrap()
    }

    /// What the stand-in for the binary on PATH says before any install replaces it.
    fn installed_before(&self) -> String {
        format!("#!/bin/sh\n{}\n", otto_body("old", ""))
    }
}

/// A landed web fix was marked verified while the running service still served the old page, because
/// the script only printed a note: the service has to be restarted from the binary just installed.
#[test]
fn an_install_restarts_a_running_service_from_the_new_binary() {
    let calls = Sandbox::new(Service::Running).install(&[]);
    assert!(calls.lines().any(|c| c == "new service restart"), "{calls}");
}

/// A change that doesn't alter the binary used to overwrite the rollback copy with the bad build
/// itself, so a rollback put back the same binary and said it had rolled back.
#[test]
fn an_install_of_the_same_build_keeps_what_a_rollback_puts_back() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.install(&[]); // the bad build replaces "old"
    sandbox.install(&[]); // a docs-only change: cargo leaves the same binary
    assert_eq!(sandbox.installed("otto.previous"), sandbox.installed_before());
    sandbox.install(&["--rollback"]);
    assert_eq!(sandbox.installed("otto"), sandbox.installed_before());
}

/// A build that does differ moves the rollback copy on, as it always has.
#[test]
fn an_install_of_a_different_build_keeps_the_one_it_replaces() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.install(&[]);
    assert_eq!(sandbox.installed("otto.previous"), sandbox.installed_before());
    otto_stand_in(&sandbox.root.join("repo/target/release/otto"), "newer");
    let new = sandbox.installed("otto");
    sandbox.install(&[]);
    assert_eq!(sandbox.installed("otto.previous"), new);
}

/// The binary a rollback puts back is the one the service must serve.
#[test]
fn a_rollback_restarts_a_running_service_from_the_binary_it_put_back() {
    let calls = Sandbox::new(Service::Running).install(&["--rollback"]);
    assert!(calls.lines().any(|c| c == "previous service restart"), "{calls}");
}

/// `otto service stop` is a decision, and this script runs after every landing: restarting whenever
/// the plist exists undid that decision at the next install.
#[test]
fn a_stopped_service_stays_stopped() {
    for args in [&[][..], &["--rollback"][..]] {
        let calls = Sandbox::new(Service::Stopped).install(args);
        assert!(!calls.contains("service"), "{args:?}: {calls}");
    }
}

/// Asking for the service is how to bring a stopped one back through this script.
#[test]
fn asking_for_the_service_starts_a_stopped_one() {
    let calls = Sandbox::new(Service::Stopped).install(&["--service"]);
    assert!(calls.lines().any(|c| c == "new service start"), "{calls}");
}

/// A machine that never wanted the web UI doesn't get one, or an error for not having one.
#[test]
fn no_service_installed_means_no_service_calls() {
    for args in [&[][..], &["--rollback"][..]] {
        let calls = Sandbox::new(Service::NeverInstalled).install(args);
        assert!(!calls.contains("service"), "{args:?}: {calls}");
    }
}

/// An install from a temporary clone (otto's own, after a landing) must leave the skills linked to
/// the main checkout: the clone is gone afterwards.
#[test]
fn the_skills_are_linked_to_the_checkout_named_not_the_one_installed_from() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    let calls = sandbox.install(&["--skills-repo", "/the/main/checkout"]);
    assert!(calls.lines().any(|c| c == "new install --repo /the/main/checkout"), "{calls}");
    let calls = Sandbox::new(Service::NeverInstalled).install(&[]);
    assert!(calls.lines().any(|c| c.starts_with("new install --repo ") && c.ends_with("/repo")), "{calls}");
}

/// An older binary refuses a config.json with keys it doesn't know, so a rollback that put back only
/// the binary could leave otto unable to start. The config goes back with it, except for launchers
/// added since: a run moved into a `sandbox-*` launcher must still find the launcher it names.
#[test]
fn a_rollback_restores_the_config_from_the_install_it_undoes_keeping_launchers_added_since() {
    assert!(which("jq").is_some(), "this test needs jq on PATH");
    let sandbox = Sandbox::new(Service::NeverInstalled);
    let bin = sandbox.root.join("bin");
    let config_a = serde_json::json!({
        "workdir": "/work/a",
        "launchers": [{"name": "plain", "command": ["claude"]}]
    });
    sandbox.write_config(&config_a);
    let build_a = sandbox.installed("otto"); // "old" stands for build A, already installed

    // Build B replaces A: both A and the config A ran with are kept.
    sandbox.install(&[]);
    let build_b = sandbox.installed("otto");
    assert_ne!(build_a, build_b);
    assert_eq!(sandbox.installed("otto.previous"), build_a);
    let snapshot = bin.join("otto.previous.config.json");
    assert_eq!(sandbox.read_json(&snapshot), config_a);
    let snapshot_time = fs::metadata(&snapshot).unwrap().modified().unwrap();
    let previous_time = fs::metadata(bin.join("otto.previous")).unwrap().modified().unwrap();

    // B again, with the config changed meanwhile: neither snapshot is replaced.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    sandbox.write_config(&serde_json::json!({"workdir": "/work/b", "launchers": []}));
    sandbox.install(&[]);
    assert_eq!(sandbox.installed("otto.previous"), build_a);
    assert_eq!(fs::metadata(bin.join("otto.previous")).unwrap().modified().unwrap(), previous_time);
    assert_eq!(sandbox.read_json(&snapshot), config_a);
    assert_eq!(fs::metadata(&snapshot).unwrap().modified().unwrap(), snapshot_time);

    // Since B: a run moved into sandbox-x, plain's definition changed, the workdir changed.
    let config_b = serde_json::json!({
        "workdir": "/work/b",
        "launchers": [
            {"name": "plain", "command": ["claude", "--changed"]},
            {"name": "sandbox-x", "command": ["sandbox-exec", "claude"], "sandbox": true}
        ]
    });
    sandbox.write_config(&config_b);

    let output = Command::new("/bin/bash")
        .arg(sandbox.root.join("repo/install.sh"))
        .arg("--rollback")
        .env_clear()
        .env("HOME", sandbox.root.join("home"))
        .env("PATH", format!("{}:/usr/bin:/bin", sandbox.root.join("stubs").display()))
        .env("OTTO_BIN_DIR", &bin)
        .env("OTTO_HOME", sandbox.root.join("otto-home"))
        .env("STUB_LOG", sandbox.root.join("calls.log"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}{}", String::from_utf8_lossy(&output.stderr));

    assert_eq!(sandbox.installed("otto"), build_a);
    let restored = sandbox.read_json(&sandbox.config());
    assert_eq!(restored["workdir"], "/work/a", "{restored}");
    assert_eq!(
        restored["launchers"],
        serde_json::json!([
            {"name": "plain", "command": ["claude"]},
            {"name": "sandbox-x", "command": ["sandbox-exec", "claude"], "sandbox": true}
        ]),
        "{restored}"
    );
    let saved = sandbox.root.join("otto-home/config.json.before-rollback");
    assert_eq!(sandbox.read_json(&saved), config_b);
    assert!(!snapshot.exists(), "the snapshot should be gone after the rollback");
    assert!(!bin.join("otto.previous").exists());
    assert!(stdout.contains("kept launcher sandbox-x"), "{stdout}");
    assert!(!stdout.contains("kept launcher plain"), "{stdout}");
    assert!(stdout.contains("config.json.before-rollback"), "{stdout}");
}

/// A rollback with nothing to restore the config from puts back the binary and says so.
#[test]
fn a_rollback_without_a_config_snapshot_leaves_the_config_alone() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    let config = serde_json::json!({"workdir": "/work/now"});
    sandbox.write_config(&config);
    sandbox.install(&["--rollback"]);
    assert_eq!(sandbox.read_json(&sandbox.config()), config);
    assert!(!sandbox.root.join("otto-home/config.json.before-rollback").exists());
}

/// A config the merge can't read leaves config.json as it is, after the binary is back, and the
/// script says so by failing.
#[test]
fn a_rollback_whose_config_merge_fails_restores_the_binary_and_leaves_the_config() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    let bin = sandbox.root.join("bin");
    fs::write(bin.join("otto.previous.config.json"), "{not json").unwrap();
    let config = serde_json::json!({"workdir": "/work/now"});
    sandbox.write_config(&config);
    let previous = sandbox.installed("otto.previous");
    let output = Command::new("/bin/bash")
        .arg(sandbox.root.join("repo/install.sh"))
        .arg("--rollback")
        .env_clear()
        .env("HOME", sandbox.root.join("home"))
        .env("PATH", format!("{}:/usr/bin:/bin", sandbox.root.join("stubs").display()))
        .env("OTTO_BIN_DIR", &bin)
        .env("OTTO_HOME", sandbox.root.join("otto-home"))
        .env("STUB_LOG", sandbox.root.join("calls.log"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("left untouched"), "{stderr}");
    assert_eq!(sandbox.installed("otto"), previous);
    assert_eq!(sandbox.read_json(&sandbox.config()), config);
    assert!(bin.join("otto.previous.config.json").exists());
}

/// A new build that rewrites a run's goal when it reads the home must not replace the one that
/// doesn't: the rehearsal on a copy catches it, and the real home is never touched.
#[test]
fn a_build_that_rewrites_a_goal_is_refused() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.with_runs();
    sandbox.new_build(r#"[ "$1" = ls ] && perl -pi -e 's/Build things/Built/' "$OTTO_HOME/runs/7/run.json""#);
    let before = sandbox.home_files();
    let config = fs::read(sandbox.config()).unwrap();
    let (ok, out) = sandbox.run(&[]);
    sandbox.assert_refused(ok, &out, &config);
    assert!(out.contains("7 goal: hash changed"), "{out}");
    assert!(sandbox.rehearsal_log().contains("Must not change (1)"), "{}", sandbox.rehearsal_log());
    assert_eq!(sandbox.home_files(), before, "the real home changed");
}

/// A journal line rewritten is lost history: refused, naming the journal.
#[test]
fn a_build_that_rewrites_a_journal_line_is_refused() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.with_runs();
    sandbox.new_build(r#"[ "$1" = ls ] && perl -pi -e 's/wake-end/wake-over/' "$OTTO_HOME/runs/8/journal.jsonl""#);
    let config = fs::read(sandbox.config()).unwrap();
    let (ok, out) = sandbox.run(&[]);
    sandbox.assert_refused(ok, &out, &config);
    assert!(out.contains("8 journal"), "{out}");
}

#[test]
fn a_build_that_cant_list_a_run_is_refused() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.with_runs();
    sandbox.new_build(r#"if [ "$*" = "state list --json" ]; then echo '[{"id":"7"}]'; exit 0; fi"#);
    let config = fs::read(sandbox.config()).unwrap();
    let (ok, out) = sandbox.run(&[]);
    sandbox.assert_refused(ok, &out, &config);
    assert!(out.contains("can't read run 8"), "{out}");
}

#[test]
fn a_build_whose_ls_fails_is_refused() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.with_runs();
    sandbox.new_build(r#"[ "$1" = ls ] && { echo "unknown schemaVersion" >&2; exit 3; }"#);
    let config = fs::read(sandbox.config()).unwrap();
    let (ok, out) = sandbox.run(&[]);
    sandbox.assert_refused(ok, &out, &config);
    assert!(out.contains("otto ls"), "{out}");
}

/// A command failing with both builds is only shown: a broken record must not block its own fix.
#[test]
fn a_command_failing_with_both_builds_does_not_stop_the_install() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.with_runs();
    let fails = r#"[ "$1" = changes ] && exit 4"#;
    otto_stand_in_with(&sandbox.root.join("bin/otto"), "old", fails);
    sandbox.new_build(&format!("{fails}\necho fixed-elsewhere >/dev/null"));
    let (ok, out) = sandbox.run(&[]);
    assert!(ok, "{out}");
    assert!(out.contains("failed with both builds"), "{out}");
}

/// Output that only reads differently is shown, never a stop; the real home and TMPDIR are as before.
#[test]
fn a_build_that_only_prints_differently_installs_and_logs_the_diff() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.with_runs();
    sandbox.new_build(r#"[ "$1" = ls ] && echo "A NEW COLUMN""#);
    let before = sandbox.home_files();
    let (ok, out) = sandbox.run(&[]);
    assert!(ok, "{out}");
    assert!(sandbox.installed("otto").contains("new"));
    assert!(out.contains("otto ls: differs"), "{out}");
    let log = sandbox.rehearsal_log();
    assert!(log.contains("+A NEW COLUMN"), "{log}");
    assert!(log.contains("verdict: ok"), "{log}");
    assert_eq!(sandbox.home_files(), before, "the real home changed");
    assert!(sandbox.tmp_is_empty(), "the copy of the home was left in TMPDIR");
}

/// The previous log is kept once, like logs/poke.log.
#[test]
fn the_previous_rehearsal_log_is_kept() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.with_runs();
    let (ok, out) = sandbox.run(&[]);
    assert!(ok, "{out}");
    otto_stand_in(&sandbox.root.join("repo/target/release/otto"), "newer");
    let (ok, out) = sandbox.run(&[]);
    assert!(ok, "{out}");
    assert!(sandbox.otto_home().join("logs/rehearsal.log.1").exists());
}

/// A release that changes a recorded fact on purpose installs with --skip-rehearsal, which still
/// checks the new build's `otto ls`.
#[test]
fn skip_rehearsal_installs_what_the_rehearsal_would_refuse() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.with_runs();
    sandbox.new_build(r#"[ "$1" = ls ] && perl -pi -e 's/Build things/Built/' "$OTTO_HOME/runs/7/run.json""#);
    let (ok, out) = sandbox.run(&["--skip-rehearsal"]);
    assert!(ok, "{out}");
    assert!(sandbox.installed("otto").contains("new"));
    assert!(sandbox.rehearsal_log().is_empty());
    let calls = fs::read_to_string(sandbox.root.join("calls.log")).unwrap();
    assert!(calls.lines().any(|c| c == "new ls"), "{calls}");
}

/// A first install has nothing to compare with: no rehearsal, the bare read check.
#[test]
fn a_first_install_is_not_rehearsed() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.with_runs();
    fs::remove_file(sandbox.root.join("bin/otto")).unwrap();
    let (ok, out) = sandbox.run(&[]);
    assert!(ok, "{out}");
    assert!(!out.contains("rehearsing"), "{out}");
    assert!(sandbox.rehearsal_log().is_empty());
    assert!(sandbox.installed("otto").contains("new"));
}

/// --snapshot keeps a tarball of the home under ~/.otto-snapshots and never overwrites one.
#[test]
fn a_snapshot_is_kept_once_per_phase_and_day() {
    let sandbox = Sandbox::new(Service::NeverInstalled);
    sandbox.with_runs();
    let rehearse = |phase: &str| {
        Command::new("/bin/bash")
            .arg(sandbox.root.join("repo/scripts/rehearse.sh"))
            .args(["--new", sandbox.root.join("repo/target/release/otto").to_str().unwrap()])
            .args(["--installed", sandbox.root.join("bin/otto").to_str().unwrap()])
            .args(["--snapshot", phase])
            .env_clear()
            .env("HOME", sandbox.root.join("home"))
            .env("PATH", format!("{}:/usr/bin:/bin", sandbox.root.join("stubs").display()))
            .env("OTTO_HOME", sandbox.otto_home())
            .env("STUB_LOG", sandbox.root.join("calls.log"))
            .env("TMPDIR", sandbox.root.join("tmp"))
            .output()
            .unwrap()
    };
    let first = rehearse("cutover");
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let snaps: Vec<_> = fs::read_dir(sandbox.root.join("home/.otto-snapshots")).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(snaps.len(), 1, "{snaps:?}");
    let name = snaps[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(name.ends_with("-cutover.tar.gz"), "{name}");
    assert!(String::from_utf8_lossy(&first.stdout).contains(&name));
    let listing = Command::new("tar").arg("-tzf").arg(&snaps[0]).output().unwrap();
    assert!(String::from_utf8_lossy(&listing.stdout).contains("otto-home/runs/7/run.json"));
    let again = rehearse("cutover");
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("already exists"));
    assert!(sandbox.tmp_is_empty());
}
