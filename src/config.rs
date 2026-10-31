//! `$OTTO_HOME/config.json` — the machine's own settings, as opposed to any one run's.
//!
//! The **default working directory** a wake runs in, and the **launchers** — what a wake may be run
//! under besides plain `claude`. Both live here rather than in the source because they are
//! properties of the machine — where the work lives, which sandbox is installed — and differ
//! between machines running the same otto. And the **processes**: how much the person stays in the
//! loop when code changes in a repository, decided once per repository rather than once per change.
//!
//! ```json
//! {
//!   "workdir": "~/work",
//!   "launchers": [
//!     { "name": "nono (sandbox)", "command": "nono run --profile nolabs-ai/claude -- claude", "grantFlag": "--allow" }
//!   ]
//! }
//! ```
//!
//! A missing file is not an error: it means only the built-in `claude` launcher exists.

use crate::changes::LandMode;
use crate::error::OttoError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The launcher every machine has, and the default for a new run.
pub const DEFAULT_LAUNCHER: &str = "claude";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LauncherDef {
    /// What a run records and a person picks — `--launcher`, the web form, `run.json`.
    pub name: String,
    /// The command line that ends in running claude, split on whitespace. The wake's prompt and
    /// claude's own flags are appended to it, so it must end in claude or in something that
    /// passes its trailing arguments to claude.
    pub command: String,
    /// The flag that opens a directory in the sandbox (`--allow` for nono). Given, otto passes
    /// it once per directory the wake needs — `$OTTO_HOME` and each `--repo` — ahead of the
    /// command's first `--`, or at the end of the command if it has none. Without it, those
    /// grants are the sandbox profile's job: `--add-dir` alone does not open a path the kernel
    /// has closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_flag: Option<String>,
}

impl LauncherDef {
    fn builtin() -> Self {
        LauncherDef { name: DEFAULT_LAUNCHER.to_string(), command: "claude".to_string(), grant_flag: None }
    }

    pub fn command_words(&self) -> Vec<String> {
        self.command.split_whitespace().map(str::to_string).collect()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// Where a wake runs when its run didn't say (`otto run --workdir`). `~` is expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(default)]
    pub launchers: Vec<LauncherDef>,
    /// The run that lands approved changes — the only one that pushes them (`crate::changes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lander: Option<String>,
    /// Ways a change goes from started to landed, besides the built-in `standard`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<Process>,
    /// The process each repository follows, by its main checkout's absolute path. A repository not
    /// here follows `standard`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, String>,
}

/// The process every repository follows until it is given another: a pull request.
pub const STANDARD_PROCESS: &str = "standard";
/// The built-in process that pushes straight to the base, for a repository nobody else reviews.
pub const DIRECT_PROCESS: &str = "direct";

/// How a change goes from started to landed in a repository. Only `mode` and `mergeWhenReady` are
/// otto's to act on — they decide what the lander is due to do; `start` and `land` are words, for
/// the run that makes a change and for the lander.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Process {
    pub name: String,
    /// What approving a change does: lands it on its base, or opens a pull request.
    pub mode: LandMode,
    /// As a pull request: merge it once it is approved and passing, without asking again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merge_when_ready: bool,
    /// How a change begins here — a branch, a worktree, a ticket first — for the run that makes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// How an approved change lands here, for the lander.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land: Option<String>,
    /// What the babysitter may change on a pull request without the person, in words it judges by —
    /// `trivial`, `small`, `anything but security`. `None` is nothing: every change goes to its author
    /// or the person. The person takes the risk of how it reads them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_changes: Option<String>,
}

impl Process {
    fn standard() -> Self {
        Process {
            name: STANDARD_PROCESS.to_string(),
            mode: LandMode::Pr,
            merge_when_ready: false,
            start: Some("A worktree of its own on a fresh branch from the base, made with the manage-pr skill.".to_string()),
            land: Some(
                "A pull request onto the base, merged the way the repository's README or CLAUDE.md says — else squash-merged — \
                 once the person says so."
                    .to_string(),
            ),
            allow_changes: None,
        }
    }

    fn direct() -> Self {
        Process {
            name: DIRECT_PROCESS.to_string(),
            mode: LandMode::Push,
            merge_when_ready: false,
            start: Some("A worktree of its own on a fresh branch from the base, made with the manage-pr skill.".to_string()),
            land: Some(
                "Rebased onto the base if it moved, tested, and pushed to it — or the repository's own ship script, when its \
                 README or CLAUDE.md documents one."
                    .to_string(),
            ),
            allow_changes: None,
        }
    }

