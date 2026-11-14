//! The sandbox wrapper and the profile it generates, against the real `sandbox-exec` and the real
//! binary, with no model: `plans/v3.md`, "Escapes the sandbox must close". Each probe tries the
//! thing and looks at what happened on disk or what came back.
//!
//! **Where these run.** A profile can't be entered from inside a different one (exit 71), so under a
//! sandboxed wake each test says so and passes without having run; item 1.9 runs them from outside. They
//! need macOS, for `sandbox-exec`. Almost everything uses a scratch `HOME`, so a probe that wrongly
//! succeeded touches only that. `the_real_token…` is the exception, on the real Keychain: it counts bytes
//! and prints none.
#![cfg(target_os = "macos")]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const OTTO: &str = support::OTTO;
/// The claude session a wake is, as otto tells the wrapper (`OTTO_SESSION_ID`).
const SESSION: &str = "0f9a1c2b-3d4e-4f56-8789-abcdef012345";

/// Whether a different profile can be entered from here. Not from inside a sandboxed wake.
fn can_nest() -> bool {
    let inside = Command::new("/usr/bin/sandbox-exec").args(["-p", "(version 1)(allow default)", "/usr/bin/true"]).output().is_ok_and(|o| o.status.success());
    if !inside {
        eprintln!("skipped: already inside a sandbox, so a different profile can't start here. This runs from outside one (plans/v3.md, 1.9)");
    }
    inside
}

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
    /// The run under test; another one, to be out of reach.
    id: String,
    other: String,
}

impl Scratch {
    fn new(record: impl FnOnce(&Path) -> serde_json::Value) -> Scratch {
        // Not under /private/tmp, which the profile leaves open, or every refusal here would be a false alarm.
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(!root.starts_with("/private/tmp"), "CARGO_TARGET_DIR is under /private/tmp, which the profile leaves open, so these can't test anything");
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        let home = root.join("h");
        for sub in [
            ".claude/projects", ".claude/hooks", ".claude/skills", ".local/bin", "Library/LaunchAgents", ".ssh", ".config/gh", ".config/cloudgpu",
            "workspace/repo/.git/hooks", "workspace/repo/.claude", "workspace/other", ".otto", "Library/Keychains",
        ] {
            std::fs::create_dir_all(home.join(sub)).unwrap();
        }
        for file in [
            ".claude/settings.json", ".zshenv", ".ssh/id_ed25519", ".config/gh/hosts.yml", ".config/cloudgpu/secrets.env", ".netrc", ".zsh_history",
            "workspace/repo/.git/config", "workspace/repo/.git/hooks/a", "workspace/repo/tracked", "workspace/other/secret", "Library/Keychains/login.keychain-db",
        ] {
            std::fs::write(home.join(file), "seed\n").unwrap();
        }
        let stub = root.join("stub");
        write_exec(&stub, "#!/bin/sh\nexit 0\n");
        std::fs::write(home.join(".otto/config.json"), serde_json::json!({"launchers": [{"name": "claude", "command": stub.display().to_string()}]}).to_string()).unwrap();
        let mut s = Scratch { _dir: dir, root, id: String::new(), other: String::new() };
        s.id = s.run("r");
        s.other = s.run("o");
        std::fs::create_dir_all(s.otto_home().join("sandbox/runs")).unwrap();
        std::fs::write(s.record_path(), record(&home).to_string()).unwrap();
        s
    }

    fn home(&self) -> PathBuf {
        self.root.join("h")
    }

    fn otto_home(&self) -> PathBuf {
        self.home().join(".otto")
    }

    fn record_path(&self) -> PathBuf {
        self.otto_home().join("sandbox/runs").join(format!("{}.json", self.id))
    }

    fn profile_path(&self) -> PathBuf {
        self.otto_home().join("sandbox/profiles").join(format!("{}.{SESSION}.sb", self.id))
    }

    fn otto(&self) -> Command {
        let mut c = support::otto_command();
        c.env("HOME", self.home())
            .env("OTTO_HOME", self.otto_home())
            .env("TMUX_TMPDIR", &self.root)
            .env("TMPDIR", self.root.join("tmp"))
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT")
            .env_remove("OTTO_SESSION_ID")
            .current_dir(&self.root);
        c
    }

    fn run(&self, name: &str) -> String {
        let out = self.otto().args(["run", "--goal", "g", "--name", name, "--perpetual", "--period", "1h", "--detach", "none", "--workdir"]).arg(self.home().join("workspace")).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let list = self.otto().args(["state", "list", "--json"]).output().unwrap();
        let list: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
        list.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap()["id"].as_str().unwrap().to_string()
    }

    /// The wrapper, for this run. In a scratch `HOME` there's no sign-in, so it stops at the token, having
    /// written the profile: which is how these tests get the very text a wake would run under.
    fn wrap(&self, args: &[&str]) -> Output {
        self.otto().env("OTTO_RUN_ID", &self.id).env("OTTO_SESSION_ID", SESSION).args(["sandbox", "wrap"]).args(args).output().unwrap()
    }

    fn profile(&self) -> PathBuf {
        let marker = self.root.join("started");
        let stub = self.root.join("marks");
        write_exec(&stub, &format!("#!/bin/sh\ntouch '{}'\n", marker.display()));
        let out = self.wrap(&["--", stub.to_str().unwrap()]);
        assert!(!out.status.success(), "a scratch home has no sign-in: {}", String::from_utf8_lossy(&out.stderr));
        assert!(!marker.exists(), "nothing starts without a token");
        assert!(self.profile_path().exists(), "{}", String::from_utf8_lossy(&out.stderr));
        self.profile_path()
    }

    /// A shell under the generated profile, cwd in the workspace.
    fn inside(&self, script: &str) -> Output {
        let profile = self.profile();
        Command::new("/usr/bin/sandbox-exec")
            .arg("-f")
            .arg(profile)
            .args(["/bin/sh", "-c", script])
            .current_dir(self.home().join("workspace"))
            .env("HOME", self.home())
            .env("H", self.home())
            .env("O", self.otto_home())
            .env("R", &self.root)
            .env("OTTO_HOME", self.otto_home())
            .env("TMPDIR", self.root.join("tmp"))
            .env_remove("TMUX")
            .output()
            .unwrap()
    }

    fn refused(&self, what: &str, script: &str) {
        let out = self.inside(script);
        assert!(!out.status.success(), "{what} should be refused: {script}");
    }

