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

use crate::config::LauncherDef;
use crate::error::OttoError;
use crate::state::RunState;
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

/// In a run's own Claude folder (`paths::run_claude_dir`), links to the person's own in `~/.claude`, which
/// the wrapper makes before each wake. Read-only from inside: no wake can replace one, add one that isn't
/// there, or write through one, since what they say runs code, or steers claude, in every later wake.
const CLAUDE_LINKED: &[&str] = &["settings.json", "CLAUDE.md", "skills", "commands", "agents", "plugins", "hooks"];
/// Closed in that folder too, though nothing of the person's is linked there: settings by another name.
const CLAUDE_UNLINKED: &[&str] = &["settings.local.json"];

/// The claude session a wrapper was started for, as otto tells it: a wake's or a chat's `--session-id`. Passed
/// this way because nothing past the launcher's `--` is the wrapper's to read. It names the session's profile,
/// as a wake and a chat of the same run can start together.
pub const SESSION_ENV: &str = "OTTO_SESSION_ID";

/// A session id as otto makes them (`transcript::session_id`): a lowercase UUID, so nothing else can reach a profile.
fn is_session_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12]) && groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// Shared caches still writable from inside: macOS frameworks keep theirs here (`~/Library/Caches/<bundle id>`).
/// The tools that build code from what they cache are pointed elsewhere by `CACHE_ENV`.
const CACHE_WRITES: &[&str] = &["Library/Caches"];

/// Each sandboxed run's caches of its own, `$OTTO_HOME/sandbox/caches/<id>/<dir>`, by the variable that moves
/// each tool there. The shared ones (`~/.cargo/registry` and `git`, `~/.npm`, `~/.bun/install/cache`, `~/.cache`)
/// are read-only from inside: an unsandboxed build later runs what's in them, as a changed `build.rs` in the
/// registry did. Nothing outside the sandbox reads these, and later wakes of the same run reuse them.
///
/// The trade-off: a run's first build fetches every crate and package again, since its caches start empty, and
/// `--offline` fails until they're full. `CARGO_HOME` moving also moves cargo's `bin` and its own `config.toml`;
/// cargo still finds the toolchain (rustup keeps it under `RUSTUP_HOME`, not `CARGO_HOME`) and still reads
/// `~/.cargo/config.toml` for a project under the home directory, by walking up from where it builds.
const CACHE_ENV: &[(&str, &str)] = &[
    ("CARGO_HOME", "cargo"),
    ("npm_config_cache", "npm"),
    ("BUN_INSTALL_CACHE_DIR", "bun"),
    ("UV_CACHE_DIR", "uv"),
    ("PIP_CACHE_DIR", "pip"),
    ("GOCACHE", "go-build"),
    ("XDG_CACHE_HOME", "xdg"),
];

/// Shared caches whose contents a shell runs, closed to writes whatever a record says.
const SOURCED_CACHES: &[&str] = &[".cache/oh-my-zsh"];

/// Where one run's own caches are (`CACHE_ENV`).
fn caches_of(otto_home: &Path, run_id: &str) -> PathBuf {
    otto_home.join("sandbox/caches").join(run_id)
}

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
    // The person's clipboard, both ways: `pbpaste` hands out whatever they last copied, and `pbcopy` swaps in
    // what they paste next. Every pasteboard, the general one and `-pboard find` alike, is behind this one.
    "com.apple.pasteboard.1",
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
    /// The launcher this run must be under (`disagreement`).
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
    /// The wake's own terminal (`own_terminal`): the one terminal device it may open by name. None with no terminal.
    pub terminal: Option<PathBuf>,
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
            terminal: own_terminal(),
        }
    }
}

/// The terminal the wrapper was started on, learned from outside: its stdin's, stdout's or stderr's. Under tmux
/// that's the pane's pty. Only a pty slave's name (`/dev/ttysN`) is taken, so nothing else can be passed off as one.
fn own_terminal() -> Option<PathBuf> {
    (0..3).find_map(|fd| {
        let mut buf = [0 as libc::c_char; 128];
        // SAFETY: `buf` is as long as said, and `ttyname_r` writes a NUL-terminated name within it or fails.
        if unsafe { libc::ttyname_r(fd, buf.as_mut_ptr(), buf.len()) } != 0 {
            return None;
        }
        // SAFETY: on success the buffer holds a NUL-terminated string.
        let name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_str().ok()?.to_string();
        is_pty_slave(&name).then(|| PathBuf::from(name))
    })
}

