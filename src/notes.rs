//! Notes: a person's words to a run, outside any gate.
//!
//! A gate is the run asking; a note is the person telling. Without one, the only way to steer a
//! sleeping or working run was to wait for it to ask, or to stop and resume it. A note is written
//! to `notes/NNN.md` verbatim — the same rule as a gate answer, and for the same reason: acting on
//! a paraphrase two days later is how a run drifts — and handed to the wakes in their prompt.
//!
//! Two kinds:
//!
//! - **One-off** (the default): given to every wake until one that carried it *completes*, then
//!   delivered and dropped from `run.json`. Tying delivery to the contract is what makes it
//!   at-least-once: a wake that crashes or is killed never completes, so the next wake gets the
//!   note again rather than it vanishing with a session nobody can read.
//! - **Standing**: given to every wake, verbatim, until dropped. For guidance that must hold for
//!   the life of the run ("never touch legacy/"), which a one-off note would leave to the
//!   handoff — rewritten by the model every wake, and so paraphrased every wake.
//!
//! A note steers *how* the work is done. It does not change the goal or the done condition, and
//! it is not a source an outward action can be authorized from (DESIGN.md §14). The harness tells
//! the wake to open a gate when a note asks for any of those.

use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::event::Event;
use crate::state::{Note, RunState};
use std::path::Path;

pub const NOTES_DIR: &str = "notes";
/// One note is an instruction, not a document. Longer belongs in a file the note points at.
pub const NOTE_MAX_BYTES: usize = 4096;
/// Standing notes ride in every wake's prompt, so together they are capped like the handoff.
pub const STANDING_MAX_BYTES: usize = 4096;

/// The note's text, as the person wrote it. A missing file is said, not skipped: a note the run
/// thinks it has but cannot read is worth a person noticing.
pub fn read_text(run_dir: &Path, note: &Note) -> String {
    std::fs::read_to_string(run_dir.join(&note.file))
        .unwrap_or_else(|_| format!("(note file {} is missing)", note.file))
}

/// Record a note verbatim and add it to the run.
pub fn add(run_id: &str, text: &str, standing: bool) -> Result<Note, OttoError> {
    let text = text.trim_end();
    if text.trim().is_empty() {
        return Err(OttoError::usage("a note needs some text"));
    }
    if text.len() > NOTE_MAX_BYTES {
        return Err(OttoError::usage(format!(
            "a note is at most {NOTE_MAX_BYTES} bytes (this one is {}) — put the detail in a file and point the note at it",
            text.len()
        )));
    }
    crate::state::transaction(run_id, |path, state| {
        refuse_if_over(state)?;
        if standing {
            let current: usize = state.notes.iter().filter(|n| n.standing).map(|n| read_text(path, n).len()).sum();
            if current + text.len() > STANDING_MAX_BYTES {
                return Err(OttoError::usage(format!(
                    "standing notes are capped at {STANDING_MAX_BYTES} bytes together, since every wake carries them — \
                     {current} are in use; drop one first (`otto note {} --list`)",
                    crate::paths::short_id(run_id)
                )));
            }
        }
        let id = next_id(path, state);
        let file = format!("{NOTES_DIR}/{id}.md");
        crate::state::write_atomic(&path.join(&file), &format!("{text}\n"))?;
        let note = Note { id: id.clone(), file: file.clone(), standing, added_at: Timestamp::now(), given_to_wake: None };
        state.notes.push(note.clone());
        crate::event::record(path, &Event::NoteAdded { note: id, file, standing })?;
        Ok(note)
    })
}

/// Withdraw a note that has not been delivered yet, or retire a standing one. Its file stays: the
/// journal names it, and history does not get rewritten.
pub fn drop_note(run_id: &str, which: &str) -> Result<Note, OttoError> {
    let wanted = normalise_id(which);
    crate::state::transaction(run_id, |path, state| {
        let Some(at) = state.notes.iter().position(|n| n.id == wanted) else {
            let have: Vec<&str> = state.notes.iter().map(|n| n.id.as_str()).collect();
            return Err(OttoError::conflict(if have.is_empty() {
                format!("{run_id} has no undelivered or standing notes")
            } else {
                format!("{run_id} has no note {wanted} to drop — it has {}", have.join(", "))
            }));
        };
        let note = state.notes.remove(at);
        crate::event::record(path, &Event::NoteDropped { note: note.id.clone(), standing: note.standing })?;
        Ok(note)
    })
}

