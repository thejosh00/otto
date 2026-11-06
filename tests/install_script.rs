//! tests/install_script.rs — what `install.sh` does about the web UI service.
//!
//! A copy of the script runs against stand-ins for cargo and otto, under a throwaway `HOME`, so
//! nothing here builds, installs or touches launchd. Each stand-in logs which binary it is and how
//! it was called, which says whether the service was restarted and from which binary.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLIST: &str = "Library/LaunchAgents/com.joshuahill.otto-serve.plist";

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
    fn new(service_installed: bool) -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("repo")).unwrap();
        fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh"), root.join("repo/install.sh")).unwrap();
        stand_in(&root.join("stubs/cargo"), "exit 0");
        otto_stand_in(&root.join("repo/target/release/otto"), "new");
        otto_stand_in(&root.join("bin/otto"), "old");
        otto_stand_in(&root.join("bin/otto.previous"), "previous");
        fs::create_dir_all(root.join("home")).unwrap();
        if service_installed {
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
}

/// A landed web fix was marked verified while the running service still served the old page, because
/// the script only printed a note: the service has to be restarted from the binary just installed.
#[test]
fn an_install_restarts_an_installed_service_from_the_new_binary() {
    let calls = Sandbox::new(true).install(&[]);
    assert!(calls.lines().any(|c| c == "new service restart"), "{calls}");
}

/// The binary a rollback puts back is the one the service must serve.
#[test]
fn a_rollback_restarts_an_installed_service_from_the_binary_it_put_back() {
    let calls = Sandbox::new(true).install(&["--rollback"]);
    assert!(calls.lines().any(|c| c == "previous service restart"), "{calls}");
}

/// A machine that never wanted the web UI doesn't get one, or an error for not having one.
#[test]
fn no_service_installed_means_no_service_calls() {
    for args in [&[][..], &["--rollback"][..]] {
        let calls = Sandbox::new(false).install(args);
        assert!(!calls.contains("service"), "{args:?}: {calls}");
    }
}