    fn allowed(&self, what: &str, script: &str) {
        let out = self.inside(script);
        assert!(out.status.success(), "{what} should work: {script}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn journal(&self) -> String {
        std::fs::read_to_string(self.otto_home().join("runs").join(&self.id).join("journal.jsonl")).unwrap()
    }
}

fn write_exec(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Grants one repo, with the whole workspace as the working directory, as every real run has.
fn one_repo(home: &Path) -> serde_json::Value {
    serde_json::json!({"launcher": "sandbox", "workdir": home.join("workspace"), "write": [home.join("workspace/repo")]})
}

#[test]
fn what_a_wake_needs_still_works() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    s.allowed("writing in the granted repo", "echo x > \"$H/workspace/repo/new\" && cat \"$H/workspace/repo/tracked\"");
    s.allowed("a temporary file", "echo x > \"$TMPDIR/f\"");
    let own = format!("mkdir -p \"$O/sandbox/claude/{}/projects/p\" && echo x > \"$O/sandbox/claude/{}/projects/p/t.jsonl\"", s.id, s.id);
    s.allowed("a transcript, in its own Claude folder", &own);
    s.allowed("reading its own record", "cat \"$O/sandbox/runs/\"*.json");
    s.allowed("reading config", "cat \"$O/config.json\"");
    s.allowed("a repo lock's directory", "mkdir -p \"$O/runs/.locks\" && echo x > \"$O/runs/.locks/l\"");
    s.allowed("the feedback file", "echo x >> \"$O/feedback.jsonl\"");
    let own = format!("echo x > \"$O/runs/{}/note\"", s.id);
    s.allowed("its own run", &own);
    s.allowed("a git repo of its own in a temporary directory, hooks and all", "cd \"$TMPDIR\" && git init -q mine && echo x > mine/.git/hooks/h");
    // The worktree a wake makes: its `.git` is a file, its admin directory is under the repo's.
    s.allowed(
        "a worktree",
        "cd \"$H/workspace/repo\" && echo x > \"$H/workspace/repo/.git/HEAD\" && mkdir -p .git/worktrees/w work && printf 'gitdir: x\\n' > work/.git && echo y > .git/worktrees/w/HEAD && rm work/.git",
    );
}

/// Plan table, "Git hooks and config": including the ways round a rule that names a path.
#[test]
fn git_hooks_and_config_can_not_be_written() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    let repo = "\"$H/workspace/repo\"";
    for (what, script) in [
        ("a hook", format!("echo x > {repo}/.git/hooks/pre-commit")),
        ("a hook, in other case", format!("echo x > {repo}/.GIT/HOOKS/pre-commit")),
        ("config", format!("echo x >> {repo}/.git/config")),
        ("config, in other case", format!("echo x >> {repo}/.git/CONFIG")),
        ("a worktree's config", format!("mkdir -p {repo}/.git/worktrees/w && echo x > {repo}/.git/worktrees/w/config.worktree")),
        ("through a symlink", format!("ln -s {repo}/.git/config {repo}/sl && echo x >> {repo}/sl")),
        ("a hard link to config", format!("ln {repo}/.git/config {repo}/hl")),
        ("moving .git aside", format!("mv {repo}/.git {repo}/g2")),
        ("moving hooks aside", format!("mv {repo}/.git/hooks {repo}/.git/h2")),
        ("moving the repo out", format!("mv {repo} \"$TMPDIR/moved\"")),
        ("claude settings in the repo", format!("echo x > {repo}/.claude/settings.local.json")),
        ("~/.gitconfig", "echo x >> \"$H/.gitconfig\"".to_string()),
        ("~/.config/git/config", "mkdir -p \"$H/.config/git\" && echo x >> \"$H/.config/git/config\"".to_string()),
    ] {
        s.refused(what, &script);
    }
    assert_eq!(std::fs::read_to_string(s.home().join("workspace/repo/.git/config")).unwrap(), "seed\n");
}

/// `otto state`, for real, from inside: everything a wake does through it has to land, in a run whose wake
/// is the only thing that could be writing. A new place otto writes, and this is what notices.
#[test]
fn a_wake_can_do_its_job_through_otto_state() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    let repo = s.home().join("workspace/repo");
    let init = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&repo).env("HOME", s.home()).output().unwrap().status.success(), "{args:?}");
    std::fs::remove_dir_all(repo.join(".git")).unwrap();
    init(&["init", "-q", "-b", "main"]);
    init(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"]);
    init(&["checkout", "-q", "-b", "try"]);
    std::fs::write(repo.join("made"), "x\n").unwrap();
    init(&["add", "made"]);
    init(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "try"]);
    init(&["checkout", "-q", "main"]);
    assert!(s.otto().args(["config", "project", repo.to_str().unwrap(), "--process", "direct"]).output().unwrap().status.success());
    let id = &s.id;
    let r = repo.display();
    let info = repo.join(".git/info");
    std::fs::create_dir_all(&info).unwrap();
    std::fs::write(info.join("exclude"), "summary.md\nhandoff.md\n").unwrap();
    std::fs::write(repo.join("summary.md"), "What it does.\n").unwrap();
    std::fs::write(repo.join("handoff.md"), "# h\n## Goal\ng\n## Where this is\nHere.\n## Decided\nn\n## This wake did\nn\n## Next wake must\nn\n## Needs\nnothing\n## Don't re-derive\nn\n").unwrap();
    for (what, script) in [
        ("a lock", format!("'{OTTO}' state lock {id} --repo '{r}'")),
        ("the unlock", format!("'{OTTO}' state unlock {id}")),
        ("a log line", format!("'{OTTO}' state log {id} --event cycle --message m")),
        ("a fact", format!("'{OTTO}' state record-fact {id} k=v")),
        ("a todo-ish artifact", format!("echo x > \"$O/runs/{id}/artifacts/todo.md\"")),
        ("feedback", format!("'{OTTO}' state feedback {id} --body 'a report'")),
        ("moving a feedback item on", format!("'{OTTO}' feedback building 1 --change {id}/1")),
        ("a published output", format!("'{OTTO}' state publish {id} --key k --title t --file '{r}/summary.md'")),
        ("a message", format!("'{OTTO}' state message {id} --kind update --title t --body b")),
        ("a change", format!("'{OTTO}' state change submit {id} --repo '{r}' --branch try --title t --summary-file '{r}/summary.md' --tests none --no-evidence none")),
        ("a handoff", format!("'{OTTO}' state handoff {id} --stdin < '{r}/handoff.md'")),
        ("a timer", format!("'{OTTO}' state arm-timer {id} --in 600")),
    ] {
        let out = s.inside(&script);
        assert!(out.status.success(), "{what}: {script}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
}

/// Found by running `cargo test` under a profile: rustc looks at every directory on the way down to a grant,
/// so a grant below a closed directory has to leave those visible, and only visible.
#[test]
fn a_grant_below_a_closed_directory_can_be_walked_to() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(|home| serde_json::json!({"launcher": "sandbox", "workdir": home.join("workspace"), "write": [home.join("workspace/trees/deep/x")]}));
    std::fs::create_dir_all(s.home().join("workspace/trees/deep/x")).unwrap();
    std::fs::write(s.home().join("workspace/trees/sibling"), "seed\n").unwrap();
    s.allowed("looking at the directory above", "stat \"$H/workspace/trees\" && stat \"$H/workspace/trees/deep\"");
    s.allowed("the grant", "echo x > \"$H/workspace/trees/deep/x/f\"");
    s.refused("listing the directory above", "ls \"$H/workspace/trees\"");
    s.refused("what's beside it", "cat \"$H/workspace/trees/sibling\"");
    s.refused("looking at what's beside it", "stat \"$H/workspace/trees/sibling\"");
    s.refused("writing beside it", "echo x > \"$H/workspace/trees/new\"");
}

/// With `.git/config` closed, what a wake does with git still works: fetch, a worktree on a new branch from
/// `origin/main` (git can't record the upstream, and says so, and goes on), commit, rebase, remove.
#[test]
fn git_works_with_its_config_closed() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(|home| {
        serde_json::json!({"launcher": "sandbox", "workdir": home.join("workspace"), "read": [home.join("workspace/origin.git")], "write": [home.join("workspace/real"), home.join("workspace/trees")]})
    });
    let (space, real) = (s.home().join("workspace"), s.home().join("workspace/real"));
    std::fs::create_dir_all(space.join("trees")).unwrap();
    let git = |dir: &Path, args: &[&str]| {
        let out = Command::new("git").args(args).current_dir(dir).env("HOME", s.home()).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&space, &["init", "-q", "--bare", "origin.git"]);
    git(&space, &["init", "-q", "-b", "main", "real"]);
    git(&real, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"]);
    git(&real, &["remote", "add", "origin", space.join("origin.git").to_str().unwrap()]);
    git(&real, &["push", "-q", "origin", "main"]);
    let config = std::fs::read(real.join(".git/config")).unwrap();
    s.allowed(
        "the worktree flow",
        "cd \"$H/workspace/real\" && git fetch -q origin && git worktree add -q -b x \"$H/workspace/trees/x\" origin/main && cd \"$H/workspace/trees/x\" \
         && echo y > f && git add f && git -c user.name=t -c user.email=t@t commit -q -m c && git rebase -q origin/main && git log -1 --format=%s | grep -q c \
         && cd \"$H/workspace/real\" && git worktree remove --force \"$H/workspace/trees/x\" && git worktree prune && git branch -D -q x",
    );
    s.refused("git setting config", "cd \"$H/workspace/real\" && git config foo.bar 1");
    s.refused("the lock renamed onto it", "cd \"$H/workspace/real\" && echo x > .git/config.lock && mv .git/config.lock .git/config");
    std::fs::remove_file(real.join(".git/config.lock")).unwrap();
    assert_eq!(std::fs::read(real.join(".git/config")).unwrap(), config);
}

