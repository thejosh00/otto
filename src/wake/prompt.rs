//! The two halves of what a wake is told: a fixed operating procedure, and the little that is
//! specific to this wake.
//!
//! **A correction worth recording, because the opposite was believed first.** Two separate
//! `claude -p` invocations of identical *trivial* text (`--tools ""`, so the prompt was
//! essentially all prefix) did show cross-process cache reuse: the second read 49,480 tokens
//! from cache, created none, and cost $0.012 against $0.200. That looked like an argument that
//! prefix stability was worth 16× and therefore a correctness property.
//!
//! It is not. Measured on real wakes — including two wakes of the *same* run, whose prompt is
//! byte-identical — `cache_creation_input_tokens` is ~12,000 **every time** and cross-wake
//! reuse never appears. With tools enabled the request carries per-invocation content, so the
//! cacheable prefix is rewritten per wake regardless. The very large `cache_read` figures a
//! real wake reports (500k–800k tokens) are *within*-wake reuse across its own turns, which
//! happens no matter what otto does.
//!
//! So the honest cost model is: every wake pays a cold start of roughly 12k cache-creation
//! tokens, and what dominates after that is how many turns of real work it does. Trivial wakes
//! measured $0.18–$0.25. Keeping the harness byte-stable is still the right shape — embedding
//! it is simpler than resolving a path, and it cannot drift from the code that depends on it —
//! but it buys nothing measurable and must not be defended as though it did.
//!
//! The split, then, is for clarity rather than economy:
//!
//! - **`HARNESS`** is the operating procedure, `include_str!`d so there is no path to resolve
//!   and no file to go missing.
//! - **`user_prompt`** is what differs per wake. It stays small because the wake reads the rest
//!   off disk itself, which is cheaper than being told it.
//!
//! `wake::mod` journals `cacheRead`/`cacheCreation` per wake so this stays measured rather than
//! assumed — that accounting is what caught the mistake above.

/// The operating procedure every wake is given. Embedded, not read from disk — see above.
pub const HARNESS: &str = include_str!("../../harness/wake.md");

/// What this wake is told about itself. Deliberately small: paths, not contents. The wake
/// reads `run.json` and `handoff.md` with its own tools, which keeps this string — the part
/// that differs per wake and therefore cannot be cached — as short as possible.
pub fn user_prompt(run_id: &str, run_dir: &std::path::Path) -> String {
    format!(
        "Wake run `{run_id}`.\n\n\
         Its directory is `{dir}`, holding `run.json` (machine truth), `handoff.md` (the last \
         wake's account), `journal.jsonl`, `gates/` and `artifacts/`.\n\n\
         Orient from those, then do the next stretch of work and stop in a legal stopping \
         state, exactly as your instructions describe.",
        dir = run_dir.display(),
    )
}

/// A wake whose job is to check a done claim, not to do the work. Its own prompt, because nothing
/// in its orientation would otherwise make it anything but one more wake carrying on — and one
/// more wake carrying on is exactly the eye the claim was made by.
pub fn verify_prompt(run_id: &str, run_dir: &std::path::Path, claim: &crate::state::DoneClaim) -> String {
    let said = claim.reason.as_deref().map(|r| format!(", saying: {}", r.trim())).unwrap_or_default();
    format!(
        "Wake run `{run_id}` — to check a claim, not to do the work.\n\n\
         Its directory is `{dir}`. Wake {by} said the run is done{said}. It is not done until a wake that \
         did not do the work has checked: you. Follow \"Checking a done claim\" in your instructions, and \
         end with `otto state verify {run_id} --confirm` or `--reopen`.",
        dir = run_dir.display(),
        by = claim.by_wake,
    )
}

/// The wake after a check sent the run back: what was missing comes first, in the checker's words.
fn reopened_prompt(base: String, verdict: &crate::state::Verdict) -> String {
    format!(
        "{base}\n\nThe run said it was done, and a fresh check found it isn't. What it found, verbatim:\n\n\
         ---\n{}\n---\n\n\
         Fix that first, then claim done again.",
        verdict.reason.trim_end()
    )
}

/// What this wake is told about itself, given where the run stands: a checker when a done claim
/// is waiting, the fix first when the last wake was a check that reopened it, otherwise the plain
/// prompt.
pub fn for_wake(run_id: &str, run_dir: &std::path::Path, state: &crate::state::RunState) -> String {
    if let Some(claim) = &state.done_claim {
        return with_skill(verify_prompt(run_id, run_dir, claim), state);
    }
    let base = with_skill(user_prompt(run_id, run_dir), state);
    let last_wake = state.wake.as_ref().map(|w| w.n);
    match &state.last_verdict {
        Some(verdict) if !verdict.confirmed && Some(verdict.wake) == last_wake => reopened_prompt(base, verdict),
        _ => base,
    }
}