fn is_pty_slave(name: &str) -> bool {
    name.strip_prefix("/dev/ttys").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
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

/// A file name as a regex matching it in any case, since the disk doesn't tell `Skills` from `skills`.
fn any_case(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            c if c.is_ascii_alphabetic() => format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase()),
            '.' => "\\.".to_string(),
            c => c.to_string(),
        })
        .collect()
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

    p.note("Open to every wake: devices, temporary files, macOS's caches.");
    let mut open = vec![
        "(literal \"/dev/null\")".to_string(),
        "(literal \"/dev/zero\")".to_string(),
        "(literal \"/dev/random\")".to_string(),
        "(literal \"/dev/urandom\")".to_string(),
        "(literal \"/dev/tty\")".to_string(),
        "(literal \"/dev/dtracehelper\")".to_string(),
        "(regex #\"^/dev/fd/\")".to_string(),
        // New ptys (claude's Bash tool, `script`, test suites): opening `/dev/ptmx` gives this process a sandbox
        // extension for the one `/dev/ttysN` it got back, and only that one opens by this rule.
        "(literal \"/dev/ptmx\")".to_string(),
        "(require-all (regex #\"^/dev/ttys[0-9]+$\") (extension \"com.apple.sandbox.pty\"))".to_string(),
    ];
    // The person's other terminals stay closed: a write to one puts text or escape sequences on a screen
    // they're typing in. The wake's own is open, by name, and only if it has one.
    open.extend(machine.terminal.iter().filter(|t| is_pty_slave(&t.to_string_lossy())).map(|t| literal(t)));
    let mut temp: Vec<PathBuf> = machine.temp.iter().map(|t| real(t.clone())).collect();
    temp.sort();
    temp.dedup();
    open.extend(temp.iter().map(|t| subpath(t)));
    open.extend(CACHE_WRITES.iter().map(|d| subpath(&in_home(d))));
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
    p.note("Its own caches, where the wrapper points cargo, npm, bun, uv and the rest. Only this run's.");
    p.open("file-write*", &[subpath(&caches_of(&otto_real, run_id))]);
    p.note("Its own Claude folder, claude's CLAUDE_CONFIG_DIR: what claude writes as it works lands here. Only this run's.");
    let own_claude = otto_real.join("sandbox/claude").join(run_id);
    p.open("file-write*", &[subpath(&own_claude)]);
    p.note("In it, the links to the person's settings, CLAUDE.md, skills, commands, agents, plugins and hooks: none replaced, added or written to, in any case. Nor the folder moved.");
    // The id is the folder's own name, as the wrapper made it, so the kernel reports it as written here.
    let names: Vec<String> = CLAUDE_LINKED.iter().chain(CLAUDE_UNLINKED).map(|n| any_case(n)).collect();
    p.close("file-write*", &[format!("(require-all {} (regex #\"/sandbox/claude/{run_id}/({})(/|$)\"))", subpath(&own_claude), names.join("|"))]);
    p.close("file-write-unlink", &[literal(&own_claude)]);

    p.note("The real tmux server's directory: a file there is one the person's tmux finds. And caches a shell sources.");
    let mut shared = vec!["(regex #\"^/private/tmp/tmux-[0-9]+(/|$)\")".to_string()];
    shared.extend(SOURCED_CACHES.iter().map(|d| subpath(&in_home(d))));
    p.close("file-write*", &shared);

    p.note("The person's own Claude folder, whatever the record says: shell snapshots and session-env that their claude sources, memory and settings that steer it.");
    p.close("file-write*", &[subpath(&in_home(".claude")), literal(&in_home(".claude.json"))]);

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

fn record_path(run_id: &str) -> PathBuf {
    dir().join("runs").join(format!("{run_id}.json"))
}

/// Read the record for this run. Missing is a refusal too: being under this launcher with nothing it
/// could open isn't a sandbox.
pub fn read_record(run_id: &str) -> Result<Record, String> {
    let path = record_path(run_id);
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

/// The token on `TOKEN_FD`, for reading once and then gone. A pipe holds far more than a token, so the
/// write can't block, and nothing with a name on disk ever held it.
fn hand_in(token: &str) -> Result<(), String> {
    let mut fds = [0i32; 2];
    let failed = |what: &str| format!("couldn't {what} for claude's token: {}", std::io::Error::last_os_error());
    // SAFETY: `fds` has room for the two descriptors, and these are descriptors this process just made.
    // `pipe` leaves both inheritable, which the read end needs to be and the write end stops being here.
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return Err(failed("make a pipe"));
        }
        let [mut read_end, mut write_end] = fds;
        // The write end must not land on the number the read end is going to.
        if write_end == TOKEN_FD {
            write_end = libc::dup2(write_end, TOKEN_FD + 1);
            if write_end < 0 {
                return Err(failed("move a descriptor"));
            }
            libc::close(TOKEN_FD);
        }
        let bytes = token.as_bytes();
        if libc::write(write_end, bytes.as_ptr().cast(), bytes.len()) != bytes.len() as isize {
            return Err(failed("write it"));
        }
        libc::close(write_end);
        if read_end != TOKEN_FD {
            if libc::dup2(read_end, TOKEN_FD) != TOKEN_FD {
                return Err(failed("place it"));
            }
            libc::close(read_end);
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

/// One per session, as a wake and a chat of the same run can start together and each has its own. `sandbox-exec`
/// reads it once, as it starts, so the run's older ones are cleared away.
fn write_profile(run_id: &str, session: &str, text: &str) -> Result<PathBuf, String> {
    let dir = dir().join("profiles");
    let failed = |e: std::io::Error| format!("couldn't write a profile in {}: {e}", dir.display());
    std::fs::create_dir_all(&dir).map_err(failed)?;
    let ours = format!("{run_id}.{session}.sb");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > day));
            if old && name != ours && name.starts_with(&format!("{run_id}.")) && name.ends_with(".sb") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let path = dir.join(&ours);
    let temp = dir.join(format!("{ours}.{}", std::process::id()));
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

/// The gate a refused run waits on.
const REFUSED_GATE: &str = "sandbox-record";

/// A value out of `run.json` as text a person reads: quoted, so a wake can't write lines into the question
/// it's asked about (or an option under its `## Options`), and short.
fn shown(value: &str) -> String {
    let short: String = value.chars().take(200).collect();
    format!("{short:?}")
}

/// What's different between a run and its record, on the state its command line is built from. Each field of
/// `run.json` that reaches that command line, and why it's here or isn't:
///
/// - `id`: compared. `OTTO_RUN_ID` is made from it and is how the wrapper finds a record, so it would be a way
///   to another run's.
/// - `launcher.kind`, `launcher.workdir`, `launcher.repos`: compared, resolved as they would be used.
/// - `permission.mode`, `permission.allowedTools`, `permission.disallowedTools`: not compared. They go after the
///   wrapper's `--`, so whatever they say, even a flag, reaches only a process already inside the profile. `mode`
///   is an enum too.
/// - `launcher.detach`: not compared. An enum, choosing how `otto wake` starts, and that's outside the sandbox and
///   starts its child under the wrapper either way.
/// - The prompt is made from much of the rest, and is one argument after the same `--`.
fn differences(id: &str, state: &RunState, launcher: Result<&LauncherDef, &OttoError>, record: &Record, home: &Path) -> Vec<String> {
    let mut found = Vec::new();
    if state.id != id {
        found.push(format!("its run.json says it is run {}, and it is run {id}", shown(&state.id)));
    }
    let wanted = crate::config::launcher(&record.launcher);
    match (launcher, &wanted) {
        (Ok(have), Ok(want)) if have.name == want.name => {}
        (Ok(have), Ok(want)) => found.push(format!("its run.json names the launcher {}, and the record names {}", shown(&have.name), shown(&want.name))),
        (Err(_), _) => found.push(format!("its run.json names the launcher {}, which doesn't exist", shown(&state.launcher.kind))),
        (_, Err(e)) => found.push(format!("the record names the launcher {}: {e}", shown(&record.launcher))),
    }
    let place = |path: &Path| normalize(&path.display().to_string(), home).unwrap_or_else(|_| path.to_path_buf());
    let workdir = crate::config::workdir_for(state).map(|dir| place(&dir));
    let recorded = record.workdir.as_deref().map(|dir| normalize(dir, home));
    match (&workdir, &recorded) {
        (None, None) => {}
        (Some(have), Some(Ok(want))) if have == want => {}
        (_, Some(Err(e))) => found.push(format!("the record's workdir: {e}")),
        (have, want) => {
            let say = |dir: Option<&PathBuf>| dir.map_or("none".to_string(), |d| shown(&d.display().to_string()));
            found.push(format!("it would start in {}, and the record says {}", say(have.as_ref()), say(want.as_ref().and_then(|w| w.as_ref().ok()))));
        }
    }
    let mut listed = Vec::new();
    for path in record.read.iter().chain(&record.write) {
        match normalize(path, home) {
            Ok(path) => listed.push(path),
            Err(e) => found.push(format!("the record lists {e}")),
        }
    }
    let unlisted: Vec<&String> = state.launcher.repos.iter().filter(|repo| !normalize(repo, home).is_ok_and(|repo| listed.contains(&repo))).collect();
    for repo in unlisted.iter().take(10) {
        found.push(format!("its run.json lists the repo {}, and the record doesn't", shown(repo)));
    }
    if unlisted.len() > 10 {
        found.push(format!("and {} more repos the record doesn't list", unlisted.len() - 10));
    }
    found
}

/// Why this run must not start, if it has a sandbox record and disagrees with it. A wake can write its
/// `run.json` and can't write the record: otherwise its next wake could leave the sandbox. No record means
/// nothing to compare, as for every run that isn't sandboxed; one that can't be read is a refusal, never that.
/// `dir` is the run's directory, the one thing about its identity that `run.json` doesn't say.
pub fn disagreement(dir: &Path, state: &RunState, launcher: Result<&LauncherDef, &OttoError>) -> Option<String> {
    let id = dir.file_name()?.to_string_lossy().to_string();
    // `symlink_metadata`: a dangling link is a record that can't be read, not the absence of one.
    if record_path(&id).symlink_metadata().is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return None;
    }
    match read_record(&id) {
        Err(why) => Some(why),
        Ok(record) => {
            let found = differences(&id, state, launcher, &record, &crate::paths::home_dir());
            (!found.is_empty()).then(|| found.join("; "))
        }
    }
}

