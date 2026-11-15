//! Installing after a landing on the otto repository.
//!
//! Once the landing step has put a commit on the base branch of *the otto repository* — the
//! checkout the installed otto's skills are linked to, whose history holds the commit this binary
//! was built from — it installs that commit: a clone of it (a clone carries no hooks), its
//! `install.sh`, detached in its own session so it outlives the agent and service it restarts.
//! The result record says `pending` until a later pass (the next step, or poke) reads how it went;
//! only then is the change marked landed and its author told, with how the install went.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{clean_git, first_line, run, write_result, LandResult};
use crate::changes::ChangeRef;

pub const PENDING: &str = "pending";
pub const INSTALLED: &str = "installed";
pub const FAILED: &str = "failed";
pub const NOT_NEEDED: &str = "not needed";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Install {
    /// `pending`, `installed`, `failed`, `not needed`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The detached install's process, while it is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// The build it replaces had uncommitted changes: what `otto --version` said of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_dirty: Option<String>,
    /// Whether the main checkout was fast-forwarded, kept for the author's note.
    #[serde(default)]
    pub local_base: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Decision {
    /// Not the checkout the installed otto came from: nothing is installed.
    NotOtto,
    /// The installed build is clean and already has the landed commit.
    UpToDate,
    Install {
        dirty: bool,
    },
}

/// The checkout the person's last `otto install --repo` linked the skills to.
pub(super) fn linked_checkout() -> Option<PathBuf> {
    let link = std::fs::read_link(crate::install::skills_dir().join("manage-pr")).ok()?;
    link.parent()?.parent()?.canonicalize().ok()
}

/// Whether landing `sha` in `repo` is a landing on the otto repository that the installed build
/// (`build`: what `otto --version` names, `-dirty` and all) should be replaced after.
pub(super) fn decide(repo: &Path, linked: Option<&Path>, build: &str, sha: &str) -> Decision {
    let Ok(real) = repo.canonicalize() else {
        return Decision::NotOtto;
    };
    if linked != Some(real.as_path()) {
        return Decision::NotOtto;
    }
    let (commit, dirty) = match build.strip_suffix("-dirty") {
        Some(c) => (c, true),
        None => (build, false),
    };
    if commit.len() < 7 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Decision::NotOtto;
    }
    let mut contains = clean_git();
    contains
        .arg("-C")
        .arg(&real)
        .args(["for-each-ref", "--count=1", "--contains", commit]);
    if !matches!(run(contains), Ok(out) if !out.trim().is_empty()) {
        return Decision::NotOtto;
    }
    let mut behind = clean_git();
    behind
        .arg("-C")
        .arg(&real)
        .args(["merge-base", "--is-ancestor", sha, commit]);
    if !dirty && run(behind).is_ok() {
        return Decision::UpToDate;
    }
    Decision::Install { dirty }
}

fn work_dir(result: &LandResult) -> PathBuf {
    super::dir()
        .join(&result.run)
        .join(format!("{}.install", result.n))
}

const SCRIPT: &str = r#"cd "$1" || exit 1
./install.sh --skills-repo "$2" >"$3/log" 2>&1
code=$?
echo "$code" >"$3/status.tmp" && mv "$3/status.tmp" "$3/status"
"#;

/// Clone the landed commit and start its `install.sh`, detached. What comes back is `pending`
/// with the process, or `failed` with why it never started.
pub(super) fn start(
    result: &LandResult,
    repo: &Path,
    sha: &str,
    dirty: bool,
    local_base: bool,
) -> Install {
    let mut install = Install {
        state: FAILED.to_string(),
        why: None,
        pid: None,
        replaced_dirty: dirty.then(|| env!("OTTO_BUILD_COMMIT").to_string()),
        local_base,
    };
    let work = work_dir(result);
    let _ = std::fs::remove_dir_all(&work);
    let clone = work.join("otto");
    let started = (|| -> Result<u32, String> {
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let mut cmd = clean_git();
        cmd.args(["clone", "-q", "--no-checkout"])
            .arg(repo)
            .arg(&clone);
        run(cmd)
            .map_err(|e| format!("the landed commit couldn't be cloned: {}", first_line(&e)))?;
        let mut cmd = clean_git();
        cmd.arg("-C")
            .arg(&clone)
            .args(["checkout", "-q", "--detach", sha]);
        run(cmd).map_err(|e| {
            format!(
                "the landed commit couldn't be checked out: {}",
                first_line(&e)
            )
        })?;
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", SCRIPT, "otto-install"])
            .arg(&clone)
            .arg(repo)
            .arg(&work)
            .env_remove("CARGO_TARGET_DIR")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Its own session: restarting the agent or the service that started it doesn't take it down.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        cmd.spawn()
            .map(|child| child.id())
            .map_err(|e| format!("install.sh couldn't be started: {e}"))
    })();
    match started {
        Ok(pid) => {
            install.state = PENDING.to_string();
            install.pid = Some(pid);
        }
        Err(why) => install.why = Some(why),
    }
    install
}

fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 sends nothing; it only asks whether the process is there.
    let there = unsafe { libc::kill(pid, 0) } == 0;
    there || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Every install still `pending`: finished if it has said how it went, failed if it died without
