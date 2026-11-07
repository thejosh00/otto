//! A sandbox a wake can't take off: the wrapper a sandboxed launcher runs, and the Seatbelt profile
//! it generates. `plans/v3.md`, item 1.2; `docs/guides/sandboxing.md` is the account for a reader.
//!
//! A launcher is a command in front of claude, so a sandbox is a launcher whose command is
//! `$OTTO_HOME/sandbox/wrapper -- claude`. The wrapper runs outside the sandbox: it reads the run's
//! **record**, writes a profile from it, hands claude its access token on descriptor 3, and execs
//! `sandbox-exec`. Everything it can't do, it refuses: nothing here ever starts the command without
//! the sandbox.
//!
//! **The record decides.** `$OTTO_HOME/sandbox/runs/<id>.json` is the only thing that opens anything.
//! The `--allow` otto passes come from `run.json`, which the wake can write, so they are ignored. No
//! run has a record until a person gives it one, and a run under any other launcher never gets here.
//!
//! **The profile's order is the design**, because in Seatbelt the last matching rule wins:
//! closed by default, then what every wake needs, then the projects closed, then the record's grants,
//! and last what no record can open. A record listing a directory above `.git/hooks` opens the
//! directory and not the hooks.

use crate::error::OttoError;
use serde::Deserialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The launcher's command starts this; `otto install` writes it.
pub const WRAPPER: &str = "wrapper";
/// Claude's sign-in in the login Keychain. Named here and nowhere else.
const KEYCHAIN_ITEM: &str = "Claude Code-credentials";
/// Where claude is told its token is. A variable holding the token, or a file, can be read by anything
/// the model runs; this can't (`plans/spike-1.1.md`).
const TOKEN_FD: i32 = 3;

/// What's cleared on the way in, beyond `UNINHERITED`, which applies to every run where this applies
/// only to a sandboxed one. A value here would reach the model whatever the profile says.
const CLEARED: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GH_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GITHUB_TOKEN",
    "GIT_ASKPASS",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "SSH_ASKPASS",
    "SSH_AUTH_SOCK",
    // `cargo test` fails inside with it set (spike), and it names the real tmux server.
    "TMUX",
    "TMUX_PANE",
];
/// Cleared by the shape of the name, for the secrets nobody listed. A launcher's own `FOO_DIR` passes.
const CLEARED_SUFFIXES: &[&str] = &["_TOKEN", "_SECRET", "_PASSWORD", "_PASSPHRASE", "_API_KEY", "_ACCESS_KEY"];
/// Git takes config from these (`GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_0`, `GIT_CONFIG_PARAMETERS`…).
const CLEARED_PREFIXES: &[&str] = &["GIT_CONFIG"];

/// Closed to reads and writes whatever a record lists: there is no wake that needs them.
const CREDENTIALS: &[&str] = &[
    ".ssh",
    ".config/gh",
    ".git-credentials",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".aws",
    ".azure",
    ".gcloud",
    ".config/gcloud",
    ".kube",
    ".gnupg",
    ".docker",
    ".copilot",
    ".config/github-copilot",
    ".config/hub",
    ".ollama/id_ed25519",
    ".android/adbkey",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".gem/credentials",
    ".m2/settings.xml",
    ".m2/settings-security.xml",
    ".gradle/gradle.properties",
    ".conda/aau_token",
    ".zsh_history",
    ".bash_history",
    ".python_history",
    ".node_repl_history",
    "Library/Keychains",
    "Library/Cookies",
    "Library/Mail",
    "Library/Messages",
    "Library/Safari",
    "Library/Application Support/Arc",
    "Library/Application Support/BraveSoftware",
    "Library/Application Support/Firefox",
    "Library/Application Support/Google",
    "Library/Application Support/Microsoft Edge",
];
/// An app's own secrets. Closed unless the run's record lists one, because a run may be that app's
/// (`cloudgpu-build`, `gtd-task-runner`): which runs is the person's call, in the record.
const APP_SECRETS: &[&str] = &[".config/cloudgpu", ".config/wrangler", ".omnifactum/token"];

/// What claude itself writes under `~/.claude`. A list and not the whole directory, since its
/// settings, hooks, skills, agents and plugins all run code under whichever claude reads them next,
/// the person's own included.
const CLAUDE_WRITES: &[&str] = &[
    "projects",
    "todos",
    "tasks",
    "plans",
    "shell-snapshots",
    "session-env",
    "sessions",
    "file-history",
    "paste-cache",
    "debug",
    "cache",
    "state",
    "statsig",
    "downloads",
    "backups",
];
const CLAUDE_WRITE_FILES: &[&str] = &["history.jsonl", "stats-cache.json"];

