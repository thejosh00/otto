//! Changes: code a run wrote, handed back to be landed.
//!
//! Before this, every run that wrote code invented its own way of giving it back — a branch and a
//! message naming the script to ship it with, a worktree and "merge it from the main checkout", a
//! gate with the diff pasted into an output — and each left the last mile to the person. A change
//! is that hand-back made one thing. A wake commits on a branch and submits it (`otto state change
//! submit`); the person decides on one page (`otto changes`, the web's Changes); and a run of its
//! own, the **lander** — the only run that pushes — lands what was approved and cleans up after it.
//!
//! - **A change doesn't stop its run.** Submitting is not a gate: the run goes on with its next
//!   piece of work, and whatever happens to the change reaches it as a note ("About change 3@v2").
//! - **What is approved is a snapshot.** The patch is kept as it was submitted, so the page shows
//!   exactly what will land, and an approval names the version, its commit and its `git patch-id`.
//!   Just before it pushes, the lander checks the commit against that (`check`): the same commit,
//!   or a clean rebase of the same patch onto a base that moved. Anything else goes back to the
//!   person to look at again.
//! - **Every change says whose move it is** — the person's, its author's, the lander's, or no one's
//!   once it is over (`Move`). Many changes stay easy to follow that way: what waits on you is one
//!   list, and nothing else asks for your attention.
//!
//! On disk, with the run that wrote it: `changes/index.json` is everything but the text;
//! `changes/<n>-<key>/v<k>.patch` is each version's patch and `v<k>.summary.md` what its author
//! said about it, both verbatim.

use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::event::Event;
use crate::state::RunState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CHANGES_DIR: &str = "changes";
pub const INDEX_FILE: &str = "changes/index.json";
/// A change's title is a line in a list.
pub const TITLE_MAX_CHARS: usize = 120;
/// What the author says about a version: what and why, then the risk. A page, not a history.
pub const SUMMARY_MAX_BYTES: usize = 16 * 1024;
/// A patch past this is kept as its stat only; the page says how to read it in the repo.
pub const PATCH_MAX_BYTES: usize = 1024 * 1024;
/// Changed lines past which a change is flagged large: one concern a change reviews quickly.
pub const LARGE_LINES: u32 = 400;
/// What the tests said, in a line or two.
pub const TESTS_MAX_CHARS: usize = 300;
/// What a person or the lander says about a change when they move it on: it goes to the author in
/// a note, with room left for the lines around it.
pub const SAY_MAX_BYTES: usize = 3072;
/// A finished change stays in "done this week" for this long.
pub const DONE_SHOWN_DAYS: i64 = 7;
/// The opening of a note about a change, so the wake knows which, and the page can say so.
pub const NOTE_ABOUT_PREFIX: &str = "About change ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeState {
    /// Submitted, waiting for the person to decide.
    Review,
    /// Someone asked for changes; its author is making them.
    Revising,
    /// The person approved it, and it waits for the lander.
    Approved,
    /// The lander has it in hand.
    Landing,
    /// It is on its base branch.
    Landed,
    /// Not wanted: the person dropped it, or its author withdrew it.
    Dropped,
}

impl ChangeState {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeState::Review => "review",
            ChangeState::Revising => "revising",
            ChangeState::Approved => "approved",
            ChangeState::Landing => "landing",
            ChangeState::Landed => "landed",
            ChangeState::Dropped => "dropped",
        }
    }

    /// Landed or dropped: nothing more happens to the change itself, only cleaning up after it.
    pub fn is_over(self) -> bool {
        matches!(self, ChangeState::Landed | ChangeState::Dropped)
    }
}

/// Whose move it is. The person's is the only list they have to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Move {
    You,
    Author,
    Lander,
    Nobody,
}

/// How an approved change lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum LandMode {
    /// Onto its base branch, and pushed
    Push,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    /// In the order submitted.
    #[serde(default)]
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// Its number within the run: `3/2` is the run's second change.
    pub n: u32,
    /// What the run calls it; the same key again is its next version. The branch, by default.
    pub key: String,
    /// The latest version's title.
    pub title: String,
    /// The repository's main checkout, absolute.
    pub repo: String,
    /// The branch it lands on.
    pub base: String,
    pub branch: String,
    /// Where its author works on it, when it has a worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    pub state: ChangeState,
    /// When it entered its state: how long the move has been whoever's it is.
    pub state_at: Timestamp,
    pub versions: Vec<ChangeVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed: Option<Landed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleaned: Option<Cleaned>,
    /// Everything that happened to it, oldest first, whoever did it.
    #[serde(default)]
    pub timeline: Vec<Moment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeVersion {
    pub k: u32,
    pub sha: String,
    /// Where it branched from its base: what the patch is against.
    pub base_sha: String,
    /// `git patch-id --stable` of the whole patch: the same after a clean rebase.
    pub patch_id: String,
    pub at: Timestamp,
    /// `author`, or `lander` for a rebase the lander made.
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<u32>,
    /// The patch, relative to the run directory; `None` when it was too big to keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    pub patch_bytes: u64,
    pub commits: u32,
    pub stat: Stat,
    /// What its author said about it, relative to the run directory.
    pub summary: String,
    /// What the tests said, in the author's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stat {
    pub files: Vec<FileStat>,
    pub add: u32,
    pub del: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    pub path: String,
    /// `None` for a binary file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub del: Option<u32>,
}

/// The person's approval, bound to the version it was given for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub mode: LandMode,
    pub version: u32,
    pub sha: String,
    pub patch_id: String,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Landed {
    /// The commit now on the base branch.
    pub sha: String,
    pub at: Timestamp,
    /// Whether the main checkout's own base branch has it too, or only the remote does.
    pub local_base: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cleaned {
    pub at: Timestamp,
    /// What was left behind on purpose, and why: a worktree with uncommitted work in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
}

/// One thing that happened to a change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Moment {
    pub at: Timestamp,
    /// `author`, `you` or `lander`.
    pub by: String,
    pub what: String,
}

impl Change {
    pub fn latest(&self) -> &ChangeVersion {
        self.versions.last().expect("a change has at least one version")
    }

    pub fn version(&self, k: u32) -> Option<&ChangeVersion> {
        self.versions.iter().find(|v| v.k == k)
    }

    /// Whose move it is. Over and cleaned up, no one's; over and not yet, the lander's.
    pub fn whose_move(&self) -> Move {
        match self.state {
            ChangeState::Review => Move::You,
            ChangeState::Revising => Move::Author,
            ChangeState::Approved | ChangeState::Landing => Move::Lander,
            ChangeState::Landed | ChangeState::Dropped if self.cleaned.is_none() => Move::Lander,
            ChangeState::Landed | ChangeState::Dropped => Move::Nobody,
        }
    }

    fn dir(&self) -> String {
        format!("{CHANGES_DIR}/{}-{}", self.n, self.key)
    }

    fn moment(&mut self, by: &str, what: impl Into<String>) {
        self.timeline.push(Moment { at: Timestamp::now(), by: by.to_string(), what: what.into() });
    }

    fn enter(&mut self, state: ChangeState) {
        self.state = state;
        self.state_at = Timestamp::now();
    }
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

pub fn read_index(run_dir: &Path) -> Result<Index, OttoError> {
    match std::fs::read_to_string(run_dir.join(INDEX_FILE)) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| OttoError::conflict(format!("{INDEX_FILE} is not valid: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::default()),
        Err(e) => Err(e.into()),
    }
}

/// For views: an unreadable index shows as no changes rather than failing the whole page.
pub fn index_or_empty(run_dir: &Path) -> Index {
    read_index(run_dir).unwrap_or_default()
}

fn write_index(run_dir: &Path, index: &Index) -> Result<(), OttoError> {
    std::fs::create_dir_all(run_dir.join(CHANGES_DIR))?;
    crate::state::write_atomic(&run_dir.join(INDEX_FILE), &(serde_json::to_string_pretty(index)? + "\n"))
}

/// `3/2`, or `otto-feedback/2`: a run (anything `resolve_run_id` takes) and a change's number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRef {
    pub run: String,
    pub n: u32,
}

impl ChangeRef {
    pub fn parse(text: &str) -> Result<Self, OttoError> {
        let text = text.trim();
        let (run, n) = text.rsplit_once('/').ok_or_else(|| {
            OttoError::usage(format!("{text:?}: a change is <run>/<number>, like 3/2 or otto-feedback/2 — `otto changes` lists them"))
        })?;
        let n = n.trim_start_matches('#').parse::<u32>().map_err(|_| OttoError::usage(format!("{text:?}: the part after / is the change's number")))?;
        Ok(ChangeRef { run: crate::paths::resolve_run_id(run)?, n })
    }

