//! tests/quiet_notifications.rs — a test's `otto poke` never reaches the notification centre.
//!
//! A test's scratch runs open gates and hand back changes, and poke posts a notice for each through
//! `osascript`. `tests/support` turns that off (`OTTO_NOTIFY=off`) for everything a test runs, so
//! here poke runs with a recording `osascript` first on `PATH`, through support and nothing else,
//! and must not call it while still recording the notice. The second test checks every test file
//! runs otto through support, so a new one can't forget.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[test]
fn a_tests_poke_records_a_notice_but_never_sends_it() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().canonicalize().unwrap();
    let calls = root.join("osascript-calls");
    let fake = root.join("bin/osascript");
    std::fs::create_dir_all(fake.parent().unwrap()).unwrap();
    std::fs::write(&fake, format!("#!/bin/sh\necho \"$@\" >> '{}'\n", calls.display())).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", fake.parent().unwrap().display(), std::env::var("PATH").unwrap_or_default());

    let ran = |args: &[&str]| {
        let out = support::otto_command().args(args).env("PATH", &path).env_remove("OTTO_RUN_ID").output().unwrap();
        assert!(out.status.success(), "otto {args:?}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    // A stand-in for the model, named `claude` so nothing here can reach the real one.
    let home = support::own_home();
    assert!(home.starts_with(std::env::temp_dir().canonicalize().unwrap()), "not a scratch home: {}", home.display());
    let config = home.join("config.json");
    let mut settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    settings["launchers"] = serde_json::json!([{"name": "claude", "command": "/usr/bin/true"}]);
    std::fs::write(&config, settings.to_string()).unwrap();

    ran(&["run", "--goal", "g", "--name", "quiet", "--perpetual", "--period", "1h", "--detach", "none"]);
    let list: serde_json::Value = serde_json::from_str(&ran(&["state", "list", "--json"])).unwrap();
    let id = list.as_array().unwrap().iter().find(|r| r["name"] == "quiet").unwrap()["id"].as_str().unwrap().to_string();
    let question = root.join("question.md");
    std::fs::write(&question, "Which one?\n").unwrap();
    ran(&["state", "open-gate", &id, "--slug", "which", "--question-file", question.to_str().unwrap()]);

    let said = ran(&["poke", "--verbose", "--grace", "0"]);

    assert!(!calls.exists(), "poke ran osascript: {}", std::fs::read_to_string(&calls).unwrap_or_default());
    let run: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join("runs").join(&id).join("run.json")).unwrap()).unwrap();
    assert!(run["notified"].as_object().is_some_and(|n| n.keys().any(|k| k.starts_with("gate:"))), "the notice is still recorded: {run}");
    let line = said.lines().find(|l| l.contains(" notify ")).unwrap_or_else(|| panic!("poke said nothing about it: {said}"));
    assert!(line.contains("[not sent: OTTO_NOTIFY=off]") && line.contains("Needs you"), "{line}");
}

/// Every test file runs otto through `support::otto_command` (or `support::otto`), which turns
/// notifications off; none builds its own `Command` for the binary, and one that clears the
/// environment says `OTTO_NOTIFY` again after.
#[test]
fn every_test_runs_otto_through_support() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut wrong = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        // Spelt in pieces, so this file doesn't find them in itself.
        let mut direct = vec![concat!("CARGO_BIN_EXE", "_otto").to_string()];
        direct.extend(["OTTO", "support::OTTO", "otto_bin()", "support::otto_bin()"].map(|what| format!("Command::new({what}")));
        for pattern in direct.iter().filter(|p| text.contains(p.as_str())) {
            wrong.push(format!("{name}: {pattern} — use support::otto_command()"));
        }
        if text.contains("otto_command()") && text.contains(".env_clear()") && !text.contains("OTTO_NOTIFY") {
            wrong.push(format!("{name}: clears the environment without setting OTTO_NOTIFY=off again"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
