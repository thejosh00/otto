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
    /// Its pull request is open, waiting on reviewers and CI.
    PrOpen,
    /// Its pull request is approved and passing, and merging it is the person's to say.
    ReadyToMerge,
    /// Its author asked the person something about it, and waits for the answer.
    NeedsYou,
    /// Its pull request was merged.
    Merged,
    /// Its pull request was closed without merging.
    Closed,
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
            ChangeState::PrOpen => "pr-open",
            ChangeState::ReadyToMerge => "ready-to-merge",
            ChangeState::NeedsYou => "needs-you",
            ChangeState::Merged => "merged",
            ChangeState::Closed => "closed",
        }
    }

    /// Landed, merged, dropped or closed: nothing more happens to the change itself, only
    /// cleaning up after it.
    pub fn is_over(self) -> bool {
        matches!(self, ChangeState::Landed | ChangeState::Dropped | ChangeState::Merged | ChangeState::Closed)
    }
}

/// Whose move it is. The person's is the only list they have to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Move {
    You,
    Author,
    Lander,
    /// Its pull request waits on someone reviewing it.
    Reviewers,
    /// Its pull request waits on CI.
    Ci,
    Nobody,
}

/// How an approved change lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum LandMode {
    /// Onto its base branch, and pushed
    Push,
    /// As a pull request, which the lander keeps moving: reviewers' asks go to its author
    Pr,
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
    /// Its pull request, once the lander has opened one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<Pr>,
    /// Replies to reviewers its author drafted; posting one is the person's to say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,
    /// What its author asked the person about it, and their answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
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
    /// The version that lands. As a pull request, each new version its author submits is pushed
    /// to it without asking again: the pull request is where it is reviewed.
    pub version: u32,
    pub sha: String,
    pub patch_id: String,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// As a pull request: merge it once it is approved and passing, without asking again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merge_when_ready: bool,
    /// The person said to merge its pull request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_asked: Option<Timestamp>,
    /// The process its repository followed when it was approved (`crate::config::Process`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    /// That process's words on how a change lands, as they were then: changing the process later
    /// doesn't change how an approved change lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land: Option<String>,
    /// The person chose how this one lands, rather than its process.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overridden: bool,
}

/// How the person said a change lands, instead of as its process says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct How {
    pub mode: LandMode,
    pub merge_when_ready: bool,
}

/// A change's pull request, as the lander last saw it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pr {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    pub opened_at: Timestamp,
    /// The commit its branch has, as the lander last pushed it.
    pub head: String,
    /// GitHub merges it itself once it is approved and passing (`gh pr merge --auto`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_merge: bool,
    /// `pending`, `passing` or `failing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<String>,
    /// `approved`, `changes-requested`, `waiting`, or `none` when nobody has to review it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    /// `clean`, `behind` or `conflicting`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mergeable: Option<String>,
    /// The pull request's `updatedAt` when the lander last looked: anything newer is news.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReplyState {
    /// Drafted by its author; waits on the person.
    Draft,
    /// The person said to post it; waits on the lander.
    Post,
    Posted,
}

/// A reply to a reviewer's comment on a change's pull request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    /// The comment it answers: its id, or its link.
    pub comment: String,
    pub text: String,
    pub at: Timestamp,
    pub state: ReplyState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posted_at: Option<Timestamp>,
}