/// What a wake is given: the prompt text, and which notes it carried — so exactly those, and not
/// one added a moment later, are marked as given.
pub struct Given {
    pub block: String,
    pub ids: Vec<String>,
}

/// The notes section of a wake's prompt, or `None` when there is nothing to say.
pub fn for_wake(run_dir: &Path, state: &RunState) -> Option<Given> {
    if state.notes.is_empty() {
        return None;
    }
    let render = |note: &Note| {
        let kind = if note.standing { "standing" } else { "one-off" };
        format!("--- note {} ({kind}, added {}) ---\n{}\n---", note.id, note.added_at, read_text(run_dir, note).trim_end())
    };
    let standing: Vec<String> = state.notes.iter().filter(|n| n.standing).map(render).collect();
    let once: Vec<String> = state.notes.iter().filter(|n| !n.standing).map(render).collect();

    let mut block = String::from(
        "A person has left notes for this run — their words, verbatim. Follow the Notes section of \
         your instructions: they steer how you work, and never change the goal, the done condition, \
         or what is authorized.",
    );
    if !standing.is_empty() {
        block.push_str("\n\nStanding notes — they hold for every wake until the person drops them:\n\n");
        block.push_str(&standing.join("\n\n"));
    }
    if !once.is_empty() {
        block.push_str("\n\nNew notes — given until a wake that carried them completes:\n\n");
        block.push_str(&once.join("\n\n"));
    }
    Some(Given { block, ids: state.notes.iter().map(|n| n.id.clone()).collect() })
}

/// Record that wake `n`'s prompt carried `ids`. Called in the transaction that starts the wake.
pub fn mark_given(state: &mut RunState, ids: &[String], n: u32) {
    for note in state.notes.iter_mut().filter(|note| ids.contains(&note.id)) {
        note.given_to_wake = Some(n);
    }
}

/// Wake `n` completed: every one-off note it carried is delivered. Standing notes stay. Called in
/// the transaction that records the wake complete, so delivery and the contract agree.
pub fn deliver(path: &Path, state: &mut RunState, n: u32) -> Result<(), OttoError> {
    let delivered: Vec<String> = state
        .notes
        .iter()
        .filter(|note| !note.standing && note.given_to_wake == Some(n))
        .map(|note| note.id.clone())
        .collect();
    if delivered.is_empty() {
        return Ok(());
    }
    state.notes.retain(|note| !delivered.contains(&note.id));
    crate::event::record(path, &Event::NotesDelivered { notes: delivered, wake: n })
}

fn refuse_if_over(state: &RunState) -> Result<(), OttoError> {
    use crate::state::Status;
    let short = crate::paths::short_id(&state.id);
    match state.status {
        Status::Done => Err(OttoError::conflict(format!(
            "{} is done — no wake will read a note; start a new run for more work",
            state.id
        ))),
        Status::Stopped | Status::Failed => Err(OttoError::conflict(format!(
            "{} is {} — no wake will read a note; `otto resume {short}` first",
            state.id,
            crate::state::commands::status_str(state.status)
        ))),
        _ => Ok(()),
    }
}

/// One past the highest number used so far, counting files on disk as well as live notes, so a
/// delivered note's number is never reused and the journal's `note 002` always means one thing.
fn next_id(run_dir: &Path, state: &RunState) -> String {
    let on_disk = std::fs::read_dir(run_dir.join(NOTES_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".md")).and_then(|n| n.parse::<u32>().ok()));
    let live = state.notes.iter().filter_map(|n| n.id.parse::<u32>().ok());
    let highest = on_disk.chain(live).max().unwrap_or(0);
    format!("{:03}", highest + 1)
}