/// Tools' own caches. Not `~/.cargo` or `~/.bun` whole: their `bin` is on the PATH.
const CACHE_WRITES: &[&str] = &[".cache", ".cargo/registry", ".cargo/git", ".bun/install/cache", ".npm/_cacache", ".npm/_logs", ".npm/_npx", "Library/Caches"];

/// The locks cargo takes in `~/.cargo`, even to build with nothing to fetch.
const CACHE_WRITE_FILES: &[&str] = &[".cargo/.package-cache", ".cargo/.package-cache-mutate", ".cargo/.global-cache"];

/// What starts things outside the sandbox. Refused by binary first, and by the service behind it,
/// since a copy of the binary runs just as well.
const OUTSIDE_COMMANDS: &[&str] = &["/bin/launchctl", "/usr/bin/open", "/usr/bin/osascript", "/usr/bin/security", "/usr/bin/automator", "/usr/bin/shortcuts"];
const OUTSIDE_SERVICES: &[&str] = &[
    // The Keychain, whole: Seatbelt has no filter for one item.
    "com.apple.SecurityServer",
    "com.apple.securityd.xpc",
    "com.apple.secd",
    "com.apple.security.agent",
    // `osascript` to another app, and `open`.
    "com.apple.coreservices.appleevents",
    "com.apple.coreservices.launchservicesd",
    "com.apple.coreservices.quarantine-resolver",
];

/// Enforcing, or (1.7's observing) allowing the same things and logging each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Enforce,
    Report,
}

/// `$OTTO_HOME/sandbox/runs/<id>.json`. Unknown fields are refused, so a mistyped key can't quietly
/// leave something closed or open.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Record {
    /// The launcher this run must be under. Read by what compares a run to its record before starting it.
    #[allow(dead_code)]
    pub launcher: String,
    /// Where it starts. Being listed here opens nothing: this is the one directory a wake is in without
    /// being able to read it, since `~/workspace` as a working directory would otherwise open every project.
    pub workdir: Option<String>,
    /// What it may read beyond what's readable anyway.
    #[serde(default)]
    pub read: Vec<String>,
    /// What it may write, and so read. A path under `$OTTO_HOME` is how the lander's writes into other runs
    /// can be said (1.5).
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub mode: Mode,
}

/// Where this machine's paths are, so a test can generate a profile for a scratch home.
#[derive(Debug, Clone)]
pub struct Machine {
    pub home: PathBuf,
    pub otto_home: PathBuf,
    /// The configured default working directory and every configured project: closed unless listed.
    pub projects: Vec<PathBuf>,
    /// The temporary directories to leave open: `$TMPDIR` and its darwin sibling.
    pub temp: Vec<PathBuf>,
    /// Ports on this machine that act as the person: otto's own web page approves changes for anyone
    /// who asks over HTTP.
    pub web_ports: Vec<u16>,
}

impl Machine {
    pub fn here() -> Machine {
        let config = crate::config::load().unwrap_or_default();
        let mut projects: Vec<PathBuf> = config.projects.keys().map(PathBuf::from).collect();
        if let Ok(Some(dir)) = crate::config::default_workdir() {
            projects.push(dir);
        }
        let home = crate::paths::home_dir();
        let otto_home = crate::paths::otto_home();
        let temp = temp_dirs(std::env::var_os("TMPDIR").map(PathBuf::from), &[&home, &otto_home]);
        Machine {
            home,
            otto_home,
            projects,
            temp,
            web_ports: crate::launchd::web_ports(),
        }
    }
}

/// `/private/tmp`, `$TMPDIR`, and beside it the `C` that compilers and git use on a mac (`/var/folders/…/T`
/// and `…/C`). Not a `TMPDIR` holding anything of the person's: one set to `/` would open the whole disk.
fn temp_dirs(tmpdir: Option<PathBuf>, keep_closed: &[&Path]) -> Vec<PathBuf> {
    let mut temp = vec![PathBuf::from("/private/tmp")];
    let Some(dir) = tmpdir.map(|d| d.canonicalize().unwrap_or(d)) else { return temp };
    if keep_closed.iter().any(|closed| closed.canonicalize().unwrap_or_else(|_| closed.to_path_buf()).starts_with(&dir)) {
        return temp;
    }
    if dir.file_name().is_some_and(|name| name == "T") {
        temp.extend(dir.parent().map(|parent| parent.join("C")));
    }
    temp.push(dir);
    temp
}

