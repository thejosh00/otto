//! The landing step: the push to a base branch, done by otto outside any wake
//!
//! The lander asks (`otto state change request-land`): a request file in its own run directory,
//! naming a change and a commit. The request is the lander's word and proves nothing. This step
//! runs with no model, from the wake's parent once the lander's wake is over and from `otto poke`
//! as a backstop, and decides for itself:
//!
//! - the person's approval is on record (`crate::approvals`), for landing directly, and the change
//!   is being landed;
//! - the repository, base branch and remote are the record's, never the request's;
//! - the base is fetched from the remote into a scratch bare repository under
//!   `$OTTO_HOME/landings/`, which a sandboxed wake cannot write. Every git command here runs
//!   against that scratch repository, so nothing a wake wrote in the real one — config, hooks,
//!   worktree config, remote-tracking refs — is read;
//! - the commit must contain the remote's base, and be the approved commit or carry the approved
//!   patch against it; only then is it pushed, never forced.
//!
//! What it decided, and what it went by, is written to `$OTTO_HOME/landings/<run>/<n>.json` before
//! and after the push, journaled in the lander's journal and said on the change.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::changes::{ChangeRef, ChangeState, LandMode};
use crate::clock::Timestamp;
use crate::error::OttoError;

pub const PUSHING: &str = "pushing";
pub const LANDED: &str = "landed";
pub const BASE_MOVED: &str = "base moved";
pub const REMOTE_REFUSED: &str = "remote refused";
pub const REFUSED: &str = "refused";

/// What the lander asked for: this commit, for this change. Nothing else is taken from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub run: String,
    pub n: u32,
    pub head: String,
    pub at: Timestamp,
}

/// What a request was judged on: all of it from the approval on record and the remote.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgedOn {
    /// Where the change was, as its run's index says (position only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_patch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// `the approval on record`, or `the repository's config` for a record written before it kept one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_from: Option<String>,
    /// The base branch's commit, as the remote gave it to this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_base: Option<String>,
    /// The requested commit's patch against the remote's base, when it was computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LandResult {
    pub run: String,
    pub n: u32,
    /// The commit the lander asked to land.
    pub head: String,
    /// `pushing` (not final: written before the push), `landed`, `base moved`, `remote refused`, `refused`.
    pub verdict: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub judged_on: JudgedOn,
    /// The commit this step put on the base branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed: Option<String>,
    /// What happened to the main checkout after it landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    pub at: Timestamp,
}

impl LandResult {
    pub fn is_final(&self) -> bool {
        self.verdict != PUSHING
    }

    /// `landed at <sha>`, `base moved`, `remote refused: <why>`, `refused: <why>`.
    pub fn line(&self) -> String {
        let why = self.why.as_deref().unwrap_or("");
        match self.verdict.as_str() {
            LANDED => format!("landed at {}{}", crate::changes::short_sha(self.pushed.as_deref().unwrap_or(&self.head)), if why.is_empty() { String::new() } else { format!(" ({why})") }),
            BASE_MOVED => format!("base moved: {why}"),
            v => format!("{v}: {why}"),
        }
    }
}

pub fn dir() -> PathBuf {
    crate::paths::otto_home().join("landings")
}

fn result_path(run: &str, n: u32) -> Result<PathBuf, OttoError> {
    // One path component, as `approvals::run_dir` insists.
    crate::approvals::run_dir(run)?;
    Ok(dir().join(run).join(format!("{n}.json")))
}

