//! Where otto's durable runtime data lives, independent of wherever the otto binary or
//! its source checkout happen to sit.
//!
//! `$OTTO_HOME` if set, else `~/.otto`:
//!
//! ```text
//! ~/.otto/
//!   runs/<run-id>/...          run state — unchanged internal shape
//!   runs/.locks/repo-*.json    per-repo locks — run-adjacent, as today
//!   locks/poke.lock            the reviver's whole-pass advisory lock (not run state)
//!   logs/poke.log              the reviver's launchd stdout/stderr
//! ```

use crate::error::OttoError;
use std::path::{Path, PathBuf};

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn expand_home(path: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home_dir().join(rest),
        Err(_) => path.to_path_buf(),
    }
}

pub fn otto_home() -> PathBuf {
    match std::env::var_os("OTTO_HOME") {
        Some(value) if !value.is_empty() => expand_home(Path::new(&value)),
        _ => home_dir().join(".otto"),
    }
}

pub fn runs_dir() -> PathBuf {
    otto_home().join("runs")
}

/// Where Claude Code keeps its session transcripts: `~/.claude/projects/<cwd-slug>/<session-id>.jsonl`.
///
/// This is *not* under `$OTTO_HOME` — it belongs to claude, and otto only reads it. It is here
/// because a wake's token usage is now read from the transcript rather than from a JSON result
/// document on stdout (see `wake::transcript`), so the path is part of how a wake is measured.
///
/// Overridable via `$OTTO_CLAUDE_PROJECTS_DIR`, used only by tests, so they never read or write
/// the real user's transcripts. Same shape as `install::skills_dir`'s override.
pub fn claude_projects_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OTTO_CLAUDE_PROJECTS_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    home_dir().join(".claude").join("projects")
}

/// A specific run's directory. Errors (exit code 3) if it does not exist — mirrors
/// `bin/otto-state`'s `run_dir()`. Resolves `run_id` first (see `resolve_run_id`), so every
/// caller of this — `transaction`, `read_run`, `WakeLock::acquire`, `LockLiveness::probe` — takes
/// a unique prefix or a bare slug too, not just a full id.
pub fn run_dir(run_id: &str) -> Result<PathBuf, OttoError> {
    let resolved = resolve_run_id(run_id)?;
    Ok(runs_dir().join(resolved))
}

/// Every run directory under `runs/` that has a `run.json`, by name only — no parsing, unlike
/// `state::read_all_runs`. All `resolve_run_id` needs is the set of ids that exist.
pub fn all_run_ids() -> Vec<String> {
    let root = runs_dir();
    let mut ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.join("run.json").is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    ids.push(name.to_string());
                }
            }
        }
    }
    ids.sort();
    ids
}

/// The slug half of an id shaped `YYYY-MM-DD-<slug>` (what `init_run` generates by default), or
/// the whole id if it isn't shaped that way (an explicit `--id` can be anything).
fn slug_of(id: &str) -> &str {
    let parts: Vec<&str> = id.splitn(4, '-').collect();
    let is_date = parts.len() == 4
        && parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts[..3].iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if is_date {
        parts[3]
    } else {
        id
    }
}

/// Does a person-typed `input` name this run? The one rule `resolve_run_id` and `short_id_among`
/// share, so that what one prints the other accepts.
fn names(id: &str, input: &str) -> bool {
    id.starts_with(input) || slug_of(id).starts_with(input)
}

/// The shortest thing a person can type for `id` that `resolve_run_id` will bring back to it,
/// given `all_ids` (every run that exists): the shortest unique prefix of the slug, extended to
/// the end of its word so it reads as a name rather than a letter — `verify`, not `v`. Two runs
/// whose slugs share a first word get `verify-a` and `verify-b`. Falls back to the full id when
/// nothing shorter is unique.
///
/// Only as durable as the set of runs: right for a command printed to a terminal to be pasted
/// back now, wrong for anything written to disk, where a run created tomorrow could make it
/// ambiguous. Gate files and the journal always carry the full id.
pub fn short_id_among(id: &str, all_ids: &[String]) -> String {
    let slug = slug_of(id);
    let ends = slug.char_indices().map(|(i, _)| i).skip(1).chain(std::iter::once(slug.len()));
    for end in ends {
        let candidate = &slug[..end];
        let mut matching = all_ids.iter().filter(|other| names(other, candidate));
        if let (Some(only), None) = (matching.next(), matching.next()) {
            if only != id {
                continue;
            }
            let word_end = slug[end..].find('-').map(|i| end + i).unwrap_or(slug.len());
            return slug[..word_end].to_string();
        }
    }
    id.to_string()
}

