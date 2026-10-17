//! Whether a wake moved the run, by otto's own look rather than the wake's word.
//!
//! The stall guard used to count `otto state tick` calls, so it worked only for a wake that
//! remembered to tick and judged `--progress` for itself. On a real daily run six wakes in ten never
//! ticked, and a run whose wakes never tick never stalls: it wakes every period forever, the most
//! expensive way a run can fail. otto is the only writer of what a run owns, so it looks for
//! itself. After each wake that met the contract it takes a mark of the durable state — the phase,
//! the facts, the gates and their answers, what was published and sent, every file under
//! `artifacts/` (the todo among them), and the branches and worktrees of the run's repos — and a
//! wake that leaves the mark as it found it changed nothing. The handoff, the journal and the notes
//! are left out: the first two change every wake whether or not the work did, and a note is the
//! person moving, not the run.
//!
//! Work otto can't see — a reply posted on a PR, a ticket moved — is the wake's to say, with
//! `tick --progress`.

use crate::state::{RunState, Status};
use std::path::Path;

/// A file bigger than this is marked by its size and modified time rather than read whole: an
/// artifact that size is a log or a dump, not the work.
const READ_LIMIT: u64 = 1 << 20;

/// What otto can see of the run's work, as a short hash. Stable across otto builds, since it is
/// compared with one an older build wrote.
pub fn mark(run_dir: &Path, state: &RunState) -> String {
    let mut hash = Fnv::new();
    hash.field(state.phase.as_bytes());
    hash.field(serde_json::to_string(&state.facts).unwrap_or_default().as_bytes());
    for dir in ["artifacts", "gates", "messages", "outputs"] {
        hash.field(dir.as_bytes());
        tree(&mut hash, run_dir, &run_dir.join(dir));
    }
    for repo in &state.launcher.repos {
        hash.field(repo.as_bytes());
        // Stored as given, and claude reads a relative one from where the wakes run.
        let repo = match &state.launcher.workdir {
            Some(workdir) if Path::new(repo).is_relative() => Path::new(workdir).join(repo),
            _ => repo.into(),
        };
        // The run's own commits, not the world's: a fetch moving `origin/*` is someone else's work.
        hash.field(&git(&repo, &["for-each-ref", "--format=%(objectname) %(refname)", "refs/heads", "refs/tags"]));
        hash.field(&git(&repo, &["worktree", "list", "--porcelain"]));
    }
    format!("{:016x}", hash.0)
}

/// Account for wake `n`, which met the contract and left `mark`: it moved the run if the mark
/// changed or it said `tick --progress`, and otherwise adds one to the wakes in a row that changed
/// nothing. Returns whether it moved. The first wake of a run, or the first since otto took marks,
/// has nothing to compare with and counts as moving.
pub fn account(state: &mut RunState, mark: String, n: u32) -> bool {
    let moved = state.progress_mark.as_deref() != Some(mark.as_str()) || state.said_progress == Some(n);
    state.progress_mark = Some(mark);
    state.ticks_without_progress = if moved { 0 } else { state.ticks_without_progress + 1 };
    moved
}

/// The wakes in a row that changed nothing have reached `maxTicksWithoutProgress`, and nothing else
/// is already holding the run: no question waiting, no claim of done to check. A perpetual run is
/// never stalled — watching and finding nothing is its job.
pub fn stalled(state: &RunState) -> bool {
    let limit = if state.policy.perpetual { 0 } else { state.policy.max_ticks_without_progress };
    limit > 0
        && i64::from(state.ticks_without_progress) >= limit
        && matches!(state.status, Status::Sleeping | Status::Running)
        && state.gate.is_none()
        && state.done_claim.is_none()
}

/// Every file under `dir`, in a fixed order: its path from the run directory and its contents.
fn tree(hash: &mut Fnv, run_dir: &Path, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_dir() {
            tree(hash, run_dir, &path);
            continue;
        }
        hash.field(path.strip_prefix(run_dir).unwrap_or(&path).to_string_lossy().as_bytes());
        if meta.len() > READ_LIMIT {
            let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
            hash.field(format!("{} {:?}", meta.len(), modified).as_bytes());
        } else {
            hash.field(&std::fs::read(&path).unwrap_or_default());
        }
    }
}

/// What git says, or nothing for a repo that is gone: a missing repo is the same mark every time.
fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| out.stdout)
        .unwrap_or_default()
}

