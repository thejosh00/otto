//! Parsing a gate's free-form question for the option names it offers, and which one is the
//! default.
//!
//! There is no schema for a gate's question — it's prose a wake wrote (`harness/wake.md`: "Name
//! the options plainly"). What the parser has to cope with is therefore whatever markdown a
//! model reaches for when it writes a list of choices, and a real gate showed how wide that is:
//! names in inline code, a bolded lead-in followed by its consequence, `*(default)*` markers,
//! lists inside blockquotes, and — the one that actually broke `--choice` — a second bullet list
//! further down ("Worth reading") that is not options at all.
//!
//! So two rules. **Markup is not part of a name**: `` `keep-polling` `` and `**Approve**` are the
//! options `keep-polling` and `Approve`, and a `--choice` typed plainly must match them. **An
//! `Options` heading scopes the list**: when the question has one, only the bullets under it are
//! options; without one, every bullet in the question is (the older behaviour, kept because
//! offering a stray extra is a smaller failure than rejecting the right answer). A question with
//! no list at all yields no options, which is the caller's cue to fall back to free text rather
//! than refuse an answer it cannot check.

use crate::error::OttoError;
use crate::state::Gate;
use std::path::Path;

/// The question text of a gate, verbatim — everything between `## Question` and `## Answer` in
/// its file. Not the whole file: the run/phase/asked-at header and the answer (once closed)
/// aren't part of the question a person is answering.
pub fn read_question(dir: &Path, gate: &Gate) -> Result<String, OttoError> {
    let text = std::fs::read_to_string(dir.join(&gate.file))
        .map_err(|e| OttoError::conflict(format!("gate file {} is unreadable: {e}", gate.file)))?;
    let after_heading = text.split("## Question").nth(1).unwrap_or(text.as_str());
    let question = after_heading.split("## Answer").next().unwrap_or(after_heading);
    Ok(question.trim().to_string())
}

/// A name with the inline markdown a wake wrapped it in — code, bold, italics — taken off both
/// ends. The name is what a person types back, and nobody types the backticks.
fn plain(text: &str) -> String {
    text.trim().trim_matches(|c| c == '`' || c == '*' || c == '_').trim().to_string()
}

/// The text of one bullet line, if the line is one. Indentation and blockquote markers are not
/// part of the list: a question quoted from a workflow file arrives as `> - **Approve**`.
fn bullet(line: &str) -> Option<&str> {
    let mut rest = line.trim_start();
    while let Some(unquoted) = rest.strip_prefix('>') {
        rest = unquoted.trim_start();
    }
    rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")).map(str::trim)
}

/// The text of a markdown heading, if the line is one (any level).
fn heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let text = trimmed.trim_start_matches('#');
    if text.len() == trimmed.len() || !text.starts_with(' ') {
        return None;
    }
    Some(text.trim())
}

/// The marker a wake puts beside the default option in a list: `- **Approve** *(default)*`.
const DEFAULT_MARKER: &str = "(default)";

/// `text` with any `(default)` marker removed, so it never ends up inside a name.
fn without_default_marker(text: &str) -> String {
    match text.to_ascii_lowercase().find(DEFAULT_MARKER) {
        Some(at) => format!("{}{}", &text[..at], &text[at + DEFAULT_MARKER.len()..]),
        None => text.to_string(),
    }
}

/// The name inside one bullet line's text: the bolded lead-in if there is one, else everything
/// before the first "option — consequence" separator, else the whole line — with markup and any
/// default marker removed.
fn option_name(text: &str) -> String {
    let text = without_default_marker(text);
    let text = text.trim();
    if let Some(bold) = text.strip_prefix("**") {
        if let Some(end) = bold.find("**") {
            return plain(&bold[..end]);
        }
    }
    for sep in [" — ", " – ", " -- ", ": "] {
        if let Some((head, _)) = text.split_once(sep) {
            return plain(head);
        }
    }
    plain(text)
}

/// The lines options are read from: the section under an `Options` (or `Choices`) heading when
/// the question has one, up to the next heading of any level; otherwise the whole question.
fn options_section(question: &str) -> Vec<&str> {
    let lines: Vec<&str> = question.lines().collect();
    let start = lines.iter().position(|line| {
        heading(line).is_some_and(|h| matches!(plain(h).to_ascii_lowercase().as_str(), "options" | "choices"))
    });
    match start {
        Some(at) => lines[at + 1..].iter().take_while(|line| heading(line).is_none()).copied().collect(),
        None => lines,
    }
}