    /// How a person types it back: the run's short id, then the number.
    pub fn display(&self) -> String {
        format!("{}/{}", crate::paths::short_id(&self.run), self.n)
    }
}

/// Run `f` on change `r` with its run locked, and write the index back if it succeeds.
fn with_change<T>(r: &ChangeRef, f: impl FnOnce(&Path, &mut RunState, &mut Change) -> Result<T, OttoError>) -> Result<T, OttoError> {
    crate::state::transaction(&r.run, |path, state| {
        let mut index = read_index(path)?;
        let change = index
            .changes
            .iter_mut()
            .find(|c| c.n == r.n)
            .ok_or_else(|| OttoError::not_found(format!("{} has no change {}", state.label(), r.n)))?;
        let out = f(path, state, change)?;
        write_index(path, &index)?;
        Ok(out)
    })
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

fn git_raw(dir: &Path, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
    use std::io::Write as _;
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    // A person's git config can page, colour or run an external diff; none of that belongs in a
    // patch someone reviews.
    cmd.env("GIT_PAGER", "cat").env("GIT_EXTERNAL_DIFF", "").env("GIT_TERMINAL_PROMPT", "0");
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    cmd.stdin(if input.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() });
    let mut child = cmd.spawn().map_err(|e| format!("cannot run git: {e}"))?;
    if let Some(input) = input {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // patch-id reads all of it before it writes, so writing first can't deadlock.
        stdin.write_all(input).map_err(|e| format!("cannot write to git: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("git failed: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if said.is_empty() { format!("git {} failed", args.join(" ")) } else { said })
    }
}

/// Trimmed at the end only: `status --porcelain` starts a line with a space that means something.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_raw(dir, args, None).map(|out| String::from_utf8_lossy(&out).trim_end().to_string())
}

fn ok(dir: &Path, args: &[&str]) -> bool {
    git(dir, args).is_ok()
}

/// The repository's main checkout, from it or from any of its worktrees.
fn main_repo(repo: &Path) -> Result<PathBuf, OttoError> {
    // Relative to `repo` when git says it relative (`--path-format=absolute` is git 2.31; macOS has older).
    let common = git(repo, &["rev-parse", "--git-common-dir"])
        .map_err(|e| OttoError::usage(format!("--repo {}: not a git repository ({e})", repo.display())))?;
    let common = repo.join(common);
    let common = common.canonicalize().unwrap_or(common);
    let top = if common.file_name().is_some_and(|n| n == ".git") { common.parent().map(Path::to_path_buf).unwrap_or(common) } else { common };
    Ok(top.canonicalize().unwrap_or(top))
}

/// The branch a change lands on when its author didn't say: the remote's default, else `main`,
/// else `master`.
pub fn default_base(repo: &Path) -> Option<String> {
    if let Ok(head) = git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        if let Some(b) = head.strip_prefix("origin/") {
            return Some(b.to_string());
        }
    }
    ["main", "master"].into_iter().find(|b| ok(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")])).map(str::to_string)
}

/// Where the base branch is, as far as this checkout knows: the remote's copy when there is a
/// remote, which is what landing means; else the local branch, for a repo with no remote.
pub fn base_ref(repo: &Path, base: &str) -> String {
    let remote = format!("refs/remotes/origin/{base}");
    if ok(repo, &["rev-parse", "--verify", "--quiet", &remote]) { format!("origin/{base}") } else { base.to_string() }
}

fn rev(repo: &Path, what: &str) -> Result<String, OttoError> {
    git(repo, &["rev-parse", "--verify", "--quiet", &format!("{what}^{{commit}}")]).map_err(|_| OttoError::usage(format!("{what}: no such commit in {}", repo.display())))
}

/// The worktree a branch is checked out in, when it is.
pub fn worktree_of(repo: &Path, branch: &str) -> Option<String> {
    let list = git(repo, &["worktree", "list", "--porcelain"]).ok()?;
    let want = format!("branch refs/heads/{branch}");
    let mut path = None;
    for line in list.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
        } else if line == want {
            return path;
        }
    }
    None
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    ok(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
}

/// What a version is: its patch against where it branched, the patch's id, and its stat.
struct Snapshot {
    base_sha: String,
    patch: Vec<u8>,
    patch_id: String,
    stat: Stat,
    commits: u32,
}

fn snapshot(repo: &Path, base_ref: &str, sha: &str) -> Result<Snapshot, OttoError> {
    let base_sha = git(repo, &["merge-base", base_ref, sha])
        .map_err(|_| OttoError::usage(format!("{sha} has nothing in common with {base_ref} — is it a branch of this repository?")))?;
    let commits: u32 = git(repo, &["rev-list", "--count", &format!("{base_sha}..{sha}")]).ok().and_then(|n| n.parse().ok()).unwrap_or(0);
    if commits == 0 {
        return Err(OttoError::usage(format!("nothing to review: {sha} has no commits that {base_ref} doesn't — commit the work first")));
    }
    let diff_args = ["diff", "--no-color", "--no-ext-diff", "--no-textconv", "--find-renames", base_sha.as_str(), sha];
    let patch = git_raw(repo, &diff_args, None).map_err(OttoError::usage)?;
    if patch.iter().all(u8::is_ascii_whitespace) {
        return Err(OttoError::usage(format!("nothing to review: the commits on it change nothing against {base_ref}")));
    }
    let numstat = git(repo, &["diff", "--numstat", "--find-renames", base_sha.as_str(), sha]).map_err(OttoError::usage)?;
    Ok(Snapshot { patch_id: patch_id_of(repo, &patch)?, stat: parse_numstat(&numstat), base_sha, patch, commits })
}

fn patch_id_of(repo: &Path, patch: &[u8]) -> Result<String, OttoError> {
    let out = git_raw(repo, &["patch-id", "--stable"], Some(patch)).map_err(OttoError::usage)?;
    let text = String::from_utf8_lossy(&out);
    text.split_whitespace().next().map(str::to_string).ok_or_else(|| OttoError::usage("git patch-id said nothing about the patch"))
}

/// The patch id of `head` against where it meets `base_ref` — what `check` compares.
fn patch_id_against(repo: &Path, base_ref: &str, head: &str) -> Result<String, OttoError> {
    let mb = git(repo, &["merge-base", base_ref, head]).map_err(|_| OttoError::usage(format!("{head} has nothing in common with {base_ref}")))?;
    let patch = git_raw(repo, &["diff", "--no-color", "--no-ext-diff", "--no-textconv", "--find-renames", mb.as_str(), head], None).map_err(OttoError::usage)?;
    patch_id_of(repo, &patch)
}

fn parse_numstat(text: &str) -> Stat {
    let mut stat = Stat::default();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let add = add.parse::<u32>().ok();
        let del = del.parse::<u32>().ok();
        stat.add += add.unwrap_or(0);
        stat.del += del.unwrap_or(0);
        stat.files.push(FileStat { path: path.to_string(), add, del });
    }
    stat
}

/// Tracked files with changes nobody committed — what a person would review is not what is there.
fn uncommitted(worktree: &Path) -> Vec<String> {
    git(worktree, &["status", "--porcelain", "--untracked-files=no"])
        .map(|s| s.lines().map(|l| l.get(3..).unwrap_or(l).to_string()).collect())
        .unwrap_or_default()
}

fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// `/Users/x/workspace/otto` → `~/workspace/otto`, for a person reading it.
pub fn home_relative(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Submitting
// ---------------------------------------------------------------------------

/// What a wake hands `submit`, already read from wherever the CLI found it.
#[derive(Default)]
pub struct Submit<'a> {
    pub repo: &'a str,
    pub branch: &'a str,
    pub base: Option<&'a str>,
    pub key: Option<&'a str>,
    pub title: &'a str,
    pub summary: &'a str,
    pub tests: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Submitted {
    pub n: u32,
    pub key: String,
    pub version: u32,
    /// The same commit as its latest version, so nothing new.
    pub unchanged: bool,
    pub lines: u32,
    pub files: usize,
    pub large: bool,
    pub worktree: Option<String>,
    /// The patch was over `PATCH_MAX_BYTES` and only its stat was kept.
    pub too_big: bool,
    /// What it was before this version, when it was something.
    pub was: Option<ChangeState>,
}

fn check_title(title: &str) -> Result<String, OttoError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(OttoError::usage("a change needs a title: what it does, in a few words, for a list"));
    }
    if title.contains('\n') || title.chars().count() > TITLE_MAX_CHARS {
        return Err(OttoError::usage(format!("a change's title is one line of at most {TITLE_MAX_CHARS} characters")));
    }
    Ok(title.to_string())
}