/// saying, left alone while it runs. The caller holds the landing lock.
pub(super) fn finish_pending(lander: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let Ok(runs) = std::fs::read_dir(super::dir()) else {
        return lines;
    };
    for run_dir in runs.flatten() {
        let Ok(files) = std::fs::read_dir(run_dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(mut result) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<LandResult>(&t).ok())
            else {
                continue;
            };
            let Some(mut install) = result.install.clone().filter(|i| i.state == PENDING) else {
                continue;
            };
            let work = work_dir(&result);
            let status = || std::fs::read_to_string(work.join("status")).ok();
            let code = match status() {
                Some(code) => Some(code),
                None if install.pid.is_some_and(alive) => continue,
                // It may have finished between the two looks.
                None => status(),
            };
            match code.as_deref().map(str::trim) {
                Some("0") => install.state = INSTALLED.to_string(),
                Some(code) => {
                    let log = std::fs::read_to_string(work.join("log")).unwrap_or_default();
                    let last = log
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("");
                    install.state = FAILED.to_string();
                    install.why = Some(format!("install.sh exited {code}: {}", last.trim()));
                }
                None => {
                    install.state = FAILED.to_string();
                    install.why = Some("the install died before it finished".to_string());
                }
            }
            install.pid = None;
            result.install = Some(install);
            lines.push(settle(lander, result));
        }
    }
    lines
}

/// What the author's note says of the install, after what it says of the main checkout.
pub(super) fn note(result: &LandResult) -> Option<String> {
    let install = result.install.as_ref()?;
    let checkout = (!install.local_base)
        .then(|| result.checkout.clone())
        .flatten();
    let said = match install.state.as_str() {
        INSTALLED => Some(match &install.replaced_dirty {
            Some(was) => format!(
                "otto installed this commit, replacing a build with uncommitted changes ({was})"
            ),
            None => "otto installed this commit".to_string(),
        }),
        FAILED => Some(format!(
            "installing this commit failed ({}): the landing stands and the installed otto is the one from before",
            install.why.as_deref().unwrap_or("no reason recorded")
        )),
        _ => None,
    };
    let parts: Vec<String> = checkout.into_iter().chain(said).collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The install is over, one way or the other: keep the result, mark the change landed, tell its
/// author how the install went, and tell the person when it failed.
pub(super) fn settle(lander: &str, result: LandResult) -> String {
    let _ = write_result(&result);
    let _ = std::fs::remove_dir_all(work_dir(&result).join("otto"));
    let r = ChangeRef {
        run: result.run.clone(),
        n: result.n,
    };
    let sha = result.pushed.clone().unwrap_or_else(|| result.head.clone());
    let install = result.install.clone().expect("settled with an install");
    if let Ok(lander_dir) = crate::paths::run_dir(lander) {
        let _ = crate::state::journal(
            &lander_dir,
            "land-install",
            json!({"change": r.display(), "run": result.run, "n": result.n, "commit": sha, "state": install.state, "why": install.why}),
        );
    }
    if let Err(e) = crate::changes::landed_by_step(
        lander,
        &r,
        &sha,
        install.local_base,
        note(&result).as_deref(),
    ) {
        eprintln!(
            "otto: {} landed at {} and couldn't be marked landed: {}",
            r.display(),
            crate::changes::short_sha(&sha),
            e.message
        );
    }
    let short = crate::changes::short_sha(&sha);
    if install.state == FAILED {
        let why = install.why.as_deref().unwrap_or("no reason recorded");
        let body = format!(
            "{} landed at {short}, and installing it failed: {why}.\n\nThe landing stands and the installed otto is the one from before. The install's output is in {}. To install it yourself, run ./install.sh in the otto checkout.",
            r.display(),
            work_dir(&result).join("log").display()
        );
        let _ = crate::messages::send(
            lander,
            &crate::messages::Outgoing {
                kind: crate::messages::MessageKind::Update,
                title: "Installing otto after a landing failed",
                body: &body,
                draft: None,
                attachments: &[],
                output: None,
            },
        );
        format!("landing {}: installing {short} failed: {why}", r.display())
    } else {
        format!("landing {}: installed {short}", r.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Never backwards: a clean build that already has the landed commit is left; a `-dirty` one
    /// is replaced; a build ahead of nothing is replaced; another checkout is not otto's.
    #[test]
    fn the_installed_build_is_only_replaced_by_something_newer_or_when_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let one = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "two"]);
        let two = git(&repo, &["rev-parse", "HEAD"]);
        let linked = Some(repo.as_path());
        assert_eq!(decide(&repo, linked, &two[..7], &one), Decision::UpToDate);
        assert_eq!(decide(&repo, linked, &two[..7], &two), Decision::UpToDate);
        assert_eq!(
            decide(&repo, linked, &format!("{}-dirty", &two[..7]), &one),
            Decision::Install { dirty: true }
        );
        assert_eq!(
            decide(&repo, linked, &one[..7], &two),
            Decision::Install { dirty: false }
        );
        assert_eq!(decide(&repo, None, &one[..7], &two), Decision::NotOtto);
        assert_eq!(
            decide(&repo, Some(Path::new("/elsewhere")), &one[..7], &two),
            Decision::NotOtto
        );
        assert_eq!(
            decide(&repo, linked, "commit unknown", &two),
            Decision::NotOtto
        );
        assert_eq!(decide(&repo, linked, "1234567", &two), Decision::NotOtto);
    }
}
