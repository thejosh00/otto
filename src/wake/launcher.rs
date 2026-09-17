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
//! **yolo** (a nono-based sandbox for unattended Claude) takes everything after `--` and passes
//! it to claude, so the tail is identical there. Two things differ, and both are properties of
//! the sandbox rather than choices:
//!
//! - It supplies its own `--dangerously-skip-permissions` *unless* `--permission-mode` is
//!   passed, in which case the mode wins. otto always passes a mode, so the run's recorded
//!   posture is what takes effect.
//! - `~/.claude` is not visible inside the sandbox, so a skill cannot be found there. Skills
//!   arrive through `yolo --skills <dir>`, which is why `launcher.skillDirs` exists and why a
//!   `--skill` run under yolo without one is refused up front rather than failing mid-wake.
//!
//! On macOS both launchers are wrapped in `caffeinate` — see `sleep_guard`. That is a property of
//! the machine, not of the launcher, which is why it sits in front of either one.

use crate::error::OttoError;
use crate::state::{LauncherKind, RunState, WrapKind};

/// The claude flags every wake gets, whatever launcher carries them.
///
/// Flags only — the prompt is placed by `argv`, because *where* it goes differs between the two
/// launchers even though it leads claude's own arguments in both. See the module doc on why it
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

/// Refuse a run whose launcher cannot possibly resolve what it wraps. Cheap to check here,
/// expensive to discover an hour into a wake that quietly did nothing.
pub fn check_resolvable(state: &RunState) -> Result<(), OttoError> {
    if state.launcher.kind == LauncherKind::Yolo
        && state.wraps.kind == WrapKind::Skill
        && state.launcher.skill_dirs.is_empty()
    {
        return Err(OttoError::usage(format!(
            "run {} wraps the skill `{}` under yolo but has no --skills-dir: the sandbox cannot \
             see ~/.claude, so the skill would not resolve. Give the directory that contains it.",
            state.id,
            state.wraps.reference.as_deref().unwrap_or("?"),
        )));
    }
    Ok(())
}