fn check_say(text: Option<&str>, what: &str) -> Result<Option<String>, OttoError> {
    let text = text.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    if text.as_ref().is_some_and(|t| t.len() > SAY_MAX_BYTES) {
        return Err(OttoError::usage(format!("{what} is at most {SAY_MAX_BYTES} bytes — point at a file for more")));
    }
    Ok(text)
}

/// Submit a version of a change: a new change when the key is new, its next version otherwise.
/// The same commit again is not a new version.
pub fn submit(run_id: &str, s: &Submit) -> Result<Submitted, OttoError> {
    let title = check_title(s.title)?;
    let summary = s.summary.trim();
    if summary.is_empty() {
        return Err(OttoError::usage("a change needs a summary for the person reviewing it: what it does and why, then what could go wrong"));
    }
    if summary.len() > SUMMARY_MAX_BYTES {
        return Err(OttoError::usage(format!("a change's summary is at most {SUMMARY_MAX_BYTES} bytes — what and why, not a history")));
    }
    let tests = s.tests.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    if tests.as_ref().is_some_and(|t| t.chars().count() > TESTS_MAX_CHARS) {
        return Err(OttoError::usage(format!("--tests is what the tests said, in at most {TESTS_MAX_CHARS} characters")));
    }
    let repo = main_repo(Path::new(s.repo))?;
    let branch = s.branch.trim().trim_start_matches("refs/heads/").to_string();
    if !branch_exists(&repo, &branch) {
        return Err(OttoError::usage(format!("--branch {branch}: no such branch in {} — commit on a branch of its own, then submit that", repo.display())));
    }
    let base = match s.base.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => b.to_string(),
        None => default_base(&repo).ok_or_else(|| OttoError::usage(format!("{}: can't tell its base branch — say it with --base", repo.display())))?,
    };
    if base == branch {
        return Err(OttoError::usage(format!("--branch {branch} is the base branch itself — a change is a branch of its own, to land on {base}")));
    }
    let sha = rev(&repo, &format!("refs/heads/{branch}"))?;
    let worktree = worktree_of(&repo, &branch);
    if let Some(wt) = &worktree {
        let dirty = uncommitted(Path::new(wt));
        if !dirty.is_empty() {
            let shown: Vec<&str> = dirty.iter().take(5).map(String::as_str).collect();
            return Err(OttoError::usage(format!(
                "{wt} has changes nobody committed ({}{}) — what the person would review isn't what's there. Commit them, or \
                 discard them, and submit again",
                shown.join(", "),
                if dirty.len() > 5 { ", …" } else { "" }
            )));
        }
    }
    let key = match s.key {
        Some(k) => crate::outputs::normalise_key(k)?,
        None => crate::clock::slugify(&branch, "change"),
    };
    let snap = snapshot(&repo, &base_ref(&repo, &base), &sha)?;
    let repo_text = repo.display().to_string();

    crate::state::transaction(run_id, |path, state| {
        let mut index = read_index(path)?;
        let wake = state.wake.as_ref().filter(|w| w.outcome.is_none()).map(|w| w.n);
        let existing = index.changes.iter().position(|c| c.key == key);
        if let Some(i) = existing {
            let c = &index.changes[i];
            match c.state {
                ChangeState::Landed | ChangeState::Dropped => {
                    return Err(OttoError::conflict(format!(
                        "change {} ({key}) is {} — new work is a new change: a new branch, or another --key",
                        c.n,
                        c.state.as_str()
                    )))
                }
                ChangeState::Landing => {
                    return Err(OttoError::conflict(format!(
                        "change {} ({key}) is being landed right now — what lands is the version the person approved. Wait \
                         for the note saying it landed (or came back), then submit what is new as another change",
                        c.n
                    )))
                }
                _ => {}
            }
            let latest = c.latest();
            if latest.sha == sha && latest.base_sha == snap.base_sha {
                let c = &mut index.changes[i];
                c.title = title.clone();
                c.worktree = worktree.clone().or(c.worktree.take());
                let out = submitted(c, true, None);
                write_index(path, &index)?;
                return Ok(out);
            }
        }
        let (n, k) = match existing {
            Some(i) => (index.changes[i].n, index.changes[i].latest().k + 1),
            None => (index.changes.iter().map(|c| c.n).max().unwrap_or(0) + 1, 1),
        };
        let dir = format!("{CHANGES_DIR}/{n}-{key}");
        std::fs::create_dir_all(path.join(&dir))?;
        let too_big = snap.patch.len() > PATCH_MAX_BYTES;
        let patch = if too_big {
            None
        } else {
            let rel = format!("{dir}/v{k}.patch");
            crate::state::write_atomic(&path.join(&rel), &String::from_utf8_lossy(&snap.patch))?;
            Some(rel)
        };
        let summary_rel = format!("{dir}/v{k}.summary.md");
        crate::state::write_atomic(&path.join(&summary_rel), &format!("{summary}\n"))?;
        let version = ChangeVersion {
            k,
            sha: sha.clone(),
            base_sha: snap.base_sha.clone(),
            patch_id: snap.patch_id.clone(),
            at: Timestamp::now(),
            by: "author".into(),
            wake,
            patch,
            patch_bytes: snap.patch.len() as u64,
            commits: snap.commits,
            stat: snap.stat.clone(),
            summary: summary_rel,
            tests: tests.clone(),
        };
        let lines = version.stat.add + version.stat.del;
        let (c, was) = match existing {
            Some(i) => {
                let c = &mut index.changes[i];
                let was = c.state;
                c.title = title.clone();
                c.base = base.clone();
                c.worktree = worktree.clone().or(c.worktree.take());
                c.versions.push(version);
                // An approval was for the version it named; this one is new.
                c.decision = None;
                c.enter(ChangeState::Review);
                c.moment("author", format!("v{k}: {} commit{}, +{} −{}", snap.commits, if snap.commits == 1 { "" } else { "s" }, snap.stat.add, snap.stat.del));
                (c, Some(was))
            }
            None => {
                let mut c = Change {
                    n,
                    key: key.clone(),
                    title: title.clone(),
                    repo: repo_text.clone(),
                    base: base.clone(),
                    branch: branch.clone(),
                    worktree: worktree.clone(),
                    state: ChangeState::Review,
                    state_at: Timestamp::now(),
                    versions: vec![version],
                    decision: None,
                    landed: None,
                    cleaned: None,
                    timeline: Vec::new(),
                };
                c.moment("author", format!("submitted: {} commit{}, +{} −{} in {} file{}", snap.commits, if snap.commits == 1 { "" } else { "s" }, snap.stat.add, snap.stat.del, snap.stat.files.len(), if snap.stat.files.len() == 1 { "" } else { "s" }));
                index.changes.push(c);
                (index.changes.last_mut().expect("just pushed"), None)
            }
        };
        let mut out = submitted(c, false, was);
        out.too_big = too_big;
        out.lines = lines;
        write_index(path, &index)?;
        crate::event::record(path, &Event::ChangeSubmitted { change: n, version: k, title: title.clone(), sha: sha.clone(), branch: branch.clone() })?;
        Ok(out)
    })
}

fn submitted(c: &Change, unchanged: bool, was: Option<ChangeState>) -> Submitted {
    let v = c.latest();
    let lines = v.stat.add + v.stat.del;
    Submitted {
        n: c.n,
        key: c.key.clone(),
        version: v.k,
        unchanged,
        lines,
        files: v.stat.files.len(),
        large: lines > LARGE_LINES,
        worktree: c.worktree.clone(),
        too_big: v.patch.is_none(),
        was,
    }
}