/// The step's latest result for a change, if it has one.
pub fn read_result(run: &str, n: u32) -> Option<LandResult> {
    let text = std::fs::read_to_string(result_path(run, n).ok()?).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_result(result: &LandResult) -> Result<(), OttoError> {
    let path = result_path(&result.run, result.n)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::state::write_atomic(&path, &format!("{}\n", serde_json::to_string_pretty(result)?))
}

fn requests_dir(lander: &str) -> Result<PathBuf, OttoError> {
    Ok(crate::paths::run_dir(lander)?.join("land-requests"))
}

fn is_sha(text: &str) -> bool {
    (text.len() == 40 || text.len() == 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The lander asks for `head` to be landed as change `r`. Written in the lander's own run
/// directory, which it can write from inside a sandbox, and the commit is pinned by a ref so it
/// isn't collected before the step looks. Nothing is decided here.
pub fn request(lander: &str, r: &ChangeRef, head: &str) -> Result<Request, OttoError> {
    let lander = crate::changes::require_lander(lander)?;
    let (c, _) = crate::changes::for_step(r)?;
    let repo = Path::new(&c.repo);
    let sha = crate::changes::rev(repo, head)?;
    let name = format!("{}-{}", r.run, r.n);
    crate::changes::git(repo, &["update-ref", &format!("refs/otto/land/{name}"), &sha])
        .map_err(|e| OttoError::conflict(format!("couldn't keep {} for landing (refs/otto/land/{name}): {e}", crate::changes::short_sha(&sha))))?;
    let request = Request { run: r.run.clone(), n: r.n, head: sha, at: Timestamp::now() };
    let dir = requests_dir(&lander)?;
    std::fs::create_dir_all(&dir)?;
    crate::state::write_atomic(&dir.join(format!("{name}.json")), &format!("{}\n", serde_json::to_string_pretty(&request)?))?;
    let _ = crate::state::journal(&crate::paths::run_dir(&lander)?, "land-requested", json!({"change": r.display(), "run": r.run, "n": r.n, "head": request.head}));
    Ok(request)
}

/// The lander's requests, in the order they were written.
fn requests(lander: &str) -> Vec<(PathBuf, Request)> {
    let Ok(dir) = requests_dir(lander) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<(PathBuf, Request)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            // A link here would have the step read a file of the wake's choosing as a request;
            // a request proves nothing either way, but it is a plain file or it is nothing.
            let meta = std::fs::symlink_metadata(&p).ok()?;
            if !meta.file_type().is_file() {
                return None;
            }
            let request = serde_json::from_str::<Request>(&std::fs::read_to_string(&p).ok()?).ok()?;
            Some((p, request))
        })
        .collect();
    out.sort_by(|a, b| a.1.at.cmp(&b.1.at).then(a.0.cmp(&b.0)));
    out
}

/// Whether the lander has asked for anything the step hasn't dealt with.
pub fn waiting(lander: &str) -> bool {
    !requests(lander).is_empty()
}

/// The wake's parent, once a wake is over: when the run is the lander and it asked for something,
/// run the step. Whatever goes wrong here is said, and is not the wake's failure.
pub fn after_wake(run: &str) {
    let Ok(Some(lander)) = crate::config::lander() else { return };
    if lander != run || !waiting(&lander) {
        return;
    }
    match step(&lander) {
        Ok(results) => {
            for r in results {
                println!("landing {}/{}: {}", crate::paths::short_id(&r.run), r.n, r.line());
            }
        }
        Err(e) => eprintln!("otto: the landing step failed: {}", e.message),
    }
}

/// Poke's pass: a lander with no wake running and a request nothing has dealt with. The wake's
/// parent does this itself when it lives to; this is for when it didn't.
pub fn backstop() -> Vec<String> {
    use crate::liveness::Liveness as _;
    let Ok(Some(lander)) = crate::config::lander() else { return Vec::new() };
    if !waiting(&lander) || crate::liveness::LockLiveness.probe(&lander).is_busy() {
        return Vec::new();
    }
    match step(&lander) {
        Ok(results) => results.iter().map(|r| format!("landing {}/{}: {}", crate::paths::short_id(&r.run), r.n, r.line())).collect(),
        Err(e) => vec![format!("the landing step failed: {}", e.message)],
    }
}

/// Every request the lander has, judged and — where it is what the person approved — pushed.
/// One at a time across the whole machine: the wake's parent and poke never overlap.
pub fn step(lander: &str) -> Result<Vec<LandResult>, OttoError> {
    let locks = crate::paths::otto_home().join("locks");
    std::fs::create_dir_all(&locks)?;
    let lock = std::fs::OpenOptions::new().create(true).append(true).open(locks.join("landing.lock"))?;
    fs4::fs_std::FileExt::lock_exclusive(&lock)?;
    let lander_dir = crate::paths::run_dir(lander)?;
    let mut out = Vec::new();
    // Read under the lock: what another pass dealt with is gone by now.
    for (path, request) in requests(lander) {
        let r = ChangeRef { run: request.run.clone(), n: request.n };
        let before = read_result(&request.run, request.n).filter(|b| b.head == request.head);
        let result = match before {
            // Judged already, and the step died before it took the request away.
            Some(b) if b.is_final() => b,
            _ => {
                let result = judge(&request);
                write_result(&result)?;
                let _ = crate::state::journal(&lander_dir, "land-result", json!({"change": r.display(), "run": result.run, "n": result.n, "head": result.head, "verdict": result.verdict, "why": result.why, "pushed": result.pushed, "judgedOn": result.judged_on}));
                if result.verdict != LANDED {
                    let _ = crate::changes::step_said(&r, &format!("landing {}: {}", crate::changes::short_sha(&result.head), result.line()));
                }
                result
            }
        };
        let result = if result.verdict == LANDED { finish_landed(lander, &r, result) } else { result };
        let _ = std::fs::remove_file(&path);
        out.push(result);
    }
    if !out.is_empty() {
        let _ = crate::changes::wake_lander(lander);
    }
    let _ = fs4::fs_std::FileExt::unlock(&lock);
    Ok(out)
}

/// It is on the remote's base branch: what `otto state change landed` does, done here. The main
/// checkout is fast-forwarded when it is on the base branch and clean; the change is marked
/// landed; its author is told and woken. Doing it twice is harmless: a change already landed stays so.
fn finish_landed(lander: &str, r: &ChangeRef, mut result: LandResult) -> LandResult {
    let Ok((c, _)) = crate::changes::for_step(r) else { return result };
    if c.state != ChangeState::Landing {
        return result;
    }
    let sha = result.pushed.clone().unwrap_or_else(|| result.head.clone());
    let (repo, base) = (result.judged_on.repo.clone().unwrap_or_default(), result.judged_on.base.clone().unwrap_or_default());
    let (updated, said) = fast_forward(Path::new(&repo), &base, &sha);
    result.checkout = Some(said.clone());
    let _ = write_result(&result);
    let note = (!updated).then_some(said);
    if let Err(e) = crate::changes::landed_by_step(lander, r, &sha, updated, note.as_deref()) {
        eprintln!("otto: {} landed at {} and couldn't be marked landed: {}", r.display(), crate::changes::short_sha(&sha), e.message);
    }
    result
}

/// The one thing the step does in the real repository: move the main checkout's base branch up to
/// what landed, when that is a fast-forward of a clean checkout. Hooks are off.
fn fast_forward(repo: &Path, base: &str, sha: &str) -> (bool, String) {
    let git = |args: &[&str]| {
        let mut cmd = clean_git();
        cmd.arg("-C").arg(repo).args(args);
        run(cmd)
    };
    match git(&["symbolic-ref", "--short", "HEAD"]) {
        Ok(on) if on == base => {}
        Ok(on) => return (false, format!("the main checkout is on {on}, not {base}, so it wasn't updated")),
        Err(_) => return (false, format!("the main checkout isn't on {base}, so it wasn't updated")),
    }
    match git(&["status", "--porcelain", "--untracked-files=no"]) {
        Ok(s) if s.is_empty() => {}
        _ => return (false, "the main checkout has uncommitted changes, so it wasn't updated".to_string()),
    }
    match git(&["merge", "--ff-only", "--no-verify", "--quiet", sha]) {
        Ok(_) => (true, format!("the main checkout's {base} was fast-forwarded")),
        Err(e) => (false, format!("the main checkout's {base} couldn't be fast-forwarded, so it wasn't updated ({})", first_line(&e))),
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

/// git with nothing a wake could have left in the environment, and no hooks, monitors or helper
/// transports whatever config it reads.
fn clean_git() -> Command {
    let mut cmd = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GIT_PAGER", "cat");
    cmd.args(["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false", "-c", "core.alternateRefsCommand=true", "-c", "protocol.ext.allow=never", "-c", "gc.auto=0"]);
    cmd
}

/// git against the scratch repository only: `GIT_DIR` is it, and so is where it runs.
fn scratch_git(scratch: &Path) -> Command {
    let mut cmd = clean_git();
    cmd.env("GIT_DIR", scratch).current_dir(scratch);
    cmd
}

fn run_raw(mut cmd: Command, input: Option<&[u8]>) -> Result<Vec<u8>, String> {
    use std::io::Write as _;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = cmd.spawn().map_err(|e| format!("cannot run git: {e}"))?;
    if let Some(input) = input {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        stdin.write_all(input).map_err(|e| format!("cannot write to git: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("git failed: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if said.is_empty() { "git failed".to_string() } else { said })
    }
}

fn run(cmd: Command) -> Result<String, String> {
    run_raw(cmd, None).map(|o| String::from_utf8_lossy(&o).trim().to_string())
}

/// A remote this step will hand to git: not an option, and not a transport that runs a command.
fn url_refused(url: &str) -> Option<&'static str> {
    if url.starts_with('-') {
        return Some("it starts with a dash");
    }
    if url.split('/').next().is_some_and(|scheme| scheme.contains("::")) {
        return Some("it names a helper transport");
    }
    None
}

/// The repository's own `remote.origin.url`, read from its config file as text is: no includes,
/// and no git run inside the repository. Only for a record that kept no remote.
fn origin_of(repo: &Path) -> Option<String> {
    let mut cmd = clean_git();
    cmd.current_dir("/").arg("config").arg("--file").arg(repo.join(".git/config")).args(["--get", "remote.origin.url"]);
    let url = run(cmd).ok().filter(|u| !u.is_empty())?;
    // A path relative to the repository means the same from anywhere once it is joined to it.
    Some(if url.starts_with("./") || url.starts_with("../") { repo.join(&url).display().to_string() } else { url })
}

/// Decide one request, and push it if it is what the person approved. Every way out says why and
/// carries what it went by.
fn judge(request: &Request) -> LandResult {
    let mut on = JudgedOn::default();
    let mut result = LandResult { run: request.run.clone(), n: request.n, head: request.head.clone(), verdict: REFUSED.to_string(), why: None, judged_on: JudgedOn::default(), pushed: None, checkout: None, at: Timestamp::now() };
    let outcome = judge_and_push(request, &mut on);
    let (verdict, why, pushed) = match outcome {
        Ok(Outcome::Landed { pushed_now }) => (LANDED, (!pushed_now).then(|| "the remote's base already had it: nothing pushed".to_string()), Some(request.head.clone())),
        Err((verdict, why)) => (verdict, Some(why), None),
    };
    result.verdict = verdict.to_string();
    result.why = why;
    result.pushed = pushed;
    result.judged_on = on;
    result.at = Timestamp::now();
    result
}

enum Outcome {
    Landed { pushed_now: bool },
}

type Refusal = (&'static str, String);

fn refuse<T>(why: impl Into<String>) -> Result<T, Refusal> {
    Err((REFUSED, why.into()))
}

fn judge_and_push(request: &Request, on: &mut JudgedOn) -> Result<Outcome, Refusal> {
    let r = ChangeRef { run: request.run.clone(), n: request.n };
    let head = request.head.as_str();
    if !is_sha(head) {
        return refuse("the request doesn't name a commit by its full id");
    }
    // 1. The approval on record, for landing directly, and the change being landed.
    let (c, a) = crate::changes::for_step(&r).map_err(|e| (REFUSED, e.message))?;
    on.state = Some(c.state.as_str().to_string());
    let Some(a) = a else {
        return refuse(format!("no approval on record for change {}", r.display()));
    };
    let d = &a.record;
    on.version = Some(a.version);
    on.approved_sha = Some(a.sha.clone());
    on.approved_patch_id = Some(a.patch_id.clone());
    on.repo = Some(d.repo.clone());
    on.base = Some(d.base.clone());
    if d.mode != LandMode::Push {
        return refuse("it was approved to land as a pull request, not directly");
    }
    if c.state != ChangeState::Landing {
        return refuse(format!("change {} is {}, not being landed", r.display(), c.state.as_str()));
    }
    // 2. Where it goes: the record's repository, base and remote. Nothing here is the request's.
    let repo = Path::new(&d.repo);
    let objects = repo.join(".git/objects");
    if !repo.is_absolute() || !objects.is_dir() {
        return refuse(format!("{} is not a repository's main checkout", d.repo));
    }
    let base = d.base.as_str();
    if base.is_empty() || base.starts_with('-') || base.contains([' ', ':', '^', '~', '*', '?', '[', '\\']) || base.contains("..") {
        return refuse(format!("{base:?} is not a branch name"));
    }
    let (url, from) = match d.remote.clone() {
        Some(url) => (url, "the approval on record"),
        None => match origin_of(repo) {
            Some(url) => (url, "the repository's config"),
            None => return refuse(format!("{} has no origin to push to", d.repo)),
        },
    };
    on.remote = Some(url.clone());
    on.remote_from = Some(from.to_string());
    if let Some(why) = url_refused(&url) {
        return refuse(format!("its remote {url:?} isn't one otto pushes to: {why}"));
    }
    // 3. A scratch repository of the step's own, with the real one's objects and nothing else of it.
    let scratch = dir().join("scratch").join(format!("{}-{}.git", request.run, request.n));
    let _ = std::fs::remove_dir_all(&scratch);
    let made = (|| -> Result<(), String> {
        std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
        let mut init = clean_git();
        init.current_dir(&scratch).args(["init", "--quiet", "--bare", "."]);
        run(init)?;
        let objects = objects.canonicalize().map_err(|e| e.to_string())?;
        std::fs::write(scratch.join("objects/info/alternates"), format!("{}\n", objects.display())).map_err(|e| e.to_string())
    })();
    if let Err(e) = made {
        return refuse(format!("couldn't make a scratch repository under {}: {e}", dir().display()));
    }
    let git = |args: &[&str]| {
        let mut cmd = scratch_git(&scratch);
        cmd.args(args);
        run(cmd)
    };
    let outcome = (|| {
        // The remote's base, asked of the remote now: the checkout's origin/<base> is a ref a wake can write.
        let refspec = format!("+refs/heads/{base}:refs/landing/base");
        if let Err(e) = git(&["fetch", "--quiet", "--no-tags", "--no-recurse-submodules", "--", &url, &refspec]) {
            return Err((REMOTE_REFUSED, format!("couldn't fetch {base} from {url}: {}", first_line(&e))));
        }
        let b = git(&["rev-parse", "--verify", "--quiet", "refs/landing/base^{commit}"]).map_err(|_| (REMOTE_REFUSED, format!("{url} gave no {base}")))?;
        on.remote_base = Some(b.clone());
        if git(&["cat-file", "-e", &format!("{head}^{{commit}}")]).is_err() {
            return refuse(format!("{} is not a commit in {}", crate::changes::short_sha(head), d.repo));
        }
        // 4. Once only: what an earlier pass of this step pushed and didn't live to record.
        if git(&["merge-base", "--is-ancestor", head, &b]).is_ok() {
            let ours = read_result(&request.run, request.n).is_some_and(|p| p.head == head && p.verdict == PUSHING && p.judged_on.approved_sha.as_deref() == Some(a.sha.as_str()));
            if ours || head == a.sha {
                return Ok(Outcome::Landed { pushed_now: false });
            }
            return refuse(format!("the remote's {base} already contains {}, and it isn't the approved commit or one this step pushed", crate::changes::short_sha(head)));
        }
        if git(&["merge-base", "--is-ancestor", &b, head]).is_err() {
            return Err((BASE_MOVED, format!("{} doesn't contain the remote's {base} ({}): rebase onto it and ask again", crate::changes::short_sha(head), crate::changes::short_sha(&b))));
        }
        // The whole defence: the approved commit, or the approved patch on the remote's base.
        if head != a.sha {
            let mut diff = scratch_git(&scratch);
            diff.args(["diff", "--no-color", "--no-ext-diff", "--no-textconv", "--find-renames", &b, head]);
            let patch = run_raw(diff, None).map_err(|e| (REFUSED, format!("couldn't read its patch: {}", first_line(&e))))?;
            let mut id = scratch_git(&scratch);
            id.args(["patch-id", "--stable"]);
            let id = run_raw(id, Some(&patch)).ok().and_then(|o| String::from_utf8_lossy(&o).split_whitespace().next().map(str::to_string));
            on.patch_id = id.clone();
            if id.as_deref() != Some(a.patch_id.as_str()) {
                return refuse(format!(
                    "{} is not what was approved: v{} was {}, and this one's patch against the remote's {base} differs",
                    crate::changes::short_sha(head),
                    a.version,
                    crate::changes::short_sha(&a.sha)
                ));
            }
        }
        // 5. Said before the push, so a step that dies here is known to have got this far.
        let pushing = LandResult { run: request.run.clone(), n: request.n, head: head.to_string(), verdict: PUSHING.to_string(), why: None, judged_on: on.clone(), pushed: None, checkout: None, at: Timestamp::now() };
        write_result(&pushing).map_err(|e| (REFUSED, format!("couldn't record the landing before pushing: {}", e.message)))?;
        match git(&["push", "--quiet", "--no-verify", "--no-recurse-submodules", "--", &url, &format!("{head}:refs/heads/{base}")]) {
            Ok(_) => Ok(Outcome::Landed { pushed_now: true }),
            Err(e) => Err((REMOTE_REFUSED, e.lines().filter(|l| !l.trim().is_empty()).take(3).collect::<Vec<_>>().join(" / "))),
        }
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    outcome
}