/// A path as the kernel sees it, which is what a profile matches: symlinks resolved as far as the path
/// exists (`/tmp` is `/private/tmp`), `~` expanded, `..` gone. A relative path means nothing here.
pub fn normalize(path: &str, home: &Path) -> Result<PathBuf, String> {
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if path == "~" => home.to_path_buf(),
        None => PathBuf::from(path),
    };
    if !expanded.is_absolute() {
        return Err(format!("\"{path}\" is not an absolute path"));
    }
    if path.contains(['\n', '\r', '\0']) {
        return Err(format!("{path:?} has a line break or a NUL in it"));
    }
    let mut existing = expanded.as_path();
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Ok(out);
        }
        let name = existing.file_name().ok_or_else(|| format!("\"{path}\" can't be resolved"))?;
        // `..` past a part that doesn't exist can't be resolved, so it can't be what it says.
        if name == ".." {
            return Err(format!("\"{path}\" has .. past a directory that doesn't exist"));
        }
        rest.push(name);
        existing = existing.parent().ok_or_else(|| format!("\"{path}\" can't be resolved"))?;
    }
}

/// A path as text in a profile. Seatbelt reads a scheme string, so these two are all that's special.
fn quoted(path: &Path) -> String {
    let mut out = String::from("\"");
    for c in path.to_string_lossy().chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

struct Profile {
    text: String,
    mode: Mode,
    message: String,
}

impl Profile {
    /// A refusal, or under report mode the same thing allowed and logged.
    fn close(&mut self, ops: &str, filters: &[String]) {
        if filters.is_empty() {
            return;
        }
        let verb = match self.mode {
            Mode::Enforce => "deny",
            Mode::Report => "allow",
        };
        let report = if self.mode == Mode::Report { " (with report)" } else { "" };
        let _ = writeln!(self.text, "({verb} {ops}{report} (with message \"{}\")\n  {})", self.message, filters.join("\n  "));
    }

    fn open(&mut self, ops: &str, filters: &[String]) {
        if !filters.is_empty() {
            let _ = writeln!(self.text, "(allow {ops}\n  {})", filters.join("\n  "));
        }
    }

    fn note(&mut self, words: &str) {
        let _ = writeln!(self.text, "\n;; {words}");
    }
}

fn subpath(path: &Path) -> String {
    format!("(subpath {})", quoted(path))
}

fn literal(path: &Path) -> String {
    format!("(literal {})", quoted(path))
}

/// Git's way in, under a directory a wake can write. Not by a regex alone over the whole disk, as a
/// wake's own `git init` in a temporary directory has to work.
///
/// `config.lock` stays open: git takes it before writing, and with it closed `git worktree add -b x origin/main`
/// fails outright, where otherwise it warns that it can't record the upstream and carries on. The rename onto
/// `config` is what's refused, so the lock opens nothing.
const GIT_REGEX: &str = r#"/\.git/(hooks(/.*)?|config(\.worktree)?|worktrees/[^/]+/config(\.worktree)?)$"#;
/// Claude's, in a repository: read by whichever claude next starts there.
const CLAUDE_REGEX: &str = r#"/\.claude/(settings(\.local)?\.json|hooks(/.*)?)$"#;

/// Every repository at or below `dir`, a few levels down: the directories that can't be moved, since one
/// moved out of reach and changed comes back with hooks in it. A worktree's `.git` is a file, so it's not
/// counted, and moves as a wake needs it to.
fn repos_below(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if dir.join(".git").is_dir() {
        found.push(dir.to_path_buf());
    }
    if depth == 0 {
        return found;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" || name == "node_modules" || name == "target" || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        found.extend(repos_below(&path, depth - 1));
    }
    found
}

/// The whole profile for one run. The same inputs give the same text, and a path never becomes anything
/// but a quoted string.
pub fn profile(run_id: &str, record: &Record, machine: &Machine) -> Result<String, String> {
    if !run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("the run id {run_id:?} isn't one a profile can carry"));
    }
    let home = &machine.home;
    let otto = &machine.otto_home;
    let list = |paths: &[String]| paths.iter().map(|p| normalize(p, home)).collect::<Result<Vec<_>, _>>();
    let write = list(&record.write)?;
    let read = list(&record.read)?;
    let workdir = record.workdir.as_deref().map(|d| normalize(d, home)).transpose()?;
    let real = |p: PathBuf| p.canonicalize().unwrap_or(p);
    let (home_real, otto_real) = (real(home.clone()), real(otto.clone()));
    let in_home = |sub: &str| home_real.join(sub);
    let run_dir = otto_real.join("runs").join(run_id);

    let mut p = Profile { text: String::new(), mode: record.mode, message: format!("otto-run-{run_id}") };
    let _ = writeln!(p.text, ";; Generated for run {run_id} by `otto sandbox wrap`, each time it starts. Not read back.");
    let _ = writeln!(p.text, "(version 1)\n(allow default)");

    p.note("Closed by default: writes. New places that run code keep turning up, so this is a list of what's open.");
    p.close("file-write*", &["(subpath \"/\")".to_string()]);

    p.note("Open to every wake: devices, temporary files, what claude keeps, tools' caches.");
    let mut open = vec![
        "(literal \"/dev/null\")".to_string(),
        "(literal \"/dev/zero\")".to_string(),
        "(literal \"/dev/random\")".to_string(),
        "(literal \"/dev/urandom\")".to_string(),
        "(literal \"/dev/tty\")".to_string(),
        "(literal \"/dev/dtracehelper\")".to_string(),
        "(regex #\"^/dev/(fd/|ttys[0-9]+$|ptmx$|pty)\")".to_string(),
    ];
    let mut temp: Vec<PathBuf> = machine.temp.iter().map(|t| real(t.clone())).collect();
    temp.sort();
    temp.dedup();
    open.extend(temp.iter().map(|t| subpath(t)));
    open.extend(CLAUDE_WRITES.iter().map(|d| subpath(&in_home(".claude").join(d))));
    open.extend(CLAUDE_WRITE_FILES.iter().map(|f| literal(&in_home(".claude").join(f))));
    open.extend(CACHE_WRITES.iter().map(|d| subpath(&in_home(d))));
    open.extend(CACHE_WRITE_FILES.iter().map(|f| literal(&in_home(f))));
    p.open("file-write*", &open);

    p.note("Other projects, both ways, the working directory included. Its own entry can still be looked at, to start in it.");
    let mut closed: Vec<PathBuf> = machine.projects.iter().map(|d| real(d.clone())).collect();
    closed.extend(workdir.iter().cloned());
    closed.sort();
    closed.dedup();
    p.close("file-read* file-write*", &closed.iter().map(|d| subpath(d)).collect::<Vec<_>>());
    p.open("file-read-metadata", &closed.iter().map(|d| literal(d)).collect::<Vec<_>>());

    p.note("Its own run, repo locks and the feedback file: all of $OTTO_HOME is readable, and only these writable. After the projects, as a home under a closed directory is still its home.");
    p.open("file-read*", &[subpath(&otto_real)]);
    p.open(
        "file-write*",
        &[
            subpath(&run_dir),
            subpath(&otto_real.join("runs/.locks")),
            subpath(&otto_real.join("runs/.repo-flocks")),
            literal(&otto_real.join("feedback.jsonl")),
            literal(&otto_real.join(".feedback.lock")),
            // `write_atomic`'s temporary, for a run that moves an item on (`otto feedback building`). It's renamed onto the file above.
            format!(r#"(require-all {} (regex #"/\.feedback\.jsonl\.[^/]+\.tmp$"))"#, subpath(&otto_real)),
        ],
    );

    p.note("An app's own secrets: closed, unless the record lists one just below.");
    p.close("file-read* file-write*", &APP_SECRETS.iter().map(|s| subpath(&in_home(s))).collect::<Vec<_>>());

    p.note("The record: this is the only thing that opens anything for this run.");
    let readable: Vec<String> = read.iter().chain(write.iter()).map(|d| subpath(d)).collect();
    p.open("file-read*", &readable);
    p.open("file-write*", &write.iter().map(|d| subpath(d)).collect::<Vec<_>>());
    // Compilers and git walk up a path looking at each directory. Being able to see that one exists says
    // nothing of what's in it, and a grant below a closed directory doesn't work without this.
    let mut above: Vec<PathBuf> = read.iter().chain(write.iter()).flat_map(|d| d.ancestors().skip(1).map(Path::to_path_buf)).collect();
    above.sort();
    above.dedup();
    p.open("file-read-metadata", &above.iter().map(|d| literal(d)).collect::<Vec<_>>());

    p.note("Nothing below can be opened by a record: it comes after, and the last rule wins.");
    p.note("Git and claude settings in anything writable: hooks and config run code in whoever next uses them.");
    let writable: Vec<&PathBuf> = write.iter().collect();
    let in_writable = |regex: &str| writable.iter().map(|d| format!("(require-all {} (regex #\"{regex}\"))", subpath(d))).collect::<Vec<_>>();
    p.close("file-write*", &in_writable(GIT_REGEX));
    p.close("file-write*", &in_writable(CLAUDE_REGEX));
    // Moving a repository, or anything holding one, out to somewhere writable would take it past the rule above.
    let mut pinned: Vec<PathBuf> = Vec::new();
    for dir in &write {
        pinned.push(dir.clone());
        for repo in repos_below(dir, 3) {
            let mut at = repo.clone();
            while at.starts_with(dir) {
                pinned.push(at.join(".git"));
                pinned.push(at.clone());
                if !at.pop() {
                    break;
                }
            }
        }
    }
    pinned.sort();
    pinned.dedup();
    p.close("file-write-unlink", &pinned.iter().map(|d| format!("(require-all {} (vnode-type DIRECTORY))", literal(d))).collect::<Vec<_>>());

    p.note("otto's own: who runs what, who may assign, the wrapper, the profiles and the records.");
    p.close(
        "file-write*",
        &[literal(&otto_real.join("config.json")), literal(&otto_real.join("config.json.lock")), literal(&otto_real.join("feedback-driver")), subpath(&otto_real.join("sandbox"))],
    );

    p.note("Credentials, and what a person's accounts leave lying around. A list, so it closes what's named and no more.");
    p.close("file-read* file-write*", &CREDENTIALS.iter().map(|s| subpath(&in_home(s))).collect::<Vec<_>>());
    p.close("file-read* file-write*", &[literal(&in_home(".claude/.credentials.json")), "(subpath \"/Library/Keychains\")".to_string()]);

    p.note("Things outside the sandbox that would act for it.");
    p.close("mach-lookup", &OUTSIDE_SERVICES.iter().map(|s| format!("(global-name \"{s}\")")).collect::<Vec<_>>());
    p.close("process-exec*", &OUTSIDE_COMMANDS.iter().map(|c| format!("(literal \"{c}\")")).collect::<Vec<_>>());
    let mut sockets = vec![
        // The ssh agent, and the real tmux server. A private one under TMUX_TMPDIR starts inside, so it stays confined.
        "(regex #\"^/private/(tmp|var/run)/com\\.apple\\.launchd\\.[^/]+/Listeners$\")".to_string(),
        "(regex #\"^/private/tmp/tmux-[0-9]+/\")".to_string(),
        subpath(&in_home(".ssh")),
        subpath(&in_home(".docker")),
        "(literal \"/private/var/run/docker.sock\")".to_string(),
    ];
    // otto's page approves changes for whoever asks, with no sign-in: for a wake that's approving itself.
    sockets.extend(machine.web_ports.iter().map(|port| format!("(remote tcp \"localhost:{port}\")")));
    p.close("network-outbound", &sockets);
    Ok(p.text)
}

