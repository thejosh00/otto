//! tests/live_tests_outside.rs — live tests from outside the sandbox (plan 1.9), through the real binary:
//! a wake asks (`otto state live-tests`), and the runner (`otto live-tests run`) runs a fixture repo's
//! `test/live.sh` under the run's profile with a stand-in token. A scratch home and a stand-in Keychain
//! item (`OTTO_TEST_KEYCHAIN_JSON_FILE`): nothing here reads the real `~/.otto` or the real Keychain, and
//! no model runs.

mod support;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TOKEN: &str = "sk-stand-in-7f3a9c1e";

const LIVE_SH: &str = r#"#!/bin/sh
logs=$(mktemp -d "${TMPDIR:-/tmp}/otto-live.XXXXXX")
failed=0
for t in "$@"; do
    case $t in
    live_good)
        echo "the token was: $CLAUDE_CODE_OAUTH_TOKEN" >"$logs/$t.log"
        echo "claude config: $CLAUDE_CONFIG_DIR" >>"$logs/$t.log"
        echo "cargo home: $CARGO_HOME" >>"$logs/$t.log"
        if [ -n "$CLAUDE_CODE_OAUTH_TOKEN" ]; then mark=ok; else mark=FAIL failed=1; fi ;;
    live_reach)
        if [ -n "${GH_TOKEN:-}" ] || cat "$HOME/.ssh/id_test" 2>/dev/null; then mark=FAIL failed=1; else mark=ok; fi ;;
    *) echo "fails on purpose" >"$logs/$t.log"; mark=FAIL failed=1 ;;
    esac
    printf '%-4s %-13s %5ss  %s\n' "$mark" "$t" 1 ""
done
echo "logs: $logs"
exit $failed
"#;

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
    id: String,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("userhome/.ssh")).unwrap();
        std::fs::write(root.join("userhome/.ssh/id_test"), "PRIVATE KEY\n").unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("test")).unwrap();
        std::fs::write(repo.join("test/live.sh"), LIVE_SH).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "fixture"]);
        let far = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64)
            + 10 * 3600 * 1000;
        std::fs::write(
            root.join("keychain.json"),
            serde_json::json!({"claudeAiOauth": {"accessToken": TOKEN, "expiresAt": far, "refreshTokenExpiresAt": far}}).to_string(),
        )
        .unwrap();
        let mut s = Scratch {
            _dir: dir,
            root,
            id: String::new(),
        };
        let out = s.otto(&[
            "state",
            "init",
            "--name",
            "author",
            "--perpetual",
            "--period",
            "1h",
            "--goal",
            "g",
            "--repo",
            repo.to_str().unwrap(),
        ]);
        assert!(out.status.success(), "state init: {}", String::from_utf8_lossy(&out.stderr));
        let got = s.otto(&["state", "get", "author"]);
        let state: Value = serde_json::from_slice(&got.stdout).unwrap();
        s.id = state["id"].as_str().unwrap().to_string();
        s
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn run_dir(&self) -> PathBuf {
        self.root.join("home/runs").join(&self.id)
    }

    fn otto(&self, args: &[&str]) -> Output {
        let mut cmd = support::otto_command();
        cmd.args(args)
            .current_dir(self.root.join("repo"))
            .env("OTTO_HOME", self.root.join("home"))
            .env("HOME", self.root.join("userhome"))
            .env("OTTO_TEST_KEYCHAIN_JSON_FILE", self.root.join("keychain.json"))
            .env("GH_TOKEN", "gh-should-not-reach")
            // The scratch homes are below the real TMPDIR, which leaves it closed: one beside them instead.
            .env("TMPDIR", self.root.join("tmp"))
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT");
        cmd.output().unwrap_or_else(|e| panic!("could not run otto {args:?}: {e}"))
    }

    fn head(&self) -> String {
        git(&self.repo(), &["rev-parse", "HEAD"])
    }

    fn record(&self, mode: &str) {
        let dir = self.root.join("home/sandbox/runs");
        std::fs::create_dir_all(&dir).unwrap();
        let record = serde_json::json!({
            "launcher": "claude",
            "workdir": self.repo().display().to_string(),
            "read": [self.repo().display().to_string()],
            "write": [self.run_dir().display().to_string()],
            "mode": mode,
        });
        std::fs::write(dir.join(format!("{}.json", self.id)), record.to_string()).unwrap();
    }

    fn result(&self, n: u32) -> Value {
        let path = self.run_dir().join(format!("live-tests/{n}/result.json"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
    }
}

fn all_text(dir: &Path) -> String {
    let mut text = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            text += &all_text(&path);
        } else {
            text += &String::from_utf8_lossy(&std::fs::read(&path).unwrap());
        }
    }
    text
}