    /// The processes otto has without being told: `standard`, then `direct`.
    fn builtins() -> [Process; 2] {
        [Process::standard(), Process::direct()]
    }

    /// Whether this name is one otto has built in, redefined or not.
    pub fn is_builtin_name(name: &str) -> bool {
        Process::builtins().iter().any(|p| p.name.eq_ignore_ascii_case(name.trim()))
    }

    /// What approving does, in a few words: `pushed to its base`, `a pull request, merged when ready`.
    pub fn mode_words(&self) -> &'static str {
        match (self.mode, self.merge_when_ready) {
            (LandMode::Push, _) => "pushed to its base",
            (LandMode::Pr, false) => "a pull request",
            (LandMode::Pr, true) => "a pull request, merged when ready",
        }
    }
}

pub fn config_path() -> PathBuf {
    crate::paths::otto_home().join("config.json")
}

pub fn load() -> Result<Config, OttoError> {
    let path = config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(e.into()),
    };
    parse(&text).map_err(|e| OttoError::usage(format!("{}: {}", path.display(), e.message)))
}

/// The contents of a `config.json`, checked the way `load` checks the file on disk.
pub fn parse(text: &str) -> Result<Config, OttoError> {
    let config: Config = serde_json::from_str(text).map_err(|e| OttoError::usage(format!("not valid: {e}")))?;
    for l in &config.launchers {
        if l.name.trim().is_empty() || l.command.trim().is_empty() {
            return Err(OttoError::usage("every launcher needs a name and a command"));
        }
    }
    for (i, p) in config.processes.iter().enumerate() {
        if p.name.trim().is_empty() || p.name != p.name.trim() {
            return Err(OttoError::usage("every process needs a name, without spaces around it"));
        }
        if config.processes[..i].iter().any(|q| q.name.eq_ignore_ascii_case(&p.name)) {
            return Err(OttoError::usage(format!("two processes are called \"{}\"", p.name)));
        }
        if p.merge_when_ready && p.mode != LandMode::Pr {
            return Err(OttoError::usage(format!("process \"{}\": mergeWhenReady is for a pull request (mode \"pr\")", p.name)));
        }
    }
    for (repo, name) in &config.projects {
        if config.find_process(name).is_none() {
            return Err(OttoError::usage(format!("{repo} follows a process there is none of: \"{name}\"")));
        }
    }
    Ok(config)
}

impl Config {
    /// Every launcher that can be picked: the built-in `claude` first unless the config
    /// redefines it, then the configured ones in file order.
    pub fn launchers(&self) -> Vec<LauncherDef> {
        let mut all = Vec::new();
        if !self.launchers.iter().any(|l| l.name.eq_ignore_ascii_case(DEFAULT_LAUNCHER)) {
            all.push(LauncherDef::builtin());
        }
        all.extend(self.launchers.iter().cloned());
        all
    }

    /// The launcher a person-typed name means: an exact match (ignoring case), else a unique
    /// prefix — so `--launcher nono` finds `nono (sandbox)` without quoting the parentheses.
    pub fn launcher(&self, input: &str) -> Result<LauncherDef, OttoError> {
        let all = self.launchers();
        let wanted = input.trim().to_lowercase();
        if let Some(exact) = all.iter().find(|l| l.name.to_lowercase() == wanted) {
            return Ok(exact.clone());
        }
        let matching: Vec<&LauncherDef> = all.iter().filter(|l| l.name.to_lowercase().starts_with(&wanted)).collect();
        let names = || all.iter().map(|l| format!("\"{}\"", l.name)).collect::<Vec<_>>().join(", ");
        match matching.as_slice() {
            [one] if !wanted.is_empty() => Ok((*one).clone()),
            [] | [_] => Err(OttoError::usage(format!(
                "no launcher called \"{input}\" — known: {}. Add one to {}",
                names(),
                config_path().display()
            ))),
            _ => Err(OttoError::usage(format!("\"{input}\" matches more than one launcher: {}", names()))),
        }
    }
}