/// The option names a question's markdown list offers, in order, deduplicated
/// case-insensitively. Empty when the question is plain prose with no list.
pub fn parse_options(question: &str) -> Vec<String> {
    let mut options: Vec<String> = Vec::new();
    for line in options_section(question) {
        let Some(text) = bullet(line) else { continue };
        let name = option_name(text);
        if name.is_empty() {
            continue;
        }
        if !options.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            options.push(name);
        }
    }
    options
}

/// Which of `options` the question names as its default, in the option's own casing, if it
/// names one that can be matched. Two spellings are recognised, because both are in use: a
/// `*(default)*` marker on the option's own bullet, and a `Default: <name>.` sentence anywhere in
/// the question — matched exactly, or as the unique option that begins with the stated name, so
/// "Default: stop." picks "Stop the run". Anything else is no default rather than a guess: this
/// is what Enter does in the interactive prompt, and the wrong answer there is worse than none.
pub fn parse_default(question: &str, options: &[String]) -> Option<String> {
    if options.is_empty() {
        return None;
    }
    for line in options_section(question) {
        if let Some(text) = bullet(line) {
            if text.to_ascii_lowercase().contains(DEFAULT_MARKER) {
                let name = option_name(text);
                return options.iter().find(|o| o.eq_ignore_ascii_case(&name)).cloned();
            }
        }
    }
    for line in question.lines() {
        let lower = line.to_ascii_lowercase();
        let Some(at) = lower.find("default:") else { continue };
        let rest = &line[at + "default:".len()..];
        // The stated name runs to the end of its sentence or clause.
        let stated = rest.split(['.', ',', ';', '\n']).next().unwrap_or("");
        let stated = stated.split(" — ").next().unwrap_or(stated);
        let stated = plain(stated);
        if stated.is_empty() {
            continue;
        }
        if let Some(exact) = options.iter().find(|o| o.eq_ignore_ascii_case(&stated)) {
            return Some(exact.clone());
        }
        let lower_stated = stated.to_ascii_lowercase();
        let mut starts = options.iter().filter(|o| o.to_ascii_lowercase().starts_with(&lower_stated));
        if let (Some(only), None) = (starts.next(), starts.next()) {
            return Some(only.clone());
        }
    }
    None
}