/// `short_id_among` against the runs that exist right now. One directory scan; a caller printing
/// many ids at once (`otto ls`) should read `all_run_ids` once and use `short_id_among`.
pub fn short_id(id: &str) -> String {
    short_id_among(id, &all_run_ids())
}

/// Resolve a person-typed id to the real one: an exact match, else a unique prefix of the whole
/// id, or of just the slug (the part after `YYYY-MM-DD-`, if it looks like one). The slug prefix
/// is what makes this actually useful from a phone: `2026-09-11-verify-1789130664`'s date tells
/// you nothing to type, but `verify` does.
pub fn resolve_run_id(input: &str) -> Result<String, OttoError> {
    // An id is one directory name. Anything that could step out of `runs/` names no run — which
    // matters most for the web server, where the id arrives in a URL.
    if input.is_empty() || input.contains('/') || input.contains('\\') || input.starts_with('.') {
        return Err(OttoError::not_found(format!("no such run: {input}")));
    }
    // Exact match is the common case — every internal caller (poke, a wake resuming itself)
    // already has the canonical id — so it costs nothing: no directory scan at all.
    if runs_dir().join(input).is_dir() {
        return Ok(input.to_string());
    }
    let ids = all_run_ids();
    let candidates: Vec<&String> = ids.iter().filter(|id| names(id, input)).collect();
    match candidates.len() {
        0 => Err(OttoError::not_found(format!("no such run: {input} (looked in {})", runs_dir().display()))),
        1 => Ok(candidates[0].clone()),
        _ => {
            let names: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
            Err(OttoError::usage(format!("\"{input}\" matches {} runs: {} — use more of the id", names.len(), names.join(", "))))
        }
    }
}

