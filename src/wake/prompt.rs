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

/// A human answering a gate. Their words are passed through verbatim and clearly marked as
/// theirs, because the wake must record them character-for-character before acting on them —
/// paraphrasing a decision and then acting on the paraphrase two days later is how a run
/// drifts from what was actually approved.
pub fn answer_prompt(run_id: &str, run_dir: &std::path::Path, answer: &str) -> String {
    format!(
        "{base}\n\nA person has answered the open gate. Their answer, verbatim:\n\n\
         ---\n{answer}\n---\n\n\
         Record it with `otto state close-gate` before acting on it.",
        base = user_prompt(run_id, run_dir),
        answer = answer.trim_end(),
    )
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
        let prompt = answer_prompt("r1", Path::new("/tmp/r1"), answer);
        assert!(prompt.contains(answer), "the person's words must survive intact");
        assert!(prompt.contains("close-gate"));
    }
}