/// A wrapped skill's file, or that there isn't one: invoking a skill by name is the first thing a
/// wake does, and when it fails the wake is left to hunt for the skill's SKILL.md by itself — one
/// did, the first wake of a run whose skill had never been installed. Looked for every wake, since
/// a skill can be uninstalled under a run that was created with it. Nothing for a plugin's skill,
/// which otto doesn't look for (`install::skill_places`).
fn with_skill(prompt: String, state: &crate::state::RunState) -> String {
    let (crate::state::WrapKind::Skill, Some(name)) = (state.wraps.kind, &state.wraps.reference) else {
        return prompt;
    };
    let workdir = crate::config::workdir_for(state);
    let places = crate::install::skill_places(name, workdir.as_deref());
    if places.is_empty() {
        return prompt;
    }
    match places.iter().find(|p| p.is_file()) {
        Some(file) => format!("{prompt}\n\nIt wraps the skill `{name}`, whose file is `{}`.", file.display()),
        None => format!(
            "{prompt}\n\nIt wraps the skill `{name}`, which isn't installed: there is no `{}`, so invoking it by \
             name will fail.",
            places[0].display()
        ),
    }
}

/// A human answering a gate. Their words are passed through verbatim and clearly marked as
/// theirs, because they must be recorded character-for-character before anything acts on them —
/// paraphrasing a decision and then acting on the paraphrase two days later is how a run
/// drifts from what was actually approved. `otto answer` records them itself before the wake
/// starts, and then the gate is closed and there is nothing to close: telling the wake to
/// `close-gate` anyway sent it into "no gate is open", an exit 2 the harness says means stop —
/// three wakes reported it the day feedback landed. Only a wake started by hand with `--answer`
/// still has the gate open.
pub fn answer_prompt(base: &str, answer: &str, gate_open: bool) -> String {
    let record = if gate_open {
        "Record it with `otto state close-gate` before acting on it."
    } else {
        "otto has already recorded it, verbatim, and closed the gate (`gates/`): there is nothing to \
         close. Act on it."
    };
    format!(
        "{base}\n\nA person has answered the open gate. Their answer, verbatim:\n\n\
         ---\n{answer}\n---\n\n\
         {record}",
        answer = answer.trim_end(),
    )
}

/// A wake's prompt when the run's wait has run out (`state::Wait`): told first, since what it was
/// waiting on is the first thing it would otherwise check, and sleep on again.
pub fn with_overdue_wait(prompt: &str, wait: &crate::state::Wait) -> String {
    format!("{prompt}\n\nBefore anything else: {}.", crate::state::commands::overdue_wait(wait))
}