/// A record listing the directory above a repo opens that directory, and still not the hooks, and the repo
/// can't be moved out from under the rule.
#[test]
fn a_grant_above_a_repo_does_not_open_its_hooks() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(|home| serde_json::json!({"launcher": "sandbox", "workdir": home.join("workspace"), "write": [home.join("workspace")]}));
    s.allowed("the directory the record lists", "echo x > \"$H/workspace/repo/new\"");
    s.refused("the hooks below it", "echo x > \"$H/workspace/repo/.git/hooks/h\"");
    s.refused("moving the repo out", "mv \"$H/workspace/repo\" \"$TMPDIR/moved\"");
    s.refused("the credentials rule still applies", "cat \"$H/.ssh/id_ed25519\"");
}

/// Plan table: "Claude settings", "Persistence", "`otto-build` installing".
#[test]
fn nothing_that_runs_later_can_be_written() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    for (what, path) in [
        ("claude's settings", ".claude/settings.json"),
        ("claude's local settings", ".claude/settings.local.json"),
        ("a claude hook", ".claude/hooks/h"),
        ("a skill", ".claude/skills/s"),
        ("a claude agent", ".claude/agents/a.md"),
        ("~/.claude.json, with its mcpServers", ".claude.json"),
        ("a shell startup file", ".zshenv"),
        ("another", ".zshrc"),
        ("a LaunchAgent", "Library/LaunchAgents/x.plist"),
        ("the binary otto runs", ".local/bin/otto"),
        ("a cargo binary", ".cargo/bin/x"),
    ] {
        s.refused(what, &format!("mkdir -p \"$(dirname \"$H/{path}\")\" 2>/dev/null; echo x >> \"$H/{path}\""));
    }
    assert_eq!(std::fs::read_to_string(s.home().join(".claude/settings.json")).unwrap(), "seed\n");
}

/// Plan table: "Tool caches that outside builds run". The shared caches are read-only from inside, and a wake
/// writes its own, under `$OTTO_HOME/sandbox/caches/<id>`, where the wrapper points the tools.
#[test]
fn shared_caches_and_the_tmux_directory_can_not_be_written() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    let seeded = [".cargo/registry/src/x/build.rs", ".cargo/git/checkouts/x/build.rs", ".npm/_npx/f", ".npm/_cacache/f", ".bun/install/cache/f", ".cache/uv/f", ".cache/oh-my-zsh/f"];
    for path in seeded {
        std::fs::create_dir_all(s.home().join(path).parent().unwrap()).unwrap();
        std::fs::write(s.home().join(path), "seed\n").unwrap();
    }
    for path in seeded.iter().chain(&[".cargo/.package-cache", ".cache/new"]) {
        s.refused(path, &format!("echo x >> \"$H/{path}\""));
    }
    for path in seeded {
        assert_eq!(std::fs::read_to_string(s.home().join(path)).unwrap(), "seed\n", "{path}");
    }
    s.allowed("reading the registry", "cat \"$H/.cargo/registry/src/x/build.rs\"");
    // The wrapper made them, from outside.
    let own = format!("mkdir -p \"$O/sandbox/caches/{id}/cargo/registry\" && echo x > \"$O/sandbox/caches/{id}/cargo/registry/f\" && echo x > \"$O/sandbox/caches/{id}/xdg/f\"", id = s.id);
    s.allowed("its own caches", &own);
    // cargo with its home moved there still finds its toolchain and builds.
    let build = format!(
        "export CARGO_HOME=\"$O/sandbox/caches/{id}/cargo\" CARGO_TARGET_DIR=\"$TMPDIR/t\" && cd \"$TMPDIR\" && cargo new -q --vcs none p && cd p && cargo build -q --offline",
        id = s.id
    );
    s.allowed("cargo, with its home in the run's caches", &build);
    let other = format!("mkdir -p \"$O/sandbox/caches/{id}\"; echo x > \"$O/sandbox/caches/{id}/f\"", id = s.other);
    s.refused("another run's caches", &other);

    // The real tmux server's directory. Made outside first, as it is wherever the person uses tmux.
    let uid = String::from_utf8(Command::new("/usr/bin/id").arg("-u").output().unwrap().stdout).unwrap().trim().to_string();
    let tmux = PathBuf::from(format!("/private/tmp/tmux-{uid}"));
    let probe = tmux.join(format!("otto-probe-{}", std::process::id()));
    let _ = std::fs::create_dir(&tmux);
    let out = s.inside(&format!("echo x > '{}'", probe.display()));
    let made = probe.exists();
    let _ = std::fs::remove_file(&probe);
    assert!(!out.status.success() && !made, "a file in the real tmux server's directory");
}

/// Plan table, "Claude's per-session files". Every claude session, the person's own included, sources its shell
/// snapshot before each Bash command and the files in its `session-env/<id>`, and reads its memory as it starts:
/// so nothing in `~/.claude` is written from inside. A wake's claude keeps its files in the run's own folder,
/// where the links to the person's settings and skills can't be replaced, added to, or written through.
#[test]
fn the_persons_claude_folder_can_not_be_written() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    let other = "11111111-2222-4333-8444-555555555555";
    let seeded = [
        format!(".claude/session-env/{other}/hook-1.sh"),
        "snapshot-zsh-1700000000000-abcdef.sh".to_string(),
        ".claude/projects/-Users-me-workspace/memory/MEMORY.md".to_string(),
    ];
    let seeded: Vec<String> = seeded.iter().map(|p| if p.starts_with('.') { p.clone() } else { format!(".claude/shell-snapshots/{p}") }).collect();
    for path in &seeded {
        std::fs::create_dir_all(s.home().join(path).parent().unwrap()).unwrap();
        std::fs::write(s.home().join(path), "seed\n").unwrap();
    }
    for path in &seeded {
        s.refused(path, &format!("echo x >> \"$H/{path}\""));
        s.refused(path, &format!("echo x > \"$TMPDIR/new\" && mv \"$TMPDIR/new\" \"$H/{path}\""));
    }
    for (what, script) in [
        ("a new snapshot", "echo x > \"$H/.claude/shell-snapshots/snapshot-zsh-new.sh\"".to_string()),
        ("a new memory file", "echo x > \"$H/.claude/projects/-Users-me-workspace/memory/new.md\"".to_string()),
        ("a new project's memory", "mkdir -p \"$H/.claude/projects/-new/memory\"".to_string()),
        ("its own session's session-env, there", format!("mkdir -p \"$H/.claude/session-env/{SESSION}\"")),
        ("a transcript, there", "echo x > \"$H/.claude/projects/t.jsonl\"".to_string()),
        ("~/.claude.json", "echo x >> \"$H/.claude.json\"".to_string()),
        // A claude started from inside, told to use the person's folder, writing its snapshot as claude does.
        ("a nested claude's snapshot in the person's folder", "CLAUDE_CONFIG_DIR=\"$H/.claude\" sh -c 'echo x >> \"$CLAUDE_CONFIG_DIR/shell-snapshots/snapshot-zsh-nested.sh\"'".to_string()),
    ] {
        s.refused(what, &script);
    }
    for path in &seeded {
        assert_eq!(std::fs::read_to_string(s.home().join(path)).unwrap(), "seed\n", "{path}");
    }
    assert!(!s.home().join(".claude/shell-snapshots/snapshot-zsh-nested.sh").exists());
    assert!(!s.home().join(".claude/projects/-new").exists());
}