fn refused_question(id: &str, why: &str) -> String {
    let differences: String = why.split("; ").map(|d| format!("- {d}\n")).collect();
    let short = crate::paths::short_id(id);
    format!(
        "Run `{id}` has a sandbox record, and what its `run.json` says doesn't match it, so otto started nothing: \
         no wake, no chat, nothing spent.\n\n{differences}\n\
         A wake can write its own `run.json` and can't write the record, so this is what a wake trying to widen its next \
         sandbox looks like. It's also what a person editing `run.json` by hand looks like. Worth reading before \
         undoing: `otto logs {short}`, and {record}.\n\n\
         To go on, make them agree: put `run.json` back, or change the record if the change was yours. To take the run \
         out of the sandbox, delete the record and put the launcher back to one that isn't the wrapper. To end it, `otto stop {short}`.\n\n\
         ## Options\n\n\
         - **Fixed** *(default)* — they agree now. It's compared again before the next wake, and refused again if they don't.\n",
        record = record_path(id).display(),
    )
}

/// Refuse to start a run that `disagreement` found against, and leave its trail: the journal says what differed,
/// and the run is blocked with a question, which poke leaves alone rather than retrying as a crash. A dry run
/// starts nothing and records nothing. If the trail can't be written the refusal still stands.
pub fn refuse(id: &str, why: &str, dry_run: bool) -> OttoError {
    let refusal = OttoError::conflict(format!("{id}: not starting anything, it disagrees with its sandbox record: {why}"));
    if dry_run {
        return refusal;
    }
    let trail = crate::state::transaction(id, |path, state| {
        state.block(crate::state::BlockedCause::Instructions, Some(why.to_string()));
        if let Some(blocked) = state.blocked.as_mut() {
            blocked.sandbox = true;
        }
        state.spawn_attempts = 0;
        crate::event::record(path, &crate::event::Event::SandboxRefused { why: why.to_string() })?;
        Ok(state.gate.is_none())
    });
    let asked = match trail {
        Ok(true) => crate::state::ops::open_gate(id, REFUSED_GATE, &refused_question(id, why), None).map(|_| ()),
        Ok(false) => Ok(()),
        Err(e) => Err(e),
    };
    match asked {
        Ok(()) => refusal,
        Err(e) => OttoError::conflict(format!("{}, and recording that failed: {e}", refusal.message)),
    }
}