impl Config {
    /// Every process there is: the built-in `standard` and `direct` first, each unless the config
    /// redefines it, then the configured ones in file order.
    pub fn processes(&self) -> Vec<Process> {
        let mut all: Vec<Process> = Process::builtins().into_iter().filter(|b| !self.processes.iter().any(|p| p.name.eq_ignore_ascii_case(&b.name))).collect();
        all.extend(self.processes.iter().cloned());
        all
    }

    fn find_process(&self, name: &str) -> Option<Process> {
        if let Some(p) = self.processes.iter().find(|p| p.name.eq_ignore_ascii_case(name.trim())) {
            return Some(p.clone());
        }
        Process::builtins().into_iter().find(|b| b.name.eq_ignore_ascii_case(name.trim()))
    }

    /// The process a person-typed name means, ignoring case.
    pub fn process(&self, name: &str) -> Result<Process, OttoError> {
        self.find_process(name).ok_or_else(|| {
            let names = self.processes().iter().map(|p| format!("\"{}\"", p.name)).collect::<Vec<_>>().join(", ");
            OttoError::usage(format!("no process called \"{}\" — known: {names}. Make one with `otto config process {} --mode <push|pr>`", name.trim(), name.trim()))
        })
    }

    /// The process a repository follows, by its main checkout: its own, else `standard`.
    pub fn process_for(&self, repo: &str) -> Process {
        self.projects.get(repo).and_then(|name| self.find_process(name)).unwrap_or_else(|| self.process(STANDARD_PROCESS).expect("standard always exists"))
    }

    /// The repositories that follow `name`, by main checkout.
    pub fn projects_of(&self, name: &str) -> Vec<String> {
        self.projects.iter().filter(|(_, p)| p.eq_ignore_ascii_case(name)).map(|(repo, _)| repo.clone()).collect()
    }
}

/// A working directory as a person typed it — `~` expanded, made absolute, and required to exist,
/// because a wake started in a directory that isn't there fails before it does anything.
pub fn resolve_workdir(text: &str) -> Result<PathBuf, OttoError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(OttoError::usage("the working directory is empty"));
    }
    let path = match text.strip_prefix('~') {
        Some(rest) => crate::paths::home_dir().join(rest.trim_start_matches('/')),
        None => PathBuf::from(text),
    };
    let path = if path.is_absolute() { path } else { std::env::current_dir()?.join(path) };
    if !path.is_dir() {
        return Err(OttoError::usage(format!("{} is not a directory", path.display())));
    }
    Ok(path.canonicalize().unwrap_or(path))
}

/// The default working directory, resolved — `None` when `config.json` doesn't set one.
pub fn default_workdir() -> Result<Option<PathBuf>, OttoError> {
    load()?.workdir.as_deref().map(resolve_workdir).transpose()
}

/// Set the default working directory in `config.json`, keeping everything else in the file as it
/// was — including keys this version of otto doesn't know. Returns the directory as resolved.
pub fn save_workdir(text: &str) -> Result<PathBuf, OttoError> {
    let resolved = resolve_workdir(text)?;
    edit(|object| {
        // Stored as typed, so `~/work` stays portable if the file is copied to another machine.
        object.insert("workdir".to_string(), serde_json::Value::String(text.trim().to_string()));
        Ok(())
    })?;
    Ok(resolved)
}

/// Change `config.json` as JSON, keeping everything else in the file as it was, and save it only if
/// the result still loads. The whole read-modify-write holds `config.json.lock`, so two edits at
/// once (the web UI saving a process while the lander is saved) both land. A leaf lock in the
/// order documented on `state::Flock`: `change` must not take another lock.
fn edit(change: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<(), OttoError>) -> Result<Config, OttoError> {
    let path = config_path();
    let _flock = crate::state::Flock::acquire(&path.with_extension("json.lock"))?;
    let mut value = match std::fs::read_to_string(&path) {
        Ok(existing) => serde_json::from_str::<serde_json::Value>(&existing)
            .map_err(|e| OttoError::usage(format!("{} is not valid: {e}", path.display())))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(e.into()),
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| OttoError::usage(format!("{} is not a JSON object", path.display())))?;
    change(object)?;
    let config = parse(&value.to_string())?;
    std::fs::create_dir_all(crate::paths::otto_home())?;
    crate::state::write_atomic(&path, &(serde_json::to_string_pretty(&value)? + "\n"))?;
    Ok(config)
}