/// The options with the default moved to the front, for a caller that shows one command and
/// wants it to be the recommended one (`otto ls`'s `needs you:` line).
pub fn options_default_first(question: &str) -> Vec<String> {
    let mut options = parse_options(question);
    if let Some(default) = parse_default(question, &options) {
        options.retain(|o| o != &default);
        options.insert(0, default);
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;
    use crate::state::ops::open_gate;
    use crate::state::read_run;

    #[test]
    fn a_flat_bulleted_list_is_parsed_plainly() {
        assert_eq!(parse_options("Approve the plan?\n\n- Approve\n- Revise"), vec!["Approve", "Revise"]);
    }

    /// The exact shape `wake::open_stuck_gate` writes: a bolded name, an em dash, then prose.
    #[test]
    fn a_bolded_option_with_a_consequence_yields_just_the_name() {
        let question = "Run has failed.\n\n\
             - **Retry** — clear the counter and wake it again; right if you have fixed the cause.\n\
             - **Stop the run** — retire it (`otto stop x`).\n\
             - **Investigate** — read `otto logs x` and the journal first; the run stays blocked meanwhile.";
        assert_eq!(parse_options(question), vec!["Retry", "Stop the run", "Investigate"]);
    }

    #[test]
    fn prose_with_no_list_yields_no_options() {
        assert!(parse_options("Does the plan look OK? Reply with your thoughts.").is_empty());
    }

    #[test]
    fn duplicate_options_differing_only_in_case_are_not_repeated() {
        assert_eq!(parse_options("- approve\n- Approve\n- Revise"), vec!["approve", "Revise"]);
    }

    /// The question of a real gate, near-verbatim. Its `Worth reading` list was being offered as
    /// two more options, and its names were being offered with the backticks still on — so the
    /// plain `--choice keep-polling` was rejected in favour of five things nobody would type.
    const LIVE_GATE: &str = "# Keep polling the omni queue?\n\n\
        This run wakes roughly hourly and checks the queue. Since 2026-09-13 it has made about 30 \
        consecutive checks that returned an empty queue (see `artifacts/task-ledger.md`).\n\n\
        ## Options\n\n\
        - `keep-polling` — keep the hourly check running. The stall counter resets and the run will ask\n  \
          again after another ~24 empty checks.\n\
        - `stop-run` — end the run. Nothing will check the queue until you start a new run.\n\
        - `slow-down` — keep polling but less often (say, every 4 hours).\n\n\
        **Default: `keep-polling`.** No expiry: if unanswered, the run stays blocked and does nothing.\n\n\
        ## Worth reading\n\
        - `artifacts/task-ledger.md` — every task handled and its outcome.\n\
        - `artifacts/wake-cadence.md` — why wakes fire a little faster than hourly (benign).";

    #[test]
    fn an_options_heading_scopes_the_list_and_markup_is_not_part_of_a_name() {
        let options = parse_options(LIVE_GATE);
        assert_eq!(options, vec!["keep-polling", "stop-run", "slow-down"]);
        assert_eq!(parse_default(LIVE_GATE, &options).as_deref(), Some("keep-polling"));
    }

    /// `dev-flow.md`'s shape: quoted from a workflow file, bolded names, the default marked inline.
    #[test]
    fn a_blockquoted_list_with_an_inline_default_marker_parses() {
        let question = "> `KEY-1`: plan ready for review at `artifacts/plan.md`.\n>\n\
            > - **Approve** *(default)* — implement it as written.\n\
            > - **Revise** — say what to change; `plan` re-runs with your note.\n\
            > - **Different approach** — the rejected alternative, or one you name.\n\
            > - **Abandon** — the run ends `failed`.";
        let options = parse_options(question);
        assert_eq!(options, vec!["Approve", "Revise", "Different approach", "Abandon"]);
        assert_eq!(parse_default(question, &options).as_deref(), Some("Approve"));
    }

    /// otto's own budget gate says "Default: stop." against an option named "Stop the run".
    #[test]
    fn a_stated_default_matches_the_unique_option_it_begins() {
        let question = "Run has stopped because its budget is spent.\n\n\
            - **Raise the budget** — `otto state get x --field budget` shows the current one.\n\
            - **Stop the run** — `otto stop x`.\n\nDefault: stop. No further wakes will run.";
        let options = parse_options(question);
        assert_eq!(parse_default(question, &options).as_deref(), Some("Stop the run"));
        // An ambiguous or unmatched stated default is no default, not a guess.
        let vague = "- Stop now\n- Stop later\n\nDefault: stop.";
        assert_eq!(parse_default(vague, &parse_options(vague)), None);
        let unmatched = "- Approve\n- Revise\n\nDefault: rewrite.";
        assert_eq!(parse_default(unmatched, &parse_options(unmatched)), None);
    }

    #[test]
    fn inline_code_names_without_a_heading_still_come_out_plain() {
        assert_eq!(parse_options("Ship it?\n\n- `yes`\n- `no`"), vec!["yes", "no"]);
    }

    #[test]
    fn without_an_options_heading_every_bullet_is_still_an_option() {
        // The older, more permissive reading: a stray extra is better than a rejected answer.
        let question = "Context:\n- CI is red\n\nChoose:\n- Retry\n- Stop";
        assert_eq!(parse_options(question), vec!["CI is red", "Retry", "Stop"]);
    }

    #[test]
    fn the_default_leads_when_asked_for_first() {
        assert_eq!(options_default_first(LIVE_GATE)[0], "keep-polling");
        let question = "- Approve\n- Revise *(default)*";
        assert_eq!(options_default_first(question), vec!["Revise", "Approve"]);
        // No default: order untouched.
        assert_eq!(options_default_first("- A\n- B"), vec!["A", "B"]);
    }

    #[test]
    fn read_question_slices_out_just_the_question_section() {
        let _h = TempHome::new();
        test_init("gate-read", "a goal").unwrap();
        open_gate("gate-read", "review", "Approve the plan?\n\n- Approve\n- Revise", None).unwrap();
        let state = read_run("gate-read").unwrap();
        let gate = state.gate.unwrap();
        let dir = crate::paths::run_dir("gate-read").unwrap();
        let question = read_question(&dir, &gate).unwrap();
        assert!(question.contains("Approve the plan?"));
        assert!(question.contains("- Approve"));
        assert!(!question.contains("## Question"));
        assert!(!question.contains("unanswered"));
    }
}
