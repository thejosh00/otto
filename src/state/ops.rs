//! The engine's own writer surface, in plain values — no `clap`, no CLI conventions.
//!
//! `state::commands` is `otto state`'s CLI surface: it resolves `--question`/`--question-file`/
//! `--stdin` down to a string, `--expires-at`/`--expires-in` down to a `Timestamp`, and calls
//! here. Before this module existed, an internal caller that already had the string in hand had
//! no way to say so — `wake::open_stuck_gate` and `wake::block_on_budget` built a full
//! `OpenGateArgs` (seven fields) and, because the CLI layer only reads a question from stdin, a
//! file, or an inline flag, first wrote their in-memory question out to
//! `artifacts/gate-*.md` purely so they would have a file path to hand back in. That file served
//! no purpose once it existed — the gate's own file under `gates/` already carries the question
//! verbatim — so this module removes it by giving those callers a function that takes the
//! string directly.
//!
//! The CLI structs are a client of this, not the other way around: `otto state` is one caller
//! among several, not the shape the engine itself is written in.

use super::{transaction, write_atomic, Gate, Status};
use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::event::Event;

/// The next gate id to use: one past the highest existing id, scanning filenames whose
/// name starts with **3 or more** ASCII digits followed by `-`. Fixed-width (exactly 3)
/// would silently wrap at id 1000 and overwrite an already-answered gate — the bug this
/// scan is written to avoid.
fn next_gate_number(gates_dir: &std::path::Path) -> Result<u64, OttoError> {
    let mut max = 0u64;
    if gates_dir.is_dir() {
        for entry in std::fs::read_dir(gates_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some((digits, _rest)) = name.split_once('-') {
                if digits.len() >= 3 && digits.bytes().all(|b| b.is_ascii_digit()) {
                    if let Ok(n) = digits.parse::<u64>() {
                        max = max.max(n);
                    }
                }
            }
        }
    }
    Ok(max + 1)
}

/// Open a gate with `question` verbatim, gated on nothing already being open. Returns the gate
/// file's path, for a caller that wants to say where it lives — the CLI prints it; an internal
/// caller usually just needs the write to have happened.
pub fn open_gate(id: &str, slug: &str, question: &str, expires_at: Option<Timestamp>) -> Result<String, OttoError> {
    if question.trim().is_empty() {
        return Err(OttoError::usage("a gate question must not be empty — it has to be answerable cold"));
    }
    let mut gate_file_path = String::new();
    transaction(id, |path, state| {
        if let Some(existing) = &state.gate {
            return Err(OttoError::conflict(format!("gate {} is already open — close it first", existing.id)));
        }
        let gates_dir = path.join("gates");
        std::fs::create_dir_all(&gates_dir)?;
        let used = next_gate_number(&gates_dir)?;
        let gate_id = format!("{used:03}");
        let slug = crate::clock::slugify(slug, "gate");
        let gate_file = gates_dir.join(format!("{gate_id}-{slug}.md"));
        let asked = Timestamp::now();
        let deadline = match &expires_at {
            Some(e) => format!("- expires: {e} — unanswered after this is recorded as no answer\n"),
            None => String::new(),
        };
        write_atomic(
            &gate_file,
            &format!(
                "# Gate {gate_id} — {slug}\n\n- run: `{}`\n- phase: `{}`\n- asked: {asked}\n{deadline}\n## Question\n\n{}\n\n## Answer\n\n_unanswered_\n",
                state.id,
                state.phase,
                question.trim_end(),
            ),
        )?;
        state.gate = Some(Gate {
            id: gate_id.clone(),
            slug: slug.clone(),
            file: format!("gates/{gate_id}-{slug}.md"),
            asked_at: asked,
            answered_at: None,
            expires_at,
        });
        // A run that is already `blocked` stays blocked: the gate is how a person unblocks
        // it, and flattening that to `awaiting_human` would lose the distinction between
        // "stopped because it could not proceed" and "reached a normal decision point".
        // Both are legal stopping states (DESIGN.md §5.2) precisely because they differ.
        if state.status != Status::Blocked {
            state.status = Status::AwaitingHuman;
        }
        if let Some(e) = expires_at {
            // The status stays awaiting_human — a person may still answer, and a late
            // answer wins — but a wake comes at the deadline to deal with the silence.
            state.next_wake_at = Some(e);
        }
        gate_file_path = gate_file.display().to_string();
        crate::event::record(path, &Event::GateOpened { gate: gate_id, slug, phase: state.phase.clone(), expires_at })
    })?;
    Ok(gate_file_path)
}

