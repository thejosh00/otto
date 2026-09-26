//! `$OTTO_HOME/config.json` — the machine's own settings, as opposed to any one run's.
//!
//! Today that is only the launchers: what a wake may be run under besides plain `claude`. They
//! live here rather than in the source because they are properties of the machine — which
//! sandbox is installed, with which profile — and differ between machines running the same otto.
//!
//! ```json
//! {
//!   "launchers": [
//!     { "name": "nono (sandbox)", "command": "nono run --profile nolabs-ai/claude -- claude", "grantFlag": "--allow" }
//!   ]
//! }
//! ```
//!
//! A missing file is not an error: it means only the built-in `claude` launcher exists.

use crate::error::OttoError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    #[serde(default)]
    pub launchers: Vec<LauncherDef>,
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
        let config = Config { launchers: vec![def("nono (sandbox)", "nono run -- claude"), def("nono-lite", "nono")] };
        assert_eq!(config.launcher("CLAUDE").unwrap().name, "claude");
        assert_eq!(config.launcher("nono (").unwrap().name, "nono (sandbox)");
        assert_eq!(config.launcher("nono-").unwrap().name, "nono-lite");
        assert!(config.launcher("nono").unwrap_err().to_string().contains("more than one"));
        assert!(config.launcher("missing").unwrap_err().to_string().contains("no launcher"));
        assert!(config.launcher("").is_err());
    }

    #[test]
    fn the_config_can_redefine_claude() {
        let config = Config { launchers: vec![def("claude", "/opt/claude")] };
        assert_eq!(config.launchers().len(), 1);
        assert_eq!(config.launcher("claude").unwrap().command, "/opt/claude");
    }

    #[test]
    fn a_launcher_without_a_command_is_refused() {
        let _h = crate::paths::test_support::TempHome::new();
        std::fs::write(config_path(), r#"{"launchers":[{"name":"x","command":" "}]}"#).unwrap();
        assert!(load().is_err());
    }
}