/// The run's own Claude folder, laid out by the wrapper: what claude writes as it works lands there, and its
/// links to the person's settings and skills stay as they are.
#[test]
fn the_runs_own_claude_folder_is_writable_except_its_links() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    // No hooks of the person's, so none is linked and a wake could try adding its own.
    std::fs::remove_dir_all(s.home().join(".claude/hooks")).unwrap();
    s.profile();
    let c = format!("\"$O/sandbox/claude/{}\"", s.id);
    let dir = s.otto_home().join("sandbox/claude").join(&s.id);
    assert_eq!(std::fs::read_link(dir.join("skills")).unwrap(), s.home().join(".claude/skills"), "laid out before the token");
    assert_eq!(std::fs::read_link(dir.join("settings.json")).unwrap(), s.home().join(".claude/settings.json"));
    assert!(dir.join("hooks").symlink_metadata().is_err(), "nothing where the person has nothing");
    for (what, script) in [
        ("a shell snapshot, appended a line at a time", format!("mkdir -p {c}/shell-snapshots && echo a > {c}/shell-snapshots/s.sh && echo b >> {c}/shell-snapshots/s.sh")),
        ("its session-env", format!("mkdir -p {c}/session-env/{SESSION} && echo x > {c}/session-env/{SESSION}/f")),
        ("a transcript", format!("mkdir -p {c}/projects/p && echo x >> {c}/projects/p/t.jsonl")),
        ("its .claude.json, as claude replaces it", format!("echo '{{}}' > {c}/.claude.json.tmp && mv {c}/.claude.json.tmp {c}/.claude.json")),
        ("reading a skill through the link", format!("ls {c}/skills/ && cat {c}/settings.json")),
    ] {
        s.allowed(what, &script);
    }
    for (what, script) in [
        ("replacing the settings.json link", format!("rm -f {c}/settings.json; echo '{{}}' > {c}/settings.json")),
        ("renaming over the settings.json link", format!("echo '{{}}' > {c}/x.json && mv {c}/x.json {c}/settings.json")),
        ("writing through the settings.json link", format!("echo x >> {c}/settings.json")),
        ("replacing the skills link", format!("rm -f {c}/skills; mkdir {c}/skills")),
        ("a skill through the link", format!("mkdir {c}/skills/evil")),
        ("moving the skills link aside", format!("mv {c}/skills {c}/s2")),
        ("adding a hooks folder", format!("mkdir {c}/hooks")),
        ("adding a hooks folder, in other case", format!("mkdir {c}/HOOKS")),
        ("adding CLAUDE.md", format!("echo x > {c}/CLAUDE.md")),
        ("adding agents", format!("mkdir {c}/agents")),
        ("adding settings.local.json", format!("echo x > {c}/settings.local.json")),
        ("a hard link to the person's settings", format!("ln \"$H/.claude/settings.json\" {c}/hl.json")),
        ("moving the folder away", format!("mv {c} \"$TMPDIR/moved\"")),
    ] {
        s.refused(what, &script);
    }
    assert_eq!(std::fs::read_to_string(s.home().join(".claude/settings.json")).unwrap(), "seed\n");
    assert_eq!(std::fs::read_link(dir.join("settings.json")).unwrap(), s.home().join(".claude/settings.json"));
    assert_eq!(std::fs::read_link(dir.join("skills")).unwrap(), s.home().join(".claude/skills"));
    assert!(!dir.join("hooks").exists() && !s.home().join(".claude/skills/evil").exists());
    let other = format!("mkdir -p \"$O/sandbox/claude/{id}\" && echo x > \"$O/sandbox/claude/{id}/f\"", id = s.other);
    s.refused("another run's Claude folder", &other);
}

/// Plan table: "Other otto state", "The wrapper and profiles". Its own `run.json` is writable, and that's
/// what the record is for (the next part of 1.2).
#[test]
fn otto_state_past_its_own_run_can_not_be_written() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    s.profile();
    std::fs::write(s.otto_home().join("sandbox/wrapper"), "#!/bin/sh\n").unwrap();
    let other = format!("\"$O/runs/{}/run.json\"", s.other);
    for (what, script) in [
        ("config.json", "echo x >> \"$O/config.json\"".to_string()),
        ("its record", format!("echo x >> \"$O/sandbox/runs/{}.json\"", s.id)),
        ("a new record", "echo x > \"$O/sandbox/runs/new.json\"".to_string()),
        ("the wrapper", "echo x >> \"$O/sandbox/wrapper\"".to_string()),
        ("its profile", format!("echo x >> \"$O/sandbox/profiles/{}.sb\"", s.id)),
        ("moving the sandbox directory", "mv \"$O/sandbox\" \"$O/sandbox2\"".to_string()),
        ("another run", format!("echo x >> {other}")),
        ("who may assign", "echo x > \"$O/feedback-driver\"".to_string()),
        ("a new file in $OTTO_HOME", "echo x > \"$O/approvals\"".to_string()),
        ("an approval on record", format!("mkdir -p \"$O/approvals/{id}\" && echo x > \"$O/approvals/{id}/1.json\"", id = s.id)),
    ] {
        s.refused(what, &script);
    }
    assert!(!s.otto_home().join("approvals").exists());
}

/// The person's approvals (`$OTTO_HOME/approvals`) are closed after the record's grants: a record that
/// lists the folder itself under `write`, or all of `$OTTO_HOME`, opens none of it. Not to add one, change
/// one, remove one or move the folder away.
#[test]
fn no_record_opens_the_persons_approvals() {
    if !can_nest() {
        return;
    }
    for record in [
        (|home: &Path| serde_json::json!({"launcher": "sandbox", "write": [home.join(".otto/approvals")]})) as fn(&Path) -> serde_json::Value,
        |home: &Path| serde_json::json!({"launcher": "sandbox", "write": [home.join(".otto")]}),
    ] {
        let s = Scratch::new(record);
        let mine = s.otto_home().join("approvals").join(&s.id);
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::write(mine.join("1.json"), "the person's\n").unwrap();
        let id = &s.id;
        for (what, script) in [
            ("a new approval", format!("echo x > \"$O/approvals/{id}/2.json\"")),
            ("a new run's approvals", "mkdir -p \"$O/approvals/9\" && echo x > \"$O/approvals/9/1.json\"".to_string()),
            ("changing one", format!("echo x >> \"$O/approvals/{id}/1.json\"")),
            ("removing one", format!("rm \"$O/approvals/{id}/1.json\"")),
            ("moving the folder away", "mv \"$O/approvals\" \"$O/approvals2\"".to_string()),
        ] {
            s.refused(what, &script);
        }
        assert_eq!(std::fs::read_to_string(mine.join("1.json")).unwrap(), "the person's\n");
        assert!(!mine.join("2.json").exists() && !s.otto_home().join("approvals/9").exists() && !s.otto_home().join("approvals2").exists());
        s.allowed("reading one", &format!("cat \"$O/approvals/{id}/1.json\""));
    }
}

/// `otto changes approve` from inside, on the wake's own change: the approval can't be written, so the
/// command fails before the run's index is touched, says why, and the change is still in review.
#[test]
fn a_wake_can_not_approve_its_own_change() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    let repo = s.home().join("workspace/repo");
    let init = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&repo).env("HOME", s.home()).output().unwrap().status.success(), "{args:?}");
    std::fs::remove_dir_all(repo.join(".git")).unwrap();
    init(&["init", "-q", "-b", "main"]);
    init(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"]);
    init(&["checkout", "-q", "-b", "try"]);
    std::fs::write(repo.join("made"), "x\n").unwrap();
    init(&["add", "made"]);
    init(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "try"]);
    init(&["checkout", "-q", "main"]);
    let ok = |args: &[&str]| {
        let out = s.otto().args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    };
    ok(&["config", "project", repo.to_str().unwrap(), "--process", "direct"]);
    ok(&["config", "lander", &s.other]);
    let (id, r) = (&s.id, repo.display());
    std::fs::write(s.root.join("summary.md"), "What it does.\n").unwrap();
    ok(&["state", "change", "submit", id, "--repo", repo.to_str().unwrap(), "--branch", "try", "--title", "t", "--summary-file", s.root.join("summary.md").to_str().unwrap(), "--tests", "none", "--no-evidence", "none"]);
    let _ = r;
    let index = || -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(s.otto_home().join("runs").join(id).join("changes/index.json")).unwrap()).unwrap() };
    assert_eq!(index()["changes"][0]["state"], "review", "{}", index());

    let out = s.inside(&format!("'{OTTO}' changes approve {id}/1@v1 --no-wake"));
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success(), "approving from inside should fail: {said}");
    assert!(said.contains("approving is the person's") && said.contains("sandbox") && said.contains("approvals/"), "{said}");
    let change = &index()["changes"][0];
    assert_eq!(change["state"], "review", "{change}");
    assert!(change.get("decision").is_none(), "{change}");
    assert!(!s.otto_home().join("approvals").join(id).join("1.json").exists());

    // From outside, where the person is, the same command approves it and leaves the record.
    ok(&["changes", "approve", &format!("{id}/1@v1"), "--no-wake"]);
    assert_eq!(index()["changes"][0]["state"], "approved");
    assert!(s.otto_home().join("approvals").join(id).join("1.json").is_file());
}