/// The lander's run id, when one is set up.
pub fn lander() -> Result<Option<String>, OttoError> {
    Ok(load()?.lander)
}

/// Name the run that lands approved changes, or (`None`) say there is none, keeping everything else
/// in the file as it was. Kept by run id, which a rename doesn't change.
pub fn save_lander(id: Option<&str>) -> Result<Option<String>, OttoError> {
    let id = id.map(crate::paths::resolve_run_id).transpose()?;
    edit(|object| {
        match &id {
            Some(id) => object.insert("lander".to_string(), serde_json::Value::String(id.clone())),
            None => object.remove("lander"),
        };
        Ok(())
    })?;
    Ok(id)
}

/// Make or redefine a process, by name. Redefining `standard` replaces the built-in one.
pub fn save_process(process: Process) -> Result<Process, OttoError> {
    let process = Process {
        name: process.name.trim().to_string(),
        start: words(process.start),
        land: words(process.land),
        allow_changes: allowed_changes(process.allow_changes),
        ..process
    };
    edit(|object| {
        let list = object.entry("processes").or_insert_with(|| serde_json::json!([]));
        let list = list.as_array_mut().ok_or_else(|| OttoError::usage("\"processes\" in config.json is not a list"))?;
        let value = serde_json::to_value(&process)?;
        let same = |v: &serde_json::Value| v.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.eq_ignore_ascii_case(&process.name));
        match list.iter_mut().find(|v| same(v)) {
            Some(existing) => *existing = value,
            None => list.push(value),
        }
        Ok(())
    })?;
    Ok(process)
}