/// Its author no longer wants it — overtaken, or the wrong approach. The lander cleans up after it.
pub fn withdraw(r: &ChangeRef, reason: &str) -> Result<Change, OttoError> {
    let reason = check_say(Some(reason), "--reason")?.ok_or_else(|| OttoError::usage("say why, with --reason"))?;
    with_change(r, |path, _state, c| {
        refuse_unless(c, &[ChangeState::Review, ChangeState::Revising, ChangeState::Approved], "withdrawn")?;
        c.decision = None;
        c.enter(ChangeState::Dropped);
        c.moment("author", format!("withdrew it: {reason}"));
        settle_if_nothing_to_clean(c);
        moved(path, c, "author", &format!("withdrew it: {reason}"))?;
        Ok(c.clone())
    })
}

fn refuse_unless(c: &Change, allowed: &[ChangeState], doing: &str) -> Result<(), OttoError> {
    if allowed.contains(&c.state) {
        return Ok(());
    }
    let why = match c.state {
        ChangeState::Landing => "the lander is landing it right now".to_string(),
        ChangeState::Landed => format!("it landed{}", c.landed.as_ref().map(|l| format!(" at {}", short_sha(&l.sha))).unwrap_or_default()),
        ChangeState::Dropped => "it was dropped".to_string(),
        ChangeState::Review => "it is waiting for review".to_string(),
        ChangeState::Revising => "its author is revising it".to_string(),
        ChangeState::Approved => "it is approved and waiting for the lander".to_string(),
    };
    Err(OttoError::conflict(format!("change {} can't be {doing}: {why}", c.n)))
}

fn moved(path: &Path, c: &Change, by: &str, what: &str) -> Result<(), OttoError> {
    crate::event::record(path, &Event::ChangeMoved { change: c.n, state: c.state, by: by.to_string(), what: what.to_string() })
}

/// Dropped with no worktree and no branch left: nothing for the lander to do.
fn settle_if_nothing_to_clean(c: &mut Change) {
    let repo = Path::new(&c.repo);
    let worktree = worktree_of(repo, &c.branch).is_some() || c.worktree.as_ref().is_some_and(|w| Path::new(w).exists());
    if !worktree && !branch_exists(repo, &c.branch) {
        c.cleaned = Some(Cleaned { at: Timestamp::now(), left: None });
    }
}

/// The opening line of a note about a change, then what is said.
fn note_text(c: &Change, body: &str) -> String {
    format!("{NOTE_ABOUT_PREFIX}{}@v{}, \"{}\":\n\n{}", c.n, c.latest().k, c.title, body.trim())
}

/// Tell the author, when its run can still hear it. A finished run can't, and that's said on the
/// change instead of failing what the person or the lander did.
fn tell_author(path: &Path, state: &mut RunState, c: &mut Change, body: &str) -> Result<Option<String>, OttoError> {
    if state.status.is_terminal() {
        c.moment("otto", format!("its run is {}, so nobody was told", crate::state::commands::status_str(state.status)));
        return Ok(None);
    }
    let note = crate::notes::add_in(path, state, &note_text(c, body), false)?;
    Ok(Some(note.id))
}

// ---------------------------------------------------------------------------
// The person
// ---------------------------------------------------------------------------

/// What a person's decision did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Decided {
    pub reference: String,
    pub title: String,
    pub state: ChangeState,
    /// The note the author was left, when one was.
    pub note: Option<String>,
}

/// Approve the latest version of each change, to land as `mode`. Refused whole when there is no
/// lander to land them, and each one refused that isn't waiting for review.
pub fn approve(refs: &[ChangeRef], mode: LandMode, note: Option<&str>) -> Result<Vec<Decided>, OttoError> {
    if crate::config::lander()?.is_none() {
        return Err(OttoError::usage(
            "no lander is set up, so nothing would land an approved change — start one and name it with `otto config lander \
             <run>` (the README's \"Landing changes\" says how), or land it yourself: `otto changes show <change>` has the commands",
        ));
    }
    let note = check_say(note, "--note")?;
    let mut out = Vec::new();
    for r in refs {
        out.push(with_change(r, |path, _state, c| {
            refuse_unless(c, &[ChangeState::Review], "approved")?;
            let v = c.latest().clone();
            c.decision = Some(Decision { mode, version: v.k, sha: v.sha.clone(), patch_id: v.patch_id.clone(), at: Timestamp::now(), note: note.clone() });
            c.enter(ChangeState::Approved);
            let what = format!("approved v{} to land on {}{}", v.k, c.base, note.as_ref().map(|n| format!(": {n}")).unwrap_or_default());
            c.moment("you", &what);
            moved(path, c, "person", &what)?;
            Ok(Decided { reference: r.display(), title: c.title.clone(), state: c.state, note: None })
        })?);
    }
    Ok(out)
}

/// Ask its author for changes. The author is told, in the person's words.
pub fn revise(r: &ChangeRef, text: &str) -> Result<Decided, OttoError> {
    let text = check_say(Some(text), "what to change")?.ok_or_else(|| OttoError::usage("say what to change"))?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::Review, ChangeState::Approved], "sent back")?;
        c.decision = None;
        c.enter(ChangeState::Revising);
        c.moment("you", format!("asked for changes: {text}"));
        let note = tell_author(path, state, c, &format!(
            "The person asked for changes:\n\n{text}\n\nWork in the same worktree, commit, and submit it again with the same key — \
             `otto state change submit` — so it comes back to them as the next version."
        ))?;
        moved(path, c, "person", &format!("asked for changes: {text}"))?;
        Ok(Decided { reference: r.display(), title: c.title.clone(), state: c.state, note })
    })
}

/// Not wanted. The author is told; the lander cleans up its worktree and branch.
pub fn drop_change(r: &ChangeRef, text: Option<&str>) -> Result<Decided, OttoError> {
    let text = check_say(text, "why")?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::Review, ChangeState::Revising, ChangeState::Approved], "dropped")?;
        c.decision = None;
        c.enter(ChangeState::Dropped);
        let what = match &text {
            Some(t) => format!("dropped it: {t}"),
            None => "dropped it".to_string(),
        };
        c.moment("you", &what);
        settle_if_nothing_to_clean(c);
        let body = match &text {
            Some(t) => format!("The person dropped it, saying:\n\n{t}\n\nIt won't land. The lander removes its worktree and branch."),
            None => "The person dropped it. It won't land. The lander removes its worktree and branch.".to_string(),
        };
        let note = tell_author(path, state, c, &body)?;
        moved(path, c, "person", &what)?;
        Ok(Decided { reference: r.display(), title: c.title.clone(), state: c.state, note })
    })
}

// ---------------------------------------------------------------------------
// The lander
// ---------------------------------------------------------------------------

/// The lander, checked: only the run named in `config.json` lands changes.
pub fn require_lander(lander: &str) -> Result<String, OttoError> {
    let id = crate::paths::resolve_run_id(lander)?;
    match crate::config::lander()? {
        Some(l) if l == id => Ok(id),
        Some(l) => Err(OttoError::conflict(format!(
            "{} isn't the lander — {} is (`otto config lander`). Only the lander lands changes",
            crate::paths::short_id(&id),
            crate::paths::short_id(&l)
        ))),
        None => Err(OttoError::conflict("no lander is set up (`otto config lander <run>`), so nothing may land changes")),
    }
}

/// One change waiting on the lander, with what it needs to do it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Due {
    pub reference: String,
    pub run: String,
    pub run_name: String,
    pub n: u32,
    pub title: String,
    pub state: ChangeState,
    /// `land`, `finish landing` (claimed and not done: a wake that didn't finish), or `clean up`.
    pub todo: String,
    pub repo: String,
    pub base: String,
    pub branch: String,
    pub worktree: Option<String>,
    /// The approved version's commit and patch id — what lands.
    pub sha: Option<String>,
    pub patch_id: Option<String>,
    pub mode: Option<LandMode>,
    pub approved_at: Option<Timestamp>,
    /// What its author said about it, absolute: the commit message and PR body come from here.
    pub summary: String,
    pub landed_sha: Option<String>,
}

fn due_of(run: &str, name: &str, dir: &Path, c: &Change) -> Option<Due> {
    let todo = match c.state {
        ChangeState::Approved => "land",
        ChangeState::Landing => "finish landing",
        ChangeState::Landed | ChangeState::Dropped if c.cleaned.is_none() => "clean up",
        _ => return None,
    };
    let d = c.decision.as_ref();
    Some(Due {
        reference: format!("{}/{}", crate::paths::short_id(run), c.n),
        run: run.to_string(),
        run_name: name.to_string(),
        n: c.n,
        title: c.title.clone(),
        state: c.state,
        todo: todo.to_string(),
        repo: c.repo.clone(),
        base: c.base.clone(),
        branch: c.branch.clone(),
        worktree: c.worktree.clone(),
        sha: d.map(|d| d.sha.clone()),
        patch_id: d.map(|d| d.patch_id.clone()),
        mode: d.map(|d| d.mode),
        approved_at: d.map(|d| d.at),
        summary: dir.join(&c.latest().summary).display().to_string(),
        landed_sha: c.landed.as_ref().map(|l| l.sha.clone()),
    })
}

