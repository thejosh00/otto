//! Messages: what a run sends a person — a report, a reply drafted for someone else, an update.
//!
//! A gate is the run asking; a message is the run delivering. Before messages, the only things a
//! run could put in front of a person were a gate (which stops the run until it is answered) and
//! the handoff's `Needs` (which nobody is told about). Work whose whole point is a result — explain
//! one failing eval a day, draft a reply to every question in a channel — had nowhere to put it but
//! an artifact, found only by someone who went looking.
//!
//! A message is written by a wake (`otto state message`) and lands in the person's inbox, beside
//! the questions waiting on them, until they mark it done (`otto inbox done`). Poke notifies them of
//! each new one, as it does a gate. Marking it done can carry a reply, which reaches the run as a
//! note — the same channel, and the same at-least-once delivery, as any other note.
//!
//! On disk, `messages/NNN-<slug>.md` is the body, verbatim, and `messages/NNN-<slug>.json` the rest:
//! kind, title, when, any draft, what it attached, and — once done — when, and the reply. `run.json`
//! keeps only the index of open messages (`RunState::messages`), so the inbox never has to scan a
//! run's whole history to know what is waiting.

use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::event::Event;
use crate::state::{Note, OpenMessage};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MESSAGES_DIR: &str = "messages";
/// A title is a line in a list.
pub const TITLE_MAX_CHARS: usize = 120;
/// A message is read in one sitting. A longer report is an artifact the message attaches.
pub const BODY_MAX_BYTES: usize = 32 * 1024;
/// A draft is a reply someone will paste, not a document.
pub const DRAFT_MAX_BYTES: usize = 8 * 1024;
pub const ATTACHMENTS_MAX: usize = 10;
/// How much of an attached file a message shows inline; past that, the path is enough.
const ATTACHMENT_SHOW_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum MessageKind {
    /// Something the run made for the person to read: a report, a finding, a summary
    Report,
    /// Text written for the person to send to someone else — a reply, a comment. The run never
    /// sends it itself
    Draft,
    /// News the person would want: a task finished, a PR merged, something worth knowing
    Update,
}

impl MessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageKind::Report => "report",
            MessageKind::Draft => "draft",
            MessageKind::Update => "update",
        }
    }

    /// How a person reads the kind, in a list.
    pub fn label(self) -> &'static str {
        match self {
            MessageKind::Report => "Report",
            MessageKind::Draft => "Draft to send",
            MessageKind::Update => "Update",
        }
    }
}

/// The `.json` beside a message's body: everything about it but the body itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMeta {
    pub id: String,
    pub kind: MessageKind,
    pub title: String,
    pub sent_at: Timestamp,
    /// The wake that sent it, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<u32>,
    /// The body's file, relative to the run directory.
    pub body: String,
    /// Text for the person to send on, verbatim — kept apart from the body so it can be copied whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// Paths, relative to the run directory, of files the message points at (a full report, a plan).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<Timestamp>,
    /// What the person replied when they marked it done, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
}

/// What one message looks like to whoever reads it: the metadata, the body, and the text of each
/// attachment small enough to show.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    #[serde(flatten)]
    pub meta: MessageMeta,
    pub text: String,
    pub attached: Vec<Attachment>,
    /// Still in the person's inbox.
    pub open: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub path: String,
    /// `None` when the file is missing, too big, or not text — the path still says where it is.
    pub text: Option<String>,
}

/// What a wake hands `send`, already read from wherever the CLI found it.
pub struct Outgoing<'a> {
    pub kind: MessageKind,
    pub title: &'a str,
    pub body: &'a str,
    pub draft: Option<&'a str>,
    pub attachments: &'a [String],
}

fn next_number(dir: &Path) -> Result<u64, OttoError> {
    let mut max = 0u64;
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some((digits, _)) = name.split_once('-') {
                if digits.len() >= 3 && digits.bytes().all(|b| b.is_ascii_digit()) {
                    max = max.max(digits.parse().unwrap_or(0));
                }
            }
        }
    }
    Ok(max + 1)
}

/// An attachment must be a file inside the run directory, named relative to it — the page shows
/// it, so it must not be a way to read anything else on the machine.
fn check_attachment(run_dir: &Path, path: &str) -> Result<String, OttoError> {
    let rel = Path::new(path);
    if rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(OttoError::usage(format!(
            "--attach {path}: name a file inside the run directory, relative to it (artifacts/report.md)"
        )));
    }
    if !run_dir.join(rel).is_file() {
        return Err(OttoError::usage(format!("--attach {path}: no such file in the run directory")));
    }
    Ok(path.trim_start_matches("./").to_string())
}