#[test]
fn a_request_is_recorded_and_journaled() {
    let s = Scratch::new();
    let sha = s.head();
    let out = s.otto(&["state", "live-tests", "author", "--commit", "HEAD", "--test", "live_good"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let req: Value = serde_json::from_str(&std::fs::read_to_string(s.run_dir().join("live-tests/1/request.json")).unwrap()).unwrap();
    assert_eq!(req["commit"], sha.as_str());
    assert_eq!(req["tests"], serde_json::json!(["live_good"]));
    assert_eq!(req["state"], "pending");
    assert!(req["requestedAt"].is_string());
    let journal = std::fs::read_to_string(s.run_dir().join("journal.jsonl")).unwrap_or_default();
    assert!(journal.contains("live-tests-requested"), "journal: {journal}");
}

#[test]
fn a_second_request_is_refused_while_one_waits() {
    let s = Scratch::new();
    assert!(s.otto(&["state", "live-tests", "author", "--commit", "HEAD"]).status.success());
    let out = s.otto(&["state", "live-tests", "author", "--commit", "HEAD"]);
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("request 1") && said.contains("one at a time"), "said: {said}");
    assert!(!s.run_dir().join("live-tests/2").exists());
}

#[test]
fn no_record_means_couldnt_run() {
    let s = Scratch::new();
    assert!(s.otto(&["state", "live-tests", "author", "--commit", "HEAD"]).status.success());
    let out = s.otto(&["live-tests", "run", "author", "1"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let result = s.result(1);
    assert_eq!(result["state"], "couldnt-run");
    assert!(result["why"].as_str().unwrap().contains("no sandbox record for run"), "{result}");
}

#[test]
fn a_run_under_the_record_says_each_test_and_scrubs_the_token() {
    let s = Scratch::new();
    s.record("enforce");
    let out = s.otto(&[
        "state",
        "live-tests",
        "author",
        "--commit",
        "HEAD",
        "--test",
        "live_good",
        "--test",
        "live_bad",
        "--test",
        "live_reach",
        "--test",
        "live_sandbox_refused",
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = s.otto(&["live-tests", "run", "author", "1"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let result = s.result(1);
    let dir = s.run_dir().join("live-tests/1");
    let said = |name: &str| {
        result["tests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .map(|t| t["result"].as_str().unwrap().to_string())
    };
    assert_eq!(result["commit"], s.head().as_str());
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(said("live_good").as_deref(), Some("ok"), "{result}");
    assert_eq!(said("live_bad").as_deref(), Some("FAIL"), "{result}");
    assert_eq!(said("live_reach").as_deref(), Some("ok"), "ssh key or GH_TOKEN reachable: {result}");
    assert!(said("live_sandbox_refused").unwrap().starts_with("not run here"), "{result}");
    assert!(
        dir.join("logs/live_good.log").is_file(),
        "logs copied: {}",
        std::fs::read_to_string(dir.join("output.log")).unwrap_or_default()
    );
    assert!(!dir.join("checkout").exists(), "checkout removed");
    let text = all_text(&dir);
    assert!(
        text.contains("the token was: [token removed]"),
        "the stand-in token reached live.sh and was scrubbed"
    );
    assert!(!text.contains(TOKEN), "token left in the run dir");
    let good = std::fs::read_to_string(dir.join("logs/live_good.log")).unwrap();
    let home = s.root.join("home");
    let claude = home.join("sandbox/claude").join(&s.id);
    let cargo = home.join("sandbox/caches").join(&s.id).join("cargo");
    assert!(good.contains(&format!("claude config: {}", claude.display())), "{good}");
    assert!(good.contains(&format!("cargo home: {}", cargo.display())), "{good}");
    assert!(claude.is_dir() && cargo.is_dir(), "the run's own folders are made");
}

#[test]
fn poke_settles_a_runner_that_died() {
    let s = Scratch::new();
    assert!(s.otto(&["state", "live-tests", "author", "--commit", "HEAD"]).status.success());
    let path = s.run_dir().join("live-tests/1/request.json");
    let mut req: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
    let pid = gone.id();
    gone.wait().unwrap();
    req["state"] = "running".into();
    req["pid"] = pid.into();
    std::fs::write(&path, req.to_string()).unwrap();
    let out = s.otto(&["poke"]);
    assert!(out.status.code() != Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    let result = s.result(1);
    assert_eq!(result["state"], "failed");
    assert_eq!(result["why"], "died without saying");
}

#[test]
fn a_run_under_an_observed_record_says_each_test_and_scrubs_the_token() {
    let s = Scratch::new();
    s.record("report");
    assert!(s
        .otto(&[
            "state",
            "live-tests",
            "author",
            "--commit",
            "HEAD",
            "--test",
            "live_good",
            "--test",
            "live_bad"
        ])
        .status
        .success());
    let out = s.otto(&["live-tests", "run", "author", "1"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let result = s.result(1);
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(
        result["tests"],
        serde_json::json!([{"name": "live_good", "result": "ok"}, {"name": "live_bad", "result": "FAIL"}])
    );
    let dir = s.run_dir().join("live-tests/1");
    assert!(dir.join("logs/live_bad.log").is_file());
    let text = all_text(&dir);
    assert!(text.contains("the token was: [token removed]") && !text.contains(TOKEN));
}