/// `$OTTO_HOME/sandbox`, what `otto install` makes.
pub fn dir() -> PathBuf {
    crate::paths::otto_home().join("sandbox")
}

/// The wrapper and the directories beside it, there from the first install. This opens nothing and
/// moves no run: only a launcher naming the wrapper and a record do that, and both are a person's.
pub fn ensure_wrapper(otto: &Path) -> Result<PathBuf, OttoError> {
    use std::os::unix::fs::PermissionsExt;
    let dir = dir();
    std::fs::create_dir_all(dir.join("runs"))?;
    std::fs::create_dir_all(dir.join("profiles"))?;
    let path = dir.join(WRAPPER);
    let shown = otto.to_string_lossy().replace('\'', "'\\''");
    // Only `exec`, and `"$@"` carried untouched: what follows is claude's prompt, and nothing evaluates it.
    let text = format!("#!/bin/sh\n# Written by `otto install`. See docs/guides/sandboxing.md.\nexec '{shown}' sandbox wrap \"$@\"\n");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        let temp = dir.join(format!("{WRAPPER}.tmp"));
        std::fs::write(&temp, text)?;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&temp, &path)?;
    }
    Ok(path)
}

/// The run a wrapper was started for: a wake says `OTTO_RUN_ID`, a chat `OTTO_CHAT`.
fn run_from_env() -> Option<String> {
    std::env::var("OTTO_RUN_ID").ok().or_else(|| std::env::var(crate::chat::CHAT_ENV).ok()).filter(|id| !id.is_empty())
}