/// Found by a real wake: `$OTTO_HOME` under a directory the projects rule closes, and the wake can't read its
/// own `run.json`, so it can't even say what's wrong. Its home is still its home.
#[test]
fn a_home_under_a_closed_directory_is_still_open() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(|home| serde_json::json!({"launcher": "sandbox", "workdir": home}));
    s.allowed("its own run.json", &format!("cat \"$O/runs/{}/run.json\"", s.id));
    s.allowed("writing in its run", &format!("echo x > \"$O/runs/{}/note\"", s.id));
    s.allowed("a repo lock", "mkdir -p \"$O/runs/.locks\" && echo x > \"$O/runs/.locks/l\"");
    s.refused("config.json still", "echo x >> \"$O/config.json\"");
    s.refused("what the home's beside", "cat \"$H/.zshenv\"");
}

/// A record can name a place under `$OTTO_HOME`, which is how the lander's writes into other runs get said,
/// and still can't open what no record can.
#[test]
fn a_record_can_open_other_runs_and_not_config() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(|home| serde_json::json!({"launcher": "sandbox", "write": [home.join(".otto")]}));
    s.allowed("other runs", &format!("echo x >> \"$O/runs/{}/note\"", s.other));
    s.refused("config.json", "echo x >> \"$O/config.json\"");
    s.refused("the sandbox", "echo x > \"$O/sandbox/runs/new.json\"");
}

/// Plan table: "The working directory as a grant". Other projects closed both ways, with `~/workspace` as
/// the working directory; a `--allow` the record doesn't list opens nothing.
#[test]
fn other_projects_are_closed_both_ways() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    let other = s.home().join("workspace/other");
    // What otto would pass for a run.json that had an extra repo in it.
    s.wrap(&["--allow", other.to_str().unwrap(), "--", "/usr/bin/true"]);
    for (what, script) in [
        ("reading another project", "cat \"$H/workspace/other/secret\""),
        ("listing it", "ls \"$H/workspace/other\""),
        ("writing to it", "echo x >> \"$H/workspace/other/secret\""),
        ("reading through a symlink", "ln -s \"$H/workspace/other\" \"$H/workspace/repo/l\"; cat \"$H/workspace/repo/l/secret\""),
        ("in other case", "cat \"$H/WORKSPACE/OTHER/SECRET\""),
        ("listing the workspace", "ls \"$H/workspace\""),
        ("creating in the workspace", "echo x > \"$H/workspace/new\""),
    ] {
        s.refused(what, script);
    }
    s.allowed("the repo it was granted", "cat \"$H/workspace/repo/tracked\"");
    s.allowed("still being in the workspace", "test \"$(pwd -P)\" = \"$H/workspace\"");
}

/// Plan table: "Credentials in other files", and the Keychain from the spike.
#[test]
fn credentials_read_as_nothing() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    for (what, path) in [
        ("an ssh key", ".ssh/id_ed25519"),
        ("gh's token", ".config/gh/hosts.yml"),
        ("an app's secrets", ".config/cloudgpu/secrets.env"),
        ("netrc", ".netrc"),
        ("shell history", ".zsh_history"),
        ("the Keychain file", "Library/Keychains/login.keychain-db"),
    ] {
        let out = s.inside(&format!("cat \"$H/{path}\""));
        assert!(!out.status.success() && out.stdout.is_empty(), "{what}: {} bytes", out.stdout.len());
    }
    let out = s.inside("/usr/bin/security find-generic-password -s 'Claude Code-credentials' -w");
    assert!(!out.status.success() && out.stdout.is_empty(), "the Keychain: {} bytes", out.stdout.len());
}

/// An app's secrets are the one thing a record can open, for the run that's that app's.
#[test]
fn a_record_can_open_an_apps_secrets_but_not_a_credential() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(|home| serde_json::json!({"launcher": "sandbox", "read": [home.join(".config/cloudgpu"), home.join(".ssh")]}));
    s.allowed("the app's", "cat \"$H/.config/cloudgpu/secrets.env\"");
    s.refused("the ssh key, listed or not", "cat \"$H/.ssh/id_ed25519\"");
}

/// Plan table: "Talking to outside processes". None of these acts on anything: each is refused at the exec.
#[test]
fn outside_processes_can_not_be_reached() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    for (what, script) in [
        ("launchctl", "/bin/launchctl list"),
        ("open", "/usr/bin/open -R /nonexistent"),
        ("osascript", "/usr/bin/osascript -e 'return 1'"),
        ("security", "/usr/bin/security list-keychains"),
    ] {
        let out = s.inside(script);
        assert_eq!(out.status.code(), Some(126), "{what}: {}", String::from_utf8_lossy(&out.stderr));
    }
    // The socket, whichever server is or isn't there: refused is not the same as nothing listening.
    let out = s.inside("mkdir -p /private/tmp/tmux-$(id -u); exec tmux -S /private/tmp/tmux-$(id -u)/default ls");
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("not permitted") || said.contains("no server"), "{said}");
    // otto's page approves for whoever asks. Whether or not it's up, a curl from here must not connect.
    let out = s.inside("exec curl -s --max-time 5 -o /dev/null http://127.0.0.1:7878/api/inbox");
    assert!(!out.status.success(), "otto's web page must be out of reach, or a wake approves itself");
    let out = s.inside("exec curl -s --max-time 5 -o /dev/null http://localhost:7878/api/inbox");
    assert!(!out.status.success());
}

/// `otto note --now` and `otto answer` start wakes. From inside, whatever it starts is still inside, so it
/// can't be less confined: a stub launcher started that way can't write what the sandbox refuses.
#[test]
fn a_wake_started_from_inside_stays_inside() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    let escaped = s.home().join("escaped");
    write_exec(&s.root.join("stub"), &format!("#!/bin/sh\necho x > '{}'\n", escaped.display()));
    let out = s.inside(&format!("exec '{OTTO}' wake {}", s.other));
    assert!(!escaped.exists(), "a child of a sandbox wrote outside it: {}", String::from_utf8_lossy(&out.stderr));
}