/// Send a message: write it, add it to the run's inbox index, and journal it. Poke notifies the
/// person on its next pass.
pub fn send(run_id: &str, out: &Outgoing) -> Result<OpenMessage, OttoError> {
    let title = out.title.trim();
    if title.is_empty() {
        return Err(OttoError::usage("a message needs a title — what it is, in a few words, for a list"));
    }
    if title.contains('\n') {
        return Err(OttoError::usage("a message title is one line"));
    }
    if title.chars().count() > TITLE_MAX_CHARS {
        return Err(OttoError::usage(format!(
            "a message title is at most {TITLE_MAX_CHARS} characters (this one is {}) — the detail goes in the body",
            title.chars().count()
        )));
    }
    let body = out.body.trim_end();
    if body.trim().is_empty() {
        return Err(OttoError::usage("a message needs a body — written to be read cold, by someone with no context"));
    }
    if body.len() > BODY_MAX_BYTES {
        return Err(OttoError::usage(format!(
            "a message body is at most {BODY_MAX_BYTES} bytes (this one is {}) — write the full report to an \
             artifact and --attach it, with a short body saying what it found",
            body.len()
        )));
    }
    let draft = out.draft.map(str::trim_end).filter(|d| !d.trim().is_empty());
    if let Some(draft) = draft {
        if draft.len() > DRAFT_MAX_BYTES {
            return Err(OttoError::usage(format!("a draft is at most {DRAFT_MAX_BYTES} bytes (this one is {})", draft.len())));
        }
    }
    if out.attachments.len() > ATTACHMENTS_MAX {
        return Err(OttoError::usage(format!("a message attaches at most {ATTACHMENTS_MAX} files")));
    }

    crate::state::transaction(run_id, |path, state| {
        let attachments =
            out.attachments.iter().map(|a| check_attachment(path, a)).collect::<Result<Vec<_>, _>>()?;
        let dir = path.join(MESSAGES_DIR);
        std::fs::create_dir_all(&dir)?;
        let id = format!("{:03}", next_number(&dir)?);
        let stem = format!("{id}-{}", crate::clock::slugify(title, out.kind.as_str()));
        let body_file = format!("{MESSAGES_DIR}/{stem}.md");
        let meta_file = format!("{MESSAGES_DIR}/{stem}.json");
        let sent_at = Timestamp::now();
        let meta = MessageMeta {
            id: id.clone(),
            kind: out.kind,
            title: title.to_string(),
            sent_at,
            wake: state.wake.as_ref().filter(|w| w.outcome.is_none()).map(|w| w.n),
            body: body_file.clone(),
            draft: draft.map(str::to_string),
            attachments,
            done_at: None,
            reply: None,
        };
        crate::state::write_atomic(&path.join(&body_file), &format!("{body}\n"))?;
        crate::state::write_atomic(&path.join(&meta_file), &(serde_json::to_string_pretty(&meta)? + "\n"))?;
        let open = OpenMessage { id: id.clone(), kind: out.kind, title: title.to_string(), file: meta_file.clone(), sent_at };
        state.messages.push(open.clone());
        crate::event::record(path, &Event::MessageSent { message: id, kind: out.kind, title: title.to_string(), file: meta_file })?;
        Ok(open)
    })
}

/// `004`, `4` and `#4` all mean message 004.
pub fn normalise_id(which: &str) -> String {
    let digits = which.trim().trim_start_matches('#');
    match digits.parse::<u64>() {
        Ok(n) => format!("{n:03}"),
        Err(_) => digits.to_string(),
    }
}

fn read_meta(run_dir: &Path, file: &str) -> Result<MessageMeta, OttoError> {
    let text = std::fs::read_to_string(run_dir.join(file))
        .map_err(|e| OttoError::conflict(format!("message file {file} is unreadable: {e}")))?;
    serde_json::from_str(&text).map_err(|e| OttoError::conflict(format!("message file {file} is not valid: {e}")))
}

/// Every message the run has sent, newest first.
pub fn history(run_dir: &Path) -> Vec<MessageMeta> {
    let Ok(entries) = std::fs::read_dir(run_dir.join(MESSAGES_DIR)) else { return Vec::new() };
    let mut files: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    files.sort();
    files.reverse();
    files.iter().filter_map(|name| read_meta(run_dir, &format!("{MESSAGES_DIR}/{name}")).ok()).collect()
}

/// The metadata file of message `id`, whether it is still open or not.
fn meta_file(run_dir: &Path, id: &str) -> Option<String> {
    let entries = std::fs::read_dir(run_dir.join(MESSAGES_DIR)).ok()?;
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|name| name.ends_with(".json") && name.split_once('-').is_some_and(|(n, _)| n == id))
        .map(|name| format!("{MESSAGES_DIR}/{name}"))
}