/// Read the record for this run. Missing is a refusal too: being under this launcher with nothing it
/// could open isn't a sandbox.
pub fn read_record(run_id: &str) -> Result<Record, String> {
    let path = dir().join("runs").join(format!("{run_id}.json"));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => return Err(format!("no sandbox record for run {run_id} at {}: {e}", path.display())),
    };
    serde_json::from_str(&text).map_err(|e| format!("the sandbox record {} can't be used: {e}", path.display()))
}

/// The access token out of the Keychain's JSON. Errors never carry any of the input.
fn token_from(json: &[u8]) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_slice(json).map_err(|_| "claude's Keychain item isn't the JSON expected".to_string())?;
    value["claudeAiOauth"]["accessToken"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "claude's Keychain item has no access token: sign in with `claude auth login`".to_string())
}

fn read_token() -> Result<String, String> {
    let out = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", KEYCHAIN_ITEM, "-w"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run security to read claude's sign-in: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "no sign-in for claude in the Keychain (security exited {}): sign in with `claude auth login`",
            out.status.code().unwrap_or(-1)
        ));
    }
    token_from(&out.stdout)
}

/// What the sandboxed command's environment loses.
fn cleared(name: &str) -> bool {
    CLEARED.contains(&name) || CLEARED_SUFFIXES.iter().any(|s| name.ends_with(s)) || CLEARED_PREFIXES.iter().any(|p| name.starts_with(p))
}

extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn dup2(from: i32, to: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
}

/// The token on `TOKEN_FD`, for reading once and then gone. A pipe holds far more than a token, so the
/// write can't block, and nothing with a name on disk ever held it.
fn hand_in(token: &str) -> Result<(), String> {
    let mut fds = [0i32; 2];
    let failed = |what: &str| format!("couldn't {what} for claude's token: {}", std::io::Error::last_os_error());
    // SAFETY: `fds` has room for the two descriptors, and these are descriptors this process just made.
    // `pipe` leaves both inheritable, which the read end needs to be and the write end stops being here.
    unsafe {
        if pipe(fds.as_mut_ptr()) != 0 {
            return Err(failed("make a pipe"));
        }
        let [mut read_end, mut write_end] = fds;
        // The write end must not land on the number the read end is going to.
        if write_end == TOKEN_FD {
            write_end = dup2(write_end, TOKEN_FD + 1);
            if write_end < 0 {
                return Err(failed("move a descriptor"));
            }
            close(TOKEN_FD);
        }
        let bytes = token.as_bytes();
        if write(write_end, bytes.as_ptr(), bytes.len()) != bytes.len() as isize {
            return Err(failed("write it"));
        }
        close(write_end);
        if read_end != TOKEN_FD {
            if dup2(read_end, TOKEN_FD) != TOKEN_FD {
                return Err(failed("place it"));
            }
            close(read_end);
            read_end = TOKEN_FD;
        }
        debug_assert_eq!(read_end, TOKEN_FD);
    }
    Ok(())
}

/// Whether a profile compiles and can be entered, before anything's handed in. What `sandbox-exec` says
/// is kept for the refusal: 71 is a different profile tried inside a sandbox.
fn try_profile(path: &Path) -> Result<(), String> {
    let out = std::process::Command::new("/usr/bin/sandbox-exec")
        .arg("-f")
        .arg(path)
        .arg("/usr/bin/true")
        .env_clear()
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run sandbox-exec: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    Err(format!("sandbox-exec refused the profile {} (exit {}): {}", path.display(), out.status.code().unwrap_or(-1), said.trim()))
}

fn write_profile(run_id: &str, text: &str) -> Result<PathBuf, String> {
    let dir = dir().join("profiles");
    let failed = |e: std::io::Error| format!("couldn't write a profile in {}: {e}", dir.display());
    std::fs::create_dir_all(&dir).map_err(failed)?;
    let path = dir.join(format!("{run_id}.sb"));
    let temp = dir.join(format!("{run_id}.sb.{}", std::process::id()));
    std::fs::write(&temp, text).map_err(failed)?;
    std::fs::rename(&temp, &path).map_err(failed)?;
    Ok(path)
}

/// `--allow <dir>…`, then `--`, then the command. The grants are read only to be skipped, and nothing past
/// the first `--` is ever looked at, since it's claude's prompt and not otto's to trust.
fn command_after_grants(args: &[String]) -> Result<&[String], String> {
    let mut at = 0;
    loop {
        match args.get(at).map(String::as_str) {
            Some("--") => break,
            Some("--allow") if at + 1 < args.len() => at += 2,
            Some(other) => return Err(format!("unexpected \"{other}\" before the command: the launcher's command is `<wrapper> -- claude`")),
            None => return Err("no command: the launcher's command is `<wrapper> -- claude`".to_string()),
        }
    }
    match &args[at + 1..] {
        [] => Err("no command after --".to_string()),
        command => Ok(command),
    }
}

