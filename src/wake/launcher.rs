//! Building the command that runs a wake.
//!
//! Two things are deliberately not the same decision. **What runs the model** is this module:
//! it decides argv, how a skill gets resolved, and what network the wake sees. **How the wake
//! is backgrounded** is `Detach`, which is observability only. Conflating them leads to
//! believing a sandbox choice is a terminal-multiplexer choice.
//!
//! Both launchers end in the same tail of claude flags, because those flags encode facts about
//! what a wake *is* rather than preferences:
//!
//! - **No `-p`.** Print mode bills SDK credits instead of the subscription, which is the wrong
//!   budget for every wake otto has ever run. A wake is still non-interactive without it: with
//!   stdin at `/dev/null` and stdout on a pipe — which is exactly how `exec` spawns it — claude
//!   runs the prompt and exits on its own. Nothing has to drive a TUI, type into a composer, or
//!   kill anything at the end.
//! - **The prompt comes first**, before every flag, as a positional argument. Not cosmetic:
//!   `--add-dir`, `--allowed-tools` and `--disallowed-tools` are all variadic, so a prompt
//!   following any of them is swallowed as one more value and the wake runs with **no prompt at
//!   all** — silently, exit code 0, no output, nothing in the journal to explain it. `--add-dir`
//!   is the one that makes this non-negotiable rather than merely careful: otto always passes at
//!   least `--add-dir $OTTO_HOME`, so a trailing prompt is not an edge case, it is every wake.
//!   Verified against a real claude in both orders.
//! - `--session-id` — otto picks the id so it can find
//!   `~/.claude/projects/<cwd-slug>/<id>.jsonl` afterwards and read what the wake used. Without
//!   `-p` there is no JSON result document, so this is the whole of how a wake is measured; see
//!   `transcript`.
//! - `--permission-mode` — the run's recorded posture, and now the only thing standing between an
//!   unattended wake and a tool it should not run.
//!
//! Four flags that used to be here are gone with `-p`, because each one is documented as working
//! only with `--print` and is silently ignored otherwise: `--output-format json`,
//! `--permission-prompts none`, `--no-session-persistence`, and `--max-budget-usd`. Two of those
//! removals cost something real. There is no in-process dollar ceiling any more, so
//! `maxWakeMinutes` is the only thing that stops a runaway wake from inside; and sessions now
//! persist, which is merely disk — otto never resumes one, so the ephemerality that matters is
//! unaffected.
//!
//! **Every launcher is a prefix in front of claude.** `claude` is built in; anything else — a
//! sandbox, typically — is a command line in `$OTTO_HOME/config.json` (see `crate::config`) that
//! ends in running claude, so the prompt and the tail above go after it unchanged. What a sandbox
//! needs differently is only which directories to open, and that is its `grantFlag`: otto passes
//! it once per directory the wake needs, ahead of the command's first `--`, so it reaches the
//! sandbox rather than claude. Anything else a sandbox needs — seeing `~/.claude` so a skill
//! resolves, its network allowlist — belongs to its own profile.
//!
//! Whatever the launcher, otto always passes `--permission-mode`, so the run's recorded posture is
//! what takes effect even under a sandbox that would otherwise supply its own.
//!
//! On macOS every launcher is wrapped in `caffeinate` — see `sleep_guard`. That is a property of
//! the machine, not of the launcher, which is why it sits in front of whichever one runs.

use crate::config::LauncherDef;
use crate::error::OttoError;
use crate::state::RunState;

/// The claude flags every wake gets, whatever launcher carries them.
///
/// Flags only — the prompt is placed by `argv`, ahead of these. See the module doc on why it
/// cannot simply trail.
fn claude_tail(state: &RunState, session_id: &str) -> Vec<String> {
    let mut argv = vec![
        "--session-id".to_string(),
        session_id.to_string(),
        "--permission-mode".to_string(),
        state.permission.mode.as_claude_arg().to_string(),
        "--append-system-prompt".to_string(),
        super::prompt::HARNESS.to_string(),
    ];
    if !state.permission.allowed_tools.is_empty() {
        argv.push("--allowed-tools".to_string());
        argv.push(state.permission.allowed_tools.join(","));
    }
    if !state.permission.disallowed_tools.is_empty() {
        argv.push("--disallowed-tools".to_string());
        argv.push(state.permission.disallowed_tools.join(","));
    }
    argv
}