/// `2`, `02` and `002` all name note `002`.
fn normalise_id(which: &str) -> String {
    let which = which.trim();
    match which.parse::<u32>() {
        Ok(n) => format!("{n:03}"),
        Err(_) => which.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;
    use crate::state::{read_run, Status};

    fn journal(id: &str) -> String {
        std::fs::read_to_string(crate::paths::run_dir(id).unwrap().join("journal.jsonl")).unwrap()
    }

    #[test]
    fn a_note_is_recorded_verbatim_and_numbered() {
        let _h = TempHome::new();
        test_init("n-add", "a goal").unwrap();
        let text = "Skip the e2e suite — it's broken on main.\n  Keep \"quotes\" and indentation.";
        let one = add("n-add", text, false).unwrap();
        let two = add("n-add", "second", true).unwrap();
        assert_eq!((one.id.as_str(), two.id.as_str()), ("001", "002"));
        let dir = crate::paths::run_dir("n-add").unwrap();
        assert_eq!(read_text(&dir, &one).trim_end(), text);
        assert_eq!(read_run("n-add").unwrap().notes.len(), 2);
        assert_eq!(journal("n-add").matches("\"event\":\"note-added\"").count(), 2);
    }

    #[test]
    fn a_number_is_never_reused_after_delivery() {
        let _h = TempHome::new();
        test_init("n-reuse", "a goal").unwrap();
        add("n-reuse", "first", false).unwrap();
        crate::state::transaction("n-reuse", |path, state| {
            mark_given(state, &["001".to_string()], 1);
            deliver(path, state, 1)
        })
        .unwrap();
        assert!(read_run("n-reuse").unwrap().notes.is_empty());
        assert_eq!(add("n-reuse", "second", false).unwrap().id, "002");
    }

    #[test]
    fn empty_oversized_and_finished_runs_are_refused() {
        let _h = TempHome::new();
        test_init("n-refuse", "a goal").unwrap();
        assert!(add("n-refuse", "   \n", false).is_err());
        assert!(add("n-refuse", &"x".repeat(NOTE_MAX_BYTES + 1), false).is_err());
        add("n-refuse", &"s".repeat(STANDING_MAX_BYTES - 10), true).unwrap();
        let err = add("n-refuse", &"t".repeat(20), true).unwrap_err();
        assert!(err.message.contains("capped"), "{}", err.message);
        add("n-refuse", &"t".repeat(20), false).expect("the standing cap does not apply to a one-off note");
        crate::state::transaction("n-refuse", |_p, s| {
            s.status = Status::Stopped;
            Ok(())
        })
        .unwrap();
        let err = add("n-refuse", "hello", false).unwrap_err();
        assert!(err.message.contains("otto resume"), "{}", err.message);
    }

    #[test]
    fn only_the_wake_that_carried_a_one_off_note_delivers_it() {
        let _h = TempHome::new();
        test_init("n-deliver", "a goal").unwrap();
        add("n-deliver", "carried", false).unwrap();
        add("n-deliver", "keep doing this", true).unwrap();
        let dir = crate::paths::run_dir("n-deliver").unwrap();
        let given = for_wake(&dir, &read_run("n-deliver").unwrap()).unwrap();
        assert!(given.block.contains("carried") && given.block.contains("keep doing this"));
        crate::state::transaction("n-deliver", |_p, s| {
            mark_given(s, &given.ids, 7);
            Ok(())
        })
        .unwrap();
        // Added while wake 7 runs: not in its prompt, so its completing must not deliver it.
        add("n-deliver", "arrived mid-wake", false).unwrap();

        crate::state::transaction("n-deliver", |path, s| deliver(path, s, 7)).unwrap();
        let left: Vec<String> = read_run("n-deliver").unwrap().notes.iter().map(|n| n.id.clone()).collect();
        assert_eq!(left, vec!["002", "003"], "the standing note and the late one remain");
        assert!(journal("n-deliver").contains("\"event\":\"notes-delivered\",\"notes\":[\"001\"]"));
    }

    #[test]
    fn dropping_takes_any_spelling_of_the_number_and_names_what_exists_on_a_miss() {
        let _h = TempHome::new();
        test_init("n-drop", "a goal").unwrap();
        add("n-drop", "one", true).unwrap();
        add("n-drop", "two", false).unwrap();
        assert_eq!(drop_note("n-drop", "1").unwrap().id, "001");
        let err = drop_note("n-drop", "9").unwrap_err();
        assert!(err.message.contains("002"), "{}", err.message);
        assert_eq!(read_run("n-drop").unwrap().notes.len(), 1);
        assert!(journal("n-drop").contains("note-dropped"));
    }

    #[test]
    fn no_notes_means_no_prompt_section() {
        let _h = TempHome::new();
        test_init("n-none", "a goal").unwrap();
        let dir = crate::paths::run_dir("n-none").unwrap();
        assert!(for_wake(&dir, &read_run("n-none").unwrap()).is_none());
    }
}