/// `otto sandbox wrap`. Only returns on a refusal: otherwise it has become the sandboxed command.
pub fn wrap(args: &[String]) -> Result<(), OttoError> {
    let run_id = run_from_env();
    let refused = |run: Option<&str>, why: String| {
        if let Some(id) = run {
            // Beside the wake's own account of itself, as this process is about to be all there is of it.
            let _ = crate::state::log_event(id, crate::event::Event::SandboxRefused { why: why.clone() });
        }
        OttoError::conflict(format!("otto sandbox: not starting anything: {why}"))
    };
    let Some(id) = run_id else {
        return Err(refused(None, "this isn't a wake or a chat: neither OTTO_RUN_ID nor OTTO_CHAT is set".to_string()));
    };
    match prepare(&id, args) {
        Ok(prepared) => {
            let error = prepared.exec();
            Err(refused(Some(&id), format!("couldn't start sandbox-exec: {error}")))
        }
        Err(why) => Err(refused(Some(&id), why)),
    }
}

struct Prepared {
    profile: PathBuf,
    command: Vec<String>,
}

fn prepare(id: &str, args: &[String]) -> Result<Prepared, String> {
    let command = command_after_grants(args)?.to_vec();
    let id = crate::paths::resolve_run_id(id).map_err(|e| e.to_string())?;
    let record = read_record(&id)?;
    let text = profile(&id, &record, &Machine::here())?;
    let path = write_profile(&id, &text)?;
    try_profile(&path)?;
    let token = read_token()?;
    hand_in(&token)?;
    Ok(Prepared { profile: path, command })
}

impl Prepared {
    fn exec(self) -> std::io::Error {
        use std::os::unix::process::CommandExt;
        let mut command = std::process::Command::new("/usr/bin/sandbox-exec");
        command.arg("-f").arg(&self.profile).args(&self.command);
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(cleared) {
                command.env_remove(name);
            }
        }
        command.env("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR", TOKEN_FD.to_string());
        command.exec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Record {
        Record { launcher: "sandbox".into(), workdir: Some("/w/space".into()), read: vec!["/w/space/lib".into()], write: vec!["/w/space/repo".into()], mode: Mode::Enforce }
    }

    fn machine() -> Machine {
        Machine { home: "/u".into(), otto_home: "/u/.otto".into(), projects: vec!["/w/space".into()], temp: vec!["/private/tmp".into()], web_ports: vec![7878] }
    }

    fn generated(record: &Record) -> String {
        profile("r1", record, &machine()).unwrap()
    }

    /// In Seatbelt the last matching rule wins, so a grant has to come before what no record may open.
    /// A record listing a directory above `.git/hooks` opens the directory, and not the hooks.
    #[test]
    fn what_no_record_can_open_comes_after_the_record() {
        let text = generated(&record());
        let grant = text.find("(allow file-write*\n  (subpath \"/w/space/repo\")").expect("the record's write");
        for later in ["/\\.git/(hooks", "(literal \"/u/.otto/config.json\")", "(subpath \"/u/.ssh\")", "(subpath \"/u/.otto/sandbox\")", "(global-name \"com.apple.secd\")"] {
            assert!(text.find(later).expect(later) > grant, "{later} must come after the grants");
        }
        let closed = text.find("(deny file-read* file-write* (with message \"otto-run-r1\")\n  (subpath \"/w/space\")").expect("the projects are closed");
        assert!(closed < grant, "the projects close before the record opens one of them");
    }

    /// An app's secrets are the one thing a record can open: which runs need them is the person's call.
    #[test]
    fn an_apps_secrets_close_before_the_record_and_credentials_after() {
        let text = generated(&record());
        let grant = text.find("(allow file-read*\n  (subpath \"/w/space/lib\")").unwrap();
        assert!(text.find("(subpath \"/u/.config/cloudgpu\")").unwrap() < grant);
        assert!(text.find("(subpath \"/u/.config/gh\")").unwrap() > grant);
    }