/// Everything waiting on the lander, across every run: what it had in hand first, then what was
/// approved, oldest approval first, then what needs cleaning up after.
pub fn due() -> Result<Vec<Due>, OttoError> {
    let mut out = Vec::new();
    for entry in crate::state::read_all_runs()? {
        let crate::state::RunEntry::Readable(state) = entry else { continue };
        let Ok(dir) = crate::paths::run_dir(&state.id) else { continue };
        let name = state.display_name();
        for c in index_or_empty(&dir).changes {
            if let Some(d) = due_of(&state.id, &name, &dir, &c) {
                out.push(d);
            }
        }
    }
    let rank = |d: &Due| match d.todo.as_str() {
        "finish landing" => 0,
        "land" => 1,
        _ => 2,
    };
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.approved_at.cmp(&b.approved_at)));
    Ok(out)
}

/// The lander takes an approved change in hand. Taking one already in hand again is fine: a wake
/// that didn't finish landing it is picked up by the next.
pub fn claim(lander: &str, r: &ChangeRef) -> Result<Due, OttoError> {
    let lander = require_lander(lander)?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::Approved, ChangeState::Landing], "landed")?;
        let fresh = c.state == ChangeState::Approved;
        if fresh {
            c.enter(ChangeState::Landing);
            c.moment("lander", "started landing it");
            moved(path, c, &lander, "started landing it")?;
        }
        let mut due = due_of(&state.id, &state.display_name(), path, c).expect("landing is due");
        if fresh {
            due.todo = "land".to_string();
        }
        Ok(due)
    })
}

/// Whether `head` is what the person approved: the approved commit itself, or one with the same
/// patch against where it meets the base now — a clean rebase. Anything else is refused (exit 2):
/// record it with `rebased` so the person looks again, or `bounce` it to its author.
pub fn check(lander: &str, r: &ChangeRef, head: &str) -> Result<String, OttoError> {
    require_lander(lander)?;
    let state = crate::state::read_run(&r.run)?;
    let dir = crate::paths::run_dir(&r.run)?;
    let c = read_index(&dir)?.changes.into_iter().find(|c| c.n == r.n).ok_or_else(|| OttoError::not_found(format!("{} has no change {}", state.label(), r.n)))?;
    if c.state != ChangeState::Landing {
        return Err(OttoError::conflict(format!("DO NOT ACT: change {} is {}, not being landed — claim it first", r.display(), c.state.as_str())));
    }
    let d = c.decision.as_ref().ok_or_else(|| OttoError::conflict(format!("DO NOT ACT: change {} has no approval on record", r.display())))?;
    let repo = Path::new(&c.repo);
    let sha = rev(repo, head)?;
    if sha == d.sha {
        return Ok(format!("ok: {} is v{}, as approved", short_sha(&sha), d.version));
    }
    let id = patch_id_against(repo, &base_ref(repo, &c.base), &sha)?;
    if id == d.patch_id {
        return Ok(format!("ok: {} is v{} rebased onto {} with the same patch — a clean rebase of what was approved", short_sha(&sha), d.version, base_ref(repo, &c.base)));
    }
    Err(OttoError::conflict(format!(
        "DO NOT ACT: {} is not what was approved — v{} was {}, and this one's patch differs (the code around it changed, or \
         the commits did). Record it with `otto state change rebased {} {} --head {}` so the person looks again, or bounce it \
         to its author",
        short_sha(&sha),
        d.version,
        short_sha(&d.sha),
        crate::paths::short_id(lander),
        r.display(),
        short_sha(&sha)
    )))
}

/// It landed: `sha` is on the base branch now. otto looks for itself — the base, as the remote
/// knows it after the push, must contain it. The author is told, and woken to do what follows.
pub fn landed(lander: &str, r: &ChangeRef, sha: &str, note: Option<&str>) -> Result<Change, OttoError> {
    let lander = require_lander(lander)?;
    let note = check_say(note, "--note")?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::Landing], "marked landed")?;
        let repo = Path::new(&c.repo);
        let sha = rev(repo, sha)?;
        let base = base_ref(repo, &c.base);
        if !ok(repo, &["merge-base", "--is-ancestor", &sha, &base]) {
            return Err(OttoError::conflict(format!(
                "{base} doesn't contain {} — it hasn't landed yet, or this checkout hasn't heard (`git -C {} fetch`)",
                short_sha(&sha),
                c.repo
            )));
        }
        let local_base = ok(repo, &["merge-base", "--is-ancestor", &sha, &format!("refs/heads/{}", c.base)]);
        c.landed = Some(Landed { sha: sha.clone(), at: Timestamp::now(), local_base, note: note.clone() });
        c.enter(ChangeState::Landed);
        let what = format!("landed on {} at {}{}", c.base, short_sha(&sha), note.as_ref().map(|n| format!(" — {n}")).unwrap_or_default());
        c.moment("lander", &what);
        let checkout = if local_base {
            String::new()
        } else {
            format!("\n\nThe main checkout's own {} doesn't have it yet; anything you run from there runs the old code until it is updated.", c.base)
        };
        let body = format!(
            "It landed on {} at {}.{}{checkout}\n\nDo whatever comes after it landing — what your instructions say follows — and nothing \
             more to the change itself.",
            c.base,
            short_sha(&sha),
            note.as_ref().map(|n| format!(" The lander says: {n}")).unwrap_or_default()
        );
        tell_author(path, state, c, &body)?;
        wake_soon(state);
        moved(path, c, &lander, &what)?;
        Ok(c.clone())
    })
}

/// The lander couldn't land it — a conflict, tests red after a rebase — and it goes back to its
/// author, with why.
pub fn bounce(lander: &str, r: &ChangeRef, reason: &str) -> Result<Change, OttoError> {
    let lander = require_lander(lander)?;
    let reason = check_say(Some(reason), "--reason")?.ok_or_else(|| OttoError::usage("say why, with --reason"))?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::Approved, ChangeState::Landing], "sent back")?;
        c.decision = None;
        c.enter(ChangeState::Revising);
        c.moment("lander", format!("couldn't land it: {reason}"));
        let body = format!(
            "The lander couldn't land it:\n\n{reason}\n\nFix that in the same worktree — rebase onto {} if it says so — commit, and \
             submit it again with the same key. It goes back to the person as the next version.",
            c.base
        );
        tell_author(path, state, c, &body)?;
        wake_soon(state);
        moved(path, c, &lander, &format!("couldn't land it: {reason}"))?;
        Ok(c.clone())
    })
}

/// The lander rebased it onto a base that moved, and the patch changed — the code around it did —
/// so it goes back to the person as a new version, the lander's.
pub fn rebased(lander: &str, r: &ChangeRef, head: &str) -> Result<Change, OttoError> {
    let lander = require_lander(lander)?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::Landing], "rebased")?;
        let repo = PathBuf::from(&c.repo);
        let sha = rev(&repo, head)?;
        let snap = snapshot(&repo, &base_ref(&repo, &c.base), &sha)?;
        let k = c.latest().k + 1;
        let dir = c.dir();
        std::fs::create_dir_all(path.join(&dir))?;
        let patch = if snap.patch.len() > PATCH_MAX_BYTES {
            None
        } else {
            let rel = format!("{dir}/v{k}.patch");
            crate::state::write_atomic(&path.join(&rel), &String::from_utf8_lossy(&snap.patch))?;
            Some(rel)
        };
        let summary = c.latest().summary.clone();
        let tests = c.latest().tests.clone();
        c.versions.push(ChangeVersion {
            k,
            sha: sha.clone(),
            base_sha: snap.base_sha,
            patch_id: snap.patch_id,
            at: Timestamp::now(),
            by: "lander".into(),
            wake: None,
            patch,
            patch_bytes: snap.patch.len() as u64,
            commits: snap.commits,
            stat: snap.stat,
            summary,
            tests,
        });
        c.decision = None;
        c.enter(ChangeState::Review);
        let what = format!("rebased it onto {} as v{k} ({}); the code around it changed, so it needs another look", c.base, short_sha(&sha));
        c.moment("lander", &what);
        let body = format!(
            "The lander rebased it onto {} as v{k}, at {}, and it is back with the person: the code around it changed. Nothing for you \
             to do unless they ask for changes — if they do, start from {}, not your branch.",
            c.base,
            short_sha(&sha),
            short_sha(&sha)
        );
        tell_author(path, state, c, &body)?;
        moved(path, c, &lander, &what)?;
        Ok(c.clone())
    })
}