/// What a change's author asked the person, without stopping its run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<Timestamp>,
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

    /// Whose move it is. Over and cleaned up, no one's; over and not yet, the lander's. A reply
    /// drafted for a reviewer makes an open pull request the person's move too.
    pub fn whose_move(&self) -> Move {
        if self.state.is_over() {
            return if self.cleaned.is_none() { Move::Lander } else { Move::Nobody };
        }
        let replies = |s: ReplyState| self.replies.iter().any(|r| r.state == s);
        if replies(ReplyState::Draft) {
            return Move::You;
        }
        match self.state {
            ChangeState::Review | ChangeState::NeedsYou => Move::You,
            ChangeState::Revising => Move::Author,
            ChangeState::Approved | ChangeState::Landing => Move::Lander,
            _ if replies(ReplyState::Post) || self.decision.as_ref().is_some_and(|d| d.merge_asked.is_some()) => Move::Lander,
            ChangeState::ReadyToMerge => Move::You,
            _ => match self.pr.as_ref().and_then(|p| p.ci.as_deref()) {
                Some("pending") => Move::Ci,
                Some("failing") => Move::Lander,
                _ => Move::Reviewers,
            },
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
pub fn main_repo(repo: &Path) -> Result<PathBuf, OttoError> {
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
                ChangeState::NeedsYou => {
                    return Err(OttoError::conflict(format!(
                        "change {} ({key}) waits on the person's answer to what you asked about it; it reaches you as a note. \
                         Submit the next version once you have it",
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
                let what = format!("v{k}: {} commit{}, +{} −{}", snap.commits, if snap.commits == 1 { "" } else { "s" }, snap.stat.add, snap.stat.del);
                // With a pull request open, the pull request is where it is reviewed: the next
                // version goes to it. Otherwise an approval was for the version it named, and this
                // one is new.
                match (c.pr.is_some(), c.decision.as_mut()) {
                    (true, Some(d)) => {
                        d.version = k;
                        d.sha = sha.clone();
                        d.patch_id = snap.patch_id.clone();
                        c.enter(ChangeState::Approved);
                        c.moment("author", format!("{what} — for its pull request"));
                    }
                    _ => {
                        c.decision = None;
                        c.enter(ChangeState::Review);
                        c.moment("author", what);
                    }
                }
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
                    pr: None,
                    replies: Vec::new(),
                    ask: None,
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
        refuse_unless(c, &[ChangeState::Review, ChangeState::Revising, ChangeState::Approved, ChangeState::PrOpen, ChangeState::ReadyToMerge], "withdrawn")?;
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
        ChangeState::PrOpen => "its pull request is open".to_string(),
        ChangeState::ReadyToMerge => "its pull request is ready to merge".to_string(),
        ChangeState::NeedsYou => "its author asked the person something about it".to_string(),
        ChangeState::Merged => "its pull request was merged".to_string(),
        ChangeState::Closed => "its pull request was closed".to_string(),
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
    /// How it lands, for an approval: `to land on main`, `as a pull request onto main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
}

/// Approve the latest version of each change, to land as its repository's process says — or as
/// `how` says, when the person chose. Refused whole when there is no lander to land them, and each
/// one refused that isn't waiting for review.
pub fn approve(refs: &[ChangeRef], how: Option<How>, note: Option<&str>) -> Result<Vec<Decided>, OttoError> {
    if how.is_some_and(|h| h.merge_when_ready && h.mode != LandMode::Pr) {
        return Err(OttoError::usage("merging when ready is for a pull request: approve it with --pr"));
    }
    let config = crate::config::load()?;
    if config.lander.is_none() {
        return Err(OttoError::usage(
            "no lander is set up, so nothing would land an approved change — otto makes one on its next pass (`otto poke`), and \
             you start it with `otto resume lander`; or land it yourself: `otto changes show <change>` has the commands",
        ));
    }
    let note = check_say(note, "--note")?;
    let mut out = Vec::new();
    for r in refs {
        out.push(with_change(r, |path, _state, c| {
            refuse_unless(c, &[ChangeState::Review], "approved")?;
            let process = config.process_for(&c.repo);
            let chosen = how.unwrap_or(How { mode: process.mode, merge_when_ready: process.merge_when_ready });
            let overridden = chosen.mode != process.mode || chosen.merge_when_ready != process.merge_when_ready;
            let v = c.latest().clone();
            c.decision = Some(Decision {
                mode: chosen.mode,
                version: v.k,
                sha: v.sha.clone(),
                patch_id: v.patch_id.clone(),
                at: Timestamp::now(),
                note: note.clone(),
                merge_when_ready: chosen.merge_when_ready,
                merge_asked: None,
                process: Some(process.name.clone()),
                land: process.land.clone(),
                overridden,
            });
            c.enter(ChangeState::Approved);
            let how = match (chosen.mode, chosen.merge_when_ready) {
                (LandMode::Push, _) => format!("to land on {}", c.base),
                (LandMode::Pr, false) => format!("as a pull request onto {}", c.base),
                (LandMode::Pr, true) => format!("as a pull request onto {}, merged once it is approved and passing", c.base),
            };
            let by = if overridden { format!(" rather than as {} says", process.name) } else { format!(", as {} says", process.name) };
            let what = format!("approved v{} {how}{by}{}", v.k, note.as_ref().map(|n| format!(": {n}")).unwrap_or_default());
            c.moment("you", &what);
            moved(path, c, "person", &what)?;
            Ok(Decided { reference: r.display(), title: c.title.clone(), state: c.state, note: None, how: Some(how) })
        })?);
    }
    Ok(out)
}

/// Ask its author for changes. The author is told, in the person's words.
pub fn revise(r: &ChangeRef, text: &str) -> Result<Decided, OttoError> {
    let text = check_say(Some(text), "what to change")?.ok_or_else(|| OttoError::usage("say what to change"))?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::Review, ChangeState::Approved, ChangeState::PrOpen, ChangeState::ReadyToMerge], "sent back")?;
        // With its pull request open the approval stands: the next version goes to the pull request.
        if c.pr.is_none() {
            c.decision = None;
        }
        c.enter(ChangeState::Revising);
        c.moment("you", format!("asked for changes: {text}"));
        let note = tell_author(path, state, c, &format!(
            "The person asked for changes:\n\n{text}\n\nWork in the same worktree, commit, and submit it again with the same key — \
             `otto state change submit` — so it comes back to them as the next version."
        ))?;
        moved(path, c, "person", &format!("asked for changes: {text}"))?;
        Ok(Decided { reference: r.display(), title: c.title.clone(), state: c.state, note, how: None })
    })
}

/// Not wanted. The author is told; the lander cleans up its worktree and branch.
pub fn drop_change(r: &ChangeRef, text: Option<&str>) -> Result<Decided, OttoError> {
    let text = check_say(text, "why")?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::Review, ChangeState::Revising, ChangeState::Approved, ChangeState::NeedsYou, ChangeState::PrOpen, ChangeState::ReadyToMerge], "dropped")?;
        c.decision = None;
        c.enter(ChangeState::Dropped);
        let what = match &text {
            Some(t) => format!("dropped it: {t}"),
            None => "dropped it".to_string(),
        };
        c.moment("you", &what);
        settle_if_nothing_to_clean(c);
        let body = match &text {
            Some(t) => format!("The person dropped it, saying:\n\n{t}\n\nIt won't land. The lander removes its worktree and branch{}.", if c.pr.is_some() { ", and closes its pull request" } else { "" }),
            None => format!("The person dropped it. It won't land. The lander removes its worktree and branch{}.", if c.pr.is_some() { ", and closes its pull request" } else { "" }),
        };
        let note = tell_author(path, state, c, &body)?;
        moved(path, c, "person", &what)?;
        Ok(Decided { reference: r.display(), title: c.title.clone(), state: c.state, note, how: None })
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
    /// As a pull request: merge it once it is approved and passing (`gh pr merge --auto`).
    pub merge_when_ready: bool,
    /// The process it was approved under, and its words on how a change lands there: the lander
    /// follows them before anything the repository's docs say.
    pub process: Option<String>,
    pub land: Option<String>,
    pub approved_at: Option<Timestamp>,
    /// What its author said about it, absolute: the commit message and PR body come from here.
    pub summary: String,
    pub landed_sha: Option<String>,
    /// Its pull request, when it has one.
    pub pr: Option<Pr>,
    /// Replies the person said to post, for `post replies`.
    pub replies: Vec<Reply>,
    /// What `gh` said when asked about its pull request, for `look at its pull request`.
    pub news: Option<String>,
}

fn due_of(run: &str, name: &str, dir: &Path, c: &Change) -> Option<Due> {
    let d = c.decision.as_ref();
    let pr_mode = d.is_some_and(|d| d.mode == LandMode::Pr);
    let todo = match c.state {
        ChangeState::Approved if c.pr.is_some() => "update its pull request",
        ChangeState::Approved if pr_mode => "open its pull request",
        ChangeState::Approved => "land",
        ChangeState::Landing => "finish landing",
        _ if c.state.is_over() && c.cleaned.is_none() => "clean up",
        _ if c.state.is_over() => return None,
        _ if c.replies.iter().any(|r| r.state == ReplyState::Post) => "post replies",
        ChangeState::PrOpen | ChangeState::ReadyToMerge if d.is_some_and(|d| d.merge_asked.is_some()) => "merge its pull request",
        _ => return None,
    };
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
        merge_when_ready: d.is_some_and(|d| d.merge_when_ready),
        process: d.and_then(|d| d.process.clone()),
        land: d.and_then(|d| d.land.clone()),
        approved_at: d.map(|d| d.at),
        summary: dir.join(&c.latest().summary).display().to_string(),
        landed_sha: c.landed.as_ref().map(|l| l.sha.clone()),
        pr: c.pr.clone(),
        replies: c.replies.iter().filter(|r| r.state == ReplyState::Post).cloned().collect(),
        news: None,
    })
}