    /// A path comes from a file and becomes profile text. A quote in it must stay in the string.
    #[test]
    fn a_path_can_not_become_profile_text() {
        let mut r = record();
        r.write = vec!["/w/a\") (allow default) (literal \"/b".into()];
        let text = generated(&r);
        assert!(text.contains(r#"(subpath "/w/a\") (allow default) (literal \"/b")"#), "{text}");
        r.write = vec!["/w/back\\slash".into()];
        assert!(generated(&r).contains(r#"(subpath "/w/back\\slash")"#));
        r.write = vec!["/w/line\nbreak".into()];
        assert!(profile("r1", &r, &machine()).unwrap_err().contains("line break"));
        r.write = vec!["relative/path".into()];
        assert!(profile("r1", &r, &machine()).unwrap_err().contains("absolute"));
        assert!(profile("r1\") (allow default", &record(), &machine()).is_err(), "the id is in each message");
    }

    /// Every rule carries the run's id, which is how 1.7's report tells runs apart, and report mode changes
    /// only the verb: the same rules, in the same order.
    #[test]
    fn report_mode_is_the_same_rules_allowed_and_logged() {
        let enforcing = generated(&record());
        let mut r = record();
        r.mode = Mode::Report;
        let reporting = generated(&r);
        assert!(!reporting.contains("(deny "), "{reporting}");
        assert_eq!(enforcing.matches("(deny ").count(), reporting.matches("(with report)").count());
        assert_eq!(enforcing.matches("(deny ").count(), enforcing.matches("(with message \"otto-run-r1\")").count());
    }

    #[test]
    fn a_record_with_a_mistyped_field_is_refused() {
        assert!(serde_json::from_str::<Record>(r#"{"launcher":"s","writes":["/x"]}"#).is_err());
        assert!(serde_json::from_str::<Record>(r#"{"launcher":"s","mode":"off"}"#).is_err());
        let least: Record = serde_json::from_str(r#"{"launcher":"s"}"#).unwrap();
        assert_eq!(least.mode, Mode::Enforce);
    }

    #[test]
    fn paths_resolve_as_the_kernel_sees_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::os::unix::fs::symlink(&root, root.join("link")).unwrap();
        let home = root.clone();
        assert_eq!(normalize("~/x", &home).unwrap(), root.join("x"));
        assert_eq!(normalize(&format!("{}/link/not/yet/", root.display()), &home).unwrap(), root.join("not/yet"));
        std::fs::create_dir(root.join("real")).unwrap();
        assert_eq!(normalize(&format!("{}/real/../real", root.display()), &home).unwrap(), root.join("real"));
        assert!(normalize("x", &home).is_err());
    }

    /// `TMPDIR=/` must not be how the disk opens, and a mac's real one brings its cache directory.
    #[test]
    fn a_temporary_directory_opens_only_itself() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (t, home) = (root.join("T"), root.join("home"));
        std::fs::create_dir_all(&t).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        assert_eq!(temp_dirs(Some(t.clone()), &[&home]), [PathBuf::from("/private/tmp"), root.join("C"), t.clone()]);
        for holds_home in ["/", root.to_str().unwrap()] {
            assert_eq!(temp_dirs(Some(holds_home.into()), &[&home]), [PathBuf::from("/private/tmp")], "{holds_home}");
        }
        assert_eq!(temp_dirs(None, &[&home]), [PathBuf::from("/private/tmp")]);
    }

    /// Nothing after the first `--` is the wrapper's: a prompt that looks like an option stays the prompt.
    #[test]
    fn nothing_past_the_double_dash_is_read() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let args = words(&["--allow", "/x", "--allow", "/y", "--", "claude", "--allow /z", "--", "-f", "/evil"]);
        assert_eq!(command_after_grants(&args).unwrap(), &args[5..]);
        assert!(command_after_grants(&words(&["claude"])).is_err(), "the launcher's own -- is required");
        assert!(command_after_grants(&words(&["--allow", "/x"])).is_err());
        assert!(command_after_grants(&words(&["--"])).is_err());
    }

    #[test]
    fn the_right_variables_are_cleared() {
        for name in ["GH_TOKEN", "GITHUB_TOKEN", "TMUX", "SSH_AUTH_SOCK", "GIT_ASKPASS", "GIT_CONFIG_COUNT", "GIT_CONFIG_KEY_0", "NPM_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "SOME_API_KEY"] {
            assert!(cleared(name), "{name}");
        }
        for name in ["LEGENDA_DIR", "PATH", "HOME", "OTTO_RUN_ID", "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR", "TMUX_TMPDIR"] {
            assert!(!cleared(name), "{name}");
        }
    }

    #[test]
    fn a_bad_keychain_item_says_so_without_repeating_it() {
        for bad in ["not json at all SECRET", "{\"other\":\"SECRET\"}", "{\"claudeAiOauth\":{\"accessToken\":\"\"}}"] {
            assert!(!token_from(bad.as_bytes()).unwrap_err().contains("SECRET"));
        }
        assert_eq!(token_from(br#"{"claudeAiOauth":{"accessToken":"t"}}"#).unwrap(), "t");
    }

    /// The wrapper goes in where no wake can write, and an install says the same thing again harmlessly.
    #[test]
    fn the_wrapper_only_execs_and_its_directories_are_there() {
        let _h = crate::paths::test_support::TempHome::new();
        let path = ensure_wrapper(Path::new("/o'tto/bin/otto")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"exec '/o'\''tto/bin/otto' sandbox wrap "$@""#), "{text}");
        assert!(dir().join("runs").is_dir() && dir().join("profiles").is_dir());
        ensure_wrapper(Path::new("/o'tto/bin/otto")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}