/// The lander cleaned up after it. otto looks for itself: no worktree on its branch, and no branch,
/// unless what was left is said.
pub fn cleaned(lander: &str, r: &ChangeRef, left: Option<&str>) -> Result<Change, OttoError> {
    let lander = require_lander(lander)?;
    let left = check_say(left, "--left")?;
    with_change(r, |path, _state, c| {
        if !c.state.is_over() {
            return Err(OttoError::conflict(format!("change {} is {}: there is nothing to clean up after yet", r.display(), c.state.as_str())));
        }
        if left.is_none() {
            let repo = Path::new(&c.repo);
            if let Some(wt) = worktree_of(repo, &c.branch) {
                return Err(OttoError::conflict(format!("{wt} is still a worktree on {} — remove it, or say what was left and why with --left", c.branch)));
            }
            if branch_exists(repo, &c.branch) {
                return Err(OttoError::conflict(format!("branch {} is still there — delete it, or say why it stays with --left", c.branch)));
            }
        }
        c.cleaned = Some(Cleaned { at: Timestamp::now(), left: left.clone() });
        let what = match &left {
            Some(l) => format!("cleaned up, leaving {l}"),
            None => "removed its worktree and branch".to_string(),
        };
        c.moment("lander", &what);
        moved(path, c, &lander, &what)?;
        Ok(c.clone())
    })
}

/// Bring a sleeping run back at the next poke, the way a timer its own wake armed would: a check
/// script doesn't stand in front of it, since what it is woken for is a note, not the world.
fn wake_soon(state: &mut RunState) {
    if state.status != crate::state::Status::Sleeping {
        return;
    }
    let now = Timestamp::now();
    state.next_wake_at = Some(now);
    state.armed_wake_at = Some(now);
    if state.check.as_ref().is_some_and(|c| !c.pinned) {
        state.check = None;
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// One change, as a list shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub reference: String,
    pub run: String,
    pub short: String,
    pub name: String,
    pub n: u32,
    pub key: String,
    pub title: String,
    pub repo: String,
    pub base: String,
    pub branch: String,
    pub state: ChangeState,
    #[serde(rename = "move")]
    pub whose: Move,
    pub since: Timestamp,
    pub submitted_at: Timestamp,
    pub version: u32,
    pub add: u32,
    pub del: u32,
    pub files: usize,
    pub large: bool,
    /// Other open changes to the same repository that touch a file this one does.
    pub overlaps: Vec<String>,
    pub tests: Option<String>,
    pub last: Option<Moment>,
    pub landed_sha: Option<String>,
    /// Its run is over, so nobody can revise it.
    pub author_gone: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesView {
    /// How many wait on the person.
    pub yours: usize,
    /// Open first — the person's move, then the agents', longest waiting first — then finished, newest first.
    pub changes: Vec<Line>,
    /// The lander, when one is set up.
    pub lander: Option<LanderRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanderRef {
    pub id: String,
    pub short: String,
    pub name: String,
    /// It exists, and isn't stopped or done.
    pub alive: bool,
}

pub fn lander_ref() -> Option<LanderRef> {
    let id = crate::config::lander().ok().flatten()?;
    let state = crate::state::read_run(&id).ok();
    Some(LanderRef {
        short: crate::paths::short_id(&id),
        name: state.as_ref().map(|s| s.display_name()).unwrap_or_else(|| id.clone()),
        alive: state.is_some_and(|s| !s.status.is_terminal()),
        id,
    })
}

fn line(run: &RunState, short: &str, c: &Change) -> Line {
    let v = c.latest();
    let lines = v.stat.add + v.stat.del;
    Line {
        reference: format!("{short}/{}", c.n),
        run: run.id.clone(),
        short: short.to_string(),
        name: run.display_name(),
        n: c.n,
        key: c.key.clone(),
        title: c.title.clone(),
        repo: home_relative(&c.repo),
        base: c.base.clone(),
        branch: c.branch.clone(),
        state: c.state,
        whose: c.whose_move(),
        since: c.state_at,
        submitted_at: c.versions.first().map(|v| v.at).unwrap_or(c.state_at),
        version: v.k,
        add: v.stat.add,
        del: v.stat.del,
        files: v.stat.files.len(),
        large: lines > LARGE_LINES,
        overlaps: Vec::new(),
        tests: v.tests.clone(),
        last: c.timeline.last().cloned(),
        landed_sha: c.landed.as_ref().map(|l| l.sha.clone()),
        author_gone: run.status.is_terminal(),
    }
}

/// Every change across every run: open ones, and those finished in the last week (all, with `all`).
pub fn list(all: bool) -> Result<ChangesView, OttoError> {
    let ids = crate::paths::all_run_ids();
    let cutoff = crate::clock::now() - time::Duration::days(DONE_SHOWN_DAYS);
    let mut rows: Vec<(Line, Vec<String>)> = Vec::new();
    for entry in crate::state::read_all_runs()? {
        let crate::state::RunEntry::Readable(state) = entry else { continue };
        let Ok(dir) = crate::paths::run_dir(&state.id) else { continue };
        let short = crate::paths::short_id_among(&state.id, &ids);
        for c in index_or_empty(&dir).changes {
            if c.whose_move() == Move::Nobody && !all && c.state_at.dt() < cutoff {
                continue;
            }
            let files = c.latest().stat.files.iter().map(|f| f.path.clone()).collect();
            rows.push((line(&state, &short, &c), files));
        }
    }
    // Two open changes to one repository that touch one file: whichever lands second may not apply.
    for i in 0..rows.len() {
        if rows[i].0.state.is_over() {
            continue;
        }
        let overlaps: Vec<String> = (0..rows.len())
            .filter(|&j| j != i && !rows[j].0.state.is_over() && rows[j].0.repo == rows[i].0.repo && rows[j].1.iter().any(|f| rows[i].1.contains(f)))
            .map(|j| rows[j].0.reference.clone())
            .collect();
        rows[i].0.overlaps = overlaps;
    }
    let mut changes: Vec<Line> = rows.into_iter().map(|(l, _)| l).collect();
    let rank = |l: &Line| match l.whose {
        Move::You => 0,
        Move::Author | Move::Lander => 1,
        Move::Nobody => 2,
    };
    changes.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| if rank(a) == 2 { b.since.cmp(&a.since) } else { a.since.cmp(&b.since) })
    });
    Ok(ChangesView { yours: changes.iter().filter(|l| l.whose == Move::You).count(), changes, lander: lander_ref() })
}

/// How many changes wait on the person, for a count in the top bar.
pub fn yours() -> usize {
    let mut n = 0;
    let Ok(runs) = crate::state::read_all_runs() else { return 0 };
    for entry in runs {
        let crate::state::RunEntry::Readable(state) = entry else { continue };
        let Ok(dir) = crate::paths::run_dir(&state.id) else { continue };
        n += index_or_empty(&dir).changes.iter().filter(|c| c.whose_move() == Move::You).count();
    }
    n
}

/// A run's own changes, newest first, for its page.
pub fn of_run(state: &RunState) -> Vec<Line> {
    let Ok(dir) = crate::paths::run_dir(&state.id) else { return Vec::new() };
    let short = crate::paths::short_id(&state.id);
    let mut out: Vec<Line> = index_or_empty(&dir).changes.iter().map(|c| line(state, &short, c)).collect();
    out.reverse();
    out
}

/// A run's changes that are still under way — for a done claim, where each is unfinished work.
pub fn open_of(run_dir: &Path) -> Vec<String> {
    index_or_empty(run_dir)
        .changes
        .iter()
        .filter(|c| !c.state.is_over())
        .map(|c| format!("change {} \"{}\" is {}", c.n, c.title, c.state.as_str()))
        .collect()
}

