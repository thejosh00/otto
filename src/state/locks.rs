//! Per-repo locks
//!
//! One lock per repo, under `runs/.locks/repo-<slug>.json`. A lock is breakable only when
//! nothing is coming back for it: the owner is terminal, or no wake holds its wake lock and
//! it is neither parked at a gate nor sleeping toward a future wake. Liveness is observed
//! (the wake lock, on the host filesystem) rather than inferred from a heartbeat the owner
//! promised to keep writing — see `liveness`.

use super::{journal, write_atomic, RunLock};
use crate::error::OttoError;
use crate::liveness::Liveness;
use serde_json::{json, Value};
use std::path::PathBuf;

fn expand_tilde(path: &str) -> String {
    if path == "~" {
        return crate::paths::home_dir().display().to_string();
    }
    match path.strip_prefix("~/") {
        Some(rest) => crate::paths::home_dir().join(rest).display().to_string(),
        None => path.to_string(),
    }
}

/// One lock per repo, named after its full (expanded, not resolved) path — the whole
/// path, not the basename, so two checkouts of differently-named repos never share a
/// lock.
fn repo_lock_path(repo: &str) -> Result<(PathBuf, String), OttoError> {
    let expanded = expand_tilde(repo);
    let slug = crate::clock::slugify(&expanded, "repo");
    let dir = crate::paths::repo_locks_dir()?;
    Ok((dir.join(format!("repo-{slug}.json")), expanded))
}

fn repo_lock_entries() -> Result<Vec<PathBuf>, OttoError> {
    let dir = crate::paths::repo_locks_dir()?;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("repo-") && n.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect();
    entries.sort();
    Ok(entries)
}

/// Whether the run holding a lock has stopped existing in any useful sense.
///
/// v1 asked this by reading `heartbeatAt` and deciding whether the owner had gone quiet.
/// That was an inference about a timestamp a conductor had promised to keep writing, and
/// its default when the field was absent was "quiet" — so deleting heartbeat without
/// replacing the signal would have made every non-sleeping lock instantly breakable.
///
/// Now it is an observation plus two facts on disk. A lock is only breakable when nothing
/// is coming back for it: no wake is running, no timer will fire, and no human has been
/// asked anything. A run parked at a gate is *not* breakable even though nothing is
/// running — someone may answer in a minute, and the next wake continues the work. A lock
/// held across a gate is a bug in the wrapped instructions (DESIGN.md §15), but breaking it
/// under a run that is about to resume is worse than leaving it; `--force` is how a human
/// resolves that, deliberately.
fn holder_is_gone(run_id: &str) -> (bool, String) {
    let run_json = crate::paths::runs_dir().join(run_id).join("run.json");
    let value: Value = match std::fs::read_to_string(&run_json)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
    {
        Some(v) => v,
        None => return (true, "its run.json is gone or unreadable".to_string()),
    };
    let status = value.get("status").and_then(Value::as_str).unwrap_or("");
    if matches!(status, "done" | "failed" | "stopped") {
        return (true, format!("it is {status}"));
    }
    // Observed, not inferred: either a process holds the wake lock or it does not.
    // `Unknown` counts as busy, so an unreadable lock never licenses a break.
    if crate::liveness::LockLiveness.probe(run_id).is_busy() {
        return (false, "a wake is running for it right now".to_string());
    }
    if value.get("gate").map(|g| !g.is_null()).unwrap_or(false) {
        return (false, "it is waiting on an answer to an open gate".to_string());
    }
    let wake = value.get("nextWakeAt").and_then(Value::as_str);
    let wake_at = match wake.map(crate::clock::parse_iso) {
        Some(Ok(dt)) => Some(dt),
        Some(Err(_)) => return (false, "its nextWakeAt is unreadable — not safe to break".to_string()),
        None => None,
    };
    let overdue = wake_at.map(|w| w <= crate::clock::now()).unwrap_or(true);
    if overdue {
        return (
            true,
            format!(
                "no wake is running and it is not sleeping toward a wake (nextWakeAt {})",
                wake.unwrap_or("None")
            ),
        );
    }
    (false, format!("it is sleeping toward {}", wake.unwrap_or("None")))
}