/// The full command line for one wake. `session_id` is where the wake's usage will be read from
/// afterwards, so it is chosen by the caller before the spawn and recorded in `wake.session`.
pub fn argv(state: &RunState, prompt: &str, session_id: &str) -> Vec<String> {
    // The guard leads, so what follows is the launcher's own command line either way.
    let guard = sleep_guard();
    match state.launcher.kind {
        LauncherKind::Claude => {
            // The prompt goes before every flag, `--add-dir` included — see the module doc. This
            // is the case that made the rule non-negotiable: `--add-dir` is variadic and otto
            // always passes at least one, so `claude --add-dir <home> <prompt>` fed the prompt to
            // `--add-dir` as a second directory and ran a wake with no prompt at all.
            let mut argv = guard;
            argv.push("claude".to_string());
            argv.push(prompt.to_string());
            // The run directory is not optional. `$OTTO_HOME` is normally outside the working
            // directory, so without this the wake cannot read its own `run.json` or write its
            // handoff — and nobody is attached to widen the grant, so the wake burns its whole
            // deadline achieving nothing.
            // Granting all of `$OTTO_HOME` rather than just this run's directory is deliberate:
            // repo locks live in a sibling (`runs/.locks`).
            argv.push("--add-dir".to_string());
            argv.push(crate::paths::otto_home().display().to_string());
            for repo in &state.launcher.repos {
                argv.push("--add-dir".to_string());
                argv.push(repo.clone());
            }
            argv.extend(claude_tail(state, session_id));
            argv
        }
        LauncherKind::Yolo => {
            let mut argv = guard;
            argv.push("yolo".to_string());
            // Same reason as above, but under yolo the grant must be a sandbox grant: nono
            // only opens paths it was told about, so `--add-dir` alone would let claude try to
            // read a path the kernel has not made visible.
            argv.push("--repo".to_string());
            argv.push(crate::paths::otto_home().display().to_string());
            for repo in &state.launcher.repos {
                // Under yolo this grants both sandbox and tool access; --add-dir alone
                // would let claude try to read a path nono has not opened.
                argv.push("--repo".to_string());
                argv.push(repo.clone());
            }
            for dir in &state.launcher.skill_dirs {
                argv.push("--skills".to_string());
                argv.push(dir.clone());
            }
            // Under yolo everything after `--` is claude's, so the prompt leads there instead —
            // same rule, different position. yolo's own flags are all before the `--`.
            argv.push("--".to_string());
            argv.push(prompt.to_string());
            argv.extend(claude_tail(state, session_id));
            argv
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Wraps;

    fn state(kind: LauncherKind, wraps: WrapKind) -> RunState {
        let mut state = crate::state::test_run_state("r1");
        state.launcher.kind = kind;
        state.wraps = Wraps {
            kind: wraps,
            reference: match wraps {
                WrapKind::Skill => Some("manage-pr".into()),
                WrapKind::Instructions => Some("/tmp/flow.md".into()),
                WrapKind::GoalOnly => None,
            },
        };
        state
    }

    fn joined(argv: &[String]) -> String {
        argv.join(" ")
    }

    /// argv from the launcher's own command onward. The sleep guard is there on macOS and absent
    /// everywhere else, so every position-sensitive assertion counts from here rather than from 0.
    fn launched(argv: &[String]) -> Vec<String> {
        argv[sleep_guard().len()..].to_vec()
    }

    /// A stand-in for the id the wake will be measured by.
    const SID: &str = "0f9a1c2b-3d4e-4f56-8789-abcdef012345";

    #[test]
    fn claude_gets_the_unattended_tail() {
        let argv = argv(&state(LauncherKind::Claude, WrapKind::GoalOnly), "wake r1", SID);
        assert_eq!(launched(&argv)[0], "claude");
        let line = joined(&argv);
        assert!(line.contains("--permission-mode acceptEdits"));
        assert!(line.contains(&format!("--session-id {SID}")), "usage is read back by session id");
    }

    /// The reason this whole change exists: print mode bills SDK credits, and every flag that only
    /// works with `-p` went with it. A regression here is expensive and completely silent.
    #[test]
    fn no_wake_is_launched_in_print_mode() {
        for kind in [LauncherKind::Claude, LauncherKind::Yolo] {
            let mut s = state(kind, WrapKind::GoalOnly);
            // Set every input that used to add a print-only flag.
            s.budget.usd = 40.0;
            s.budget.spent_usd = 1.0;
            let argv = argv(&s, "wake r1", SID);
            assert!(!argv.iter().any(|a| a == "-p" || a == "--print"), "{kind:?} must not print");
            for flag in ["--output-format", "--permission-prompts", "--no-session-persistence", "--max-budget-usd"] {
                assert!(!argv.iter().any(|a| a == flag), "{flag} only works with -p, so {kind:?} must not pass it");
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
        for kind in [LauncherKind::Claude, LauncherKind::Yolo] {
            let mut s = state(kind, WrapKind::GoalOnly);
            s.launcher.repos = vec!["/w/a".into()];
            s.permission.allowed_tools = vec!["Read".into()];
            s.permission.disallowed_tools = vec!["WebFetch".into()];
            let argv = argv(&s, "wake r1", SID);
            let prompt = argv.iter().position(|a| a == "wake r1").expect("the prompt must be there");
            // Every variadic flag claude parses. `--repo`/`--skills` are yolo's own and sit before
            // the `--`, where they can only ever eat yolo's arguments.
            for variadic in ["--add-dir", "--allowed-tools", "--disallowed-tools"] {
                if let Some(idx) = argv.iter().position(|a| a == variadic) {
                    assert!(prompt < idx, "{kind:?}: {variadic} would swallow a prompt that follows it");
                }
            }
        }
    }

    /// Nothing claude parses may precede the prompt, whatever the launcher puts in front.
    #[test]
    fn the_prompt_leads_claudes_own_arguments() {
        let mut c = state(LauncherKind::Claude, WrapKind::GoalOnly);
        c.launcher.repos = vec!["/w/a".into()];
        assert_eq!(launched(&argv(&c, "wake r1", SID))[1], "wake r1", "straight after `claude`");

        let mut y = state(LauncherKind::Yolo, WrapKind::GoalOnly);
        y.launcher.repos = vec!["/w/a".into()];
        let argv = argv(&y, "wake r1", SID);
        let dashdash = argv.iter().position(|a| a == "--").unwrap();
        assert_eq!(argv[dashdash + 1], "wake r1", "straight after the `--`");
    }

    /// The whole point of the yolo seam: same tail, wrapped, after a `--`.
    #[test]
    fn yolo_passes_the_same_tail_after_a_double_dash() {
        let argv = argv(&state(LauncherKind::Yolo, WrapKind::GoalOnly), "wake r1", SID);
        assert_eq!(launched(&argv)[0], "yolo");
        let dashdash = argv.iter().position(|a| a == "--").expect("yolo needs a --");
        // The prompt leads the tail under yolo too, for the same variadic reason.
        assert_eq!(argv[dashdash + 1], "wake r1");
        let tail = joined(&argv[dashdash + 1..]);
        assert!(tail.contains(&format!("--session-id {SID}")));
        // Passing a mode is what stops yolo adding --dangerously-skip-permissions, so the
        // run's recorded posture is the one that takes effect.
        assert!(tail.contains("--permission-mode acceptEdits"));
    }

    #[test]
    fn the_harness_rides_in_the_system_prompt_not_the_user_prompt() {
        let argv = argv(&state(LauncherKind::Claude, WrapKind::GoalOnly), "wake r1", SID);
        let idx = argv.iter().position(|a| a == "--append-system-prompt").unwrap();
        assert_eq!(argv[idx + 1], super::super::prompt::HARNESS);
        // The cacheable prefix must not be the thing that varies per wake.
        assert!(argv.iter().any(|a| a == "wake r1"), "the prompt is its own argument");
    }

    #[test]
    fn repos_use_add_dir_for_claude_and_repo_for_yolo() {
        let mut s = state(LauncherKind::Claude, WrapKind::GoalOnly);
        s.launcher.repos = vec!["/w/a".into(), "/w/b".into()];
        assert!(joined(&argv(&s, "p", SID)).contains("--add-dir /w/a --add-dir /w/b"));

        let mut y = state(LauncherKind::Yolo, WrapKind::GoalOnly);
        y.launcher.repos = vec!["/w/a".into()];
        let line = joined(&argv(&y, "p", SID));
        assert!(line.contains("--repo /w/a"));
        assert!(!line.contains("--add-dir"), "yolo grants sandbox access with --repo");
    }

    /// Discovered the hard way, by watching a real wake get `Read` denied on its own
    /// `run.json`: the run directory must always be granted, under either launcher.
    #[test]
    fn the_run_directory_is_always_granted() {
        let _h = crate::paths::test_support::TempHome::new();
        let home = crate::paths::otto_home().display().to_string();
        let c = joined(&argv(&state(LauncherKind::Claude, WrapKind::GoalOnly), "p", SID));
        assert!(c.contains(&format!("--add-dir {home}")), "claude wake needs its run dir");
        let y = joined(&argv(&state(LauncherKind::Yolo, WrapKind::GoalOnly), "p", SID));
        assert!(y.contains(&format!("--repo {home}")), "yolo needs a sandbox grant, not --add-dir");
    }

    #[test]
    fn yolo_skill_dirs_are_passed_as_skills() {
        let mut y = state(LauncherKind::Yolo, WrapKind::Skill);
        y.launcher.skill_dirs = vec!["/w/otto/skills".into()];
        let argv = argv(&y, "p", SID);
        let line = joined(&argv);
        assert!(line.contains("--skills /w/otto/skills"));
        // Everything yolo needs must precede the --.
        let dashdash = argv.iter().position(|a| a == "--").unwrap();
        let skills = argv.iter().position(|a| a == "--skills").unwrap();
        assert!(skills < dashdash);
    }

    /// A skill under yolo with nowhere to find it is refused now, not discovered later.
    #[test]
    fn a_skill_under_yolo_without_a_skills_dir_is_refused() {
        let y = state(LauncherKind::Yolo, WrapKind::Skill);
        let err = check_resolvable(&y).expect_err("must refuse");
        assert!(err.to_string().contains("--skills-dir"));

        let mut ok = y;
        ok.launcher.skill_dirs = vec!["/w/skills".into()];
        assert!(check_resolvable(&ok).is_ok());
    }

    #[test]
    fn the_same_wrap_under_plain_claude_is_fine_without_skill_dirs() {
        // ~/.claude/skills is visible without a sandbox, so nothing to declare.
        assert!(check_resolvable(&state(LauncherKind::Claude, WrapKind::Skill)).is_ok());
    }

    /// A mac that falls asleep mid-wake kills the child and burns the deadline for nothing, so
    /// the launcher — either one — runs under `caffeinate`. Elsewhere there is nothing to wrap.
    #[test]
    fn a_mac_is_kept_awake_for_the_length_of_the_wake() {
        for kind in [LauncherKind::Claude, LauncherKind::Yolo] {
            let argv = argv(&state(kind, WrapKind::GoalOnly), "wake r1", SID);
            if cfg!(target_os = "macos") {
                assert_eq!(&argv[..3], ["caffeinate", "-i", "-s"], "{kind:?} must not let the mac sleep");
                assert!(matches!(argv[3].as_str(), "claude" | "yolo"), "the launcher follows the guard");
            } else {
                assert_ne!(argv[0], "caffeinate", "caffeinate is macOS-only");
            }
        }
    }

    #[test]
    fn tool_lists_are_comma_joined_when_present() {
        let mut s = state(LauncherKind::Claude, WrapKind::GoalOnly);
        s.permission.allowed_tools = vec!["Bash(git *)".into(), "Read".into()];
        s.permission.disallowed_tools = vec!["WebFetch".into()];
        let line = joined(&argv(&s, "p", SID));
        assert!(line.contains("--allowed-tools Bash(git *),Read"));
        assert!(line.contains("--disallowed-tools WebFetch"));
    }
}