/// One message in full. `which` is its number; `None` means the run's newest open message, or its
/// newest message of all when none is open.
pub fn read(run_id: &str, which: Option<&str>) -> Result<MessageView, OttoError> {
    let state = crate::state::read_run(run_id)?;
    let dir = crate::paths::run_dir(&state.id)?;
    let id = match which {
        Some(which) => normalise_id(which),
        None => match state.messages.last() {
            Some(open) => open.id.clone(),
            None => history(&dir)
                .first()
                .map(|m| m.id.clone())
                .ok_or_else(|| OttoError::conflict(format!("{} has sent no messages", state.label())))?,
        },
    };
    let file = meta_file(&dir, &id).ok_or_else(|| OttoError::not_found(format!("{} has no message {id}", state.label())))?;
    let meta = read_meta(&dir, &file)?;
    Ok(view(&dir, meta, state.messages.iter().any(|m| m.id == id)))
}

fn view(dir: &Path, meta: MessageMeta, open: bool) -> MessageView {
    let text = std::fs::read_to_string(dir.join(&meta.body)).unwrap_or_else(|_| format!("(message body {} is missing)", meta.body));
    let attached = meta
        .attachments
        .iter()
        .map(|path| Attachment {
            path: path.clone(),
            text: std::fs::read(dir.join(path))
                .ok()
                .filter(|bytes| bytes.len() <= ATTACHMENT_SHOW_MAX_BYTES)
                .and_then(|bytes| String::from_utf8(bytes).ok()),
        })
        .collect();
    MessageView { meta, text, attached, open }
}

/// What marking a message done did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Done {
    pub message: MessageMeta,
    /// The note the reply became, when there was a reply.
    pub note: Option<Note>,
    /// It had been marked done before; the reply, if any, still went to the run.
    pub already_done: bool,
}