#[derive(clap::Args, Debug)]
pub struct LockArgs {
    pub id: String,
    #[arg(long, required = true)]
    pub repo: String,
    /// break a lock you are sure is dead
    #[arg(long)]
    pub force: bool,
}

/// Claim a repo for this run. Re-entrant: claiming your own lock again is fine, which is
/// what makes a resume after a crash cheap.
pub fn lock(args: LockArgs) -> Result<(), OttoError> {
    let (path, expanded_repo) = repo_lock_path(&args.repo)?;
    let run_path = crate::paths::run_dir(&args.id)?;
    let _lock = RunLock::acquire(&run_path)?;

    if path.exists() {
        let held: Value =
            std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
        let owner = held.get("run").and_then(Value::as_str).map(str::to_string);
        if owner.as_deref() == Some(args.id.as_str()) {
            let mut refreshed = held;
            refreshed["refreshedAt"] = json!(crate::clock::now_iso());
            write_atomic(&path, &(serde_json::to_string_pretty(&refreshed)? + "\n"))?;
            println!("already held by {}", args.id);
            return Ok(());
        }
        if let Some(owner) = &owner {
            let (gone, why) = holder_is_gone(owner);
            if !(gone || args.force) {
                return Err(OttoError::conflict(format!(
                    "{} is locked by run {owner} — {why}. Wait for it, or pass --force if you are certain it is dead.",
                    args.repo
                )));
            }
            let reason = if args.force && !gone { "forced".to_string() } else { why };
            journal(&run_path, "lock-broken", json!({"repo": args.repo, "previousOwner": owner, "reason": reason}))?;
            // Tell the run that lost it, too: it may still wake up believing it owns the
            // worktree. Best-effort — its own run dir may no longer exist.
            if let Ok(owner_path) = crate::paths::run_dir(owner) {
                let _ = journal(&owner_path, "lock-lost", json!({"repo": args.repo, "takenBy": args.id, "reason": reason}));
            }
        }
    }
    write_atomic(
        &path,
        &(serde_json::to_string_pretty(&json!({
            "repo": expanded_repo,
            "run": args.id,
            "acquiredAt": crate::clock::now_iso(),
            "refreshedAt": crate::clock::now_iso(),
        }))? + "\n"),
    )?;
    journal(&run_path, "lock-acquired", json!({"repo": args.repo}))?;
    println!("{}", path.display());
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct UnlockArgs {
    pub id: String,
    /// default: every lock this run holds
    #[arg(long)]
    pub repo: Option<String>,
}

/// Release this run's locks. Never touches a lock held by another run.
pub fn unlock(args: UnlockArgs) -> Result<(), OttoError> {
    let run_path = crate::paths::run_dir(&args.id)?;
    let _lock = RunLock::acquire(&run_path)?;
    let targets: Vec<PathBuf> = match &args.repo {
        Some(repo) => vec![repo_lock_path(repo)?.0],
        None => repo_lock_entries()?,
    };
    let mut released = Vec::new();
    for path in &targets {
        let held: Value = match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
            Some(v) => v,
            None => continue,
        };
        if held.get("run").and_then(Value::as_str) != Some(args.id.as_str()) {
            continue;
        }
        let repo_value = held.get("repo").cloned().unwrap_or(Value::Null);
        let repo_display = held
            .get("repo")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string());
        let _ = std::fs::remove_file(path);
        released.push(repo_display);
        journal(&run_path, "lock-released", json!({"repo": repo_value}))?;
    }
    for repo in &released {
        println!("{repo}");
    }
    if let Some(repo) = &args.repo {
        if released.is_empty() {
            return Err(OttoError::conflict(format!("{repo} is not locked by {}", args.id)));
        }
    }
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct LocksArgs {}

