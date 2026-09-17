//! Parsing a gate's free-form question for the option names it offers.
//!
//! There is no schema for a gate's question — it's prose a wake wrote (`harness/wake.md`: "Name
//! the options plainly"). Two shapes exist in this codebase today: a flat list (`- Approve\n-
//! Revise`) and a bolded lead-in followed by its consequence (`wake::open_stuck_gate`'s `-
//! **Retry** — clear the counter...`). `parse_options` reads a top-level markdown list item
//! either way; a question with no list at all yields no options, which is the caller's cue to
//! fall back to free text rather than refuse an answer it cannot check.

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

/// The name inside one bullet line's text: the bolded lead-in if there is one, else everything
/// before the first "option — consequence" separator, else the whole line.
fn option_name(text: &str) -> String {
    if let Some(bold) = text.strip_prefix("**") {
        if let Some(end) = bold.find("**") {
            return bold[..end].trim().to_string();
        }
    }
    for sep in [" — ", " – ", " -- ", ": "] {
        if let Some((head, _)) = text.split_once(sep) {
            return head.trim().to_string();
        }
    }
    text.trim().to_string()
}

/// The option names a question's markdown list offers, in order, deduplicated
/// case-insensitively. Empty when the question is plain prose with no list.
pub fn parse_options(question: &str) -> Vec<String> {
    let mut options: Vec<String> = Vec::new();
    for line in question.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) else {
            continue;
        };
        let name = option_name(rest.trim());
        if name.is_empty() {
            continue;
        }
        if !options.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            options.push(name);
        }
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
