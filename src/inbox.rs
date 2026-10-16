//! The inbox: everything any run has put in front of the person, in one list.
//!
//! Two kinds of thing land here, and to a person they are the same kind of thing: something from
//! a run, waiting for them. A **question** is an open gate — the run is stopped until it is
//! answered. A **message** is a report, a draft or an update (`messages`) — the run carries on, and
//! the message waits until the person marks it done. Both used to be found by visiting each run in
//! turn; now `otto inbox` and the page's inbox are where they are, newest first, with a count.

use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::state::{read_all_runs, RunEntry};
use serde::Serialize;
use serde_json::Value;

/// How many finished items per run `--all` brings back: enough for a history, not a whole year of
/// daily reports.
const DONE_PER_RUN: usize = 30;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub run: String,
    pub short: String,
    pub name: String,
    /// `question` for a gate; otherwise the message's kind (`report`, `draft`, `update`).
    pub kind: String,
    /// How a person reads the kind.
    pub label: String,
    /// The gate's or the message's number within its run.
    pub id: String,
    pub title: String,
    pub at: Timestamp,
    /// Still waiting for the person.
    pub open: bool,
    pub done_at: Option<Timestamp>,
    /// For a question: the options it offers, the default first.
    pub options: Vec<String>,
    /// For a question, what was answered; for a message, what the person replied.
    pub answer: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inbox {
    pub items: Vec<Item>,
    /// How many are still waiting.
    pub open: usize,
}

/// Every open question and message across every run, newest first; with `all`, the answered and
/// done ones too.
pub fn inbox(all: bool) -> Result<Inbox, OttoError> {
    let ids = crate::paths::all_run_ids();
    let mut items = Vec::new();
    for entry in read_all_runs()? {
        let RunEntry::Readable(state) = entry else { continue };
        let Ok(dir) = crate::paths::run_dir(&state.id) else { continue };
        let short = crate::paths::short_id_among(&state.id, &ids);
        let name = state.display_name();
        let item = |kind: &str, label: &str, id: &str, title: String, at: Timestamp| Item {
            run: state.id.clone(),
            short: short.clone(),
            name: name.clone(),
            kind: kind.to_string(),
            label: label.to_string(),
            id: id.to_string(),
            title,
            at,
            open: true,
            done_at: None,
            options: Vec::new(),
            answer: None,
        };
        if let Some(gate) = &state.gate {
            let question = crate::gate::read_question(&dir, gate).unwrap_or_default();
            let title = crate::gate::title(&question, crate::story::QUESTION_TITLE_CHARS);
            let title = if title.is_empty() { gate.slug.replace('-', " ") } else { title };
            items.push(Item {
                options: crate::gate::options_default_first(&question),
                ..item("question", "Needs your answer", &gate.id, title, gate.asked_at)
            });
        }
        for message in &state.messages {
            items.push(item(message.kind.as_str(), message.kind.label(), &message.id, message.title.clone(), message.sent_at));
        }
        if !all {
            continue;
        }
        for message in crate::messages::history(&dir).into_iter().filter(|m| m.done_at.is_some()).take(DONE_PER_RUN) {
            items.push(Item {
                open: false,
                done_at: message.done_at,
                answer: message.reply.clone(),
                ..item(message.kind.as_str(), message.kind.label(), &message.id, message.title.clone(), message.sent_at)
            });
        }
        for answered in answered_questions(&dir).into_iter().take(DONE_PER_RUN) {
            items.push(Item { open: false, done_at: Some(answered.1), answer: Some(answered.3), ..item("question", "Question", &answered.0, answered.2, answered.4) });
        }
    }
    items.sort_by(|a, b| b.at.cmp(&a.at).then(a.run.cmp(&b.run)));
    let open = items.iter().filter(|i| i.open).count();
    Ok(Inbox { items, open })
}

/// The questions a run has had answered (or let expire), newest first: (gate, answered at, title,
/// answer, asked at) — read from the journal, which has every one verbatim.
fn answered_questions(dir: &std::path::Path) -> Vec<(String, Timestamp, String, String, Timestamp)> {
    let Ok(text) = std::fs::read_to_string(dir.join("journal.jsonl")) else { return Vec::new() };
    let mut out = Vec::new();
    for line in text.lines().rev() {
        if !line.contains("\"gate-closed\"") && !line.contains("\"gate-expired\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let ts = |k: &str| Timestamp::parse(&s(k)).ok();
        let (gate, slug) = (s("gate"), s("slug"));
        let Some(asked) = ts("askedAt") else { continue };
        let answered = ts("answeredAt").unwrap_or(asked);
        let question = std::fs::read_to_string(dir.join("gates").join(format!("{gate}-{slug}.md")))
            .ok()
            .map(|t| t.split("## Question").nth(1).unwrap_or("").split("## Answer").next().unwrap_or("").to_string())
            .unwrap_or_default();
        let title = crate::gate::title(&question, crate::story::QUESTION_TITLE_CHARS);
        let title = if title.is_empty() { slug.replace('-', " ") } else { title };
        out.push((gate, answered, title, s("answer"), asked));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{send, MessageKind, Outgoing};
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;

    fn update(title: &str) -> Outgoing<'_> {
        Outgoing { kind: MessageKind::Update, title, body: "body", draft: None, attachments: &[] }
    }

    #[test]
    fn questions_and_messages_from_every_run_are_one_list() {
        let _h = TempHome::new();
        test_init("1", "a goal").unwrap();
        test_init("2", "another").unwrap();
        crate::state::ops::open_gate("1", "plan-review", "# Approve the plan?\n\n## Options\n- Approve\n- Revise", None).unwrap();
        send("2", &update("PR #482 merged")).unwrap();
        let inbox = inbox(false).unwrap();
        assert_eq!(inbox.open, 2);
        let question = inbox.items.iter().find(|i| i.kind == "question").unwrap();
        assert_eq!(question.title, "Approve the plan?", "the question's own heading, not its slug");
        assert_eq!(question.options, vec!["Approve", "Revise"]);
        let message = inbox.items.iter().find(|i| i.kind == "update").unwrap();
        assert_eq!((message.run.as_str(), message.title.as_str()), ("2", "PR #482 merged"));

        crate::state::ops::close_gate("1", Some("Approve"), crate::state::Status::Running).unwrap();
        crate::messages::done("2", None, None).unwrap();
        assert_eq!(super::inbox(false).unwrap().open, 0);
        let history = super::inbox(true).unwrap();
        assert_eq!(history.items.len(), 2, "answered and done, kept for --all");
        let answered = history.items.iter().find(|i| i.kind == "question").unwrap();
        assert_eq!((answered.answer.as_deref(), answered.open), (Some("Approve"), false));
        assert_eq!(answered.title, "Approve the plan?");
    }
}
