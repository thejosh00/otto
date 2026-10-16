//! Keeping otto's own vocabulary out of what a person reads.
//!
//! Two sections of the handoff are written for the person as well as the next wake: `Where this
//! is`, shown under the run in every list, and `Needs`. The harness asks for them in plain words,
//! and was ignored in practice — the gap between the request and what a wake wrote was most of what
//! made the list read like a log: "A person-pinned `check.sh` gates each wake … Wake 63: queue empty,
//! ticked." A wake is told three hundred lines of procedure in otto's terms, so that is the language
//! it narrates in, and a sentence of guidance does not outweigh it.
//!
//! So it is checked, the way the handoff's size cap is: `otto state handoff` refuses a handoff whose
//! person-facing sections use otto's words for its own machinery, and says which, and the wake
//! rewrites them before it stops. Guidance can be ignored; a refusal cannot.
//!
//! The list is short on purpose. It holds the words that only ever mean otto's machinery — a wrapped
//! task's own vocabulary ("the release gate", "the review period") is the work's, and refusing it
//! would make a wake fight its own subject. Paths, timestamps and commands go too: they belong in the
//! sections below, where the next wake looks for them.

/// The handoff sections a person reads, by heading.
pub const PERSON_SECTIONS: &[&str] = &["where this is", "needs"];

/// Words that only ever name otto's own machinery, matched as whole words, case-insensitively.
///
/// "tick" is not here, though otto uses it: it is ordinary English too ("ticked off the list"), and a
/// task about counting ticks was refused for saying so. A line that is all otto-speak names a wake
/// anyway.
const ENGINE_WORDS: &[&str] = &[
    "wake", "wakes", "woken", "noop-tick", "poke", "poked", "handoff", "nextwakeat", "gate", "gates", "check.sh",
    "run.json", "journal.jsonl", "arm-timer", "set-check",
];

/// Phrases that only ever name otto's machinery, matched anywhere.
const ENGINE_PHRASES: &[&str] = &["otto state", "check script", "safety net", "stall guard", "artifacts/"];

/// The otto-speak in `text`, each distinct find once, in the order found.
pub fn jargon(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut found: Vec<String> = Vec::new();
    let mut add = |what: String| {
        if !found.contains(&what) {
            found.push(what);
        }
    };
    // Whole words: split on anything that cannot be inside one of the listed words.
    for word in lower.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '.' || c == '_')) {
        let word = word.trim_matches(|c: char| c == '.' || c == '-' || c == '_');
        if ENGINE_WORDS.contains(&word) {
            add(format!("\"{word}\""));
        }
    }
    for phrase in ENGINE_PHRASES {
        if lower.contains(phrase) {
            add(format!("\"{}\"", phrase.trim_end_matches('/')));
        }
    }
    if has_timestamp(&lower) {
        add("a timestamp".to_string());
    }
    found
}

/// `2026-10-15T23:59` — an ISO timestamp, which a person reads as noise and the page shows better.
fn has_timestamp(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(16).any(|w| {
        let digits = |r: std::ops::Range<usize>| w[r].iter().all(u8::is_ascii_digit);
        digits(0..4) && w[4] == b'-' && digits(5..7) && w[7] == b'-' && digits(8..10) && w[10] == b't' && digits(11..13) && w[13] == b':' && digits(14..16)
    })
}

/// The body of each person-facing section of a handoff, by its heading as written.
fn person_sections(handoff: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in handoff.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(done) = current.take() {
                out.push(done);
            }
            let name = heading.split(['—', '-', '(']).next().unwrap_or(heading).trim().to_string();
            if PERSON_SECTIONS.contains(&name.to_lowercase().as_str()) {
                current = Some((name, String::new()));
            }
        } else if line.starts_with("# ") {
            if let Some(done) = current.take() {
                out.push(done);
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out.extend(current);
    out
}

/// Why a handoff's person-facing sections would be refused, or `None` when they read plainly.
pub fn check_handoff(handoff: &str) -> Option<String> {
    let problems: Vec<String> = person_sections(handoff)
        .into_iter()
        .filter_map(|(name, body)| {
            let found = jargon(&body);
            (!found.is_empty()).then(|| format!("\"{name}\" says {}", found.join(", ")))
        })
        .collect();
    if problems.is_empty() {
        return None;
    }
    Some(format!(
        "{} — those sections are what a person reads, in the list and on the run's page. Say where the work \
         stands in the work's own words (the queue, the PR, the report, what is next), with no wake numbers, \
         timestamps, paths or otto commands; those belong in the sections below, for the next wake",
        problems.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a real run's list line said, word for word — the reason this module exists.
    #[test]
    fn the_real_line_that_started_this_is_refused() {
        let line = "Perpetual hourly poll of the omnifactum queue, wrapping the `omni-work` skill. A person-pinned \
                    `check.sh` gates each wake (only wakes when `omni next` is non-empty, or after 24 no-change \
                    results as a safety net). Wake 63: queue empty, ticked. No task in flight, no repo lock held.";
        let found = jargon(line);
        for word in ["\"check.sh\"", "\"gates\"", "\"wake\"", "\"wakes\"", "\"safety net\""] {
            assert!(found.contains(&word.to_string()), "{word} not found in {found:?}");
        }
        let other = "Steady state: one curation run per wake. Wake 14 was started by hand at 2026-10-15T23:59Z.";
        assert!(jargon(other).contains(&"a timestamp".to_string()));
    }

    #[test]
    fn plain_words_and_the_work_s_own_vocabulary_pass() {
        for fine in [
            "Watching your omni next list. Nothing has been tagged for me since Sep 26. Next look at 9:03pm.",
            "The PR is green and waiting for Sam's review; the release gateway change is next.",
            "Ticket PROJ-123 is half done: the plan is approved and the tests are written.",
            "Woke up to a red build on main, fixed the flaky test, and it is green again.",
            "Second tick done; ticked off the last item on the list.",
            "nothing",
        ] {
            assert!(jargon(fine).is_empty(), "{fine:?} flagged as {:?}", jargon(fine));
        }
    }

    #[test]
    fn only_the_person_facing_sections_are_checked() {
        let handoff = "# 7 · working · wake 12\n\n## Goal\nonce an hour check for tasks\n\n\
                       ## Where this is\nWatching the queue; nothing new since Monday.\n\n\
                       ## This wake did\nRan check.sh, ticked, armed the timer.\n\n\
                       ## Next wake must\nRun the queue check; tick if empty.\n\n## Needs\nnothing\n";
        assert_eq!(check_handoff(handoff), None, "the machine sections may say whatever the next wake needs");

        let jargony = handoff.replace("Watching the queue; nothing new since Monday.", "Wake 12 ticked; gate 002 closed.");
        let why = check_handoff(&jargony).expect("refused");
        assert!(why.contains("\"Where this is\" says \"wake\", \"gate\""), "got: {why}");

        let needs = handoff.replace("## Needs\nnothing", "## Needs\nAnswer gate 003, or see artifacts/plan.md");
        assert!(check_handoff(&needs).unwrap().contains("\"Needs\" says \"gate\", \"artifacts\""));
    }

    /// The harness shows the headings with their gloss after a dash; a wake that copies that shape
    /// is still writing the same section.
    #[test]
    fn a_heading_with_its_gloss_is_the_same_section() {
        let handoff = "## Where this is — for the person\nWake 3 is done.\n";
        assert!(check_handoff(handoff).is_some());
    }
}