/// A wake's prompt with the person's notes appended (see `notes::for_wake`). Last, so that an
/// answer's "record it before acting" instruction is not buried under them.
pub fn with_notes(prompt: &str, notes: &str) -> String {
    format!("{prompt}\n\n{notes}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Hygiene, not economy — see the module note. Kept because a harness that varied per run
    /// would be a sign something run-specific had leaked into the shared procedure.
    #[test]
    fn the_harness_is_byte_identical_across_runs() {
        let one = HARNESS;
        let two = HARNESS;
        assert_eq!(one.as_bytes(), two.as_bytes());
        assert!(
            !HARNESS.contains("{}") && !HARNESS.contains("{run"),
            "the harness must not be a format template — the operating procedure is shared by \
             every run, so anything run-specific belongs in the user prompt"
        );
    }

    /// A run id in the shared procedure would mean one run's instructions leaking into
    /// another's, so assert the harness names no concrete run.
    #[test]
    fn the_harness_names_no_particular_run() {
        for probe in ["2026-", "run-", "/Users/"] {
            assert!(
                !HARNESS.contains(probe),
                "harness must carry nothing run-specific, found {probe:?}"
            );
        }
    }

    #[test]
    fn the_harness_carries_the_stopping_contract() {
        for required in ["awaiting_human", "sleeping", "handoff.md", "arm-timer", "open-gate"] {
            assert!(HARNESS.contains(required), "harness must cover {required}");
        }
    }

    /// A wrapped skill that names its own way of handing code back — its app's submit, a message
    /// saying the branch is ready to merge — left the wake to choose between it and a change, and
    /// to work out which from an earlier wake's precedent. "Changes to code" settles it where a wake
    /// handing code back reads: the code still goes back as a change, the skill's own part still
    /// happens beside it, and the skill's way of landing it does not.
    #[test]
    fn a_wrapped_way_back_still_goes_back_as_a_change() {
        let changes = HARNESS
            .split("\n## Changes to code\n")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .expect("the harness has a Changes to code section")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for said in ["its own way back", "still goes back as a change", "alongside the change", "not its way of landing"] {
            assert!(changes.contains(said), "Changes to code must say {said:?}");
        }
    }

    /// "Staying cheap" told a wake to delegate to subagents, and only shell jobs were said to die
    /// with the wake. Subagents run in the background by default, so a wake delegated four jobs,
    /// exited cleanly, and the next wake redid all of it (feedback from lectio-teach, wake 16). Where
    /// a wake reads to delegate, it must also read that it waits for them before it stops.
    #[test]
    fn a_wake_waits_for_its_subagents_before_it_stops() {
        let cheap = HARNESS
            .split("\n## Staying cheap\n")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .expect("the harness has a Staying cheap section")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for said in ["Wait for every subagent before you stop", "dies with this wake", "in the foreground"] {
            assert!(cheap.contains(said), "Staying cheap must say {said:?}");
        }
        let waiting = HARNESS.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            waiting.contains("a background subagent or a watcher can die when this wake ends"),
            "A background job is not a way back must name subagents"
        );
    }

    #[test]
    fn the_user_prompt_is_small_and_run_specific() {
        let prompt = user_prompt("2026-09-12-thing", Path::new("/tmp/otto/runs/2026-09-12-thing"));
        assert!(prompt.contains("2026-09-12-thing"));
        assert!(prompt.contains("/tmp/otto/runs/2026-09-12-thing"));
        assert!(
            prompt.len() < 600,
            "the per-wake half stays small because the wake reads state off disk itself; was {} bytes",
            prompt.len()
        );
    }

    #[test]
    fn an_answer_is_passed_through_verbatim() {
        let answer = "Approve, but rename the flag to --dry-run first.";
        let prompt = answer_prompt(&user_prompt("r1", Path::new("/tmp/r1")), answer, true);
        assert!(prompt.contains(answer), "the person's words must survive intact");
        assert!(prompt.contains("close-gate"));
        let recorded = answer_prompt(&user_prompt("r1", Path::new("/tmp/r1")), answer, false);
        assert!(recorded.contains(answer));
        assert!(!recorded.contains("otto state close-gate"), "a closed gate is not the wake's to close");
    }

    /// A wake of a skill's run is told where the skill's file is, so a failed invocation isn't
    /// followed by a hunt for it — and told plainly when it has been uninstalled since. A checker
    /// is told too: it reads the skill for what finished means.
    #[test]
    fn a_wrapped_skill_is_named_with_its_file() {
        let home = crate::paths::test_support::TempHome::new();
        let mut state = crate::state::test_run_state("r1");
        state.wraps = crate::state::Wraps { kind: crate::state::WrapKind::Skill, reference: Some("curate".into()) };
        state.launcher.workdir = Some(home.path().display().to_string());
        let skill = home.path().join(".claude/skills/curate/SKILL.md");

        let missing = for_wake("r1", Path::new("/tmp/r1"), &state);
        assert!(missing.starts_with(&user_prompt("r1", Path::new("/tmp/r1"))), "{missing}");
        assert!(missing.contains("`curate`, which isn't installed"), "{missing}");
        assert!(missing.contains(&skill.display().to_string()), "says where it isn't: {missing}");

        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(&skill, "---\nname: curate\n---\nbody").unwrap();
        let found = for_wake("r1", Path::new("/tmp/r1"), &state);
        assert!(found.contains(&format!("whose file is `{}`", skill.display())), "{found}");

        state.done_claim = Some(crate::state::DoneClaim {
            by_wake: 2,
            at: crate::clock::Timestamp::now(),
            reason: None,
            left_open: Vec::new(),
        });
        assert!(for_wake("r1", Path::new("/tmp/r1"), &state).contains(&skill.display().to_string()));

        state.done_claim = None;
        state.wraps.reference = Some("some-plugin:curate".into());
        assert_eq!(for_wake("r1", Path::new("/tmp/r1"), &state), user_prompt("r1", Path::new("/tmp/r1")));
    }

    /// A claim makes the next wake a checker, and it has to be told — nothing else it reads first
    /// would stop it carrying on as one more doer.
    #[test]
    fn a_waiting_claim_makes_the_wake_a_checker() {
        let mut state = crate::state::test_run_state("r1");
        state.wake = Some(crate::state::Wake {
            n: 3,
            started_at: crate::clock::Timestamp::now(),
            deadline_at: crate::clock::Timestamp::now(),
            launcher: "claude".into(),
            pid: None,
            session: None,
            outcome: None,
        });
        assert_eq!(for_wake("r1", Path::new("/tmp/r1"), &state), user_prompt("r1", Path::new("/tmp/r1")));

        state.done_claim = Some(crate::state::DoneClaim {
            by_wake: 3,
            at: crate::clock::Timestamp::now(),
            reason: Some("merged and cleaned up".into()),
            left_open: vec![],
        });
        let prompt = for_wake("r1", Path::new("/tmp/r1"), &state);
        assert!(prompt.contains("Wake 3 said the run is done, saying: merged and cleaned up"));
        assert!(prompt.contains("Checking a done claim"), "it is pointed at the procedure");
        assert!(prompt.contains("otto state verify r1 --confirm"));

        // The check reopened it (wake 4): the wake after is told what was missing, first.
        state.done_claim = None;
        state.wake.as_mut().unwrap().n = 4;
        state.last_verdict = Some(crate::state::Verdict {
            confirmed: false,
            reason: "the worktree at /w/x is still there".into(),
            wake: 4,
            at: crate::clock::Timestamp::now(),
        });
        let prompt = for_wake("r1", Path::new("/tmp/r1"), &state);
        assert!(prompt.contains("the worktree at /w/x is still there"));
        // Once a wake has been told, the next one is not told again.
        state.wake.as_mut().unwrap().n = 5;
        assert_eq!(for_wake("r1", Path::new("/tmp/r1"), &state), user_prompt("r1", Path::new("/tmp/r1")));
    }
}