/// Check 3 of the plan's: each way of editing `run.json` to widen the next wake, through the commands themselves.
/// None of this enters a profile, so it runs inside a sandbox too.
#[test]
fn a_run_edited_past_its_record_starts_nothing_until_it_agrees() {
    let s = Scratch::new(|home| serde_json::json!({"launcher": "claude", "workdir": home.join("workspace"), "write": [home.join("workspace/repo")]}));
    let started = s.root.join("started-a-wake");
    // The other run in this home has wakes of its own to retry, and they aren't what's being watched.
    write_exec(&s.root.join("stub"), &format!("#!/bin/sh\n[ \"$OTTO_RUN_ID$OTTO_CHAT\" = '{}' ] && touch '{}'\nexit 0\n", s.id, started.display()));
    let config = s.otto_home().join("config.json");
    let mut with_second: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    with_second["launchers"].as_array_mut().unwrap().push(serde_json::json!({"name": "second", "command": s.root.join("stub").display().to_string()}));
    std::fs::write(&config, with_second.to_string()).unwrap();

    let run_json = s.otto_home().join("runs").join(&s.id).join("run.json");
    let original = std::fs::read_to_string(&run_json).unwrap();
    let edit = |change: &dyn Fn(&mut serde_json::Value)| {
        let mut run: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&run_json).unwrap()).unwrap();
        change(&mut run);
        std::fs::write(&run_json, run.to_string()).unwrap();
    };
    let run = || -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(&run_json).unwrap()).unwrap() };
    let ssh = s.home().join(".ssh").display().to_string();
    let elsewhere = s.home().join("workspace/other").display().to_string();
    let edits: Vec<(&str, Box<dyn Fn(&mut serde_json::Value)>)> = vec![
        ("second", Box::new(|r| r["launcher"]["kind"] = "second".into())),
        (".ssh", Box::new(move |r| r["launcher"]["repos"] = serde_json::json!([ssh.clone()]))),
        ("workspace/other", Box::new(move |r| r["launcher"]["workdir"] = elsewhere.clone().into())),
    ];
    for (word, change) in &edits {
        std::fs::write(&run_json, &original).unwrap();
        edit(&**change);
        let before = s.journal().matches("sandbox-refused").count();

        let out = s.otto().args(["wake", &s.id]).output().unwrap();
        let said = String::from_utf8_lossy(&out.stderr).to_string();
        assert_eq!(out.status.code(), Some(2), "wake: {said}");
        assert!(said.contains(word), "the refusal says what differed: {said}");

        let out = s.otto().args(["chat", &s.id, "hello"]).stdin(std::process::Stdio::null()).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "chat: {}", String::from_utf8_lossy(&out.stderr));
        assert!(!started.exists(), "{word}: something started");

        assert_eq!(s.journal().matches("sandbox-refused").count(), before + 2, "both are in the journal");
        assert!(s.journal().contains(word));
        assert_eq!(run()["status"], "blocked");
        assert_eq!(run()["gate"]["slug"], "sandbox-record");

        let poked = s.otto().args(["poke", "--verbose", "--grace", "0"]).output().unwrap();
        let poked = String::from_utf8_lossy(&poked.stdout);
        assert!(poked.lines().any(|l| l.contains(&s.id) && l.contains("awaiting a person")), "{poked}");
        assert_eq!(run()["status"], "blocked", "poke leaves it visibly stopped");
        assert_eq!(run()["spawnAttempts"], 0, "and spends nothing retrying");
        assert!(!started.exists());

        let listed = s.otto().arg("ls").output().unwrap();
        let listed = String::from_utf8_lossy(&listed.stdout);
        assert!(listed.contains("blocked (sandbox)"), "{listed}");
        let shown = s.otto().args(["show", &s.id]).output().unwrap();
        let shown = String::from_utf8_lossy(&shown.stdout);
        assert!(shown.contains("sandbox record") && shown.contains(word), "{shown}");

        // Its launcher put back, the rest as it stands, and it runs: no answer needed.
        let launcher: serde_json::Value = serde_json::from_str::<serde_json::Value>(&original).unwrap()["launcher"].clone();
        edit(&|r| r["launcher"] = launcher.clone());
        assert!(!started.exists());
        let out = s.otto().args(["wake", &s.id]).output().unwrap();
        assert!(started.exists(), "{word}: agreeing again, it starts: {}", String::from_utf8_lossy(&out.stderr));
        assert_ne!(run()["status"], "blocked");
        std::fs::remove_file(&started).unwrap();
    }

    // Out of the sandbox: no record, and the launcher a person chose.
    std::fs::write(&run_json, &original).unwrap();
    edit(&|r| r["launcher"]["kind"] = "second".into());
    assert!(!s.otto().args(["wake", &s.id]).status().unwrap().success());
    std::fs::remove_file(s.record_path()).unwrap();
    let out = s.otto().args(["wake", &s.id]).output().unwrap();
    assert!(started.exists(), "with no record it runs as any run does: {}", String::from_utf8_lossy(&out.stderr));
    assert_ne!(run()["status"], "blocked");
    assert!(run()["gate"].is_null());
}

/// Every way the wrapper can fail: nothing starts, and it says so where it will be seen.
#[test]
fn the_wrapper_fails_closed_and_says_why() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    let marker = s.root.join("started");
    let stub = s.root.join("marks");
    write_exec(&stub, &format!("#!/bin/sh\ntouch '{}'\n", marker.display()));
    let stub = stub.to_str().unwrap();
    let check = |s: &Scratch, what: &str, said: &str| {
        let out = s.wrap(&["--", stub]);
        assert!(!out.status.success(), "{what}");
        assert!(!marker.exists(), "{what}: the command started");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(said), "{what}: {stderr}");
        assert!(s.journal().lines().last().unwrap().contains(said), "{what}: {}", s.journal());
        assert!(s.journal().contains("sandbox-refused"));
    };
    check(&s, "no token", "sign");
    std::fs::write(s.record_path(), "{ garbled").unwrap();
    check(&s, "a garbled record", "can't be used");
    std::fs::write(s.record_path(), r#"{"launcher":"s","writes":["/x"]}"#).unwrap();
    check(&s, "a mistyped record", "can't be used");
    std::fs::remove_file(s.record_path()).unwrap();
    check(&s, "no record", "no sandbox record");
    std::fs::write(s.record_path(), r#"{"launcher":"s","write":["relative"]}"#).unwrap();
    check(&s, "a relative path", "absolute");

    // No session, or one otto wouldn't make: there's no telling which of claude's directories is its own.
    std::fs::write(s.record_path(), one_repo(&s.home()).to_string()).unwrap();
    let session = |value: Option<&str>| {
        let mut c = s.otto();
        c.env("OTTO_RUN_ID", &s.id).args(["sandbox", "wrap", "--", stub]);
        if let Some(v) = value {
            c.env("OTTO_SESSION_ID", v);
        }
        c.output().unwrap()
    };
    for (value, said) in [(None, "OTTO_SESSION_ID isn't set"), (Some("../other"), "session id")] {
        let out = session(value);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && !marker.exists() && stderr.contains(said), "{value:?}: {stderr}");
    }

    // Not a wake or a chat at all.
    let out = s.otto().args(["sandbox", "wrap", "--", stub]).output().unwrap();
    assert!(!out.status.success() && !marker.exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("OTTO_RUN_ID"));
    // The launcher's own -- missing.
    std::fs::write(s.record_path(), one_repo(&s.home()).to_string()).unwrap();
    let out = s.wrap(&[stub]);
    assert!(!out.status.success() && !marker.exists());
}