/// A pull request that has moved since the lander last looked, as `gh` says: its `updatedAt` is not
/// the one on record. `gh` failing is news too, so a lander that can't see is woken to say so.
fn pr_news(c: &Change) -> Option<String> {
    let pr = c.pr.as_ref()?;
    if c.state.is_over() {
        return None;
    }
    let out = std::process::Command::new("gh")
        .args(["pr", "view", &pr.url, "--json", "updatedAt", "--jq", ".updatedAt"])
        .current_dir(&c.repo)
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(out) if out.status.success() => {
            let updated = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (pr.seen.as_deref() != Some(updated.as_str())).then(|| format!("updated {updated}"))
        }
        Ok(out) => Some(format!("gh couldn't say: {}", String::from_utf8_lossy(&out.stderr).trim())),
        Err(e) => Some(format!("gh couldn't run: {e}")),
    }
}

/// `due`, and every open pull request that has news — what the lander's check script asks, since
/// otto can't see a pull request move without asking `gh`.
pub fn due_with_prs() -> Result<Vec<Due>, OttoError> {
    let mut out = due()?;
    for entry in crate::state::read_all_runs()? {
        let crate::state::RunEntry::Readable(state) = entry else { continue };
        let Ok(dir) = crate::paths::run_dir(&state.id) else { continue };
        let name = state.display_name();
        for c in index_or_empty(&dir).changes {
            if out.iter().any(|d| d.run == state.id && d.n == c.n) {
                continue;
            }
            let Some(news) = pr_news(&c) else { continue };
            out.push(Due {
                reference: format!("{}/{}", crate::paths::short_id(&state.id), c.n),
                run: state.id.clone(),
                run_name: name.clone(),
                n: c.n,
                title: c.title.clone(),
                state: c.state,
                todo: "look at its pull request".to_string(),
                repo: c.repo.clone(),
                base: c.base.clone(),
                branch: c.branch.clone(),
                worktree: c.worktree.clone(),
                sha: c.decision.as_ref().map(|d| d.sha.clone()),
                patch_id: c.decision.as_ref().map(|d| d.patch_id.clone()),
                mode: c.decision.as_ref().map(|d| d.mode),
                merge_when_ready: c.decision.as_ref().is_some_and(|d| d.merge_when_ready),
                process: c.decision.as_ref().and_then(|d| d.process.clone()),
                land: c.decision.as_ref().and_then(|d| d.land.clone()),
                approved_at: c.decision.as_ref().map(|d| d.at),
                summary: dir.join(&c.latest().summary).display().to_string(),
                landed_sha: None,
                pr: c.pr.clone(),
                replies: Vec::new(),
                news: Some(news),
            });
        }
    }
    Ok(out)
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
        "land" | "open its pull request" | "update its pull request" => 1,
        "merge its pull request" | "post replies" => 2,
        _ => 3,
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
    let d = c.decision.as_ref().ok_or_else(|| OttoError::conflict(format!("DO NOT ACT: change {} has no approval on record", r.display())))?;
    let repo = Path::new(&c.repo);
    let sha = rev(repo, head)?;
    // Merging its pull request: what merges must be what the lander pushed to it, not a commit
    // someone else added since.
    if matches!(c.state, ChangeState::PrOpen | ChangeState::ReadyToMerge) && d.merge_asked.is_some() {
        let pr = c.pr.as_ref().ok_or_else(|| OttoError::conflict(format!("DO NOT ACT: change {} has no pull request on record", r.display())))?;
        if sha == pr.head || patch_id_against(repo, &base_ref(repo, &c.base), &sha).is_ok_and(|id| id == d.patch_id) {
            return Ok(format!("ok: {} is what was pushed to its pull request — merge it", short_sha(&sha)));
        }
        return Err(OttoError::conflict(format!(
            "DO NOT ACT: its pull request's head {} isn't what the lander pushed ({}) — someone else pushed to it. Look at what \
             they added; it goes back to its author with `otto state change relay`, not into main",
            short_sha(&sha),
            short_sha(&pr.head)
        )));
    }
    if c.state != ChangeState::Landing {
        return Err(OttoError::conflict(format!("DO NOT ACT: change {} is {}, not being landed — claim it first", r.display(), c.state.as_str())));
    }
    if sha == d.sha {
        return Ok(format!("ok: {} is v{}, as approved", short_sha(&sha), d.version));
    }
    // A commit with no patch of its own against the base — the base itself, say — matches nothing.
    if patch_id_against(repo, &base_ref(repo, &c.base), &sha).is_ok_and(|id| id == d.patch_id) {
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
        // On its base branch: the remote's, or — for a repo that ships from its main checkout, as one
        // with a ship script of its own may — the local one.
        let base = base_ref(repo, &c.base);
        let local_base = ok(repo, &["merge-base", "--is-ancestor", &sha, &format!("refs/heads/{}", c.base)]);
        if !local_base && !ok(repo, &["merge-base", "--is-ancestor", &sha, &base]) {
            return Err(OttoError::conflict(format!(
                "{base} doesn't contain {} — it hasn't landed yet, or this checkout hasn't heard (`git -C {} fetch`)",
                short_sha(&sha),
                c.repo
            )));
        }
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

// ---------------------------------------------------------------------------
// Pull requests
// ---------------------------------------------------------------------------

fn pr_number(url: &str) -> Option<u32> {
    url.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

/// The lander pushed it to its pull request: opened it (`url`), or pushed the next version to it.
/// Its author hears the first time, so it knows reviewers' asks will come.
pub fn pr_pushed(lander: &str, r: &ChangeRef, head: &str, url: Option<&str>, auto_merge: bool) -> Result<Change, OttoError> {
    let lander = require_lander(lander)?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::Landing], "pushed to a pull request")?;
        if c.decision.as_ref().is_none_or(|d| d.mode != LandMode::Pr) {
            return Err(OttoError::conflict(format!("change {} was approved to land on {}, not as a pull request", r.display(), c.base)));
        }
        let head = rev(Path::new(&c.repo), head)?;
        let k = c.latest().k;
        let what = match (&mut c.pr, url) {
            (Some(pr), _) => {
                pr.head = head.clone();
                pr.ci = Some("pending".into());
                format!("pushed v{k} to its pull request ({})", short_sha(&head))
            }
            (None, Some(url)) => {
                let url = url.trim().to_string();
                if !url.starts_with("http") {
                    return Err(OttoError::usage(format!("--url {url:?}: the pull request's address")));
                }
                c.pr = Some(Pr {
                    number: pr_number(&url),
                    url: url.clone(),
                    opened_at: Timestamp::now(),
                    head: head.clone(),
                    auto_merge,
                    ci: Some("pending".into()),
                    review: None,
                    mergeable: None,
                    seen: None,
                    seen_at: None,
                });
                format!("opened its pull request: {url}{}", if auto_merge { ", to merge once approved and passing" } else { "" })
            }
            (None, None) => return Err(OttoError::usage("it has no pull request yet: say where it is, with --url")),
        };
        c.enter(ChangeState::PrOpen);
        c.moment("lander", &what);
        if url.is_some() {
            let body = format!(
                "Its pull request is open: {}. What reviewers ask, and CI failing, comes to you as notes like this one: \
                 change the code in the same worktree and submit it again with the same key, and the lander pushes it to \
                 the pull request. A reply to a reviewer is `otto state change reply`; the person posts it.",
                c.pr.as_ref().map(|p| p.url.as_str()).unwrap_or("")
            );
            tell_author(path, state, c, &body)?;
        }
        moved(path, c, &lander, &what)?;
        Ok(c.clone())
    })
}