/// FNV-1a, 64-bit: a change detector, not a defence — and std's hasher is free to change between
/// Rust releases, which would make every run look as if it had moved after an upgrade.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    /// One field, length first, so `ab`+`c` and `a`+`bc` differ.
    fn field(&mut self, bytes: &[u8]) {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;
    use crate::state::read_run;

    fn mark_of(id: &str) -> String {
        mark(&crate::paths::run_dir(id).unwrap(), &read_run(id).unwrap())
    }

    /// The work changes the mark; what is rewritten every wake regardless does not.
    #[test]
    fn the_mark_moves_with_the_work_and_not_with_the_narration() {
        let _h = TempHome::new();
        test_init("p-mark", "a goal").unwrap();
        let dir = crate::paths::run_dir("p-mark").unwrap();
        let start = mark_of("p-mark");
        assert_eq!(mark_of("p-mark"), start, "the same state, the same mark");

        std::fs::write(dir.join("handoff.md"), "# handoff\n## Where this is\nStill waiting.\n").unwrap();
        crate::state::journal(&dir, "cycle", serde_json::json!({"message": "looked again"})).unwrap();
        crate::notes::add("p-mark", "Skip the e2e suite.", false).unwrap();
        assert_eq!(mark_of("p-mark"), start, "the handoff, the journal and a note are not the run's work");

        std::fs::create_dir_all(dir.join("artifacts")).unwrap();
        std::fs::write(dir.join("artifacts/todo.md"), "- [ ] one\n").unwrap();
        let todo = mark_of("p-mark");
        assert_ne!(todo, start);
        std::fs::write(dir.join("artifacts/todo.md"), "- [x] one\n").unwrap();
        assert_ne!(mark_of("p-mark"), todo, "an item ticked off");

        let before = mark_of("p-mark");
        crate::state::transaction("p-mark", |_p, s| {
            s.facts.insert("ciState".into(), "green".into());
            Ok(())
        })
        .unwrap();
        assert_ne!(mark_of("p-mark"), before, "a fact the run tracks moved");
    }

    /// A commit in one of the run's repos is progress; nothing happening in it is not.
    #[test]
    fn a_commit_in_the_run_s_repo_moves_the_mark() {
        let _h = TempHome::new();
        let repo = tempfile::tempdir().unwrap();
        let git_ok = |args: &[&str]| {
            let out = std::process::Command::new("git").arg("-C").arg(repo.path()).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git_ok(&["init", "--quiet", "--initial-branch=main"]);
        git_ok(&["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "--allow-empty", "-m", "one"]);
        test_init("p-repo", "a goal").unwrap();
        crate::state::transaction("p-repo", |_p, s| {
            s.launcher.repos = vec![repo.path().display().to_string()];
            Ok(())
        })
        .unwrap();
        let start = mark_of("p-repo");
        assert_eq!(mark_of("p-repo"), start);
        git_ok(&["-c", "user.email=otto@local", "-c", "user.name=otto", "commit", "--quiet", "--allow-empty", "-m", "two"]);
        assert_ne!(mark_of("p-repo"), start);
    }

    #[test]
    fn a_wake_that_leaves_the_mark_as_it_was_counts_toward_a_stall() {
        let _h = TempHome::new();
        test_init("p-count", "a goal").unwrap();
        let mut state = read_run("p-count").unwrap();
        assert!(account(&mut state, "a".into(), 1), "nothing to compare the first with");
        assert!(!account(&mut state, "a".into(), 2));
        assert!(!account(&mut state, "a".into(), 3));
        assert_eq!(state.ticks_without_progress, 2);
        state.said_progress = Some(4);
        assert!(account(&mut state, "a".into(), 4), "it said what changed where otto can't see");
        assert_eq!(state.ticks_without_progress, 0);
        assert!(!account(&mut state, "a".into(), 5), "saying so once covers that wake only");
        assert!(account(&mut state, "b".into(), 6));
        assert_eq!(state.ticks_without_progress, 0);
    }

    #[test]
    fn only_a_run_nothing_else_holds_is_stalled() {
        let _h = TempHome::new();
        test_init("p-stall", "a goal").unwrap();
        let mut state = read_run("p-stall").unwrap();
        state.status = Status::Sleeping;
        state.ticks_without_progress = state.policy.max_ticks_without_progress as u32;
        assert!(stalled(&state));
        state.policy.perpetual = true;
        assert!(!stalled(&state), "watching and finding nothing is a perpetual run's job");
        state.policy.perpetual = false;
        state.policy.max_ticks_without_progress = 0;
        assert!(!stalled(&state), "0 turns the guard off");
        state.policy.max_ticks_without_progress = 24;
        state.status = Status::AwaitingHuman;
        assert!(!stalled(&state), "a run waiting on a person is not looking at all");
    }
}