/// Mark a message done, and — with a reply — hand the person's words to the run as a note, so the
/// next wake reads them. `which` is the message's number; `None` means the run's only open one.
pub fn done(run_id: &str, which: Option<&str>, reply: Option<&str>) -> Result<Done, OttoError> {
    let state = crate::state::read_run(run_id)?;
    let run_id = state.id.clone();
    let dir = crate::paths::run_dir(&run_id)?;
    let id = match which {
        Some(which) => normalise_id(which),
        None => match state.messages.as_slice() {
            [only] => only.id.clone(),
            [] => return Err(OttoError::conflict(format!("{} has no open messages", state.label()))),
            several => {
                let ids: Vec<&str> = several.iter().map(|m| m.id.as_str()).collect();
                return Err(OttoError::usage(format!(
                    "{} has {} open messages — say which: {}",
                    state.label(),
                    several.len(),
                    ids.join(", ")
                )));
            }
        },
    };
    let file = meta_file(&dir, &id).ok_or_else(|| OttoError::not_found(format!("{} has no message {id}", state.label())))?;
    let title = read_meta(&dir, &file)?.title;
    let reply = reply.map(str::trim).filter(|r| !r.is_empty());
    // The note first: it takes the run's lock itself, and a refusal (a finished run reads no notes)
    // should leave the message as it was.
    let note = match reply {
        Some(reply) => Some(crate::notes::add(&run_id, &format!("In reply to message {id}, \"{title}\":\n\n{reply}"), false)?),
        None => None,
    };
    let note_id = note.as_ref().map(|n| n.id.clone());
    crate::state::transaction(&run_id, |path, state| {
        let was_open = state.messages.iter().any(|m| m.id == id);
        let mut meta = read_meta(path, &file)?;
        if !was_open && reply.is_none() {
            return Ok(Done { message: meta, note: None, already_done: true });
        }
        state.messages.retain(|m| m.id != id);
        if meta.done_at.is_none() {
            meta.done_at = Some(Timestamp::now());
        }
        if let Some(reply) = reply {
            meta.reply = Some(reply.to_string());
        }
        crate::state::write_atomic(&path.join(&file), &(serde_json::to_string_pretty(&meta)? + "\n"))?;
        crate::event::record(
            path,
            &Event::MessageDone { message: id.clone(), reply: reply.map(str::to_string), note: note_id.clone() },
        )?;
        Ok(Done { message: meta, note: note.clone(), already_done: !was_open })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;
    use crate::state::read_run;

    fn report(title: &str) -> Outgoing<'_> {
        Outgoing { kind: MessageKind::Report, title, body: "It isn't the model.\n\n## Why\n\nStale index.", draft: None, attachments: &[] }
    }

    #[test]
    fn a_sent_message_is_on_disk_verbatim_open_and_journaled() {
        let _h = TempHome::new();
        test_init("m-send", "a goal").unwrap();
        let open = send("m-send", &report("Why refund-policy-qa fails")).unwrap();
        assert_eq!(open.id, "001");
        assert_eq!(open.file, "messages/001-why-refund-policy-qa-fails.json");

        let state = read_run("m-send").unwrap();
        assert_eq!(state.messages.len(), 1, "it waits in the inbox");
        let dir = crate::paths::run_dir("m-send").unwrap();
        let body = std::fs::read_to_string(dir.join("messages/001-why-refund-policy-qa-fails.md")).unwrap();
        assert_eq!(body, "It isn't the model.\n\n## Why\n\nStale index.\n", "the body is kept verbatim, headings and all");
        let journal = std::fs::read_to_string(dir.join("journal.jsonl")).unwrap();
        assert!(journal.contains(r#""event":"message-sent","message":"001","kind":"report""#), "got: {journal}");

        let second = send("m-send", &report("Why tool-call-json fails")).unwrap();
        assert_eq!(second.id, "002");
        let view = read("m-send", None).unwrap();
        assert_eq!(view.meta.id, "002", "with no number, the newest open one");
        assert!(view.open);
        assert!(view.text.contains("Stale index."));
    }

    #[test]
    fn a_message_is_refused_rather_than_sent_badly() {
        let _h = TempHome::new();
        test_init("m-bad", "a goal").unwrap();
        let no_title = Outgoing { title: "  ", ..report("x") };
        assert!(send("m-bad", &no_title).is_err());
        let two_lines = report("one\ntwo");
        assert!(send("m-bad", &two_lines).is_err());
        let long = "x".repeat(TITLE_MAX_CHARS + 1);
        assert!(send("m-bad", &report(&long)).is_err());
        let empty = Outgoing { body: "\n", ..report("t") };
        assert!(send("m-bad", &empty).is_err());
        let big = "y".repeat(BODY_MAX_BYTES + 1);
        let err = send("m-bad", &Outgoing { body: &big, ..report("t") }).unwrap_err();
        assert!(err.to_string().contains("--attach"), "says what to do instead: {err}");
        for outside in ["/etc/passwd", "../other/run.json", "artifacts/missing.md"] {
            let attachments = vec![outside.to_string()];
            assert!(send("m-bad", &Outgoing { attachments: &attachments, ..report("t") }).is_err(), "{outside}");
        }
        assert!(read_run("m-bad").unwrap().messages.is_empty(), "nothing half-sent");
    }

    #[test]
    fn an_attachment_and_a_draft_ride_along() {
        let _h = TempHome::new();
        test_init("m-att", "a goal").unwrap();
        let dir = crate::paths::run_dir("m-att").unwrap();
        std::fs::write(dir.join("artifacts/full.md"), "the whole report\n").unwrap();
        let attachments = vec!["artifacts/full.md".to_string()];
        let out = Outgoing {
            kind: MessageKind::Draft,
            title: "Sam asked about rate limits",
            body: "Their key is on the default tier.",
            draft: Some("Hi Sam! Your key is on the default tier."),
            attachments: &attachments,
        };
        send("m-att", &out).unwrap();
        let view = read("m-att", Some("1")).unwrap();
        assert_eq!(view.meta.kind, MessageKind::Draft);
        assert_eq!(view.meta.draft.as_deref(), Some("Hi Sam! Your key is on the default tier."));
        assert_eq!(view.attached[0].text.as_deref(), Some("the whole report\n"));
    }

    #[test]
    fn done_leaves_the_inbox_and_a_reply_becomes_a_note() {
        let _h = TempHome::new();
        test_init("m-done", "a goal").unwrap();
        send("m-done", &report("First")).unwrap();
        send("m-done", &report("Second")).unwrap();
        let err = done("m-done", None, None).unwrap_err();
        assert!(err.to_string().contains("say which"), "two are open: {err}");

        let first = done("m-done", Some("1"), None).unwrap();
        assert!(first.note.is_none());
        assert!(first.message.done_at.is_some());
        let state = read_run("m-done").unwrap();
        assert_eq!(state.messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["002"]);
        assert!(state.notes.is_empty());

        let second = done("m-done", None, Some("dig into the grader next")).unwrap();
        let note = second.note.expect("a reply is a note");
        let state = read_run("m-done").unwrap();
        assert!(state.messages.is_empty());
        let dir = crate::paths::run_dir("m-done").unwrap();
        let text = crate::notes::read_text(&dir, &state.notes[0]);
        assert!(text.contains("In reply to message 002, \"Second\"") && text.contains("dig into the grader next"), "got: {text}");
        assert_eq!(note.id, state.notes[0].id);
        let view = read("m-done", Some("2")).unwrap();
        assert!(!view.open);
        assert_eq!(view.meta.reply.as_deref(), Some("dig into the grader next"));

        let again = done("m-done", Some("1"), None).unwrap();
        assert!(again.already_done, "marking it done twice is not an error");
        assert_eq!(history(&dir).iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["002", "001"]);
    }
}