/// One change in full: a version of it, its patch and what its author said.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub line: Line,
    pub change: Change,
    pub version: ChangeVersion,
    /// `None` when it was too big to keep.
    pub patch: Option<String>,
    pub summary: String,
    pub lander: Option<LanderRef>,
    /// To look at it, or land it by hand, from a terminal.
    pub commands: Vec<String>,
}

pub fn read(r: &ChangeRef, version: Option<u32>) -> Result<Detail, OttoError> {
    let state = crate::state::read_run(&r.run)?;
    let dir = crate::paths::run_dir(&state.id)?;
    let ids = crate::paths::all_run_ids();
    let short = crate::paths::short_id_among(&state.id, &ids);
    let change = read_index(&dir)?.changes.into_iter().find(|c| c.n == r.n).ok_or_else(|| OttoError::not_found(format!("{} has no change {}", state.label(), r.n)))?;
    let v = match version {
        Some(k) => change.version(k).cloned().ok_or_else(|| OttoError::not_found(format!("change {} has no version {k}", r.display())))?,
        None => change.latest().clone(),
    };
    let patch = v.patch.as_ref().and_then(|p| std::fs::read_to_string(dir.join(p)).ok());
    let summary = std::fs::read_to_string(dir.join(&v.summary)).unwrap_or_default();
    let repo = home_relative(&change.repo);
    let mut commands = vec![format!("git -C {repo} diff {}...{}", short_sha(&v.base_sha), short_sha(&v.sha))];
    if !change.state.is_over() {
        commands.push(format!("git -C {repo} push origin {}:{}", short_sha(&v.sha), change.base));
    }
    let mut line = line(&state, &short, &change);
    if let Ok(all) = list(true) {
        if let Some(l) = all.changes.into_iter().find(|l| l.run == state.id && l.n == r.n) {
            line.overlaps = l.overlaps;
        }
    }
    Ok(Detail { line, change, version: v, patch, summary, lander: lander_ref(), commands })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repository with a remote, `main` pushed, and a worktree on branch `fix` one commit ahead.
    struct Repo {
        _tmp: tempfile::TempDir,
        main: PathBuf,
        origin: PathBuf,
        wt: PathBuf,
    }

    fn repo() -> Repo {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        let main = tmp.path().join("app");
        let wt = tmp.path().join("wt");
        sh(tmp.path(), &["init", "--quiet", "--bare", "--initial-branch=main", origin.to_str().unwrap()]);
        sh(tmp.path(), &["clone", "--quiet", origin.to_str().unwrap(), main.to_str().unwrap()]);
        std::fs::write(main.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(main.join("b.txt"), "alpha\n").unwrap();
        sh(&main, &["add", "."]);
        sh(&main, &["commit", "--quiet", "-m", "start"]);
        sh(&main, &["push", "--quiet", "origin", "main"]);
        sh(&main, &["remote", "set-head", "origin", "main"]);
        sh(&main, &["worktree", "add", "--quiet", "-b", "fix", wt.to_str().unwrap()]);
        std::fs::write(wt.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        sh(&wt, &["commit", "--quiet", "-am", "shout two"]);
        let main = main.canonicalize().unwrap();
        let wt = wt.canonicalize().unwrap();
        Repo { _tmp: tmp, main, origin, wt }
    }

    fn submit_fix<'a>(r: &'a Repo, title: &'a str) -> Submit<'a> {
        Submit { repo: r.main.to_str().unwrap(), branch: "fix", title, summary: "Shouts the second line.", tests: Some("1 passed"), ..Default::default() }
    }

    fn change(run: &str, n: u32) -> Change {
        read_index(&crate::paths::run_dir(run).unwrap()).unwrap().changes.into_iter().find(|c| c.n == n).unwrap()
    }

    fn r(run: &str, n: u32) -> ChangeRef {
        ChangeRef { run: run.into(), n }
    }

    fn set_lander(id: &str) {
        crate::config::save_lander(Some(id)).unwrap();
    }

    #[test]
    fn a_submit_keeps_the_patch_and_the_same_commit_again_is_nothing_new() {
        let _h = TempHome::new();
        test_init("c-sub", "a goal").unwrap();
        let repo = repo();
        let out = submit("c-sub", &submit_fix(&repo, "Shout the second line")).unwrap();
        assert_eq!((out.n, out.version, out.unchanged, out.files, out.lines), (1, 1, false, 1, 2));
        assert_eq!(out.worktree.as_deref(), Some(repo.wt.to_str().unwrap()), "its worktree is found from its branch");
        let c = change("c-sub", 1);
        assert_eq!((c.state, c.whose_move(), c.base.as_str(), c.key.as_str()), (ChangeState::Review, Move::You, "main", "fix"));
        let patch = std::fs::read_to_string(crate::paths::run_dir("c-sub").unwrap().join(c.latest().patch.as_ref().unwrap())).unwrap();
        assert!(patch.contains("-two\n+TWO"), "{patch}");

        let again = submit("c-sub", &submit_fix(&repo, "Shout the second line")).unwrap();
        assert!(again.unchanged && again.version == 1, "the same commit is not a new version");
        std::fs::write(repo.wt.join("b.txt"), "ALPHA\n").unwrap();
        sh(&repo.wt, &["commit", "--quiet", "-am", "and alpha"]);
        let v2 = submit("c-sub", &submit_fix(&repo, "Shout two lines")).unwrap();
        assert_eq!((v2.n, v2.version, v2.files), (1, 2, 2), "the same branch is the next version of the same change");
        let j = std::fs::read_to_string(crate::paths::run_dir("c-sub").unwrap().join("journal.jsonl")).unwrap();
        assert_eq!(j.matches("\"change-submitted\"").count(), 2, "{j}");
    }

    #[test]
    fn what_a_person_could_not_review_is_refused() {
        let _h = TempHome::new();
        test_init("c-bad", "a goal").unwrap();
        let repo = repo();
        std::fs::write(repo.wt.join("a.txt"), "uncommitted\n").unwrap();
        let err = submit("c-bad", &submit_fix(&repo, "t")).unwrap_err();
        assert!(err.message.contains("nobody committed") && err.message.contains("a.txt"), "{}", err.message);
        sh(&repo.wt, &["checkout", "--quiet", "--", "a.txt"]);

        let none = Submit { branch: "main", ..submit_fix(&repo, "t") };
        assert!(submit("c-bad", &none).unwrap_err().message.contains("base branch itself"));
        sh(&repo.main, &["branch", "--quiet", "empty"]);
        let empty = Submit { branch: "empty", ..submit_fix(&repo, "t") };
        assert!(submit("c-bad", &empty).unwrap_err().message.contains("no commits"), "a branch with nothing on it");
        let missing = Submit { branch: "nope", ..submit_fix(&repo, "t") };
        assert!(submit("c-bad", &missing).unwrap_err().message.contains("no such branch"));
        assert!(submit("c-bad", &Submit { summary: "  ", ..submit_fix(&repo, "t") }).is_err(), "a summary is required");
        assert!(submit("c-bad", &Submit { title: "", ..submit_fix(&repo, "t") }).is_err());
        assert!(read_index(&crate::paths::run_dir("c-bad").unwrap()).unwrap().changes.is_empty());
    }

    #[test]
    fn approving_needs_a_lander_and_binds_to_the_version() {
        let _h = TempHome::new();
        test_init("c-app", "a goal").unwrap();
        test_init("c-lander", "land changes").unwrap();
        let repo = repo();
        submit("c-app", &submit_fix(&repo, "t")).unwrap();
        let err = approve(&[r("c-app", 1)], LandMode::Push, None).unwrap_err();
        assert!(err.message.contains("no lander"), "{}", err.message);
        set_lander("c-lander");
        let out = approve(&[r("c-app", 1)], LandMode::Push, Some("ship it")).unwrap();
        assert_eq!(out[0].state, ChangeState::Approved);
        let c = change("c-app", 1);
        let d = c.decision.as_ref().unwrap();
        assert_eq!((d.version, d.sha.as_str()), (1, c.latest().sha.as_str()));
        assert_eq!(c.whose_move(), Move::Lander);
        assert!(approve(&[r("c-app", 1)], LandMode::Push, None).is_err(), "only a change waiting for review is approved");
        assert_eq!(due().unwrap().len(), 1);
    }

    #[test]
    fn only_the_lander_lands_and_it_lands_what_was_approved() {
        let _h = TempHome::new();
        test_init("c-land", "a goal").unwrap();
        test_init("c-l", "land changes").unwrap();
        test_init("c-other", "not the lander").unwrap();
        let repo = repo();
        submit("c-land", &submit_fix(&repo, "t")).unwrap();
        set_lander("c-l");
        approve(&[r("c-land", 1)], LandMode::Push, None).unwrap();
        assert!(claim("c-other", &r("c-land", 1)).unwrap_err().message.contains("isn't the lander"));
        let d = claim("c-l", &r("c-land", 1)).unwrap();
        assert_eq!(d.todo, "land");
        let sha = d.sha.unwrap();
        assert!(check("c-l", &r("c-land", 1), &sha).unwrap().contains("as approved"));

        // main moves on elsewhere; a clean rebase keeps the patch, and passes.
        std::fs::write(repo.main.join("b.txt"), "alpha\nbeta\n").unwrap();
        sh(&repo.main, &["commit", "--quiet", "-am", "elsewhere"]);
        sh(&repo.main, &["push", "--quiet", "origin", "main"]);
        sh(&repo.wt, &["fetch", "--quiet", "origin"]);
        sh(&repo.wt, &["rebase", "--quiet", "origin/main"]);
        let rebased_sha = sh(&repo.wt, &["rev-parse", "HEAD"]);
        assert_ne!(rebased_sha, sha);
        assert!(check("c-l", &r("c-land", 1), &rebased_sha).unwrap().contains("clean rebase"));

        // A commit that changes the patch is not what was approved.
        std::fs::write(repo.wt.join("a.txt"), "one\nTWO\nTHREE\n").unwrap();
        sh(&repo.wt, &["commit", "--quiet", "-am", "more"]);
        let other = sh(&repo.wt, &["rev-parse", "HEAD"]);
        let err = check("c-l", &r("c-land", 1), &other).unwrap_err();
        assert_eq!(err.code, 2, "a conflict: exit 2");
        assert!(err.message.starts_with("DO NOT ACT"), "{}", err.message);

        // Not landed until the base has it.
        assert!(landed("c-l", &r("c-land", 1), &rebased_sha, None).unwrap_err().message.contains("doesn't contain"));
        sh(&repo.wt, &["push", "--quiet", "origin", &format!("{rebased_sha}:main")]);
        let c = landed("c-l", &r("c-land", 1), &rebased_sha, None).unwrap();
        assert_eq!((c.state, c.whose_move()), (ChangeState::Landed, Move::Lander), "landed, with cleaning up still to do");
        assert!(!c.landed.as_ref().unwrap().local_base, "the main checkout's main is behind");
        let state = crate::state::read_run("c-land").unwrap();
        let note = state.notes.last().expect("the author is told");
        let text = crate::notes::read_text(&crate::paths::run_dir("c-land").unwrap(), note);
        assert!(text.starts_with("About change 1@v1") && text.contains("landed on main"), "{text}");

        // Cleaning up is checked, not taken on the lander's word.
        assert!(cleaned("c-l", &r("c-land", 1), None).unwrap_err().message.contains("still a worktree"));
        sh(&repo.main, &["worktree", "remove", repo.wt.to_str().unwrap()]);
        assert!(cleaned("c-l", &r("c-land", 1), None).unwrap_err().message.contains("still there"));
        sh(&repo.main, &["branch", "--quiet", "-D", "fix"]);
        let c = cleaned("c-l", &r("c-land", 1), None).unwrap();
        assert_eq!(c.whose_move(), Move::Nobody);
        assert!(due().unwrap().is_empty());
        let _ = &repo.origin;
    }

    #[test]
    fn revising_and_dropping_tell_the_author_and_a_new_version_comes_back_to_review() {
        let _h = TempHome::new();
        test_init("c-rev", "a goal").unwrap();
        test_init("c-lr", "land changes").unwrap();
        set_lander("c-lr");
        let repo = repo();
        submit("c-rev", &submit_fix(&repo, "t")).unwrap();
        approve(&[r("c-rev", 1)], LandMode::Push, None).unwrap();
        let out = revise(&r("c-rev", 1), "call it louder").unwrap();
        assert_eq!(out.state, ChangeState::Revising);
        let c = change("c-rev", 1);
        assert!(c.decision.is_none(), "an approval is withdrawn by asking for changes");
        let dir = crate::paths::run_dir("c-rev").unwrap();
        let state = crate::state::read_run("c-rev").unwrap();
        assert!(crate::notes::read_text(&dir, state.notes.last().unwrap()).contains("call it louder"));

        std::fs::write(repo.wt.join("a.txt"), "one\nTWO!\nthree\n").unwrap();
        sh(&repo.wt, &["commit", "--quiet", "-am", "louder"]);
        let v2 = submit("c-rev", &submit_fix(&repo, "t")).unwrap();
        assert_eq!((v2.version, v2.was), (2, Some(ChangeState::Revising)));
        assert_eq!(change("c-rev", 1).whose_move(), Move::You);

        let dropped = drop_change(&r("c-rev", 1), Some("not needed")).unwrap();
        assert_eq!(dropped.state, ChangeState::Dropped);
        let c = change("c-rev", 1);
        assert!(c.cleaned.is_none(), "its worktree and branch are still there, so the lander has cleaning up to do");
        assert_eq!(due().unwrap()[0].todo, "clean up");
        assert!(submit("c-rev", &submit_fix(&repo, "t")).unwrap_err().message.contains("new change"), "a dropped change takes no versions");
    }

    #[test]
    fn a_lander_that_cannot_land_sends_it_back_or_shows_the_person_again() {
        let _h = TempHome::new();
        test_init("c-b", "a goal").unwrap();
        test_init("c-bl", "land changes").unwrap();
        set_lander("c-bl");
        let repo = repo();
        submit("c-b", &submit_fix(&repo, "t")).unwrap();
        approve(&[r("c-b", 1)], LandMode::Push, None).unwrap();
        claim("c-bl", &r("c-b", 1)).unwrap();
        let c = bounce("c-bl", &r("c-b", 1), "a.txt conflicts with main").unwrap();
        assert_eq!((c.state, c.whose_move()), (ChangeState::Revising, Move::Author));

        assert!(submit("c-b", &submit_fix(&repo, "t")).unwrap().unchanged, "the same commit again is still nothing new");
        std::fs::write(repo.wt.join("a.txt"), "one\nTwo\nthree\n").unwrap();
        sh(&repo.wt, &["commit", "--quiet", "-am", "again"]);
        submit("c-b", &submit_fix(&repo, "t")).unwrap();
        approve(&[r("c-b", 1)], LandMode::Push, None).unwrap();
        claim("c-bl", &r("c-b", 1)).unwrap();
        let head = sh(&repo.wt, &["rev-parse", "HEAD"]);
        let c = rebased("c-bl", &r("c-b", 1), &head).unwrap();
        assert_eq!((c.state, c.latest().k, c.latest().by.as_str()), (ChangeState::Review, 3, "lander"));
    }

    #[test]
    fn the_list_puts_your_move_first_and_says_what_overlaps() {
        let _h = TempHome::new();
        test_init("c-l1", "a goal").unwrap();
        test_init("c-l2", "another goal").unwrap();
        let repo = repo();
        submit("c-l1", &submit_fix(&repo, "first")).unwrap();
        sh(&repo.main, &["worktree", "add", "--quiet", "-b", "fix2", repo.main.parent().unwrap().join("wt2").to_str().unwrap()]);
        let wt2 = repo.main.parent().unwrap().join("wt2");
        std::fs::write(wt2.join("a.txt"), "ONE\ntwo\nthree\n").unwrap();
        sh(&wt2, &["commit", "--quiet", "-am", "shout one"]);
        submit("c-l2", &Submit { branch: "fix2", ..submit_fix(&repo, "second") }).unwrap();
        let view = list(false).unwrap();
        assert_eq!(view.yours, 2);
        assert_eq!(view.changes.len(), 2);
        assert!(view.changes.iter().all(|l| l.overlaps.len() == 1), "both touch a.txt: {:?}", view.changes.iter().map(|l| &l.overlaps).collect::<Vec<_>>());
        assert_eq!(yours(), 2);
        let detail = read(&ChangeRef::parse("c-l1/1").unwrap(), None).unwrap();
        assert!(detail.patch.unwrap().contains("+TWO") && detail.summary.contains("Shouts"));
    }
}