/// Who holds which repo.
pub fn locks(_args: LocksArgs) -> Result<(), OttoError> {
    for path in repo_lock_entries()? {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => {
                println!("{name}\tunreadable");
                continue;
            }
        };
        let held: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => {
                println!("{name}\tunreadable");
                continue;
            }
        };
        let owner = held.get("run").and_then(Value::as_str).unwrap_or("?").to_string();
        let (gone, why) = holder_is_gone(&owner);
        println!(
            "{}\t{owner}\t{}\t{why}",
            held.get("repo").and_then(Value::as_str).unwrap_or("?"),
            if gone { "breakable" } else { "held" },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::{set_status, test_init, SetStatusArgs};
    use crate::state::{transaction, Status};

    fn set_wake(id: &str, wake_minutes_from_now: Option<i64>) {
        transaction(id, |_p, state| {
            state.next_wake_at = wake_minutes_from_now.map(crate::clock::Timestamp::in_minutes);
            Ok(())
        })
        .unwrap();
    }

    fn journal_text(id: &str) -> String {
        let path = crate::paths::run_dir(id).unwrap().join("journal.jsonl");
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn relocking_own_lock_is_reentrant() {
        let _home = TempHome::new();
        test_init("lock-a", "dev-flow").unwrap();
        lock(LockArgs { id: "lock-a".into(), repo: "/tmp/repo-a".into(), force: false }).unwrap();
        lock(LockArgs { id: "lock-a".into(), repo: "/tmp/repo-a".into(), force: false }).unwrap();
        let (path, _) = repo_lock_path("/tmp/repo-a").unwrap();
        let held: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(held.get("run").unwrap(), "lock-a");
    }

    #[test]
    fn terminal_owner_lock_is_breakable_without_force() {
        let _home = TempHome::new();
        test_init("lock-owner-1", "dev-flow").unwrap();
        test_init("lock-taker-1", "dev-flow").unwrap();
        lock(LockArgs { id: "lock-owner-1".into(), repo: "/tmp/repo-b".into(), force: false }).unwrap();
        set_status(SetStatusArgs { id: "lock-owner-1".into(), status: Status::Done, reason: None }).unwrap();
        lock(LockArgs { id: "lock-taker-1".into(), repo: "/tmp/repo-b".into(), force: false }).unwrap();
        let (path, _) = repo_lock_path("/tmp/repo-b").unwrap();
        let held: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(held.get("run").unwrap(), "lock-taker-1");
        assert!(journal_text("lock-taker-1").contains("lock-broken"));
    }

    #[test]
    fn a_running_wake_makes_its_lock_unbreakable_without_force() {
        let _home = TempHome::new();
        test_init("lock-owner-2", "a goal").unwrap();
        test_init("lock-taker-2", "a goal").unwrap();
        lock(LockArgs { id: "lock-owner-2".into(), repo: "/tmp/repo-c".into(), force: false }).unwrap();
        // A real wake lock, held — the observation that replaces v1's fresh heartbeat.
        let _wake = crate::liveness::WakeLock::acquire("lock-owner-2").unwrap();
        let result = lock(LockArgs { id: "lock-taker-2".into(), repo: "/tmp/repo-c".into(), force: false });
        assert!(result.is_err(), "a lock must never be broken under a running wake");
        assert!(result.unwrap_err().to_string().contains("a wake is running"));
    }

    #[test]
    fn no_wake_running_and_nothing_scheduled_is_breakable() {
        let _home = TempHome::new();
        test_init("lock-owner-3", "a goal").unwrap();
        test_init("lock-taker-3", "a goal").unwrap();
        lock(LockArgs { id: "lock-owner-3".into(), repo: "/tmp/repo-d".into(), force: false }).unwrap();
        set_wake("lock-owner-3", None);
        lock(LockArgs { id: "lock-taker-3".into(), repo: "/tmp/repo-d".into(), force: false }).unwrap();
    }

    #[test]
    fn sleeping_toward_a_future_wake_is_not_breakable() {
        let _home = TempHome::new();
        test_init("lock-owner-4", "a goal").unwrap();
        test_init("lock-taker-4", "a goal").unwrap();
        lock(LockArgs { id: "lock-owner-4".into(), repo: "/tmp/repo-e".into(), force: false }).unwrap();
        // Nothing running, but a timer will bring it back in 30 minutes.
        set_wake("lock-owner-4", Some(30));
        let result = lock(LockArgs { id: "lock-taker-4".into(), repo: "/tmp/repo-e".into(), force: false });
        assert!(result.is_err(), "a run that is coming back still owns its worktree");
    }

    /// New in v2: a run parked at a gate has no wake running, but a person may answer at
    /// any moment and the next wake continues the work. Breaking under it would hand the
    /// worktree away mid-review.
    #[test]
    fn a_run_waiting_at_a_gate_is_not_breakable() {
        let _home = TempHome::new();
        test_init("lock-owner-6", "a goal").unwrap();
        test_init("lock-taker-6", "a goal").unwrap();
        lock(LockArgs { id: "lock-owner-6".into(), repo: "/tmp/repo-g".into(), force: false }).unwrap();
        crate::state::commands::open_gate(crate::state::commands::OpenGateArgs {
            id: "lock-owner-6".into(),
            slug: "review".into(),
            question: Some("Approve?".into()),
            question_file: None,
            stdin: false,
            expires_at: None,
            expires_in: None,
        })
        .unwrap();
        let result = lock(LockArgs { id: "lock-taker-6".into(), repo: "/tmp/repo-g".into(), force: false });
        assert!(result.is_err(), "a gated run is waiting, not gone");
        assert!(result.unwrap_err().to_string().contains("open gate"));
    }

    #[test]
    fn force_breaks_a_live_lock_and_records_reason_forced() {
        let _home = TempHome::new();
        test_init("lock-owner-5", "dev-flow").unwrap();
        test_init("lock-taker-5", "dev-flow").unwrap();
        lock(LockArgs { id: "lock-owner-5".into(), repo: "/tmp/repo-f".into(), force: false }).unwrap();
        let _wake = crate::liveness::WakeLock::acquire("lock-owner-5").unwrap();
        lock(LockArgs { id: "lock-taker-5".into(), repo: "/tmp/repo-f".into(), force: true }).unwrap();
        let line = journal_text("lock-taker-5");
        assert!(line.contains("lock-broken"));
        assert!(line.contains("\"reason\":\"forced\""));
    }

    #[test]
    fn missing_owner_run_dir_does_not_fail_the_break() {
        let _home = TempHome::new();
        test_init("lock-owner-6", "dev-flow").unwrap();
        test_init("lock-taker-6", "dev-flow").unwrap();
        lock(LockArgs { id: "lock-owner-6".into(), repo: "/tmp/repo-g".into(), force: false }).unwrap();
        std::fs::remove_dir_all(crate::paths::run_dir("lock-owner-6").unwrap()).unwrap();
        // The owner's run.json is now unreadable, so holder_is_gone treats it as gone —
        // the taker should succeed even though the "tell the loser" write has nowhere to land.
        lock(LockArgs { id: "lock-taker-6".into(), repo: "/tmp/repo-g".into(), force: false }).unwrap();
    }

    #[test]
    fn unlock_without_repo_releases_every_lock_this_run_holds() {
        let _home = TempHome::new();
        test_init("lock-h", "upgrade-flow").unwrap();
        lock(LockArgs { id: "lock-h".into(), repo: "/tmp/repo-h1".into(), force: false }).unwrap();
        lock(LockArgs { id: "lock-h".into(), repo: "/tmp/repo-h2".into(), force: false }).unwrap();
        unlock(UnlockArgs { id: "lock-h".into(), repo: None }).unwrap();
        assert!(!repo_lock_path("/tmp/repo-h1").unwrap().0.exists());
        assert!(!repo_lock_path("/tmp/repo-h2").unwrap().0.exists());
    }

    #[test]
    fn unlock_a_lock_not_held_by_this_run_errors() {
        let _home = TempHome::new();
        test_init("lock-i-owner", "dev-flow").unwrap();
        test_init("lock-i-other", "dev-flow").unwrap();
        lock(LockArgs { id: "lock-i-owner".into(), repo: "/tmp/repo-i".into(), force: false }).unwrap();
        let result = unlock(UnlockArgs { id: "lock-i-other".into(), repo: Some("/tmp/repo-i".into()) });
        assert!(result.is_err());
    }
}