/// A path that would break out of its string doesn't, so the profile still compiles and still closes.
#[test]
fn a_path_with_a_quote_in_it_stays_a_path() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(|home| serde_json::json!({"launcher": "sandbox", "write": [home.join("workspace/a\") (allow default) (literal \"b")]}));
    let profile = s.profile();
    assert!(std::fs::read_to_string(profile).unwrap().contains(r#"a\") (allow default) (literal \"b"#));
    s.refused("the credentials are still closed", "cat \"$H/.ssh/id_ed25519\"");
    s.refused("and writes", "echo x >> \"$H/.zshenv\"");
}

/// A profile `sandbox-exec` won't take stops before anything starts, including the token being read.
#[test]
fn a_profile_that_will_not_compile_starts_nothing() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(one_repo);
    // Not a directory where profiles go: the wrapper can't write one.
    std::fs::remove_dir_all(s.otto_home().join("sandbox/profiles")).ok();
    std::fs::write(s.otto_home().join("sandbox/profiles"), "").unwrap();
    let out = s.wrap(&["--", "/usr/bin/true"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("profile"));
}

/// The real thing, with a real sign-in and a stub for claude: the token is on descriptor 3, in no variable,
/// and a planted credential doesn't get in. Passes without running if this machine has no sign-in.
#[test]
fn the_real_token_arrives_on_the_descriptor_and_nowhere_else() {
    if !can_nest() {
        return;
    }
    let signed_in = Command::new("/usr/bin/security").args(["find-generic-password", "-s", "Claude Code-credentials"]).output().is_ok_and(|o| o.status.success());
    if !signed_in {
        eprintln!("skipped: no claude sign-in in this Keychain");
        return;
    }
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let root = dir.path().canonicalize().unwrap();
    let otto_home = root.join("home");
    std::fs::create_dir_all(otto_home.join("sandbox/runs")).unwrap();
    let otto = |args: &[&str]| {
        support::otto_command()
            .args(args)
            .env("OTTO_HOME", &otto_home)
            .env("TMUX_TMPDIR", &root)
            .env("OTTO_BIN", OTTO)
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_CHAT")
            .current_dir(&root)
            .output()
            .unwrap()
    };
    std::fs::write(otto_home.join("config.json"), serde_json::json!({"launchers": [{"name": "claude", "command": "/usr/bin/true"}]}).to_string()).unwrap();
    assert!(otto(&["run", "--goal", "g", "--name", "r", "--perpetual", "--period", "1h", "--detach", "none", "--workdir", root.to_str().unwrap()]).status.success());
    let list: serde_json::Value = serde_json::from_slice(&otto(&["state", "list", "--json"]).stdout).unwrap();
    let id = list[0]["id"].as_str().unwrap().to_string();
    std::fs::write(otto_home.join("sandbox/runs").join(format!("{id}.json")), serde_json::json!({"launcher": "sandbox", "write": [root]}).to_string()).unwrap();

    // Counts only. Nothing prints the token.
    let script = "printf 'fd3=%s\\n' \"$(wc -c <&3 | tr -d ' ')\"; printf 'again=%s\\n' \"$(wc -c <&3 2>/dev/null | tr -d ' ')\"; \
                  printf 'fd=%s gh=%s github=%s tmux=%s kept=%s\\n' \"$CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR\" \"${GH_TOKEN:-none}\" \"${GITHUB_TOKEN:-none}\" \"${TMUX:-none}\" \"$KEPT_DIR\"; \
                  printf 'oauth=%s\\n' \"${CLAUDE_CODE_OAUTH_TOKEN:-none}\"; printf 'gitcfg=%s\\n' \"${GIT_CONFIG_COUNT:-none}\"; \
                  printf 'ghfile=%s\\n' \"$(cat \"$HOME/.config/gh/hosts.yml\" 2>/dev/null | wc -c | tr -d ' ')\"; \
                  printf 'config=%s\\n' \"$CLAUDE_CONFIG_DIR\"";
    let out = support::otto_command()
        .args(["sandbox", "wrap", "--", "/bin/sh", "-c", script])
        .current_dir(&root)
        .env("OTTO_HOME", &otto_home)
        .env("OTTO_RUN_ID", &id)
        .env("OTTO_SESSION_ID", SESSION)
        .env("GH_TOKEN", "planted")
        .env("GITHUB_TOKEN", "planted")
        .env("GIT_CONFIG_COUNT", "1")
        .env("TMUX", "planted")
        .env("KEPT_DIR", "kept")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bytes: usize = said.lines().next().unwrap().trim_start_matches("fd3=").parse().unwrap();
    assert!(bytes > 20, "the token is on the descriptor: {bytes} bytes");
    assert!(said.contains("again=0"), "it can be read once: {said}");
    assert!(said.contains("fd=3 gh=none github=none tmux=none kept=kept"), "{said}");
    assert!(said.contains("oauth=none") && said.contains("gitcfg=none") && said.contains("ghfile=0"), "{said}");
    assert!(!said.contains("planted"), "{said}");
    let config = otto_home.canonicalize().unwrap().join("sandbox/claude").join(&id);
    assert!(said.contains(&format!("config={}\n", otto_home.join("sandbox/claude").join(&id).display())) || said.contains(&format!("config={}\n", config.display())), "claude's folder is the run's own: {said}");
    assert!(config.join(".claude.json").is_file(), "and laid out");
}

// --- check scripts (plans/v3.md, 1.3) ---------------------------------------------------------

/// A record naming the stub launcher these runs really have, so the run agrees with it.
fn checked_record(home: &Path) -> serde_json::Value {
    serde_json::json!({"launcher": "claude", "workdir": home.join("workspace"), "write": [home.join("workspace/repo")]})
}

/// What a check script needs of a scratch: a run whose last wake completed, so that a check stands in
/// front of its next one, and ways to make it due and see what poke did.
impl Scratch {
    fn run_json(&self) -> PathBuf {
        self.otto_home().join("runs").join(&self.id).join("run.json")
    }

    fn state(&self) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(self.run_json()).unwrap()).unwrap()
    }

    fn artifacts(&self) -> PathBuf {
        let dir = self.otto_home().join("runs").join(&self.id).join("artifacts");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The stub writes a handoff from here on, and the run's wake is run again so that it completes.
    fn complete_a_wake(&self) {
        write_exec(
            &self.root.join("stub"),
            &format!(
                "#!/bin/sh\nprintf '# h\\n## Goal\\ng\\n## Where this is\\nHere.\\n## Decided\\nn\\n## This wake did\\nn\\n## Next wake must\\nn\\n## Needs\\nnothing\\n## Don'\"'\"'t re-derive\\nn\\n' | '{OTTO}' state handoff \"$OTTO_RUN_ID\" --stdin >/dev/null\n"
            ),
        );
        let out = self.otto().args(["wake", &self.id]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        self.wait_for("sleeping");
    }

    /// Set the run's check, which `otto check` also runs once on the spot.
    fn set_check(&self, body: &str) -> Output {
        let script = self.root.join("check.sh");
        write_exec(&script, &format!("#!/bin/sh\n{body}\n"));
        self.otto().args(["check", &self.id, "--script"]).arg(&script).output().unwrap()
    }

    fn make_due(&self) {
        let mut state = self.state();
        assert_eq!(state["status"], "sleeping", "{state:#}");
        state["nextWakeAt"] = "2020-01-01T00:00:00Z".into();
        std::fs::write(self.run_json(), serde_json::to_string_pretty(&state).unwrap()).unwrap();
    }

    /// One pass of poke; the line it printed for the run under test.
    fn poke(&self) -> String {
        let out = self.otto().env("OTTO_BIN", OTTO).args(["poke", "--verbose", "--grace", "0"]).output().unwrap();
        let said = String::from_utf8_lossy(&out.stdout).to_string();
        said.lines().find(|l| l.contains(&self.id)).unwrap_or_else(|| panic!("poke said nothing of the run: {said}{}", String::from_utf8_lossy(&out.stderr))).to_string()
    }

    fn wait_for(&self, status: &str) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while self.state()["status"] != status {
            assert!(std::time::Instant::now() < until, "never {status}: {:#}\n{}", self.state(), self.journal());
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    /// Wait for the wake poke started to be over.
    fn wait_for_wakes(&self, wakes: usize) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let journal = self.journal();
            let finished = journal.matches("\"wake-complete\"").count() + journal.matches("\"wake-incomplete\"").count();
            if finished >= wakes && self.state()["status"] == "sleeping" {
                return;
            }
            assert!(std::time::Instant::now() < until, "never got to {wakes} wakes: {journal}");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    fn wakes_finished(&self) -> usize {
        let journal = self.journal();
        journal.matches("\"wake-complete\"").count() + journal.matches("\"wake-incomplete\"").count()
    }

    fn check_codes(&self) -> Vec<i64> {
        std::fs::read_to_string(self.otto_home().join("runs").join(&self.id).join("check-history.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["code"].as_i64().unwrap())
            .collect()
    }
}

/// Writes inside its own run, then tries what a check has no business doing, and says "nothing new".
const REACHING_CHECK: &str = r#"A="$OTTO_HOME/runs/$OTTO_RUN_ID/artifacts"
echo in > "$A/inside"
echo out > "$OTTO_HOME/runs/OTHER/planted"
echo out > "$HOME/workspace/other/planted"
echo out >> "$HOME/.zshenv"
cat "$HOME/.ssh/id_ed25519" > "$A/read-ssh" 2>/dev/null
cat "$HOME/workspace/other/secret" > "$A/read-other" 2>/dev/null
{ if (: >&3) 2>/dev/null; then echo descriptor-3-open; fi; env | grep -i claude; } > "$A/sign-in"
echo looked
exit 0"#;

/// The plan's required test: a check can't write outside its run, whoever runs it (`otto check` once, or poke in
/// front of a due wake), and reads no credentials and gets no sign-in. The same script for a run with no
/// record does all of it, as before.
#[test]
fn a_check_can_not_write_outside_its_run() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(checked_record);
    s.complete_a_wake();
    let artifacts = s.artifacts();
    let body = REACHING_CHECK.replace("OTHER", &s.other);
    let outside = [s.otto_home().join("runs").join(&s.other).join("planted"), s.home().join("workspace/other/planted")];
    let zshenv = s.home().join(".zshenv");
    let read = |name: &str| std::fs::read_to_string(artifacts.join(name)).unwrap_or_default();
    let held = |who: &str| {
        assert_eq!(read("inside"), "in\n", "{who}: its own run is still writable");
        for path in &outside {
            assert!(!path.exists(), "{who}: the check wrote {}", path.display());
        }
        assert_eq!(std::fs::read_to_string(&zshenv).unwrap(), "seed\n", "{who}: the check wrote .zshenv");
        assert_eq!(read("read-ssh"), "", "{who}: the check read an ssh key");
        assert_eq!(read("read-other"), "", "{who}: the check read another project");
        assert_eq!(read("sign-in"), "", "{who}: a check gets nothing of claude's");
    };

    let out = s.set_check(&body);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("nothing new"), "{}", String::from_utf8_lossy(&out.stdout));
    held("otto check");

    std::fs::remove_file(artifacts.join("inside")).unwrap();
    s.make_due();
    let poked = s.poke();
    assert!(poked.contains("no change; sleeping again"), "{poked}");
    held("otto poke");
    assert_eq!(s.state()["status"], "sleeping");
    assert_eq!(s.check_codes(), [0]);

    // No record: exactly as before, which is everything.
    std::fs::remove_file(s.record_path()).unwrap();
    s.make_due();
    let poked = s.poke();
    assert!(poked.contains("no change; sleeping again"), "{poked}");
    for path in &outside {
        assert!(path.exists(), "with no record the check writes {}", path.display());
    }
    assert_eq!(std::fs::read_to_string(&zshenv).unwrap(), "seed\nout\n");
    assert_eq!(read("read-ssh"), "seed\n");
    assert_eq!(read("read-other"), "seed\n");
    let out = s.set_check(&body);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read_to_string(&zshenv).unwrap(), "seed\nout\nout\n", "and so does `otto check`");
}

/// Under the sandbox the exit code still decides: 0 sleeps, 1 wakes, anything else wakes as an error.
#[test]
fn the_checks_exit_code_still_decides_under_the_sandbox() {
    if !can_nest() {
        return;
    }
    let s = Scratch::new(checked_record);
    s.complete_a_wake();
    let artifacts = s.artifacts();
    let planted = s.home().join("workspace/other/planted");
    std::fs::write(artifacts.join("says"), "exit 0\n").unwrap();
    let out = s.set_check("echo out > \"$HOME/workspace/other/planted\"\n. \"$OTTO_HOME/runs/$OTTO_RUN_ID/artifacts/says\"");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    s.make_due();
    let poked = s.poke();
    assert!(poked.contains("no change; sleeping again"), "{poked}");
    assert_eq!(s.state()["status"], "sleeping");

    let wakes = s.wakes_finished();
    std::fs::write(artifacts.join("says"), "exit 1\n").unwrap();
    s.make_due();
    let poked = s.poke();
    assert!(poked.contains("check script reported a change"), "{poked}");
    s.wait_for_wakes(wakes + 1);
    assert!(s.journal().contains("check-changed"), "{}", s.journal());

    std::fs::write(artifacts.join("says"), "exit 7\n").unwrap();
    s.make_due();
    let poked = s.poke();
    assert!(poked.contains("check script errored (exit 7"), "{poked}");
    s.wait_for_wakes(wakes + 2);
    assert!(s.journal().contains("check-error"), "{}", s.journal());

    assert_eq!(s.check_codes(), [0, 1, 7]);
    assert!(!planted.exists(), "each of those ran inside the sandbox");
}

/// `run.json` naming another launcher than the record: the check is not run, sandboxed or not. Poke starts a
/// wake, as for a check that errored, and `otto wake` refuses it as it always does. `otto check` says why.
/// None of this enters a profile, so it runs inside a sandbox too.
#[test]
fn a_check_is_not_run_for_a_run_that_disagrees_with_its_record() {
    let s = Scratch::new(checked_record);
    s.complete_a_wake();
    let marker = s.artifacts().join("ran");
    let config = s.otto_home().join("config.json");
    let mut with_second: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    with_second["launchers"].as_array_mut().unwrap().push(serde_json::json!({"name": "second", "command": s.root.join("stub").display().to_string()}));
    std::fs::write(&config, with_second.to_string()).unwrap();
    let body = "touch \"$OTTO_HOME/runs/$OTTO_RUN_ID/artifacts/ran\"\nexit 0";
    if can_nest() {
        assert!(s.set_check(body).status.success());
        assert!(marker.exists(), "while they agree, it runs");
        std::fs::remove_file(&marker).unwrap();
    } else {
        s.set_check(body);
    }

    let mut state = s.state();
    state["launcher"]["kind"] = "second".into();
    std::fs::write(s.run_json(), state.to_string()).unwrap();

    let out = s.set_check(body);
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "otto check ran it: {}", String::from_utf8_lossy(&out.stdout));
    assert!(said.contains("second") && said.contains("sandbox record"), "{said}");
    assert!(!marker.exists(), "otto check ran the script of a run that disagrees with its record");

    s.make_due();
    let poked = s.poke();
    assert!(poked.contains("not running its check") && poked.contains("second"), "{poked}");
    s.wait_for("blocked");
    assert!(!marker.exists(), "poke ran the script of a run that disagrees with its record");
    // The wake poke started refuses in two steps, blocked and then its question (sandbox::refuse), and it is
    // another process: wait for the second step rather than read between them.
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while s.state()["gate"]["slug"] != "sandbox-record" {
        assert!(std::time::Instant::now() < until, "blocked, and never asked: {:#}\n{}", s.state(), s.journal());
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(s.journal().contains("sandbox-refused") && s.journal().contains("second"), "{}", s.journal());
    assert!(s.check_codes().is_empty(), "no script ran, so there is no result");
}

/// A sandbox that can't be had is never "nothing new", and never a reason to run the script without one: the
/// script is not run and a wake is started to look.
#[test]
fn a_sandbox_that_will_not_start_is_not_nothing_new() {
    let s = Scratch::new(checked_record);
    s.complete_a_wake();
    let marker = s.artifacts().join("ran");
    let body = "touch \"$OTTO_HOME/runs/$OTTO_RUN_ID/artifacts/ran\"\nexit 0";
    s.set_check(body);
    std::fs::remove_file(&marker).ok();
    // Not a directory where profiles go: none can be written.
    std::fs::remove_dir_all(s.otto_home().join("sandbox/profiles")).ok();
    std::fs::write(s.otto_home().join("sandbox/profiles"), "").unwrap();

    let out = s.set_check(body);
    assert!(!out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(String::from_utf8_lossy(&out.stderr).contains("profile"), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!marker.exists(), "otto check ran it with no sandbox");

    let wakes = s.wakes_finished();
    s.make_due();
    let poked = s.poke();
    assert!(!poked.contains("no change"), "{poked}");
    assert!(poked.contains("not running its check") && poked.contains("profile"), "{poked}");
    s.wait_for_wakes(wakes + 1);
    assert!(!marker.exists(), "poke ran it with no sandbox");
    assert!(s.journal().contains("check-error"), "{}", s.journal());
}