/// What the lander saw on its pull request: CI, review, whether it merges cleanly, and GitHub's
/// `updatedAt`, so the same news isn't news twice. Approved and passing, with nothing to merge it
/// itself, it is the person's to merge.
pub struct Seen<'a> {
    pub updated: &'a str,
    pub ci: Option<&'a str>,
    pub review: Option<&'a str>,
    pub mergeable: Option<&'a str>,
    pub what: Option<&'a str>,
}

pub fn pr_seen(lander: &str, r: &ChangeRef, seen: &Seen) -> Result<Change, OttoError> {
    let lander = require_lander(lander)?;
    let what = check_say(seen.what, "--what")?;
    let one_of = |value: Option<&str>, allowed: &[&str], flag: &str| -> Result<Option<String>, OttoError> {
        match value.map(str::trim) {
            None => Ok(None),
            Some(v) if allowed.contains(&v) => Ok(Some(v.to_string())),
            Some(v) => Err(OttoError::usage(format!("{flag} {v:?}: one of {}", allowed.join(", ")))),
        }
    };
    let ci = one_of(seen.ci, &["pending", "passing", "failing"], "--ci")?;
    let review = one_of(seen.review, &["approved", "changes-requested", "waiting", "none"], "--review")?;
    let mergeable = one_of(seen.mergeable, &["clean", "behind", "conflicting"], "--mergeable")?;
    with_change(r, |path, _state, c| {
        if c.state.is_over() || c.pr.is_none() {
            return Err(OttoError::conflict(format!("change {} has no open pull request", r.display())));
        }
        let pr = c.pr.as_mut().expect("checked");
        pr.seen = Some(seen.updated.trim().to_string());
        pr.seen_at = Some(Timestamp::now());
        if ci.is_some() {
            pr.ci = ci.clone();
        }
        if review.is_some() {
            pr.review = review.clone();
        }
        if mergeable.is_some() {
            pr.mergeable = mergeable.clone();
        }
        let ready = pr.ci.as_deref() == Some("passing")
            && matches!(pr.review.as_deref(), Some("approved" | "none"))
            && pr.mergeable.as_deref() != Some("conflicting")
            && !pr.auto_merge;
        match c.state {
            ChangeState::PrOpen if ready => {
                c.enter(ChangeState::ReadyToMerge);
                c.moment("reviewer", "approved and passing — ready for you to merge");
            }
            ChangeState::ReadyToMerge if !ready => c.enter(ChangeState::PrOpen),
            _ => {}
        }
        if let Some(what) = &what {
            c.moment("reviewer", what);
            moved(path, c, &lander, what)?;
        }
        Ok(c.clone())
    })
}

/// Hand what a reviewer, or CI, asked of its pull request to the change's author, in their words.
/// The author changes the code and submits again; the lander pushes that to the pull request.
pub fn relay(lander: &str, r: &ChangeRef, who: &str, text: &str) -> Result<Change, OttoError> {
    let lander = require_lander(lander)?;
    let who = who.trim();
    let text = check_say(Some(text), "--text")?.ok_or_else(|| OttoError::usage("--text: what they asked"))?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::PrOpen, ChangeState::ReadyToMerge, ChangeState::Revising], "sent back")?;
        if c.pr.is_none() {
            return Err(OttoError::conflict(format!("change {} has no pull request: send it back with `bounce`", r.display())));
        }
        c.enter(ChangeState::Revising);
        c.moment("reviewer", format!("{who}: {}", first_line(&text)));
        let url = c.pr.as_ref().map(|p| p.url.clone()).unwrap_or_default();
        let body = format!(
            "On its pull request ({url}), {who} said:\n\n{text}\n\nIf it asks for a change, make it in the same worktree, commit, and \
             submit again with the same key — the lander pushes it to the pull request. If it asks a question, draft the answer \
             with `otto state change reply`. If it needs the person's judgement, `otto state change ask`."
        );
        tell_author(path, state, c, &body)?;
        wake_soon(state);
        moved(path, c, &lander, &format!("{who}: {}", first_line(&text)))?;
        Ok(c.clone())
    })
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > 160 { format!("{}…", line.chars().take(159).collect::<String>()) } else { line.to_string() }
}

/// Its pull request was merged: `sha` is on the base branch, which otto checks as for `landed`.
/// The author is told, and woken to do what follows.
pub fn merged(lander: &str, r: &ChangeRef, sha: &str) -> Result<Change, OttoError> {
    let lander = require_lander(lander)?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::PrOpen, ChangeState::ReadyToMerge, ChangeState::Revising], "marked merged")?;
        let repo = Path::new(&c.repo);
        let sha = rev(repo, sha)?;
        let base = base_ref(repo, &c.base);
        if !ok(repo, &["merge-base", "--is-ancestor", &sha, &base]) {
            return Err(OttoError::conflict(format!("{base} doesn't contain {} — fetch first (`git -C {} fetch`)", short_sha(&sha), c.repo)));
        }
        let local_base = ok(repo, &["merge-base", "--is-ancestor", &sha, &format!("refs/heads/{}", c.base)]);
        c.landed = Some(Landed { sha: sha.clone(), at: Timestamp::now(), local_base, note: None });
        c.replies.retain(|r| r.state == ReplyState::Posted);
        c.enter(ChangeState::Merged);
        let what = format!("its pull request was merged into {} at {}", c.base, short_sha(&sha));
        c.moment("lander", &what);
        let checkout = if local_base { String::new() } else { format!(" The main checkout's own {} doesn't have it yet.", c.base) };
        let body = format!("{what}.{checkout}\n\nDo whatever comes after it landing — what your instructions say follows — and nothing more to the change itself.");
        tell_author(path, state, c, &body)?;
        wake_soon(state);
        moved(path, c, &lander, &what)?;
        Ok(c.clone())
    })
}