/// The wrapper that keeps the machine awake for exactly as long as a wake is working.
///
/// An unattended wake has nobody touching the keyboard, so a mac idles its way to sleep in the
/// middle of one. The wake does not fail cleanly when that happens — it comes back to a killed
/// child and a deadline it never had a chance to spend, which reads in the journal like a wake
/// that did nothing.
///
/// `caffeinate` holds the assertion for the lifetime of the process it runs, so wrapping the
/// launcher scopes it to the wake rather than to otto: nothing is held while a run sleeps between
/// wakes. `-i` is idle sleep, `-s` is system sleep while on AC power (ignored on battery).
/// Display sleep is deliberately left alone — nobody is watching the screen.
///
/// The deadline in `exec` still reaches claude: killing `caffeinate` takes down what it wraps.
fn sleep_guard() -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec!["caffeinate".to_string(), "-i".to_string(), "-s".to_string()]
    } else {
        Vec::new()
    }
}

/// The launcher this run's wakes go under, from the config as it is now — so a launcher edited
/// in `config.json` applies from the next wake, and one removed from it refuses the wake up front
/// rather than spawning something that is not there.
pub fn resolve(state: &RunState) -> Result<LauncherDef, OttoError> {
    crate::config::launcher(&state.launcher.kind)
        .map_err(|e| OttoError::usage(format!("run {}: {e}", state.id)))
}