fn words(text: Option<String>) -> Option<String> {
    text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// What may change without the person, or `None` for nothing — which `none`, said in words, is too.
pub fn allowed_changes(text: Option<String>) -> Option<String> {
    words(text).filter(|t| !t.eq_ignore_ascii_case("none"))
}

/// Remove a process. One a repository follows is refused: give the repository another first. Removing
/// a redefined built-in brings back otto's own.
pub fn delete_process(name: &str) -> Result<(), OttoError> {
    let config = load()?;
    let process = config.process(name)?;
    let followers = config.projects_of(&process.name);
    if !followers.is_empty() && !Process::is_builtin_name(&process.name) {
        return Err(OttoError::usage(format!(
            "{} follow{} \"{}\" — give {} another process first (`otto config project <repo> --process <name>`)",
            followers.iter().map(|r| crate::changes::home_relative(r)).collect::<Vec<_>>().join(", "),
            if followers.len() == 1 { "s" } else { "" },
            process.name,
            if followers.len() == 1 { "it" } else { "them" }
        )));
    }
    if !config.processes.iter().any(|p| p.name.eq_ignore_ascii_case(&process.name)) {
        return Err(OttoError::usage(format!("\"{}\" is built in: redefine it instead", process.name)));
    }
    edit(|object| {
        if let Some(list) = object.get_mut("processes").and_then(|l| l.as_array_mut()) {
            list.retain(|v| !v.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.eq_ignore_ascii_case(&process.name)));
            if list.is_empty() {
                object.remove("processes");
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// A repository as a person typed it — any checkout of it, or a directory inside one — as its main
/// checkout, which is what a change records and what a project is kept by.
pub fn resolve_project(repo: &str) -> Result<String, OttoError> {
    let dir = resolve_workdir(repo)?;
    Ok(crate::changes::main_repo(&dir)?.display().to_string())
}

/// Have a repository follow a process, or (`None`) the standard one again. Returns the repository as
/// it is kept and the process it now follows.
pub fn save_project(repo: &str, process: Option<&str>) -> Result<(String, Process), OttoError> {
    let repo = resolve_project(repo)?;
    let process = process.map(|name| load()?.process(name)).transpose()?;
    let config = edit(|object| {
        let map = object.entry("projects").or_insert_with(|| serde_json::json!({}));
        let map = map.as_object_mut().ok_or_else(|| OttoError::usage("\"projects\" in config.json is not an object"))?;
        match &process {
            Some(p) if !p.name.eq_ignore_ascii_case(STANDARD_PROCESS) => map.insert(repo.clone(), serde_json::Value::String(p.name.clone())),
            _ => map.remove(&repo),
        };
        if map.is_empty() {
            object.remove("projects");
        }
        Ok(())
    })?;
    let process = config.process_for(&repo);
    Ok((repo, process))
}

/// The process a repository follows, as stored: `repo` is a main checkout's absolute path, as a
/// change records it. `standard` when the config can't be read, so a broken file never blocks a read.
pub fn process_for(repo: &Path) -> Process {
    load().map(|c| c.process_for(&repo.display().to_string())).unwrap_or_else(|_| Process::standard())
}

/// Where a run's wakes run: the directory the run recorded, else today's default. `None` — a run
/// from before working directories existed, on a machine with no default — leaves a wake where
/// whatever started it happens to be.
pub fn workdir_for(state: &crate::state::RunState) -> Option<PathBuf> {
    match &state.launcher.workdir {
        Some(dir) => Some(PathBuf::from(dir)),
        None => default_workdir().ok().flatten(),
    }
}

/// `Config::launcher` against the config on disk.
pub fn launcher(name: &str) -> Result<LauncherDef, OttoError> {
    load()?.launcher(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str, command: &str) -> LauncherDef {
        LauncherDef { name: name.into(), command: command.into(), grant_flag: None }
    }

    #[test]
    fn concurrent_edits_keep_every_change() {
        let _h = crate::paths::test_support::TempHome::new();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    edit(|object| {
                        // Widen the read-modify-write window, so an unlocked edit would lose one.
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        let launchers = object.entry("launchers").or_insert_with(|| serde_json::json!([]));
                        launchers.as_array_mut().unwrap().push(serde_json::json!({"name": format!("l{i}"), "command": "claude"}));
                        Ok(())
                    })
                    .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let names: Vec<String> = load().unwrap().launchers().into_iter().map(|l| l.name).collect();
        for i in 0..8 {
            assert!(names.contains(&format!("l{i}")), "edit {i} was lost: {names:?}");
        }
    }

    #[test]
    fn a_missing_file_leaves_only_claude() {
        let _h = crate::paths::test_support::TempHome::new();
        let config = load().unwrap();
        assert_eq!(config.launchers(), vec![LauncherDef::builtin()]);
    }

    #[test]
    fn configured_launchers_follow_the_builtin() {
        let _h = crate::paths::test_support::TempHome::new();
        std::fs::write(
            config_path(),
            r#"{"launchers":[{"name":"nono (sandbox)","command":"nono run --profile p -- claude","grantFlag":"--allow"}]}"#,
        )
        .unwrap();
        let names: Vec<String> = load().unwrap().launchers().into_iter().map(|l| l.name).collect();
        assert_eq!(names, ["claude", "nono (sandbox)"]);
    }

    #[test]
    fn a_name_resolves_exactly_or_by_unique_prefix() {
        let config = Config { workdir: None, launchers: vec![def("nono (sandbox)", "nono run -- claude"), def("nono-lite", "nono")], lander: None, ..Default::default() };
        assert_eq!(config.launcher("CLAUDE").unwrap().name, "claude");
        assert_eq!(config.launcher("nono (").unwrap().name, "nono (sandbox)");
        assert_eq!(config.launcher("nono-").unwrap().name, "nono-lite");
        assert!(config.launcher("nono").unwrap_err().to_string().contains("more than one"));
        assert!(config.launcher("missing").unwrap_err().to_string().contains("no launcher"));
        assert!(config.launcher("").is_err());
    }

    #[test]
    fn the_config_can_redefine_claude() {
        let config = Config { workdir: None, launchers: vec![def("claude", "/opt/claude")], lander: None, ..Default::default() };
        assert_eq!(config.launchers().len(), 1);
        assert_eq!(config.launcher("claude").unwrap().command, "/opt/claude");
    }

    #[test]
    fn saving_the_workdir_keeps_the_rest_of_the_file() {
        let h = crate::paths::test_support::TempHome::new();
        let work = h.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(config_path(), r#"{"launchers":[{"name":"nono","command":"nono -- claude"}]}"#).unwrap();
        let saved = save_workdir(work.to_str().unwrap()).unwrap();
        assert_eq!(saved, work.canonicalize().unwrap());
        let config = load().unwrap();
        assert_eq!(config.launchers.len(), 1, "the launchers survive");
        assert_eq!(default_workdir().unwrap(), Some(work.canonicalize().unwrap()));
    }

    #[test]
    fn a_workdir_that_does_not_exist_is_refused() {
        let _h = crate::paths::test_support::TempHome::new();
        assert!(resolve_workdir("/no/such/place").unwrap_err().to_string().contains("not a directory"));
        assert!(save_workdir("/no/such/place").is_err());
        assert_eq!(default_workdir().unwrap(), None, "nothing was saved");
    }

    #[test]
    fn a_workdir_expands_the_home_directory() {
        let resolved = resolve_workdir("~").unwrap();
        assert_eq!(resolved, crate::paths::home_dir().canonicalize().unwrap());
    }

    fn process(name: &str, mode: LandMode) -> Process {
        Process { name: name.into(), mode, merge_when_ready: false, start: None, land: None, allow_changes: None }
    }

    #[test]
    fn every_repository_follows_standard_until_given_another() {
        let _h = crate::paths::test_support::TempHome::new();
        let config = load().unwrap();
        assert_eq!(config.processes().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["standard", "direct"]);
        assert_eq!(config.process_for("/any/repo").mode, LandMode::Pr, "a pull request, until a repository is given another");
        assert_eq!(config.process("STANDARD").unwrap().name, "standard");
        assert_eq!(config.process("direct").unwrap().mode, LandMode::Push);
        assert!(config.process("reviewed").unwrap_err().to_string().contains("no process called"));
    }

    #[test]
    fn a_process_is_made_changed_and_removed_keeping_the_rest_of_the_file() {
        let _h = crate::paths::test_support::TempHome::new();
        std::fs::write(config_path(), r#"{"lander":"9"}"#).unwrap();
        save_process(Process { start: Some("  a branch  ".into()), ..process("reviewed", LandMode::Pr) }).unwrap();
        save_process(Process { merge_when_ready: true, ..process("Reviewed", LandMode::Pr) }).unwrap();
        let config = load().unwrap();
        assert_eq!(config.lander.as_deref(), Some("9"), "the rest of the file survives");
        assert_eq!(config.processes.len(), 1, "the same name, in any case, is the same process");
        assert!(config.processes[0].merge_when_ready);
        assert!(save_process(Process { merge_when_ready: true, ..process("direct", LandMode::Push) }).is_err(), "merging when ready is for a pull request");
        assert!(config.projects.is_empty());
        std::fs::write(config_path(), r#"{"processes":[{"name":"reviewed","mode":"pr"}],"projects":{"/r":"reviewed"}}"#).unwrap();
        assert_eq!(load().unwrap().process_for("/r").name, "reviewed");
        assert!(delete_process("reviewed").unwrap_err().to_string().contains("/r follows"), "a followed process is kept");
        assert!(delete_process("standard").unwrap_err().to_string().contains("built in"));
        std::fs::write(config_path(), r#"{"processes":[{"name":"reviewed","mode":"pr"}]}"#).unwrap();
        delete_process("reviewed").unwrap();
        assert!(load().unwrap().processes.is_empty());
    }

    #[test]
    fn standard_can_be_redefined_and_reset() {
        let _h = crate::paths::test_support::TempHome::new();
        save_process(process("standard", LandMode::Push)).unwrap();
        let config = load().unwrap();
        assert_eq!(config.processes().len(), 2, "the redefinition replaces the built-in one");
        assert_eq!(config.process_for("/any").mode, LandMode::Push);
        delete_process("standard").unwrap();
        assert_eq!(load().unwrap().process_for("/any").mode, LandMode::Pr);
        assert!(delete_process("direct").unwrap_err().to_string().contains("built in"));
    }

    #[test]
    fn a_project_following_a_process_there_is_none_of_is_refused() {
        let _h = crate::paths::test_support::TempHome::new();
        std::fs::write(config_path(), r#"{"projects":{"/r":"missing"}}"#).unwrap();
        assert!(load().unwrap_err().to_string().contains("there is none of"));
        std::fs::write(config_path(), r#"{"processes":[{"name":"a","mode":"pr"},{"name":"A","mode":"push"}]}"#).unwrap();
        assert!(load().unwrap_err().to_string().contains("two processes"));
    }

    #[test]
    fn a_launcher_without_a_command_is_refused() {
        let _h = crate::paths::test_support::TempHome::new();
        std::fs::write(config_path(), r#"{"launchers":[{"name":"x","command":" "}]}"#).unwrap();
        assert!(load().is_err());
    }
}