/// Close the open gate. `answer: None` means expired — nobody answered, and the gate must carry
/// its own `expiresAt` or this refuses: silence only has a meaning where an expiry said what it
/// was in advance.
pub fn close_gate(id: &str, answer: Option<&str>, status: Status) -> Result<(), OttoError> {
    transaction(id, |path, state| {
        let gate = state.gate.clone().ok_or_else(|| OttoError::conflict("no gate is open"))?;
        let expired = answer.is_none();
        let answer_text = match answer {
            Some(text) => text.to_string(),
            None => {
                let deadline = gate.expires_at.ok_or_else(|| {
                    OttoError::conflict(format!(
                        "gate {} has no expiry — it waits for a person, however long that takes. Answer it, or abandon the run.",
                        gate.id
                    ))
                })?;
                format!("(no answer by {deadline}; recorded as unanswered by otto)")
            }
        };
        let answered = Timestamp::now();
        let gate_file = path.join(&gate.file);
        let body = std::fs::read_to_string(&gate_file)
            .map_err(|e| OttoError::conflict(format!("gate file {} is unreadable: {e}", gate.file)))?;
        let body = body.replacen("## Answer\n\n_unanswered_\n", "## Answer\n", 1);
        write_atomic(
            &gate_file,
            &format!("{}\n\n### {answered} (verbatim)\n\n{}\n", body.trim_end(), answer_text.trim_end()),
        )?;
        state.gate = None;
        state.status = status;
        state.next_wake_at = None;
        let answer_text = answer_text.trim_end().to_string();
        crate::event::record(
            path,
            &if expired {
                Event::GateExpired { gate: gate.id, slug: gate.slug, asked_at: gate.asked_at, answered_at: answered, answer: answer_text }
            } else {
                Event::GateClosed { gate: gate.id, slug: gate.slug, asked_at: gate.asked_at, answered_at: answered, answer: answer_text }
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;
    use crate::state::read_run;

    #[test]
    fn a_question_already_in_memory_needs_no_file_round_trip() {
        let _home = TempHome::new();
        test_init("ops-a", "a goal").unwrap();
        let gate_file = open_gate("ops-a", "stuck", "are you stuck?", None).unwrap();
        assert!(std::path::Path::new(&gate_file).exists());
        let body = std::fs::read_to_string(&gate_file).unwrap();
        assert!(body.contains("are you stuck?"));
    }

    #[test]
    fn closing_with_an_answer_records_it_verbatim() {
        let _home = TempHome::new();
        test_init("ops-b", "a goal").unwrap();
        open_gate("ops-b", "review", "well?", None).unwrap();
        close_gate("ops-b", Some("approve"), Status::Running).unwrap();
        let state = read_run("ops-b").unwrap();
        assert!(state.gate.is_none());
        assert_eq!(state.status, Status::Running);
    }

    #[test]
    fn closing_expired_with_no_deadline_is_refused() {
        let _home = TempHome::new();
        test_init("ops-c", "a goal").unwrap();
        open_gate("ops-c", "review", "well?", None).unwrap();
        let err = close_gate("ops-c", None, Status::Running).expect_err("must refuse");
        assert!(err.to_string().contains("no expiry"));
    }
}