/// Per-repo lock namespace. Created on demand.
pub fn repo_locks_dir() -> Result<PathBuf, OttoError> {
    let path = runs_dir().join(".locks");
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

/// otto's own non-run-state directory for cross-pass coordination (the reviver's advisory
/// lock) and logs. Created on demand.
fn otto_locks_dir() -> Result<PathBuf, OttoError> {
    let path = otto_home().join("locks");
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

pub fn poke_lock_path() -> Result<PathBuf, OttoError> {
    Ok(otto_locks_dir()?.join("poke.lock"))
}

fn logs_dir() -> Result<PathBuf, OttoError> {
    let path = otto_home().join("logs");
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

pub fn poke_log_path() -> Result<PathBuf, OttoError> {
    Ok(logs_dir()?.join("poke.log"))
}

/// The real, canonical path to the binary currently executing — what `otto start` embeds
/// in the generated launchd plist so it invokes the actual installed binary regardless of
/// how it was launched.
pub fn current_exe() -> Result<PathBuf, OttoError> {
    let exe = std::env::current_exe()
        .map_err(|e| OttoError::usage(format!("cannot find the running otto binary: {e}")))?;
    exe.canonicalize()
        .map_err(|e| OttoError::usage(format!("cannot resolve {}: {e}", exe.display())))
}

/// Test-only helpers for pointing `OTTO_HOME` at a scratch directory. Centralized here
/// (rather than duplicated per test module) so every test across the crate that touches
/// this process-global env var serializes on the *same* mutex — `cargo test` runs on
/// multiple threads by default, and two tests racing on `OTTO_HOME` through separate
/// mutexes would still corrupt each other.
#[cfg(test)]
pub(crate) mod test_support {
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};

    pub static ENV_MUTEX: Mutex<()> = Mutex::new(());

    pub fn lock_env() -> MutexGuard<'static, ()> {
        ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Points `OTTO_HOME` (and `OTTO_CLAUDE_SKILLS_DIR` / `OTTO_CLAUDE_PROJECTS_DIR`, the
    /// test-only overrides for `~/.claude/skills` and `~/.claude/projects`) at a fresh temp
    /// directory for the life of the guard, restoring whatever they were before on drop. Every
    /// test that exercises real filesystem side effects should hold one of these — the
    /// alternative is silently mutating the real user's home directory, which is exactly the
    /// mistake this guards against.
    pub struct TempHome {
        dir: tempfile::TempDir,
        _lock: MutexGuard<'static, ()>,
        previous_home: Option<OsString>,
        previous_skills_dir: Option<OsString>,
        previous_projects_dir: Option<OsString>,
    }

    impl TempHome {
        pub fn new() -> Self {
            let lock = lock_env();
            let dir = tempfile::TempDir::new().expect("tempdir");
            let previous_home = std::env::var_os("OTTO_HOME");
            let previous_skills_dir = std::env::var_os("OTTO_CLAUDE_SKILLS_DIR");
            let previous_projects_dir = std::env::var_os("OTTO_CLAUDE_PROJECTS_DIR");
            std::env::set_var("OTTO_HOME", dir.path());
            std::env::set_var("OTTO_CLAUDE_SKILLS_DIR", dir.path().join(".claude").join("skills"));
            std::env::set_var("OTTO_CLAUDE_PROJECTS_DIR", dir.path().join(".claude").join("projects"));
            Self { dir, _lock: lock, previous_home, previous_skills_dir, previous_projects_dir }
        }

        pub fn path(&self) -> &std::path::Path {
            self.dir.path()
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.previous_home {
                Some(v) => std::env::set_var("OTTO_HOME", v),
                None => std::env::remove_var("OTTO_HOME"),
            }
            match &self.previous_skills_dir {
                Some(v) => std::env::set_var("OTTO_CLAUDE_SKILLS_DIR", v),
                None => std::env::remove_var("OTTO_CLAUDE_SKILLS_DIR"),
            }
            match &self.previous_projects_dir {
                Some(v) => std::env::set_var("OTTO_CLAUDE_PROJECTS_DIR", v),
                None => std::env::remove_var("OTTO_CLAUDE_PROJECTS_DIR"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::lock_env;
    use super::*;

    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let lock = lock_env();
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous, _lock: lock }
        }

        fn unset(key: &'static str) -> Self {
            let lock = lock_env();
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, previous, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn otto_home_defaults_under_home() {
        let _guard = EnvGuard::unset("OTTO_HOME");
        let expected = home_dir().join(".otto");
        assert_eq!(otto_home(), expected);
    }

    #[test]
    fn otto_home_respects_override() {
        let _guard = EnvGuard::set("OTTO_HOME", "/tmp/otto-test-home");
        assert_eq!(otto_home(), PathBuf::from("/tmp/otto-test-home"));
    }

    #[test]
    fn otto_home_expands_tilde_override() {
        let _guard = EnvGuard::set("OTTO_HOME", "~/otto-test-home");
        assert_eq!(otto_home(), home_dir().join("otto-test-home"));
    }

    fn stub_run(id: &str) {
        let dir = runs_dir().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.json"), "{}").unwrap();
    }

    #[test]
    fn resolve_run_id_matches_exact() {
        let _h = test_support::TempHome::new();
        stub_run("2026-09-11-verify-1789130664");
        assert_eq!(resolve_run_id("2026-09-11-verify-1789130664").unwrap(), "2026-09-11-verify-1789130664");
    }

    #[test]
    fn resolve_run_id_matches_a_unique_prefix() {
        let _h = test_support::TempHome::new();
        stub_run("2026-09-11-verify-1789130664");
        stub_run("2026-09-10-improve");
        assert_eq!(resolve_run_id("2026-09-11").unwrap(), "2026-09-11-verify-1789130664");
        assert_eq!(resolve_run_id("2026-09-10").unwrap(), "2026-09-10-improve");
    }

    #[test]
    fn resolve_run_id_matches_the_bare_slug() {
        let _h = test_support::TempHome::new();
        stub_run("2026-09-11-verify-1789130664");
        assert_eq!(resolve_run_id("verify-1789130664").unwrap(), "2026-09-11-verify-1789130664");
    }

    /// The case this exists for: the date tells a person nothing to type, but a prefix of the
    /// memorable slug does.
    #[test]
    fn resolve_run_id_matches_a_prefix_of_the_slug() {
        let _h = test_support::TempHome::new();
        stub_run("2026-09-11-verify-1789130664");
        assert_eq!(resolve_run_id("verify").unwrap(), "2026-09-11-verify-1789130664");
    }

    #[test]
    fn resolve_run_id_is_ambiguous_when_several_match_and_names_them() {
        let _h = test_support::TempHome::new();
        stub_run("2026-09-11-verify-a");
        stub_run("2026-09-11-verify-b");
        let err = resolve_run_id("2026-09-11").expect_err("must be ambiguous");
        assert_eq!(err.code, 1);
        assert!(err.to_string().contains("2026-09-11-verify-a"));
        assert!(err.to_string().contains("2026-09-11-verify-b"));
    }

    #[test]
    fn resolve_run_id_not_found_when_nothing_matches() {
        let _h = test_support::TempHome::new();
        stub_run("2026-09-11-verify-a");
        let err = resolve_run_id("no-such-thing").expect_err("must refuse");
        assert_eq!(err.code, 3);
        assert!(err.to_string().contains("no such run"));
    }

    /// What `otto ls` prints in its SHORT column, and every printed command uses in place of
    /// the id: one word of the slug when that is enough, more when runs share it.
    #[test]
    fn short_id_is_the_first_unique_word_of_the_slug() {
        let ids: Vec<String> = vec![
            "2026-09-11-verify-1789130664".into(),
            "2026-09-13-once-an-hour-check-for-tasks-to-complete".into(),
            "2026-09-11-upgrade-a".into(),
            "2026-09-12-upgrade-b".into(),
            "h-answer".into(),
        ];
        assert_eq!(short_id_among(&ids[0], &ids), "verify");
        assert_eq!(short_id_among(&ids[1], &ids), "once");
        assert_eq!(short_id_among(&ids[2], &ids), "upgrade-a");
        assert_eq!(short_id_among(&ids[3], &ids), "upgrade-b");
        // An explicit --id with no date is its own slug.
        assert_eq!(short_id_among(&ids[4], &ids), "h");
        // An id not in the set can't be shortened, so it is itself.
        assert_eq!(short_id_among("2026-09-14-elsewhere", &ids), "2026-09-14-elsewhere");
    }

    /// The property that matters: whatever is printed must resolve back to the run it was
    /// printed for, under the same rule and the same set of runs.
    #[test]
    fn short_id_round_trips_through_resolve_run_id() {
        let _h = test_support::TempHome::new();
        for id in ["2026-09-11-verify-1789130664", "2026-09-11-upgrade-a", "2026-09-12-upgrade-b", "2026-09-12-u"] {
            stub_run(id);
        }
        let ids = all_run_ids();
        for id in &ids {
            let short = short_id_among(id, &ids);
            assert_eq!(&resolve_run_id(&short).unwrap(), id, "{short} must resolve to {id}");
        }
        // The slug `u` is a prefix of `upgrade-…`, so nothing shorter than the id itself is unique.
        assert_eq!(short_id_among("2026-09-12-u", &ids), "2026-09-12-u");
    }

    #[test]
    fn run_dir_resolves_a_prefix_too() {
        let _h = test_support::TempHome::new();
        stub_run("2026-09-11-verify-1789130664");
        let dir = run_dir("verify").unwrap();
        assert_eq!(dir, runs_dir().join("2026-09-11-verify-1789130664"));
    }
}