/// The full command line for one wake. `session_id` is where the wake's usage will be read from
/// afterwards, so it is chosen by the caller before the spawn and recorded in `wake.session`.
pub fn argv(state: &RunState, launcher: &LauncherDef, prompt: &str, session_id: &str) -> Vec<String> {
    // The run directory is not optional. `$OTTO_HOME` is normally outside the working directory,
    // so without this the wake cannot read its own `run.json` or write its handoff — and nobody
    // is attached to widen the grant, so the wake burns its whole deadline achieving nothing.
    // Granting all of `$OTTO_HOME` rather than just this run's directory is deliberate: repo
    // locks live in a sibling (`runs/.locks`).
    let mut dirs = vec![crate::paths::otto_home().display().to_string()];
    dirs.extend(state.launcher.repos.iter().cloned());

    // The guard leads, so what follows is the launcher's own command line.
    let mut argv = sleep_guard();
    let mut command = launcher.command_words();
    if let Some(flag) = &launcher.grant_flag {
        // A sandbox only opens paths it was told about, so `--add-dir` alone would let claude try
        // to read a path the kernel has not made visible. The grants go before the command's
        // first `--`, where they are the sandbox's arguments and not claude's.
        let at = command.iter().position(|w| w == "--").unwrap_or(command.len());
        let grants = dirs.iter().flat_map(|d| [flag.clone(), d.clone()]);
        command.splice(at..at, grants);
    }
    argv.extend(command);
    // The prompt goes before every claude flag, `--add-dir` included — see the module doc. This
    // is the case that made the rule non-negotiable: `--add-dir` is variadic and otto always
    // passes at least one, so `claude --add-dir <home> <prompt>` fed the prompt to `--add-dir` as
    // a second directory and ran a wake with no prompt at all.
    argv.push(prompt.to_string());
    for dir in &dirs {
        argv.push("--add-dir".to_string());
        argv.push(dir.clone());
    }
    argv.extend(claude_tail(state, session_id));
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> RunState {
        crate::state::test_run_state("r1")
    }

    fn claude() -> LauncherDef {
        LauncherDef { name: "claude".into(), command: "claude".into(), grant_flag: None }
    }

    fn nono() -> LauncherDef {
        LauncherDef {
            name: "nono (sandbox)".into(),
            command: "nono run --profile nolabs-ai/claude -- claude".into(),
            grant_flag: Some("--allow".into()),
        }
    }

    /// A wrapper with no `--` of its own, whose trailing arguments go to claude.
    fn wrapper() -> LauncherDef {
        LauncherDef { name: "wrapper".into(), command: "wrap".into(), grant_flag: Some("--repo".into()) }
    }

    fn all() -> [LauncherDef; 3] {
        [claude(), nono(), wrapper()]
    }

    fn joined(argv: &[String]) -> String {
        argv.join(" ")
    }

    /// argv from the launcher's own command onward. The sleep guard is there on macOS and absent
    /// everywhere else, so every position-sensitive assertion counts from here rather than from 0.
    fn launched(argv: &[String]) -> Vec<String> {
        argv[sleep_guard().len()..].to_vec()
    }

    /// Where claude's own arguments start: straight after the launcher's command and grants.
    fn claude_args(argv: &[String], launcher: &LauncherDef) -> Vec<String> {
        let skip = launcher.command_words().len()
            + launcher.grant_flag.as_ref().map_or(0, |_| 2 * (1 + state().launcher.repos.len()));
        launched(argv)[skip..].to_vec()
    }

    /// A stand-in for the id the wake will be measured by.
    const SID: &str = "0f9a1c2b-3d4e-4f56-8789-abcdef012345";

    #[test]
    fn claude_gets_the_unattended_tail() {
        let argv = argv(&state(), &claude(), "wake r1", SID);
        assert_eq!(launched(&argv)[0], "claude");
        let line = joined(&argv);
        assert!(line.contains("--permission-mode acceptEdits"));
        assert!(line.contains(&format!("--session-id {SID}")), "usage is read back by session id");
    }

    /// The reason this whole change exists: print mode bills SDK credits, and every flag that only
    /// works with `-p` went with it. A regression here is expensive and completely silent.
    #[test]
    fn no_wake_is_launched_in_print_mode() {
        for launcher in all() {
            let mut s = state();
            // Set every input that used to add a print-only flag.
            s.budget.usd = 40.0;
            s.budget.spent_usd = 1.0;
            let argv = argv(&s, &launcher, "wake r1", SID);
            let name = &launcher.name;
            assert!(!argv.iter().any(|a| a == "-p" || a == "--print"), "{name} must not print");
            for flag in ["--output-format", "--permission-prompts", "--no-session-persistence", "--max-budget-usd"] {
                assert!(!argv.iter().any(|a| a == flag), "{flag} only works with -p, so {name} must not pass it");
            }
        }
    }

    /// The prompt must precede every variadic flag. `claude --add-dir /tmp '<prompt>'` eats the
    /// prompt as a second directory and runs with none at all — exit 0, no output, nothing in the
    /// journal to explain it. Verified against a real claude in both orders.
    ///
    /// `--add-dir` is why this is a test and not a comment: otto always passes at least one, so
    /// getting the order wrong breaks *every* wake rather than only the configured ones.
    #[test]
    fn the_prompt_comes_before_every_variadic_flag() {
        for launcher in all() {
            let mut s = state();
            s.launcher.repos = vec!["/w/a".into()];
            s.permission.allowed_tools = vec!["Read".into()];
            s.permission.disallowed_tools = vec!["WebFetch".into()];
            let argv = argv(&s, &launcher, "wake r1", SID);
            let prompt = argv.iter().position(|a| a == "wake r1").expect("the prompt must be there");
            for variadic in ["--add-dir", "--allowed-tools", "--disallowed-tools"] {
                if let Some(idx) = argv.iter().position(|a| a == variadic) {
                    assert!(prompt < idx, "{}: {variadic} would swallow a prompt that follows it", launcher.name);
                }
            }
        }
    }

    /// Nothing claude parses may precede the prompt, whatever the launcher puts in front.
    #[test]
    fn the_prompt_leads_claudes_own_arguments() {
        for launcher in all() {
            let argv = argv(&state(), &launcher, "wake r1", SID);
            assert_eq!(claude_args(&argv, &launcher)[0], "wake r1", "{}", launcher.name);
        }
    }

    /// A configured launcher is its command, then the same tail claude gets on its own.
    #[test]
    fn a_configured_launcher_carries_the_same_tail() {
        let argv = argv(&state(), &nono(), "wake r1", SID);
        assert_eq!(&launched(&argv)[..3], ["nono", "run", "--profile"]);
        let dashdash = argv.iter().position(|a| a == "--").expect("the command's own --");
        assert_eq!(argv[dashdash + 1], "claude");
        assert_eq!(argv[dashdash + 2], "wake r1");
        let tail = joined(&argv[dashdash + 1..]);
        assert!(tail.contains(&format!("--session-id {SID}")));
        // Passing a mode is what stops a sandbox substituting its own, so the run's recorded
        // posture is the one that takes effect.
        assert!(tail.contains("--permission-mode acceptEdits"));
    }

    #[test]
    fn the_harness_rides_in_the_system_prompt_not_the_user_prompt() {
        let argv = argv(&state(), &claude(), "wake r1", SID);
        let idx = argv.iter().position(|a| a == "--append-system-prompt").unwrap();
        assert_eq!(argv[idx + 1], super::super::prompt::HARNESS);
        // The cacheable prefix must not be the thing that varies per wake.
        assert!(argv.iter().any(|a| a == "wake r1"), "the prompt is its own argument");
    }

    #[test]
    fn repos_get_add_dir_and_a_sandbox_grant_before_the_double_dash() {
        let mut s = state();
        s.launcher.repos = vec!["/w/a".into(), "/w/b".into()];
        assert!(joined(&argv(&s, &claude(), "p", SID)).contains("--add-dir /w/a --add-dir /w/b"));

        let argv = argv(&s, &nono(), "p", SID);
        let line = joined(&argv);
        assert!(line.contains("--allow /w/a --allow /w/b -- claude"), "{line}");
        assert!(line.contains("--add-dir /w/a"), "claude is still told, as well as the sandbox");
    }

    /// With no `--` in the command, grants go at its end — still ahead of the prompt.
    #[test]
    fn grants_follow_a_command_without_a_double_dash() {
        let mut s = state();
        s.launcher.repos = vec!["/w/a".into()];
        let argv = argv(&s, &wrapper(), "p", SID);
        let home = crate::paths::otto_home().display().to_string();
        assert_eq!(&launched(&argv)[..6], ["wrap", "--repo", home.as_str(), "--repo", "/w/a", "p"]);
    }

    /// Discovered the hard way, by watching a real wake get `Read` denied on its own
    /// `run.json`: the run directory must always be granted, under any launcher.
    #[test]
    fn the_run_directory_is_always_granted() {
        let _h = crate::paths::test_support::TempHome::new();
        let home = crate::paths::otto_home().display().to_string();
        let c = joined(&argv(&state(), &claude(), "p", SID));
        assert!(c.contains(&format!("--add-dir {home}")), "claude wake needs its run dir");
        let n = joined(&argv(&state(), &nono(), "p", SID));
        assert!(n.contains(&format!("--allow {home}")), "a sandbox needs a grant, not only --add-dir");
    }

    /// A run whose launcher is no longer in the config is refused, not spawned.
    #[test]
    fn a_launcher_missing_from_the_config_is_refused() {
        let _h = crate::paths::test_support::TempHome::new();
        let mut s = state();
        assert_eq!(resolve(&s).unwrap().name, "claude");
        s.launcher.kind = "nono (sandbox)".into();
        assert!(resolve(&s).unwrap_err().to_string().contains("no launcher"));
    }

    /// A mac that falls asleep mid-wake kills the child and burns the deadline for nothing, so
    /// the launcher — any one — runs under `caffeinate`. Elsewhere there is nothing to wrap.
    #[test]
    fn a_mac_is_kept_awake_for_the_length_of_the_wake() {
        for launcher in all() {
            let argv = argv(&state(), &launcher, "wake r1", SID);
            if cfg!(target_os = "macos") {
                assert_eq!(&argv[..3], ["caffeinate", "-i", "-s"], "{} must not let the mac sleep", launcher.name);
                assert_eq!(argv[3], launcher.command_words()[0], "the launcher follows the guard");
            } else {
                assert_ne!(argv[0], "caffeinate", "caffeinate is macOS-only");
            }
        }
    }

    #[test]
    fn tool_lists_are_comma_joined_when_present() {
        let mut s = state();
        s.permission.allowed_tools = vec!["Bash(git *)".into(), "Read".into()];
        s.permission.disallowed_tools = vec!["WebFetch".into()];
        let line = joined(&argv(&s, &claude(), "p", SID));
        assert!(line.contains("--allowed-tools Bash(git *),Read"));
        assert!(line.contains("--disallowed-tools WebFetch"));
    }
}
