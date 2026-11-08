//! tests/launchd_restart.rs — restarting a launchd job when launchd hasn't finished removing it.
//!
//! `launchctl bootout` can return before the job is gone, and a `bootstrap` that arrives in that
//! gap is refused. On Nov 7 that left the web service unloaded, where `KeepAlive` can't help, until
//! someone noticed. The real binary runs against a stand-in for launchctl under a throwaway `HOME`,
//! so nothing here touches launchd, and the race happens every time rather than once in a while.

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Output;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Sandbox {
    /// A launchctl that refuses this many bootstraps, as launchd does while it is still removing
    /// the job, then accepts them.
    fn refusing(refusals: u32) -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("stubs")).unwrap();
        let launchctl = root.join("stubs/launchctl");
        fs::write(
            &launchctl,
            format!(
                r#"#!/bin/sh
echo "$1" >> "$STUB_LOG"
if [ "$1" = bootstrap ]; then
    tries=$(grep -c '^bootstrap$' "$STUB_LOG")
    if [ "$tries" -le {refusals} ]; then
        echo "Bootstrap failed: 5: Input/output error" >&2
        exit 5
    fi
fi
exit 0
"#
            ),
        )
        .unwrap();
        fs::set_permissions(&launchctl, fs::Permissions::from_mode(0o755)).unwrap();
        Sandbox { _dir: dir, root }
    }

    fn otto(&self, args: &[&str]) -> Output {
        support::otto_command()
            .args(args)
            .env_clear()
            .env("OTTO_NOTIFY", "off")
            .env("HOME", self.root.join("home"))
            .env("OTTO_HOME", self.root.join("home/.otto"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.root.join("stubs").display()))
            .env("STUB_LOG", self.root.join("calls.log"))
            .output()
            .unwrap()
    }

    /// What launchctl was asked to do, one subcommand a line.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("calls.log")).unwrap().lines().map(str::to_string).collect()
    }
}

fn says(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn assert_outlasts_the_removal(args: &[&str]) {
    let sandbox = Sandbox::refusing(2);
    let output = sandbox.otto(args);
    assert!(output.status.success(), "{}", says(&output));
    assert_eq!(sandbox.calls(), ["bootout", "bootstrap", "bootstrap", "bootstrap", "enable"]);
}

#[test]
fn the_web_service_comes_back_when_launchd_is_still_removing_it() {
    assert_outlasts_the_removal(&["service", "start", "--port", "7979"]);
}

// The one that matters more: without the reviver a sleeping run never wakes.
#[test]
fn the_reviver_comes_back_when_launchd_is_still_removing_it() {
    assert_outlasts_the_removal(&["agent", "start"]);
}

#[test]
fn a_refusal_that_lasts_is_still_reported_in_launchctl_s_words() {
    let sandbox = Sandbox::refusing(u32::MAX);
    let output = sandbox.otto(&["service", "start"]);
    assert!(!output.status.success());
    assert!(says(&output).contains("Bootstrap failed: 5: Input/output error"), "{}", says(&output));
    assert!(!sandbox.calls().contains(&"enable".to_string()), "{:?}", sandbox.calls());
}
