//! Live tests from outside the sandbox (plan 1.9). A wake asks (`otto state live-tests`), poke starts the
//! runner detached in its own session (as `landing::install::start` does), and the runner reads claude's
//! token once, outside, and runs the commit's `test/live.sh` under the run's own profile with the token in
//! `CLAUDE_CODE_OAUTH_TOKEN` for that process tree only. What it says lands in `$RUN_DIR/live-tests/<n>/`,
//! with the literal token scrubbed from every file.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::OttoError;

pub const PENDING: &str = "pending";
pub const RUNNING: &str = "running";
pub const DONE: &str = "done";

/// The tests that need a different profile, or none: never run from here.
pub const ELSEWHERE: &[&str] = &["live_sandbox_refused", "live_check_sandboxed", "live_sandbox"];
pub const NOT_RUN_HERE: &str = "not run here: needs to start outside any sandbox, on a commit the person approved";

/// How long the token must last: the whole set, with room.
const TOKEN_MINUTES: i64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub commit: String,
    #[serde(default)]
    pub tests: Vec<String>,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<String>,
    pub repo: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveResult {
    pub commit: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default)]
    pub tests: Vec<TestResult>,
}

#[derive(clap::Args, Debug)]
pub struct RequestArgs {
    /// The run asking
    pub id: String,
    /// The commit whose live tests to run, resolved in the repo
    #[arg(long)]
    pub commit: String,
    /// One test file's name (`live_gate`); repeat for more. None: live.sh's routine set
    #[arg(long = "test")]
    pub tests: Vec<String>,
    /// The repo the commit is in. Default: the current directory's
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

fn dir_of(run_dir: &Path) -> PathBuf {
    run_dir.join("live-tests")
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    // The run's directory is writable inside the sandbox: never write through anything planted there.
    let _ = std::fs::remove_file(&temp);
    std::fs::write(&temp, text + "\n").map_err(|e| format!("couldn't write {}: {e}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

/// Every numbered request of a run, in order.
fn numbered(run_dir: &Path) -> Vec<(u32, PathBuf)> {
    let mut found: Vec<(u32, PathBuf)> = std::fs::read_dir(dir_of(run_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()).map(|n| (n, e.path())))
        .collect();
    found.sort();
    found
}

fn git(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        cmd.env_remove(var);
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// `otto state live-tests`.
pub fn request(args: RequestArgs) -> Result<(), OttoError> {
    let id = crate::paths::resolve_run_id(&args.id)?;
    let run_dir = crate::paths::run_dir(&id)?;
    let repo = match args.repo {
        Some(repo) => repo,
        None => std::env::current_dir()?,
    };
    let out = git(&repo)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{}^{{commit}}", args.commit))
        .output()?;
    if !out.status.success() {
        return Err(OttoError::conflict(format!(
            "otto state live-tests: no commit {} in {}",
            args.commit,
            repo.display()
        )));
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let top = git(&repo).args(["rev-parse", "--show-toplevel"]).output()?;
    let repo = if top.status.success() {
        PathBuf::from(String::from_utf8_lossy(&top.stdout).trim())
    } else {
        repo
    };
    let all = numbered(&run_dir);
    for (n, path) in &all {
        if path.join("result.json").exists() {
            continue;
        }
        if let Some(req) = read_json::<Request>(&path.join("request.json")) {
            if req.state == PENDING || req.state == RUNNING {
                return Err(OttoError::conflict(format!(
                    "otto state live-tests: request {n} for {} is still {}: one at a time; its result will be in {}",
                    req.commit,
                    req.state,
                    path.join("result.json").display()
                )));
            }
        }
    }
    let n = all.last().map(|(n, _)| n + 1).unwrap_or(1);
    let dir = dir_of(&run_dir).join(n.to_string());
    std::fs::create_dir_all(&dir)?;
    let req = Request {
        commit: sha.clone(),
        tests: args.tests.clone(),
        requested_at: crate::clock::format_iso(crate::clock::now()),
        wake: std::env::var("OTTO_SESSION_ID").ok().filter(|s| !s.is_empty()),
        repo: repo.display().to_string(),
        state: PENDING.to_string(),
        pid: None,
    };
    write_json(&dir.join("request.json"), &req).map_err(OttoError::conflict)?;
    crate::state::log_event(
        &id,
        crate::event::Event::LiveTestsRequested {
            n,
            commit: sha.clone(),
            tests: args.tests,
        },
    )?;
    println!(
        "live tests {n} requested for {sha}: poke starts them; the result will be in {}",
        dir.join("result.json").display()
    );
    Ok(())
}

fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 sends nothing; it only asks whether the process is there.
    let there = unsafe { libc::kill(pid, 0) } == 0;
    there || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Poke's part: start every pending request detached, and settle every running one whose process is
/// gone without a result. Never waits on a runner.
pub fn pickup(runs_dir: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let Ok(runs) = std::fs::read_dir(runs_dir) else {
        return lines;
    };
    let otto = std::env::current_exe().ok();
    for run in runs.flatten() {
        let run_dir = run.path();
        let Some(id) = run.file_name().to_str().map(str::to_string) else {
            continue;
        };
        for (n, dir) in numbered(&run_dir) {
            let req_path = dir.join("request.json");
            let Some(mut req) = read_json::<Request>(&req_path) else {
                continue;
            };
            if dir.join("result.json").exists() {
                if req.state != DONE {
                    req.state = DONE.to_string();
                    req.pid = None;
                    let _ = write_json(&req_path, &req);
                }
                continue;
            }
            match req.state.as_str() {
                PENDING => {
                    let Some(otto) = &otto else { continue };
                    let mut cmd = Command::new(otto);
                    cmd.args(["live-tests", "run", &id, &n.to_string()])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null());
                    // Its own session: poke ending, or the agent restarting, doesn't take it down.
                    unsafe {
                        cmd.pre_exec(|| {
                            libc::setsid();
                            Ok(())
                        });
                    }
                    match cmd.spawn() {
                        Ok(child) => {
                            req.state = RUNNING.to_string();
                            req.pid = Some(child.id());
                            let _ = write_json(&req_path, &req);
                            lines.push(format!("{id}: started live tests {n} for {}", req.commit));
                        }
                        Err(e) => {
                            let _ = finish(
                                &dir,
                                &id,
                                n,
                                &req.commit,
                                "couldnt-run",
                                Some(format!("the runner couldn't be started: {e}")),
                                vec![],
                                None,
                            );
                            lines.push(format!("{id}: live tests {n} couldn't start: {e}"));
                        }
                    }
                }
                RUNNING if !req.pid.is_some_and(alive) && !dir.join("result.json").exists() => {
                    let _ = finish(
                        &dir,
                        &id,
                        n,
                        &req.commit,
                        "failed",
                        Some("died without saying".to_string()),
                        vec![],
                        None,
                    );
                    lines.push(format!("{id}: live tests {n} died without saying"));
                }
                _ => {}
            }
        }
    }
    lines
}

/// Replace every copy of the token in every file below `dir`.
fn scrub(dir: &Path, token: &str) {
    if token.is_empty() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            scrub(&path, token);
        } else if kind.is_file() {
            if let Ok(bytes) = std::fs::read(&path) {
                let needle = token.as_bytes();
                if bytes.windows(needle.len()).any(|w| w == needle) {
                    let text = String::from_utf8_lossy(&bytes).replace(token, "[token removed]");
                    let _ = std::fs::write(&path, text);
                }
            }
        }
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finish(
    dir: &Path,
    id: &str,
    n: u32,
    commit: &str,
    state: &str,
    why: Option<String>,
    tests: Vec<TestResult>,
    token: Option<&str>,
) -> Result<(), String> {
    let result = LiveResult {
        commit: commit.to_string(),
        state: state.to_string(),
        why,
        tests,
    };
    write_json(&dir.join("result.json"), &result)?;
    if let Some(token) = token {
        scrub(dir, token);
    }
    if let Some(mut req) = read_json::<Request>(&dir.join("request.json")) {
        req.state = DONE.to_string();
        req.pid = None;
        let _ = write_json(&dir.join("request.json"), &req);
    }
    let _ = crate::state::log_event(
        id,
        crate::event::Event::LiveTestsFinished {
            n,
            state: state.to_string(),
        },
    );
    Ok(())
}

/// live.sh's routine set, from the line that says it, when the request names no tests.
fn routine_set(live_sh: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(live_sh).unwrap_or_default();
    text.lines()
        .find_map(|l| l.strip_prefix("tests=${*:-"))
        .map(|rest| rest.trim_end_matches('}').split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// `otto live-tests run <id> <n>`: what poke starts, outside any sandbox.
pub fn run(id: &str, n: u32) -> Result<(), OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let run_dir = crate::paths::run_dir(&id)?;
    let dir = dir_of(&run_dir).join(n.to_string());
    let Some(req) = read_json::<Request>(&dir.join("request.json")) else {
        return Err(OttoError::conflict(format!("otto live-tests: no request {n} for run {id}")));
    };
    let commit = req.commit.clone();
    let done = |state: &str, why: String, tests: Vec<TestResult>, token: Option<&str>| -> Result<(), OttoError> {
        finish(&dir, &id, n, &commit, state, Some(why).filter(|w| !w.is_empty()), tests, token).map_err(OttoError::conflict)
    };
    let record = match crate::sandbox::read_record(&id) {
        Ok(record) => record,
        Err(_) => return done("couldnt-run", format!("couldn't run: no sandbox record for run {id}"), vec![], None),
    };
    let deadline = crate::clock::now() + time::Duration::minutes(TOKEN_MINUTES);
    let token = match crate::sandbox::token_lasting_to(deadline) {
        Ok(token) => token,
        Err(why) => return done("couldnt-run", format!("couldn't run: {why}"), vec![], None),
    };
    let checkout = dir.join("checkout");
    let _ = std::fs::remove_dir_all(&checkout);
    let cloned = git(&dir)
        .args(["clone", "-q", "--no-checkout", "--shared"])
        .arg(&req.repo)
        .arg(&checkout)
        .output();
    if !cloned.as_ref().is_ok_and(|o| o.status.success()) {
        return done(
            "couldnt-run",
            format!("couldn't run: the commit couldn't be cloned from {}", req.repo),
            vec![],
            Some(&token),
        );
    }
    let checked = git(&checkout).args(["checkout", "-q", "--detach", &req.commit]).output();
    if !checked.as_ref().is_ok_and(|o| o.status.success()) {
        let _ = std::fs::remove_dir_all(&checkout);
        return done(
            "couldnt-run",
            format!("couldn't run: {} couldn't be checked out", req.commit),
            vec![],
            Some(&token),
        );
    }
    let live_sh = checkout.join("test/live.sh");
    let asked = if req.tests.is_empty() {
        routine_set(&live_sh)
    } else {
        req.tests.clone()
    };
    let (elsewhere, here): (Vec<String>, Vec<String>) = asked.into_iter().partition(|t| ELSEWHERE.contains(&t.as_str()));
    let mut tests: Vec<TestResult> = Vec::new();
    let outcome = (|| -> Result<(String, Option<String>), String> {
        if here.is_empty() {
            return Ok(("ok".to_string(), Some("nothing to run here".to_string())));
        }
        let machine = crate::sandbox::Machine::here();
        let text = crate::sandbox::profile(&id, &record, &machine)?;
        let profile = crate::sandbox::write_profile(&id, &format!("live-{n}"), &text)?;
        crate::sandbox::try_profile(&profile)?;
        let (caches, claude) = crate::sandbox::run_folders(&id, &record, &machine.home)?;
        let output_path = dir.join("output.log");
        let _ = std::fs::remove_file(&output_path);
        let out = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|e| format!("couldn't write {}: {e}", output_path.display()))?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        // Read back through this handle, not the path: what's at the path is the sandboxed tree's to replace.
        let mut reader = out.try_clone().map_err(|e| e.to_string())?;
        // Its own temporary directory, so the log directory live.sh names can be held to it.
        let tmp = dir.join("tmp");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| format!("couldn't make {}: {e}", tmp.display()))?;
        let mut cmd = Command::new("/usr/bin/sandbox-exec");
        cmd.arg("-f")
            .arg(&profile)
            .arg("/bin/sh")
            .arg(&live_sh)
            .args(&here)
            .current_dir(&checkout);
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_none_or(crate::sandbox::cleared) {
                cmd.env_remove(&name);
            }
        }
        crate::sandbox::run_env(&mut cmd, &caches, &claude);
        cmd.env("TMPDIR", &tmp)
            .env("CLAUDE_CODE_OAUTH_TOKEN", &token)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        let status = cmd.status().map_err(|e| format!("couldn't start sandbox-exec: {e}"))?;
        let said = {
            use std::io::{Read, Seek};
            let mut bytes = Vec::new();
            let _ = reader
                .seek(std::io::SeekFrom::Start(0))
                .and_then(|_| reader.read_to_end(&mut bytes));
            String::from_utf8_lossy(&bytes).into_owned()
        };
        for line in said.lines() {
            let mut words = line.split_whitespace();
            if let (Some(mark @ ("ok" | "FAIL")), Some(name)) = (words.next(), words.next()) {
                if here.iter().any(|t| t == name) {
                    tests.push(TestResult {
                        name: name.to_string(),
                        result: mark.to_string(),
                    });
                }
            }
        }
        if let Some(logs) = said.lines().rev().find_map(|l| l.strip_prefix("logs: ")) {
            // Only a directory in its own temporary directory, itself and not a link: the name is the
            // sandboxed tree's to say, and the copy is made from outside.
            let logs = PathBuf::from(logs.trim());
            let own = logs.parent() == Some(tmp.as_path()) && std::fs::symlink_metadata(&logs).is_ok_and(|m| m.is_dir());
            let target = dir.join("logs");
            match std::fs::symlink_metadata(&target) {
                Ok(m) if m.is_dir() => drop(std::fs::remove_dir_all(&target)),
                Ok(_) => drop(std::fs::remove_file(&target)),
                Err(_) => {}
            }
            if own {
                let _ = copy_dir(&logs, &target);
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
        if tests.is_empty() {
            let last = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            return Ok((
                "failed".to_string(),
                Some(format!(
                    "live.sh said no test's result (exit {}): {last}",
                    status.code().unwrap_or(-1)
                )),
            ));
        }
        let failed = tests.iter().any(|t| t.result != "ok") || !status.success();
        Ok((if failed { "failed" } else { "ok" }.to_string(), None))
    })();
    for name in elsewhere {
        tests.push(TestResult {
            name,
            result: NOT_RUN_HERE.to_string(),
        });
    }
    let _ = std::fs::remove_dir_all(&checkout);
    match outcome {
        Ok((state, why)) => done(&state, why.unwrap_or_default(), tests, Some(&token)),
        Err(why) => done("couldnt-run", format!("couldn't run: {why}"), tests, Some(&token)),
    }
}