/// Its pull request was closed without merging — by a reviewer, or by the lander for a change the
/// person dropped.
pub fn closed(lander: &str, r: &ChangeRef, reason: &str) -> Result<Change, OttoError> {
    let lander = require_lander(lander)?;
    let reason = check_say(Some(reason), "--reason")?.ok_or_else(|| OttoError::usage("say why, with --reason"))?;
    with_change(r, |path, state, c| {
        if c.pr.is_none() || c.state.is_over() && c.state != ChangeState::Dropped {
            return Err(OttoError::conflict(format!("change {} has no open pull request", r.display())));
        }
        let was_dropped = c.state == ChangeState::Dropped;
        if !was_dropped {
            c.enter(ChangeState::Closed);
        }
        c.replies.retain(|r| r.state == ReplyState::Posted);
        let what = format!("its pull request was closed: {reason}");
        c.moment("lander", &what);
        if !was_dropped {
            tell_author(path, state, c, &format!("{what}\n\nIt won't land. The lander removes its worktree and branch."))?;
        }
        moved(path, c, &lander, &what)?;
        Ok(c.clone())
    })
}

/// The lander posted a reply the person said to post.
pub fn posted(lander: &str, r: &ChangeRef, comment: &str) -> Result<Change, OttoError> {
    let lander = require_lander(lander)?;
    with_change(r, |path, _state, c| {
        let reply = c
            .replies
            .iter_mut()
            .find(|x| x.comment == comment.trim() && x.state == ReplyState::Post)
            .ok_or_else(|| OttoError::conflict(format!("change {} has no reply to post to {comment}", r.display())))?;
        reply.state = ReplyState::Posted;
        reply.posted_at = Some(Timestamp::now());
        let what = format!("posted the reply to {}", comment.trim());
        c.moment("lander", &what);
        moved(path, c, &lander, &what)?;
        Ok(c.clone())
    })
}

/// The person says to merge its pull request, now that it is approved and passing.
pub fn merge(r: &ChangeRef) -> Result<Decided, OttoError> {
    with_change(r, |path, _state, c| {
        refuse_unless(c, &[ChangeState::ReadyToMerge, ChangeState::PrOpen], "merged")?;
        let d = c.decision.as_mut().ok_or_else(|| OttoError::conflict(format!("change {} has no approval on record", r.display())))?;
        d.merge_asked = Some(Timestamp::now());
        c.moment("you", "said to merge its pull request");
        moved(path, c, "person", "said to merge its pull request")?;
        Ok(Decided { reference: r.display(), title: c.title.clone(), state: c.state, note: None, how: None })
    })
}

/// The person says to post replies its author drafted: the one to `comment`, or all of them.
pub fn post(r: &ChangeRef, comment: Option<&str>) -> Result<Decided, OttoError> {
    with_change(r, |path, _state, c| {
        let mut n = 0;
        for reply in c.replies.iter_mut().filter(|x| x.state == ReplyState::Draft && comment.is_none_or(|w| x.comment == w.trim())) {
            reply.state = ReplyState::Post;
            n += 1;
        }
        if n == 0 {
            return Err(OttoError::conflict(format!("change {} has no drafted reply{}", r.display(), comment.map(|w| format!(" to {w}")).unwrap_or_default())));
        }
        let what = format!("said to post {n} repl{}", if n == 1 { "y" } else { "ies" });
        c.moment("you", &what);
        moved(path, c, "person", &what)?;
        Ok(Decided { reference: r.display(), title: c.title.clone(), state: c.state, note: None, how: None })
    })
}

/// The person answers what its author asked about it. The author hears it in their words.
pub fn answer(r: &ChangeRef, text: &str) -> Result<Decided, OttoError> {
    let text = check_say(Some(text), "the answer")?.ok_or_else(|| OttoError::usage("say the answer"))?;
    with_change(r, |path, state, c| {
        refuse_unless(c, &[ChangeState::NeedsYou], "answered")?;
        let ask = c.ask.as_mut().expect("needs-you has its question");
        let answer = match ask.options.iter().find(|o| o.eq_ignore_ascii_case(text.trim())) {
            Some(o) => o.clone(),
            None => text.clone(),
        };
        ask.answer = Some(answer.clone());
        ask.answered_at = Some(Timestamp::now());
        let question = crate::gate::title(&ask.question, 120);
        c.enter(ChangeState::Revising);
        c.moment("you", format!("answered: {}", first_line(&answer)));
        let note = tell_author(path, state, c, &format!("You asked: {question}\n\nThe person answered:\n\n{answer}"))?;
        moved(path, c, "person", &format!("answered: {}", first_line(&answer)))?;
        Ok(Decided { reference: r.display(), title: c.title.clone(), state: c.state, note, how: None })
    })
}

/// Its author drafts a reply to a reviewer's comment, for the person to post.
pub fn reply_draft(r: &ChangeRef, comment: &str, text: &str) -> Result<Change, OttoError> {
    let comment = comment.trim().to_string();
    if comment.is_empty() {
        return Err(OttoError::usage("--comment: the comment it answers, its id or its link"));
    }
    let text = check_say(Some(text), "the reply")?.ok_or_else(|| OttoError::usage("the reply is empty"))?;
    with_change(r, |path, _state, c| {
        if c.pr.is_none() || c.state.is_over() {
            return Err(OttoError::conflict(format!("change {} has no open pull request to reply on", r.display())));
        }
        c.replies.retain(|x| !(x.comment == comment && x.state == ReplyState::Draft));
        c.replies.push(Reply { comment: comment.clone(), text: text.clone(), at: Timestamp::now(), state: ReplyState::Draft, posted_at: None });
        // Answering what was relayed with words rather than code: once the reply is posted, it is
        // with its reviewers again, not its author.
        if c.state == ChangeState::Revising {
            c.enter(ChangeState::PrOpen);
        }
        let what = format!("drafted a reply to {comment}");
        c.moment("author", &what);
        moved(path, c, "author", &what)?;
        Ok(c.clone())
    })
}