/// A run refused for disagreeing, which agrees now, goes back to how it was: `otto wake` after a person fixed
/// it, with no question left for that wake to ask again. Only a refusal's own question is closed.
pub fn lift_refusal(id: &str, state: &RunState) -> Result<(), OttoError> {
    use crate::state::Status;
    const AGREES: &str = "(otto) run.json and the sandbox record agree now, or the record is gone.";
    if state.gate.as_ref().is_some_and(|g| g.slug == REFUSED_GATE) {
        return crate::state::ops::close_gate(id, Some(AGREES), Status::Sleeping);
    }
    crate::state::transaction(id, |path, state| {
        state.status = if state.gate.is_some() { Status::AwaitingHuman } else { Status::Sleeping };
        crate::event::record(
            path,
            &crate::event::Event::StatusChanged {
                from: "blocked".to_string(),
                to: crate::state::commands::status_str(state.status).to_string(),
                reason: Some(AGREES.to_string()),
                because: None,
                left_open: vec![],
            },
        )
    })
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
    let session = std::env::var(SESSION_ENV).unwrap_or_default();
    match prepare(&id, &session, args) {
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
    /// This run's own caches (`CACHE_ENV`).
    caches: PathBuf,
    /// This run's own Claude folder, claude's `CLAUDE_CONFIG_DIR` (`lay_out_claude`).
    claude: PathBuf,
}

/// `session` is what otto said in `OTTO_SESSION_ID`, and it names the profile. Without one, or with one otto
/// wouldn't make, that's a refusal like any other.
fn prepare(id: &str, session: &str, args: &[String]) -> Result<Prepared, String> {
    let command = command_after_grants(args)?.to_vec();
    if session.is_empty() {
        return Err(format!("{SESSION_ENV} isn't set, so there's no telling which claude session this profile is for"));
    }
    if !is_session_id(session) {
        return Err(format!("the session id {} isn't one otto makes", shown(session)));
    }
    let id = crate::paths::resolve_run_id(id).map_err(|e| e.to_string())?;
    let record = read_record(&id)?;
    let machine = Machine::here();
    let text = profile(&id, &record, &machine)?;
    let path = write_profile(&id, session, &text)?;
    try_profile(&path)?;
    // From outside: inside, only what's below this is writable.
    let caches = caches_of(&crate::paths::otto_home(), &id);
    for (_, sub) in CACHE_ENV {
        std::fs::create_dir_all(caches.join(sub)).map_err(|e| format!("couldn't make the run's caches in {}: {e}", caches.display()))?;
    }
    let claude = lay_out_claude(&id, &record, &machine.home)?;
    let token = read_token()?;
    hand_in(&token)?;
    Ok(Prepared { profile: path, command, caches, claude })
}

/// What `.claude.json` starts as in a run's own Claude folder: enough that claude asks nothing as it starts, and
/// the run's folders trusted. Nothing of the person's: no MCP servers, no account, no sign-in.
fn seeded_claude_json(record: &Record, home: &Path) -> String {
    let mut trusted = serde_json::Map::new();
    for dir in record.workdir.iter().chain(&record.write) {
        if let Ok(dir) = normalize(dir, home) {
            trusted.insert(dir.display().to_string(), serde_json::json!({"hasTrustDialogAccepted": true}));
        }
    }
    serde_json::json!({
        "hasCompletedOnboarding": true,
        "bypassPermissionsModeAccepted": true,
        "skipDangerousModePermissionPrompt": true,
        "projects": trusted,
    })
    .to_string()
}

/// A run's own Claude folder, laid out from outside before each wake or chat: made if it isn't there, a link to
/// each of `CLAUDE_LINKED` the person has in `~/.claude` and nothing under the other read-only names, and
/// `.claude.json` seeded afresh. A wake can write this folder, so nothing here follows a link it may have left:
/// every step is relative to the folder opened without following one, and a link standing where the folder
/// should be is removed, not entered.
fn lay_out_claude(id: &str, record: &Record, home: &Path) -> Result<PathBuf, String> {
    let dir = crate::paths::run_claude_dir(id);
    let failed = |what: &str, e: std::io::Error| format!("couldn't {what} in the run's Claude folder {}: {e}", dir.display());
    let parent = dir.parent().expect("a run's Claude folder has a parent");
    // Under `sandbox/`, which no wake can write.
    std::fs::create_dir_all(parent).map_err(|e| failed("make the folder above", e))?;
    let above = nofollow::Dir::open(parent).map_err(|e| failed("open the folder above", e))?;
    let folder = above.dir(id).map_err(|e| failed("open the folder", e))?;
    let ours = home.join(".claude");
    for name in CLAUDE_LINKED {
        let target = ours.join(name);
        let want = target.exists().then_some(target.as_path());
        folder.link_only(name, want).map_err(|e| failed(&format!("link {name}"), e))?;
    }
    for name in CLAUDE_UNLINKED {
        folder.link_only(name, None).map_err(|e| failed(&format!("clear {name}"), e))?;
    }
    folder.replace(".claude.json", seeded_claude_json(record, home).as_bytes()).map_err(|e| failed("seed .claude.json", e))?;
    Ok(dir)
}

/// Steps in a directory a wake can write, from outside it, that never follow a link: `*at` calls on a directory
/// opened with `O_NOFOLLOW`.
mod nofollow {
    use std::ffi::CString;
    use std::io::{Error, ErrorKind, Result, Write as _};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub struct Dir(OwnedFd);

    fn c(name: &str) -> Result<CString> {
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(Error::new(ErrorKind::InvalidInput, format!("{name:?} isn't a name in a directory")));
        }
        CString::new(name).map_err(|_| Error::new(ErrorKind::InvalidInput, "a NUL in a name"))
    }

    fn checked(rc: libc::c_int) -> Result<libc::c_int> {
        if rc < 0 {
            Err(Error::last_os_error())
        } else {
            Ok(rc)
        }
    }

    const DIR_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    impl Dir {
        /// A directory no wake can write, by its path.
        pub fn open(path: &Path) -> Result<Dir> {
            let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| Error::new(ErrorKind::InvalidInput, "a NUL in a path"))?;
            // SAFETY: a valid C string, and the descriptor returned is this process's to own.
            let fd = checked(unsafe { libc::open(path.as_ptr(), DIR_FLAGS) })?;
            Ok(Dir(unsafe { OwnedFd::from_raw_fd(fd) }))
        }

        /// `name` in here as a directory, made if it isn't there. Anything else standing there, a link
        /// included, is removed first, never followed.
        pub fn dir(&self, name: &str) -> Result<Dir> {
            let n = c(name)?;
            for _ in 0..2 {
                // SAFETY: a descriptor this owns, and a valid C string.
                match checked(unsafe { libc::mkdirat(self.0.as_raw_fd(), n.as_ptr(), 0o700) }) {
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
                // SAFETY: as above; the descriptor returned is this process's to own.
                match checked(unsafe { libc::openat(self.0.as_raw_fd(), n.as_ptr(), DIR_FLAGS) }) {
                    Ok(fd) => return Ok(Dir(unsafe { OwnedFd::from_raw_fd(fd) })),
                    // A link (ELOOP) or a file (ENOTDIR) where the directory should be.
                    Err(e) if matches!(e.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENOTDIR)) => self.remove(name)?,
                    Err(e) => return Err(e),
                }
            }
            Err(Error::other(format!("{name} kept turning into something other than a directory")))
        }

        /// What `name` is, without following it: `None` if nothing's there.
        fn kind(&self, n: &CString) -> Result<Option<libc::mode_t>> {
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: a descriptor this owns, a valid C string, and room for the answer.
            match checked(unsafe { libc::fstatat(self.0.as_raw_fd(), n.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) }) {
                Ok(_) => Ok(Some(st.st_mode & libc::S_IFMT)),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        }

        /// Whatever is at `name`, gone: a link or a file, or an empty directory. A directory with something in
        /// it stays, and that's an error, as only the wrapper ever puts one at a name it removes.
        pub fn remove(&self, name: &str) -> Result<()> {
            let n = c(name)?;
            let flags = match self.kind(&n)? {
                None => return Ok(()),
                Some(libc::S_IFDIR) => libc::AT_REMOVEDIR,
                Some(_) => 0,
            };
            // SAFETY: a descriptor this owns, and a valid C string.
            match checked(unsafe { libc::unlinkat(self.0.as_raw_fd(), n.as_ptr(), flags) }) {
                Ok(_) => Ok(()),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        }

        /// What the link at `name` points to, if it is a link.
        fn link_target(&self, n: &CString) -> Option<Vec<u8>> {
            let mut buf = vec![0u8; libc::PATH_MAX as usize];
            // SAFETY: a descriptor this owns, a valid C string, and a buffer of the length given.
            let len = unsafe { libc::readlinkat(self.0.as_raw_fd(), n.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
            (len >= 0).then(|| {
                buf.truncate(len as usize);
                buf
            })
        }

        /// `name` a link to `target`, and nothing else; with no target, nothing at all.
        pub fn link_only(&self, name: &str, target: Option<&Path>) -> Result<()> {
            let n = c(name)?;
            if let Some(target) = target {
                if self.link_target(&n).as_deref() == Some(target.as_os_str().as_bytes()) {
                    return Ok(());
                }
            }
            self.remove(name)?;
            let Some(target) = target else { return Ok(()) };
            let t = CString::new(target.as_os_str().as_bytes()).map_err(|_| Error::new(ErrorKind::InvalidInput, "a NUL in a path"))?;
            // SAFETY: valid C strings and a descriptor this owns.
            checked(unsafe { libc::symlinkat(t.as_ptr(), self.0.as_raw_fd(), n.as_ptr()) }).map(|_| ())
        }

        /// `name` replaced by a new file holding `bytes`: written beside it under a name made here, then
        /// renamed over it, which replaces a link rather than writing through one.
        pub fn replace(&self, name: &str, bytes: &[u8]) -> Result<()> {
            let n = c(name)?;
            let temp = format!(".{name}.otto-{}", std::process::id());
            self.remove(&temp)?;
            let t = c(&temp)?;
            let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            // SAFETY: a descriptor this owns and a valid C string; the descriptor returned is this process's to own.
            let fd = checked(unsafe { libc::openat(self.0.as_raw_fd(), t.as_ptr(), flags, 0o600 as libc::c_uint) })?;
            let mut file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
            file.write_all(bytes)?;
            drop(file);
            // SAFETY: a descriptor this owns, twice, and valid C strings.
            checked(unsafe { libc::renameat(self.0.as_raw_fd(), t.as_ptr(), self.0.as_raw_fd(), n.as_ptr()) }).map(|_| ())
        }
    }
}

impl Prepared {
    fn exec(self) -> std::io::Error {
        use std::os::unix::process::CommandExt;
        self.sandboxed().exec()
    }

    fn sandboxed(&self) -> std::process::Command {
        let mut command = std::process::Command::new("/usr/bin/sandbox-exec");
        command.arg("-f").arg(&self.profile).args(&self.command);
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(cleared) {
                command.env_remove(name);
            }
        }
        command.env("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR", TOKEN_FD.to_string());
        command.env("CLAUDE_CONFIG_DIR", &self.claude);
        for (name, sub) in CACHE_ENV {
            command.env(name, self.caches.join(sub));
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Record {
        Record { launcher: "sandbox".into(), workdir: Some("/w/space".into()), read: vec!["/w/space/lib".into()], write: vec!["/w/space/repo".into()], mode: Mode::Enforce }
    }

    fn machine() -> Machine {
        Machine { home: "/u".into(), otto_home: "/u/.otto".into(), projects: vec!["/w/space".into()], temp: vec!["/private/tmp".into()], web_ports: vec![7878], terminal: None }
    }

    /// The `file-write*` rules that open something: every `(allow file-write*` block's lines.
    fn write_openings(text: &str) -> String {
        text.split("\n(").filter(|b| b.starts_with("allow file-write*")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn only_the_wakes_own_terminal_is_open() {
        let mine = Machine { terminal: Some("/dev/ttys007".into()), ..machine() };
        let text = profile("r1", &record(), &mine).unwrap();
        let open = write_openings(&text);
        assert!(open.contains(r#"(literal "/dev/ttys007")"#), "{text}");
        // No rule that would open another terminal by its name: only one handed out by /dev/ptmx to this process.
        for line in open.lines().filter(|l| l.contains("ttys")) {
            assert!(line.contains(r#"(literal "/dev/ttys007")"#) || line.contains(r#"(extension "com.apple.sandbox.pty")"#), "{line}");
        }
        assert!(!open.contains("|pty)"), "{text}");
        assert!(open.contains(r#"(literal "/dev/tty")"#), "{text}");
    }

    #[test]
    fn with_no_terminal_none_is_open() {
        let open = write_openings(&generated(&record()));
        for line in open.lines().filter(|l| l.contains("ttys")) {
            assert!(line.contains(r#"(require-all (regex #"^/dev/ttys[0-9]+$") (extension "com.apple.sandbox.pty"))"#), "{line}");
        }
    }

    #[test]
    fn only_a_pty_slave_is_taken_for_the_terminal() {
        assert!(is_pty_slave("/dev/ttys000") && is_pty_slave("/dev/ttys12"));
        for bad in ["/dev/ttys", "/dev/console", "/dev/tty", "/dev/ttys0\")(allow default", "/dev/ttysa", "/u/x"] {
            assert!(!is_pty_slave(bad), "{bad}");
            let text = profile("r1", &record(), &Machine { terminal: Some(bad.into()), ..machine() }).unwrap();
            assert!(!text.contains(&format!("(literal {})", quoted(Path::new(bad)))) || bad == "/dev/tty", "{bad}: {text}");
        }
    }

    #[test]
    fn the_clipboard_is_closed_both_ways() {
        let text = generated(&record());
        let services = text.split("\n(").find(|b| b.starts_with("deny mach-lookup")).unwrap_or_default();
        assert!(services.contains(r#"(global-name "com.apple.pasteboard.1")"#), "{text}");
    }

    /// The session the wrapper was told about, as otto passes it.
    const SESSION: &str = "0f9a1c2b-3d4e-4f56-8789-abcdef012345";

    fn generated(record: &Record) -> String {
        profile("r1", record, &machine()).unwrap()
    }

    /// In Seatbelt the last matching rule wins, so a grant has to come before what no record may open.
    /// A record listing a directory above `.git/hooks` opens the directory, and not the hooks.
    /// A run and a record that agree, on a scratch home, with this launcher configured.
    fn agreeing(h: &crate::paths::test_support::TempHome) -> (RunState, Record, PathBuf) {
        let root = h.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("work/repo")).unwrap();
        std::fs::write(crate::paths::otto_home().join("config.json"), r#"{"launchers":[{"name":"sandbox","command":"/w"},{"name":"nono (x)","command":"/n"}]}"#).unwrap();
        let mut state = crate::state::test_run_state("r1");
        state.launcher.kind = "sandbox".into();
        state.launcher.workdir = Some(root.join("work").display().to_string());
        state.launcher.repos = vec![root.join("work/repo").display().to_string()];
        let record = Record { launcher: "sandbox".into(), workdir: state.launcher.workdir.clone(), read: vec![], write: state.launcher.repos.clone(), mode: Mode::Enforce };
        (state, record, root)
    }

    fn differs(state: &RunState, record: &Record, root: &Path) -> Vec<String> {
        let resolved = crate::wake::launcher::resolve(state);
        differences("r1", state, resolved.as_ref(), record, root)
    }

    #[test]
    fn a_run_that_agrees_with_its_record_has_no_differences_however_its_paths_are_written() {
        let h = crate::paths::test_support::TempHome::new();
        let (mut state, record, root) = agreeing(&h);
        assert_eq!(differs(&state, &record, &root), Vec::<String>::new());
        state.launcher.kind = "SANDBOX".into();
        state.launcher.repos = vec![format!("{}/work/repo/", root.display()), "~/work/repo".into()];
        state.launcher.workdir = Some(format!("{}/work/", root.display()));
        std::os::unix::fs::symlink(root.join("work/repo"), root.join("link")).unwrap();
        state.launcher.repos.push(root.join("link").display().to_string());
        assert_eq!(differs(&state, &record, &root), Vec::<String>::new());
    }

    #[test]
    fn each_field_that_decides_a_sandbox_is_compared() {
        let h = crate::paths::test_support::TempHome::new();
        let (state, record, root) = agreeing(&h);
        let said = |change: &dyn Fn(&mut RunState), word: &str| {
            let mut changed = state.clone();
            change(&mut changed);
            let found = differs(&changed, &record, &root).join("; ");
            assert!(found.contains(word), "{word} in {found:?}");
        };
        said(&|s| s.launcher.kind = "claude".into(), "launcher");
        said(&|s| s.launcher.kind = "gone".into(), "doesn't exist");
        said(&|s| s.launcher.repos.push("~/.ssh".into()), ".ssh");
        said(&|s| s.launcher.repos.push("relative".into()), "relative");
        said(&|s| s.launcher.workdir = Some("/elsewhere".into()), "start in");
        said(&|s| s.launcher.workdir = None, "start in");
        said(&|s| s.id = "another".into(), "another");
    }

    /// A link to somewhere else is judged by where it goes, not by what it's called.
    #[test]
    fn a_repo_that_is_a_link_to_something_unlisted_is_not_listed() {
        let h = crate::paths::test_support::TempHome::new();
        let (mut state, mut record, root) = agreeing(&h);
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        std::os::unix::fs::symlink(root.join(".ssh"), root.join("work/innocent")).unwrap();
        state.launcher.repos = vec![root.join("work/innocent").display().to_string()];
        record.read = vec![];
        assert_eq!(differs(&state, &record, &root).len(), 1);
    }

    /// What a wake writes in `run.json` ends up in a question to a person, so it can't add lines to it.
    #[test]
    fn what_differed_can_not_write_lines_into_the_question() {
        let h = crate::paths::test_support::TempHome::new();
        let (mut state, record, root) = agreeing(&h);
        state.launcher.repos.push("/x\n\n## Options\n\n- **Approve**".into());
        for line in differs(&state, &record, &root) {
            assert!(!line.contains('\n'), "{line}");
        }
    }

    #[test]
    fn a_run_with_no_record_has_nothing_to_compare_and_a_bad_record_is_a_refusal() {
        let h = crate::paths::test_support::TempHome::new();
        let (state, _, _) = agreeing(&h);
        let run = crate::paths::runs_dir().join("r1");
        let resolved = crate::wake::launcher::resolve(&state);
        assert_eq!(disagreement(&run, &state, resolved.as_ref()), None);
        std::fs::create_dir_all(dir().join("runs")).unwrap();
        std::fs::write(record_path("r1"), "{ not json").unwrap();
        assert!(disagreement(&run, &state, resolved.as_ref()).unwrap().contains("can't be used"));
        std::fs::remove_file(record_path("r1")).unwrap();
        std::os::unix::fs::symlink("/no/such/file", record_path("r1")).unwrap();
        assert!(disagreement(&run, &state, resolved.as_ref()).is_some(), "a link to nothing isn't no record");
    }

    /// Everything in `permission` goes after the wrapper's `--`, so the wrapper never reads it as its own.
    #[test]
    fn permissions_and_the_prompt_only_ever_reach_what_is_inside() {
        let h = crate::paths::test_support::TempHome::new();
        let (mut state, _, _) = agreeing(&h);
        state.permission.allowed_tools = vec!["--settings=/tmp/evil.json".into()];
        state.permission.disallowed_tools = vec!["--".into()];
        let wrapper = LauncherDef { name: "sandbox".into(), command: "/w -- claude".into(), grant_flag: Some("--allow".into()) };
        let argv = crate::wake::launcher::argv(&state, &wrapper, "--allow /evil -- /bin/sh", "sid");
        let at = argv.iter().position(|w| w == "/w").unwrap();
        let command = command_after_grants(&argv[at + 1..]).unwrap();
        assert_eq!(command[0], "claude");
        for hostile in ["--settings=/tmp/evil.json", "--allow /evil -- /bin/sh"] {
            assert!(command.iter().any(|w| w == hostile), "{hostile} is inside");
        }
    }

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

    /// Plan table, "Tool caches that outside builds run": the shared caches aren't granted, and the run's own is.
    #[test]
    fn a_wake_writes_caches_of_its_own_and_not_the_shared_ones() {
        let text = generated(&record());
        for shared in [
            "(subpath \"/u/.cache\")",
            "(subpath \"/u/.cargo/registry\")",
            "(subpath \"/u/.cargo/git\")",
            "(subpath \"/u/.bun/install/cache\")",
            "(subpath \"/u/.npm/_npx\")",
            "(subpath \"/u/.npm/_cacache\")",
            "(literal \"/u/.cargo/.package-cache\")",
        ] {
            assert!(!text.contains(shared), "{shared} is granted: {text}");
        }
        let own = text.find("(allow file-write*\n  (subpath \"/u/.otto/sandbox/caches/r1\")").expect("its own caches are granted");
        assert!(own > text.find("(subpath \"/u/.otto/sandbox\")").unwrap(), "after otto's own closes, or it stays closed");
        assert!(!text.contains("/u/.otto/sandbox/caches\")"), "not every run's");
    }

    /// oh-my-zsh sources what's in its cache, and the real tmux server finds what's in its directory.
    #[test]
    fn a_sourced_cache_and_the_tmux_directory_can_not_be_written_whatever_the_record_says() {
        let mut r = record();
        r.write.push("/u/.cache".into());
        let text = generated(&r);
        let grant = text.find("(subpath \"/u/.cache\")").expect("the record's grant");
        let temp = text.find("(subpath \"/private/tmp\")").expect("temporary files are open");
        let closed = text.find("(deny file-write* (with message \"otto-run-r1\")\n  (regex #\"^/private/tmp/tmux-[0-9]+(/|$)\")").expect("tmux's directory is closed");
        assert!(closed > grant && closed > temp, "closed after what opens it");
        let omz = text.find("(subpath \"/u/.cache/oh-my-zsh\")").expect("oh-my-zsh's cache is closed");
        assert!(omz > closed && omz > grant);
    }

    /// Plan table, "Claude's per-session files": another claude session sources its shell snapshot and its
    /// `session-env`, and the person's own reads its memory and settings, so nothing in `~/.claude` is written
    /// from inside, whatever the record says. Claude writes in the run's own folder instead.
    #[test]
    fn the_persons_claude_folder_is_closed_and_the_runs_own_is_open() {
        let mut r = record();
        r.write.push("/u/.claude".into());
        r.write.push("/u".into());
        let text = generated(&r);
        let grant = text.find("(subpath \"/u/.claude\")").expect("the record's grant");
        let closed = text.find("(deny file-write* (with message \"otto-run-r1\")\n  (subpath \"/u/.claude\")\n  (literal \"/u/.claude.json\")").expect("~/.claude is closed");
        assert!(closed > grant, "closed after what opens it");
        assert!(!text[closed..].contains("(allow file-write*\n  (subpath \"/u/.claude/"), "nothing in it opens again after: {text}");
        assert!(!text.contains("/session-env") && !text.contains("/shell-snapshots"), "no exception for any of claude's own: {text}");

        let own = text.find("(allow file-write*\n  (subpath \"/u/.otto/sandbox/claude/r1\")").expect("its own folder is open");
        assert!(own > text.find("(subpath \"/u/.otto/sandbox\")").unwrap(), "after otto's own closes, or it stays closed");
        assert!(!text.contains("/u/.otto/sandbox/claude\")"), "not every run's");
        let links = text.find("(deny file-write* (with message \"otto-run-r1\")\n  (require-all (subpath \"/u/.otto/sandbox/claude/r1\") (regex #\"/sandbox/claude/r1/(").expect("its links are closed");
        assert!(links > own, "closed after the folder opens");
        for name in ["[sS][eE][tT][tT][iI][nN][gG][sS]\\.[jJ][sS][oO][nN]", "[cC][lL][aA][uU][dD][eE]\\.[mM][dD]", "[sS][kK][iI][lL][lL][sS]", "[hH][oO][oO][kK][sS]", "[pP][lL][uU][gG][iI][nN][sS]", "[aA][gG][eE][nN][tT][sS]", "[cC][oO][mM][mM][aA][nN][dD][sS]"] {
            assert!(text[links..].lines().nth(1).unwrap().contains(name), "{name}: {text}");
        }
        assert!(text.contains("(deny file-write-unlink (with message \"otto-run-r1\")\n  (literal \"/u/.otto/sandbox/claude/r1\")"), "the folder can't be moved: {text}");
    }

    #[test]
    fn a_name_matches_in_any_case() {
        assert_eq!(any_case("a.B-1"), "[aA]\\.[bB]-1");
    }

    /// The folder is laid out from outside, in a place the wake could write: links the person has, nothing at
    /// the read-only names they don't, `.claude.json` seeded afresh, and nothing a wake left there followed.
    #[test]
    fn the_runs_claude_folder_is_laid_out_without_following_a_link() {
        let h = crate::paths::test_support::TempHome::new();
        let root = h.path().canonicalize().unwrap();
        let home = root.join("person");
        std::fs::create_dir_all(home.join(".claude/skills/s")).unwrap();
        std::fs::write(home.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(home.join(".claude/.credentials.json"), "secret").unwrap();
        std::fs::write(home.join(".claude.json"), r#"{"mcpServers":{"x":{}}}"#).unwrap();
        let record = Record { launcher: "s".into(), workdir: Some(root.join("work").display().to_string()), read: vec![], write: vec![root.join("repo").display().to_string()], mode: Mode::Enforce };
        let dir = crate::paths::run_claude_dir("r1");

        // What a wake might have left: the folder a link to somewhere else.
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        assert_eq!(lay_out_claude("r1", &record, &home).unwrap(), dir);
        assert!(!dir.symlink_metadata().unwrap().file_type().is_symlink(), "the link was removed, not entered");
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0, "nothing was written where the link went");
        assert_eq!(std::fs::read_link(dir.join("skills")).unwrap(), home.join(".claude/skills"));
        assert_eq!(std::fs::read_link(dir.join("settings.json")).unwrap(), home.join(".claude/settings.json"));
        for absent in ["CLAUDE.md", "hooks", "agents", "commands", "plugins", "settings.local.json", ".credentials.json"] {
            assert!(dir.join(absent).symlink_metadata().is_err(), "{absent}");
        }
        let seeded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(seeded["hasCompletedOnboarding"], true);
        assert_eq!(seeded["projects"][root.join("repo").display().to_string()]["hasTrustDialogAccepted"], true);
        assert_eq!(seeded["projects"][root.join("work").display().to_string()]["hasTrustDialogAccepted"], true);
        assert!(seeded.get("mcpServers").is_none(), "nothing of the person's is copied");

        // A wake's leavings inside: .claude.json a link out, a hooks folder, settings.json pointing elsewhere.
        let victim = root.join("victim");
        std::fs::write(&victim, "untouched").unwrap();
        std::fs::remove_file(dir.join(".claude.json")).unwrap();
        std::os::unix::fs::symlink(&victim, dir.join(".claude.json")).unwrap();
        std::fs::create_dir(dir.join("hooks")).unwrap();
        std::fs::remove_file(dir.join("settings.json")).unwrap();
        std::os::unix::fs::symlink(&victim, dir.join("settings.json")).unwrap();
        std::fs::write(dir.join("history.jsonl"), "kept\n").unwrap();
        lay_out_claude("r1", &record, &home).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
        assert!(!dir.join(".claude.json").symlink_metadata().unwrap().file_type().is_symlink());
        assert!(dir.join("hooks").symlink_metadata().is_err());
        assert_eq!(std::fs::read_link(dir.join("settings.json")).unwrap(), home.join(".claude/settings.json"));
        assert_eq!(std::fs::read_to_string(dir.join("history.jsonl")).unwrap(), "kept\n", "what claude wrote stays");

        // The person makes a hooks folder: the next layout links it.
        std::fs::create_dir(home.join(".claude/hooks")).unwrap();
        lay_out_claude("r1", &record, &home).unwrap();
        assert_eq!(std::fs::read_link(dir.join("hooks")).unwrap(), home.join(".claude/hooks"));
    }

    /// A session id names a profile file, so only the shape otto makes gets that far.
    #[test]
    fn only_a_session_id_otto_makes_reaches_a_profile() {
        let made = crate::wake::transcript::session_id("r1", 1, "2026-11-07T00:00:00Z");
        assert!(is_session_id(&made) && is_session_id(SESSION), "{made}");
        for bad in ["", "..", "../11111111-2222-4333-8444-555555555555", "0F9A1C2B-3D4E-4F56-8789-ABCDEF012345", "0f9a1c2b-3d4e-4f56-8789-abcdef01234\")", "0f9a1c2b3d4e4f568789abcdef012345"] {
            assert!(prepare("r1", bad, &["--".into(), "x".into()]).err().unwrap().contains("session"), "{bad}");
        }
    }

    /// The wrapper points each tool at the run's own caches.
    #[test]
    fn the_wrapper_points_the_tools_at_the_runs_own_caches() {
        let prepared = Prepared { profile: "/p.sb".into(), command: vec!["claude".into()], caches: "/u/.otto/sandbox/caches/r1".into(), claude: "/u/.otto/sandbox/claude/r1".into() };
        let command = prepared.sandboxed();
        let envs: std::collections::HashMap<_, _> = command.get_envs().filter_map(|(k, v)| Some((k.to_str()?.to_string(), PathBuf::from(v?)))).collect();
        for (name, sub) in [
            ("CARGO_HOME", "cargo"),
            ("npm_config_cache", "npm"),
            ("BUN_INSTALL_CACHE_DIR", "bun"),
            ("UV_CACHE_DIR", "uv"),
            ("XDG_CACHE_HOME", "xdg"),
            ("PIP_CACHE_DIR", "pip"),
            ("GOCACHE", "go-build"),
        ] {
            assert_eq!(envs.get(name), Some(&PathBuf::from("/u/.otto/sandbox/caches/r1").join(sub)), "{name}");
        }
        assert_eq!(caches_of(Path::new("/u/.otto"), "r1"), PathBuf::from("/u/.otto/sandbox/caches/r1"));
        assert_eq!(envs.get("CLAUDE_CONFIG_DIR"), Some(&PathBuf::from("/u/.otto/sandbox/claude/r1")), "claude keeps its files in the run's own folder");
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
