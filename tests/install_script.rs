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

fn otto_stand_in(path: &Path, which: &str) {
    stand_in(path, &format!("echo \"{which} $*\" >> \"$STUB_LOG\""));
}

impl Sandbox {
    fn new(service: Service) -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("repo")).unwrap();
        fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh"), root.join("repo/install.sh")).unwrap();
        stand_in(&root.join("stubs/cargo"), "exit 0");
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
            .env("STUB_LOG", self.root.join("calls.log"))
            .output()
            .unwrap();
        assert!(output.status.success(), "install.sh failed: {}", String::from_utf8_lossy(&output.stderr));
        fs::read_to_string(self.root.join("calls.log")).unwrap()
    }

    fn installed(&self, name: &str) -> String {
        fs::read_to_string(self.root.join("bin").join(name)).unwrap()
    }

    /// What the stand-in for the binary on PATH says before any install replaces it.
    fn installed_before(&self) -> String {
        "#!/bin/sh\necho \"old $*\" >> \"$STUB_LOG\"\n".to_string()
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