/// Its author asks the person something about it, written like a gate's question — answerable
/// cold, options under `## Options` — without stopping its run.
pub fn ask(r: &ChangeRef, question: &str) -> Result<Change, OttoError> {
    let question = question.trim();
    if question.is_empty() || question.len() > crate::notes::NOTE_MAX_BYTES {
        return Err(OttoError::usage(format!("a question about a change is 1 to {} bytes", crate::notes::NOTE_MAX_BYTES)));
    }
    let options = crate::gate::parse_options(question);
    let default = crate::gate::parse_default(question, &options);
    with_change(r, |path, _state, c| {
        if c.state.is_over() || c.state == ChangeState::Landing {
            return Err(OttoError::conflict(format!("change {} is {}: there is nothing to decide about it", r.display(), c.state.as_str())));
        }
        c.ask = Some(Ask { question: question.to_string(), options: options.clone(), default: default.clone(), at: Timestamp::now(), answer: None, answered_at: None });
        c.enter(ChangeState::NeedsYou);
        let what = format!("asked: {}", crate::gate::title(question, 120));
        c.moment("author", &what);
        moved(path, c, "author", &what)?;
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
    /// Its pull request, when it has one.
    pub pr: Option<Pr>,
    /// What its author asked the person, when it waits on their answer.
    pub asking: Option<String>,
    /// Replies to reviewers drafted for the person to post.
    pub replies: usize,
    /// Why it looks stuck, when it does: its move has sat too long, or it keeps coming back.
    pub stuck: Option<String>,
    /// The process it follows: the one it was approved under, else its repository's now.
    pub process: String,
    /// How it lands, or would if approved now as its process says.
    pub mode: LandMode,
}

/// How long a change may sit with an agent, and with reviewers, before it reads as stuck.
pub const STUCK_AGENT_HOURS: i64 = 6;
pub const STUCK_REVIEW_DAYS: i64 = 3;
/// Times the lander or CI may send it back before that reads as stuck too.
pub const STUCK_BOUNCES: usize = 3;

/// Why `c` looks stuck at `now`, in a few words, or `None`.
pub fn stuck(c: &Change, now: time::OffsetDateTime) -> Option<String> {
    let waited = now - c.state_at.dt();
    let bounces = c.timeline.iter().filter(|m| m.by == "lander" && m.what.starts_with("couldn't land it")).count();
    match c.whose_move() {
        Move::Nobody | Move::You => None,
        _ if bounces >= STUCK_BOUNCES && !c.state.is_over() => Some(format!("sent back {bounces} times")),
        Move::Author | Move::Lander if waited > time::Duration::hours(STUCK_AGENT_HOURS) => Some(format!(
            "{} hasn't moved it in {}h",
            if c.whose_move() == Move::Author { "its agent" } else { "the lander" },
            waited.whole_hours()
        )),
        Move::Reviewers | Move::Ci if waited > time::Duration::days(STUCK_REVIEW_DAYS) => Some(format!(
            "waiting on {} for {} days",
            if c.whose_move() == Move::Ci { "CI" } else { "reviewers" },
            waited.whole_days()
        )),
        _ => None,
    }
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
    /// otto made it, and nobody has started it yet (`crate::agents`).
    pub not_started: bool,
}

pub fn lander_ref() -> Option<LanderRef> {
    let id = crate::config::lander().ok().flatten()?;
    let state = crate::state::read_run(&id).ok();
    Some(LanderRef {
        short: crate::paths::short_id(&id),
        name: state.as_ref().map(|s| s.display_name()).unwrap_or_else(|| id.clone()),
        not_started: state.as_ref().is_some_and(crate::agents::not_started),
        alive: state.is_some_and(|s| !s.status.is_terminal()),
        id,
    })
}

fn line(run: &RunState, short: &str, c: &Change, config: &crate::config::Config) -> Line {
    let v = c.latest();
    let (process, mode) = match &c.decision {
        Some(d) => (d.process.clone().unwrap_or_else(|| crate::config::STANDARD_PROCESS.to_string()), d.mode),
        None => {
            let p = config.process_for(&c.repo);
            (p.name, p.mode)
        }
    };
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
        pr: c.pr.clone(),
        asking: (c.state == ChangeState::NeedsYou).then(|| c.ask.as_ref().map(|a| crate::gate::title(&a.question, 120))).flatten(),
        replies: c.replies.iter().filter(|r| r.state == ReplyState::Draft).count(),
        stuck: stuck(c, crate::clock::now()),
        process,
        mode,
    }
}

/// Every change across every run: open ones, and those finished in the last week (all, with `all`).
pub fn list(all: bool) -> Result<ChangesView, OttoError> {
    let ids = crate::paths::all_run_ids();
    let cutoff = crate::clock::now() - time::Duration::days(DONE_SHOWN_DAYS);
    let config = crate::config::load().unwrap_or_default();
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
            rows.push((line(&state, &short, &c, &config), files));
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
        Move::Author | Move::Lander | Move::Reviewers | Move::Ci => 1,
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

/// Every repository with a change not yet cleaned up after: where the standard agents work.
pub fn repos_in_play() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Ok(runs) = crate::state::read_all_runs() else { return out };
    for entry in runs {
        let crate::state::RunEntry::Readable(state) = entry else { continue };
        let Ok(dir) = crate::paths::run_dir(&state.id) else { continue };
        for c in index_or_empty(&dir).changes {
            if !(c.state.is_over() && c.cleaned.is_some()) && !out.contains(&c.repo) {
                out.push(c.repo);
            }
        }
    }
    out.sort();
    out
}

/// A run's own changes, newest first, for its page.
pub fn of_run(state: &RunState) -> Vec<Line> {
    let Ok(dir) = crate::paths::run_dir(&state.id) else { return Vec::new() };
    let short = crate::paths::short_id(&state.id);
    let config = crate::config::load().unwrap_or_default();
    let mut out: Vec<Line> = index_or_empty(&dir).changes.iter().map(|c| line(state, &short, c, &config)).collect();
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
    /// The process its repository follows now: what approving it does.
    pub process: crate::config::Process,
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
    let config = crate::config::load().unwrap_or_default();
    let process = config.process_for(&change.repo);
    let mut line = line(&state, &short, &change, &config);
    if let Ok(all) = list(true) {
        if let Some(l) = all.changes.into_iter().find(|l| l.run == state.id && l.n == r.n) {
            line.overlaps = l.overlaps;
        }
    }
    Ok(Detail { line, change, version: v, patch, summary, lander: lander_ref(), process, commands })
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
        let err = approve(&[r("c-app", 1)], None, None).unwrap_err();
        assert!(err.message.contains("no lander"), "{}", err.message);
        set_lander("c-lander");
        let out = approve(&[r("c-app", 1)], None, Some("ship it")).unwrap();
        assert_eq!(out[0].state, ChangeState::Approved);
        let c = change("c-app", 1);
        let d = c.decision.as_ref().unwrap();
        assert_eq!((d.version, d.sha.as_str()), (1, c.latest().sha.as_str()));
        assert_eq!(c.whose_move(), Move::Lander);
        assert!(approve(&[r("c-app", 1)], None, None).is_err(), "only a change waiting for review is approved");
        assert_eq!(due().unwrap().len(), 1);
    }

    #[test]
    fn approving_follows_the_repositorys_process_and_keeps_it() {
        let _h = TempHome::new();
        test_init("c-proc", "a goal").unwrap();
        test_init("c-procl", "land changes").unwrap();
        set_lander("c-procl");
        let repo = repo();
        submit("c-proc", &submit_fix(&repo, "one")).unwrap();
        assert_eq!(read(&r("c-proc", 1), None).unwrap().process.name, "standard", "a repository not given one follows standard");

        crate::config::save_process(crate::config::Process {
            name: "reviewed".into(),
            mode: LandMode::Pr,
            merge_when_ready: true,
            start: None,
            land: Some("squash-merge it".into()),
        })
        .unwrap();
        // Any checkout of it names the repository: here, its worktree.
        crate::config::save_project(repo.wt.to_str().unwrap(), Some("reviewed")).unwrap();
        let l = &list(false).unwrap().changes[0];
        assert_eq!((l.process.as_str(), l.mode), ("reviewed", LandMode::Pr));

        let out = approve(&[r("c-proc", 1)], None, None).unwrap();
        assert!(out[0].how.as_deref().unwrap().contains("pull request"));
        let d = change("c-proc", 1).decision.unwrap();
        assert_eq!((d.mode, d.merge_when_ready, d.process.as_deref(), d.land.as_deref(), d.overridden), (LandMode::Pr, true, Some("reviewed"), Some("squash-merge it"), false));
        let due = &due().unwrap()[0];
        assert_eq!((due.todo.as_str(), due.merge_when_ready, due.land.as_deref()), ("open its pull request", true, Some("squash-merge it")));

        // Changing the process later doesn't change what was approved under it.
        crate::config::save_project(repo.main.to_str().unwrap(), Some("standard")).unwrap();
        assert!(crate::config::load().unwrap().projects.is_empty(), "standard is no assignment at all");
        assert_eq!(change("c-proc", 1).decision.unwrap().mode, LandMode::Pr);
        assert_eq!(list(false).unwrap().changes[0].process, "reviewed");
    }

    #[test]
    fn the_person_can_land_one_change_another_way_than_its_process() {
        let _h = TempHome::new();
        test_init("c-ovr", "a goal").unwrap();
        test_init("c-ovrl", "land changes").unwrap();
        set_lander("c-ovrl");
        let repo = repo();
        submit("c-ovr", &submit_fix(&repo, "one")).unwrap();
        approve(&[r("c-ovr", 1)], Some(How { mode: LandMode::Pr, merge_when_ready: false }), None).unwrap();
        let c = change("c-ovr", 1);
        let d = c.decision.as_ref().unwrap();
        assert_eq!((d.mode, d.process.as_deref(), d.overridden), (LandMode::Pr, Some("standard"), true));
        assert!(c.timeline.last().unwrap().what.contains("rather than as standard says"), "{}", c.timeline.last().unwrap().what);
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
        approve(&[r("c-land", 1)], None, None).unwrap();
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
        approve(&[r("c-rev", 1)], None, None).unwrap();
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
        approve(&[r("c-b", 1)], None, None).unwrap();
        claim("c-bl", &r("c-b", 1)).unwrap();
        let c = bounce("c-bl", &r("c-b", 1), "a.txt conflicts with main").unwrap();
        assert_eq!((c.state, c.whose_move()), (ChangeState::Revising, Move::Author));

        assert!(submit("c-b", &submit_fix(&repo, "t")).unwrap().unchanged, "the same commit again is still nothing new");
        std::fs::write(repo.wt.join("a.txt"), "one\nTwo\nthree\n").unwrap();
        sh(&repo.wt, &["commit", "--quiet", "-am", "again"]);
        submit("c-b", &submit_fix(&repo, "t")).unwrap();
        approve(&[r("c-b", 1)], None, None).unwrap();
        claim("c-bl", &r("c-b", 1)).unwrap();
        let head = sh(&repo.wt, &["rev-parse", "HEAD"]);
        let c = rebased("c-bl", &r("c-b", 1), &head).unwrap();
        assert_eq!((c.state, c.latest().k, c.latest().by.as_str()), (ChangeState::Review, 3, "lander"));
    }

    #[test]
    fn a_change_waiting_for_review_is_a_notice_once_a_version() {
        let _h = TempHome::new();
        test_init("c-n", "a goal").unwrap();
        test_init("c-nl", "land changes").unwrap();
        set_lander("c-nl");
        let repo = repo();
        submit("c-n", &submit_fix(&repo, "Shout")).unwrap();
        let dir = crate::paths::run_dir("c-n").unwrap();
        let keys = |d: &Path| crate::notify::change_notices(d).into_iter().map(|n| n.key).collect::<Vec<_>>();
        assert_eq!(keys(&dir), vec!["change:1@v1:review"]);
        revise(&r("c-n", 1), "louder").unwrap();
        assert!(keys(&dir).is_empty(), "its author's move, not the person's");
        std::fs::write(repo.wt.join("a.txt"), "one\nTWO!\nthree\n").unwrap();
        sh(&repo.wt, &["commit", "--quiet", "-am", "louder"]);
        submit("c-n", &submit_fix(&repo, "Shout")).unwrap();
        assert_eq!(keys(&dir), vec!["change:1@v2:review"], "the next version is new to review");
    }

    fn note_texts(run: &str) -> Vec<String> {
        let dir = crate::paths::run_dir(run).unwrap();
        crate::state::read_run(run).unwrap().notes.iter().map(|n| crate::notes::read_text(&dir, n)).collect()
    }

    #[test]
    fn a_pull_request_is_kept_moving_and_what_reviewers_ask_goes_to_its_author() {
        let _h = TempHome::new();
        test_init("c-pr", "a goal").unwrap();
        test_init("c-prl", "land changes").unwrap();
        set_lander("c-prl");
        let repo = repo();
        submit("c-pr", &submit_fix(&repo, "Shout")).unwrap();
        assert!(approve(&[r("c-pr", 1)], Some(How { mode: LandMode::Push, merge_when_ready: true }), None).is_err(), "merging when ready is for a pull request");
        approve(&[r("c-pr", 1)], Some(How { mode: LandMode::Pr, merge_when_ready: false }), None).unwrap();
        assert_eq!(due().unwrap()[0].todo, "open its pull request");
        let d = claim("c-prl", &r("c-pr", 1)).unwrap();
        let v1 = d.sha.unwrap();
        check("c-prl", &r("c-pr", 1), &v1).unwrap();
        assert!(pr_pushed("c-prl", &r("c-pr", 1), &v1, None, false).unwrap_err().message.contains("--url"));
        let c = pr_pushed("c-prl", &r("c-pr", 1), &v1, Some("https://github.com/o/app/pull/41"), false).unwrap();
        assert_eq!((c.state, c.whose_move(), c.pr.as_ref().unwrap().number), (ChangeState::PrOpen, Move::Ci, Some(41)));
        assert!(note_texts("c-pr").last().unwrap().contains("pull/41"), "its author hears the pull request is open");

        let seen = |ci, review, what| Seen { updated: "2026-10-21T12:00:00Z", ci: Some(ci), review: Some(review), mergeable: Some("clean"), what };
        let c = pr_seen("c-prl", &r("c-pr", 1), &seen("passing", "waiting", None)).unwrap();
        assert_eq!(c.whose_move(), Move::Reviewers);
        let c = relay("c-prl", &r("c-pr", 1), "a reviewer (sam)", "call it louder, please").unwrap();
        assert_eq!((c.state, c.whose_move()), (ChangeState::Revising, Move::Author));
        assert!(c.decision.is_some(), "the approval to open it as a pull request stands");
        assert!(note_texts("c-pr").last().unwrap().contains("call it louder"));

        // The next version goes to the pull request without asking the person again.
        std::fs::write(repo.wt.join("a.txt"), "one\nTWO!\nthree\n").unwrap();
        sh(&repo.wt, &["commit", "--quiet", "-am", "louder"]);
        submit("c-pr", &submit_fix(&repo, "Shout")).unwrap();
        let c = change("c-pr", 1);
        assert_eq!((c.state, c.decision.as_ref().unwrap().version), (ChangeState::Approved, 2));
        assert_eq!(due().unwrap()[0].todo, "update its pull request");
        claim("c-prl", &r("c-pr", 1)).unwrap();
        let v2 = sh(&repo.wt, &["rev-parse", "HEAD"]);
        check("c-prl", &r("c-pr", 1), &v2).unwrap();
        assert!(check("c-prl", &r("c-pr", 1), &v1).is_err(), "the old version is not what goes to it now");
        pr_pushed("c-prl", &r("c-pr", 1), &v2, None, false).unwrap();

        // A reply its author drafts waits on the person, then on the lander.
        let c = reply_draft(&r("c-pr", 1), "c-77", "Done in the latest push.").unwrap();
        assert_eq!(c.whose_move(), Move::You);
        post(&r("c-pr", 1), None).unwrap();
        assert_eq!(due().unwrap()[0].todo, "post replies");
        posted("c-prl", &r("c-pr", 1), "c-77").unwrap();

        // An ask answered with words alone, not code: once posted, it is with its reviewers again.
        relay("c-prl", &r("c-pr", 1), "a reviewer (sam)", "why three tries?").unwrap();
        let c = reply_draft(&r("c-pr", 1), "c-78", "It matches the sync client.").unwrap();
        assert_eq!((c.state, c.whose_move()), (ChangeState::PrOpen, Move::You));
        post(&r("c-pr", 1), Some("c-78")).unwrap();
        let c = posted("c-prl", &r("c-pr", 1), "c-78").unwrap();
        assert!(matches!(c.whose_move(), Move::Reviewers | Move::Ci), "not its author's — it has answered: {:?}", c.whose_move());

        // Approved and passing, it is the person's to merge; what merges is what was pushed.
        let c = pr_seen("c-prl", &r("c-pr", 1), &seen("passing", "approved", Some("Sam approved"))).unwrap();
        assert_eq!((c.state, c.whose_move()), (ChangeState::ReadyToMerge, Move::You));
        merge(&r("c-pr", 1)).unwrap();
        assert_eq!(due().unwrap()[0].todo, "merge its pull request");
        check("c-prl", &r("c-pr", 1), &v2).unwrap();
        assert!(check("c-prl", &r("c-pr", 1), &repo_head(&repo)).unwrap_err().message.contains("someone else pushed"));
        assert!(merged("c-prl", &r("c-pr", 1), &v2).unwrap_err().message.contains("fetch first"));
        sh(&repo.wt, &["push", "--quiet", "origin", &format!("{v2}:main")]);
        let c = merged("c-prl", &r("c-pr", 1), &v2).unwrap();
        assert_eq!((c.state, c.whose_move()), (ChangeState::Merged, Move::Lander));
        assert!(note_texts("c-pr").last().unwrap().contains("merged"));
    }

    /// A commit that isn't on the change's branch: main's own.
    fn repo_head(repo: &Repo) -> String {
        sh(&repo.main, &["rev-parse", "main"])
    }

    #[test]
    fn an_author_can_ask_the_person_without_stopping_and_hears_the_answer() {
        let _h = TempHome::new();
        test_init("c-ask", "a goal").unwrap();
        let repo = repo();
        submit("c-ask", &submit_fix(&repo, "Shout")).unwrap();
        let c = ask(&r("c-ask", 1), "Sam wants the flag renamed, which breaks two callers.\n\n## Options\n- **Rename** *(default)* — and fix both callers\n- **Keep** — reply why\n").unwrap();
        assert_eq!((c.state, c.whose_move()), (ChangeState::NeedsYou, Move::You));
        assert_eq!(c.ask.as_ref().unwrap().options, vec!["Rename", "Keep"]);
        assert!(submit("c-ask", &submit_fix(&repo, "Shout")).unwrap_err().message.contains("answer"));
        let out = answer(&r("c-ask", 1), "keep").unwrap();
        assert_eq!(out.state, ChangeState::Revising);
        assert!(note_texts("c-ask").last().unwrap().contains("The person answered:\n\nKeep"), "an option is matched, case aside");
        let stale = Change { state_at: Timestamp::at(crate::clock::now() - time::Duration::hours(9)), ..change("c-ask", 1) };
        assert_eq!(stuck(&stale, crate::clock::now()).as_deref(), Some("its agent hasn't moved it in 9h"));
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
