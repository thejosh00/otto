//! A run as a person reads it, rather than as the engine keeps it: one line for a list, its week
//! as a picture, and what has happened lately, in words.
//!
//! Everything here is read off what a run already writes — `run.json`, the handoff, the journal,
//! the gate and note files — so it adds nothing to the run directory and can never disagree with
//! it. What it adds is translation. The journal says `wake-started`, `noop-tick`, `wake-complete`,
//! `timer-armed`, and what a person wants from those four lines is "looked, nothing new"; it says
//! `gate-opened` with a slug, and what they want is the question. So the engine's bookkeeping —
//! wake numbers, ticks, timers, locks — stays under the hood (`otto logs`), and these are the
//! views that come first.

use crate::clock::Timestamp;
use crate::state::{RunState, Status};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

// ---------------------------------------------------------------------------
// One line
// ---------------------------------------------------------------------------

/// What a run is doing, as one of a handful of states a glyph can show at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mood {
    /// It has asked the person something.
    Waiting,
    /// A wake is running.
    Working,
    /// Healthy, and a free check looks for news before each wake.
    Watching,
    /// Healthy, and waiting for its next wake.
    Idle,
    /// A person paused it.
    Paused,
    /// Something is wrong that it is retrying — or, blocked with no question, that it cannot.
    Trouble,
    /// Done, stopped or failed.
    Finished,
}

/// A run in one line: its mood, and what it is doing in words.
#[derive(Debug, Clone, Serialize)]
pub struct Glance {
    pub mood: Mood,
    pub line: String,
}

/// How long a question's title may be in a line.
pub const QUESTION_TITLE_CHARS: usize = 100;

/// `quiet` is how many minutes the running wake has gone without a word (`core::quiet_minutes`);
/// `question` is the open gate's title (`gate::title`); `said` is the handoff's `Where this is`.
pub fn glance(state: &RunState, running: bool, quiet: Option<u64>, question: Option<&str>, said: Option<&str>) -> Glance {
    let said = said.map(str::trim).filter(|s| !s.is_empty());
    let with_said = |lead: &str| match said {
        Some(s) => format!("{lead} — {s}"),
        None => lead.to_string(),
    };
    let (mood, line) = if running {
        // Not trouble: a long build or a slow test says nothing for minutes. But a person deciding
        // whether to look in should know, long before the deadline kills it.
        match quiet {
            Some(minutes) => (Mood::Working, format!("Working now, though nothing new from it for {minutes} minutes")),
            None => (Mood::Working, "Working now".to_string()),
        }
    } else if let Some(question) = question.filter(|_| !state.status.is_terminal()) {
        (Mood::Waiting, format!("Asks you: {question}"))
    } else {
        match state.status {
            Status::Done => (Mood::Finished, said.map(str::to_string).unwrap_or_else(|| "Done".to_string())),
            Status::Stopped => (Mood::Finished, with_said("Stopped")),
            Status::Failed => (Mood::Finished, with_said("Gave up")),
            _ if state.is_paused() => (Mood::Paused, paused_line(state)),
            _ if state.start_failures > 0 => (
                Mood::Trouble,
                format!(
                    "Couldn't start {}{}. {}",
                    times(state.start_failures),
                    claude_says(state.start_failure.as_deref()),
                    start_failure_hint(state.start_failure.as_deref())
                ),
            ),
            Status::Blocked => (Mood::Trouble, "Stuck, and nothing is asking you how to go on".to_string()),
            _ if state.incomplete_wakes > 0 => (Mood::Trouble, with_said("Stopped part-way last time; trying again")),
            _ if state.done_claim.is_some() => (Mood::Idle, with_said("Says it's finished; a fresh look is checking")),
            _ if state.check.as_ref().is_some_and(|c| c.pinned) => (Mood::Watching, said.unwrap_or("Watching").to_string()),
            _ => (Mood::Idle, said.unwrap_or("Nothing to report yet").to_string()),
        }
    };
    Glance { mood, line }
}

/// What a person can do about wakes that couldn't start, guessed from what claude said.
pub fn start_failure_hint(said: Option<&str>) -> &'static str {
    let said = said.unwrap_or("").to_lowercase();
    if said.contains("authenticat") || said.contains("oauth") || said.contains("log in") || said.contains("login") {
        "Log in again: run `claude` in a terminal."
    } else if said.contains("credit") || said.contains("balance") || said.contains("billing") {
        "Check the account's credit."
    } else {
        "It keeps retrying on its own."
    }
}

/// "claude says “Failed to authenticate: …”", or nothing when it said nothing.
pub fn claude_says(said: Option<&str>) -> String {
    match said.filter(|s| !s.trim().is_empty()) {
        Some(said) => format!(" — claude says “{said}”"),
        None => " — claude exits straight away".to_string(),
    }
}

fn times(n: u32) -> String {
    match n {
        1 => "once".to_string(),
        2 => "twice".to_string(),
        n => format!("{n} times in a row"),
    }
}

/// "Paused until Mon 9:00 AM", or "Paused" for a pause with no end.
pub fn paused_line(state: &RunState) -> String {
    if crate::agents::not_started(state) {
        return "Not started yet".to_string();
    }
    match state.paused.as_ref().and_then(|p| p.until) {
        Some(until) => format!("Paused until {}", crate::clock::plain_time(until)),
        None => "Paused".to_string(),
    }
}

// ---------------------------------------------------------------------------
// The week
// ---------------------------------------------------------------------------

/// How far back the timeline looks.
pub const TIMELINE_DAYS: i64 = 7;

/// How one wake went, as a mark on the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WakeMark {
    /// It did something: journaled what, sent a message, asked, made progress, finished.
    Did,
    /// It looked and found nothing to do.
    Ran,
    /// It got going but did not finish.
    Failed,
    /// It never got going.
    CouldntStart,
    /// The account's usage limit stopped it.
    Limited,
}

#[derive(Debug, Clone, Serialize)]
pub struct Mark {
    pub at: Timestamp,
    pub mark: WakeMark,
    /// What it did, in a few words, when it said.
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Span {
    pub from: Timestamp,
    pub to: Timestamp,
}

/// A run's last week: when it ran and how each went, when it waited on the person, when it was
/// paused, and when a free check was looking on its behalf.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    pub from: Timestamp,
    pub now: Timestamp,
    pub wakes: Vec<Mark>,
    pub waiting: Vec<Span>,
    pub paused: Vec<Span>,
    pub looking: Vec<Span>,
    /// What it published, and when (`outputs`).
    pub made: Vec<Made>,
    /// The next wake it has scheduled, when one is.
    pub next: Option<Timestamp>,
}

/// One version of an output, as a mark on the timeline.
#[derive(Debug, Clone, Serialize)]
pub struct Made {
    pub at: Timestamp,
    pub key: String,
    pub version: u32,
    pub title: String,
}

struct Entry {
    at: Timestamp,
    event: String,
    value: Value,
}

fn entries(dir: &Path) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(dir.join("journal.jsonl")) else { return Vec::new() };
    text.lines()
        .filter_map(|line| {
            let value: Value = serde_json::from_str(line).ok()?;
            let at = Timestamp::parse(value.get("ts")?.as_str()?).ok()?;
            let event = value.get("event")?.as_str()?.to_string();
            Some(Entry { at, event, value })
        })
        .collect()
}

fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Whether a wake's own lines say it did something a person would count.
fn did_something(lines: &[&Entry]) -> Option<Option<String>> {
    let own = lines.iter().rev().find(|e| !crate::event::OTTO_EVENTS.contains(&e.event.as_str()));
    if let Some(own) = own {
        return Some(str_of(&own.value, "message").map(str::to_string).or_else(|| Some(own.event.clone())));
    }
    if let Some(made) = lines.iter().rev().find(|e| e.event == "output-published") {
        return Some(str_of(&made.value, "title").map(|t| format!("made: {t}")));
    }
    if let Some(sent) = lines.iter().rev().find(|e| e.event == "message-sent") {
        return Some(str_of(&sent.value, "title").map(|t| format!("sent you: {t}")));
    }
    let progressed = lines.iter().any(|e| match e.event.as_str() {
        "tick" | "gate-opened" | "phase-changed" | "done-claimed" | "done-reopened" => true,
        "status-changed" => str_of(&e.value, "to") == Some("done"),
        _ => false,
    });
    progressed.then_some(None)
}

pub fn timeline(dir: &Path, state: &RunState) -> Timeline {
    let now = Timestamp::now();
    let from = Timestamp::at(now.dt() - time::Duration::days(TIMELINE_DAYS));
    let all = entries(dir);
    let clip = |at: Timestamp| if at.dt() < from.dt() { from } else { at };

    let mut wakes = Vec::new();
    let mut waiting = Vec::new();
    let mut paused = Vec::new();
    let mut looking = Vec::new();
    let mut made = Vec::new();
    let mut wake_start: Option<usize> = None;
    let mut gate_open: Option<Timestamp> = None;
    let mut pause_start: Option<(Timestamp, Option<Timestamp>)> = None;
    let mut check_start: Option<Timestamp> = None;

    for (i, entry) in all.iter().enumerate() {
        match entry.event.as_str() {
            "wake-started" => wake_start = Some(i),
            "wake-complete" | "wake-incomplete" | "wake-killed" | "start-failed" | "usage-limited" => {
                let start = wake_start.take().unwrap_or(i);
                let at = all[start].at;
                if at.dt() < from.dt() {
                    continue;
                }
                let lines: Vec<&Entry> = all[start..=i].iter().collect();
                let (mark, text) = match entry.event.as_str() {
                    "start-failed" => (WakeMark::CouldntStart, None),
                    "usage-limited" => (WakeMark::Limited, None),
                    "wake-incomplete" | "wake-killed" => (WakeMark::Failed, str_of(&entry.value, "reason").map(str::to_string)),
                    _ => match did_something(&lines) {
                        Some(text) => (WakeMark::Did, text),
                        None => (WakeMark::Ran, None),
                    },
                };
                wakes.push(Mark { at, mark, text });
            }
            "gate-opened" => gate_open = Some(entry.at),
            "gate-closed" | "gate-expired" => {
                if let Some(opened) = gate_open.take() {
                    if entry.at.dt() > from.dt() {
                        waiting.push(Span { from: clip(opened), to: entry.at });
                    }
                }
            }
            "paused" => {
                let until = str_of(&entry.value, "until").and_then(|u| Timestamp::parse(u).ok());
                pause_start = Some((entry.at, until));
            }
            "unpaused" => {
                if let Some((start, _)) = pause_start.take() {
                    if entry.at.dt() > from.dt() {
                        paused.push(Span { from: clip(start), to: entry.at });
                    }
                }
            }
            "output-published" if entry.at.dt() >= from.dt() => made.push(Made {
                at: entry.at,
                key: str_of(&entry.value, "output").unwrap_or("").to_string(),
                version: entry.value.get("version").and_then(Value::as_u64).unwrap_or(1) as u32,
                title: str_of(&entry.value, "title").unwrap_or("").to_string(),
            }),
            "check-set" => check_start = check_start.or(Some(entry.at)),
            "check-cleared" => {
                if let Some(start) = check_start.take() {
                    if entry.at.dt() > from.dt() {
                        looking.push(Span { from: clip(start), to: entry.at });
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(opened) = gate_open.filter(|_| state.gate.is_some()) {
        waiting.push(Span { from: clip(opened), to: now });
    }
    if let Some((start, until)) = pause_start.filter(|_| state.paused.is_some()) {
        let end = until.filter(|u| u.dt() < now.dt()).unwrap_or(now);
        paused.push(Span { from: clip(start), to: end });
    }
    if state.check.as_ref().is_some_and(|c| c.pinned) {
        // A check set before the journal's window, or by a version of otto that journaled none.
        looking.push(Span { from: clip(check_start.unwrap_or(from)), to: now });
    }
    let next = state.next_wake_at.filter(|_| !state.status.is_terminal() && state.gate.is_none());
    Timeline { from, now, wakes, waiting, paused, looking, made, next }
}

// ---------------------------------------------------------------------------
// Lately
// ---------------------------------------------------------------------------

/// One thing that happened, said for a person.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Happening {
    pub at: Timestamp,
    /// `did`, `sent`, `asked`, `answered`, `you`, `trouble`, `status`.
    pub kind: &'static str,
    pub text: String,
    /// The message this is about, when it is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The output this is about, when it is about one: its key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// How many times in a row it happened — five failed attempts are one line, not five.
    pub times: u32,
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() <= max {
        line.to_string()
    } else {
        format!("{}…", line.chars().take(max).collect::<String>().trim_end())
    }
}

fn gate_title(dir: &Path, gate: &str, slug: &str) -> String {
    let file = dir.join("gates").join(format!("{gate}-{slug}.md"));
    let question = std::fs::read_to_string(file)
        .ok()
        .map(|text| {
            let after = text.split("## Question").nth(1).unwrap_or(&text).to_string();
            after.split("## Answer").next().unwrap_or(&after).to_string()
        })
        .unwrap_or_default();
    let title = crate::gate::title(&question, QUESTION_TITLE_CHARS);
    if title.is_empty() { slug.replace('-', " ") } else { title }
}

/// What has happened lately, newest first, at most `limit` things: what the run did, sent and
/// asked, and what the person said back. Otto's own rhythm — wakes starting and ending, timers,
/// ticks, locks — is left out; it is in the log for whoever wants it.
pub fn lately(dir: &Path, limit: usize) -> Vec<Happening> {
    let all = entries(dir);
    let titles: std::collections::HashMap<String, String> =
        crate::messages::history(dir).into_iter().map(|m| (m.id, m.title)).collect();
    let made: std::collections::HashMap<String, String> =
        crate::outputs::index_or_empty(dir).outputs.into_iter().map(|o| (o.key, o.title)).collect();
    let mut out: Vec<Happening> = Vec::new();
    for entry in all.iter().rev() {
        if out.len() > limit {
            out.pop();
            break;
        }
        let v = &entry.value;
        let at = entry.at;
        let happening = |kind: &'static str, text: String| Some(Happening { at, kind, text, message: None, output: None, times: 1 });
        let about_output = |kind: &'static str, text: String, key: &str| {
            Some(Happening { at, kind, text, message: None, output: Some(key.to_string()), times: 1 })
        };
        let item = match entry.event.as_str() {
            e if !crate::event::OTTO_EVENTS.contains(&e) => {
                let text = str_of(v, "message").map(|m| first_line(m, 240)).unwrap_or_else(|| e.to_string());
                happening(if e == "error" { "trouble" } else { "did" }, text)
            }
            "message-sent" => Some(Happening {
                at,
                kind: "sent",
                text: format!(
                    "Sent you {}: {}",
                    match str_of(v, "kind") {
                        Some("draft") => "a draft to send",
                        Some("update") => "an update",
                        _ => "a report",
                    },
                    str_of(v, "title").unwrap_or("")
                ),
                message: str_of(v, "message").map(str::to_string),
                output: None,
                times: 1,
            }),
            "message-done" => {
                let id = str_of(v, "message").unwrap_or("");
                let title = titles.get(id).cloned().unwrap_or_else(|| format!("message {id}"));
                let text = match str_of(v, "reply") {
                    Some(reply) => format!("You replied to “{title}”: {}", first_line(reply, 200)),
                    None => format!("You marked “{title}” done"),
                };
                Some(Happening { at, kind: "you", text, message: Some(id.to_string()), output: None, times: 1 })
            }
            "gate-opened" => happening(
                "asked",
                format!("Asked you: {}", gate_title(dir, str_of(v, "gate").unwrap_or(""), str_of(v, "slug").unwrap_or(""))),
            ),
            "gate-closed" => happening("answered", format!("You answered: {}", first_line(str_of(v, "answer").unwrap_or(""), 200))),
            "gate-expired" => happening("answered", "No answer came before the question's deadline".to_string()),
            "note-added" => {
                let text = str_of(v, "file").and_then(|f| std::fs::read_to_string(dir.join(f)).ok()).unwrap_or_default();
                let standing = v.get("standing").and_then(Value::as_bool).unwrap_or(false);
                // A reply to a message is told as the reply, by `message-done`.
                // Likewise what was said about a change is told by `change-moved`.
                if text.starts_with("In reply to message ") || text.starts_with(crate::changes::NOTE_ABOUT_PREFIX) {
                    None
                } else if let Some(rest) = text.strip_prefix(crate::outputs::NOTE_ABOUT_PREFIX) {
                    // "About output design@2, \"Design\":\n\n<words>"
                    let key = rest.split('@').next().unwrap_or("");
                    let words = rest.split_once("\n\n").map(|(_, w)| w).unwrap_or("");
                    let title = made.get(key).cloned().unwrap_or_else(|| key.to_string());
                    about_output("you", format!("You said about “{title}”: {}", first_line(words, 200)), key)
                } else {
                    happening("you", format!("{}: {}", if standing { "You pinned" } else { "You said" }, first_line(&text, 200)))
                }
            }
            "run-created" => happening("you", format!("You started it: {}", first_line(str_of(v, "goal").unwrap_or(""), 200))),
            "status-changed" => match (str_of(v, "from"), str_of(v, "to")) {
                (_, Some("done")) => happening("status", "Finished".to_string()),
                (_, Some("stopped")) => happening("status", "Stopped".to_string()),
                (_, Some("failed")) => happening("trouble", format!("Gave up{}", str_of(v, "reason").map(|r| format!(": {r}")).unwrap_or_default())),
                (_, Some("blocked")) => happening("trouble", format!("Got stuck{}", str_of(v, "reason").map(|r| format!(": {}", first_line(r, 200))).unwrap_or_default())),
                (Some("stopped" | "failed"), Some(_)) => happening("you", "You brought it back".to_string()),
                _ => None,
            },
            "done-claimed" => happening("status", "Said it's finished; a fresh look checks before it is".to_string()),
            "done-verified" => {
                happening("status", format!("Checked that it's finished: {}", first_line(str_of(v, "reason").unwrap_or(""), 200)))
            }
            "done-reopened" => happening(
                "trouble",
                format!("Not finished after all — a fresh look found: {}", first_line(str_of(v, "reason").unwrap_or(""), 200)),
            ),
            "start-failed" if v.get("consecutive").and_then(Value::as_u64) == Some(1) => {
                happening("trouble", "Couldn't start — claude exited straight away; retrying until it can".to_string())
            }
            "wake-incomplete" => happening("trouble", "Stopped part-way; it will try again".to_string()),
            "wake-killed" => happening("trouble", "Ran past its time limit and was stopped".to_string()),
            "usage-limited" => happening("trouble", "Hit the account's usage limit; waiting for it to reset".to_string()),
            "paused" => happening(
                "you",
                match str_of(v, "until").and_then(|u| Timestamp::parse(u).ok()) {
                    Some(until) => format!("You paused it until {}", crate::clock::plain_time(until)),
                    None => "You paused it".to_string(),
                },
            ),
            "unpaused" => happening("you", if str_of(v, "by") == Some("time") { "The pause ended" } else { "You resumed it" }.to_string()),
            "period-set" => happening(
                "you",
                format!(
                    "You set it to run every {}",
                    crate::clock::format_minutes(v.get("periodMinutes").and_then(Value::as_u64).unwrap_or(0))
                ),
            ),
            "check-set" => happening("status", "Now looks for news for free before each run".to_string()),
            "check-cleared" => happening("status", "Stopped checking for news before each run".to_string()),
            "renamed" => happening("you", format!("You renamed it from {}", str_of(v, "from").unwrap_or("?"))),
            "budget-exhausted" => happening("trouble", format!("Reached its budget: {}", str_of(v, "reason").unwrap_or(""))),
            "change-submitted" => {
                let title = str_of(v, "title").unwrap_or("");
                match v.get("version").and_then(Value::as_u64).unwrap_or(1) {
                    1 => happening("made", format!("Handed back code: {title}")),
                    n => happening("made", format!("Handed back code again: {title} (v{n})")),
                }
            }
            "change-moved" => {
                let what = first_line(str_of(v, "what").unwrap_or(""), 200);
                match (str_of(v, "by"), str_of(v, "state")) {
                    (Some("person"), _) => happening("you", format!("You {what}")),
                    (_, Some("landed")) if what.starts_with("landed") => happening("status", format!("Its change {what}")),
                    (_, Some("merged")) => happening("status", format!("Its change: {what}")),
                    (Some("author"), _) => happening("status", format!("It {what}")),
                    (_, Some("revising")) if what.starts_with("couldn't land") => happening("trouble", format!("The lander {what}")),
                    // A reviewer's ask, or CI's, relayed: "a reviewer (sam): …"
                    (_, Some("revising")) => happening("asked", format!("On its pull request, {what}")),
                    (_, Some("pr-open")) if what.starts_with("opened") => happening("status", format!("The lander {what}")),
                    _ => None,
                }
            }
            "output-published" => {
                let key = str_of(v, "output").unwrap_or("");
                let title = str_of(v, "title").unwrap_or(key);
                let text = match v.get("version").and_then(Value::as_u64).unwrap_or(1) {
                    1 if v.get("link").is_some() => format!("Linked: {title}"),
                    1 => format!("Made: {title}"),
                    n => format!("Updated: {title} (v{n})"),
                };
                about_output("made", text, key)
            }
            "output-state" => {
                let key = str_of(v, "output").unwrap_or("");
                let title = made.get(key).cloned().unwrap_or_else(|| key.to_string());
                let note = str_of(v, "note").map(|n| format!(" — {n}")).unwrap_or_default();
                match (str_of(v, "by"), str_of(v, "state")) {
                    (Some("person"), Some("done")) => about_output("you", format!("You marked “{title}” done"), key),
                    (Some("person"), Some("dropped")) => about_output("you", format!("You dropped “{title}”"), key),
                    (_, Some("dropped")) => about_output("status", format!("“{title}” isn't needed any more{note}"), key),
                    _ => None,
                }
            }
            "stages-set" => {
                let stages: Vec<&str> =
                    v.get("stages").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                happening("status", format!("Laid out its steps: {}", stages.join(" → ")))
            }
            _ => None,
        };
        match (item, out.last_mut()) {
            // Newest first, so the earlier repeat folds into the line already there.
            (Some(item), Some(last)) if last.text == item.text && last.kind == item.kind && last.message.is_none() => last.times += 1,
            (Some(item), _) => out.push(item),
            (None, _) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::test_init;
    use crate::state::read_run;

    fn journal(dir: &Path, lines: &[Value]) {
        let text: String = lines.iter().map(|v| v.to_string() + "\n").collect();
        std::fs::write(dir.join("journal.jsonl"), text).unwrap();
    }

    fn ago(minutes: i64) -> String {
        Timestamp::in_minutes(-minutes).to_string()
    }

    #[test]
    fn the_line_says_what_matters_first() {
        let mut state = crate::state::test_run_state("g");
        state.status = Status::Sleeping;
        assert_eq!(glance(&state, false, None, None, Some("Halfway.")).line, "Halfway.");
        assert_eq!(glance(&state, false, None, None, None).mood, Mood::Idle);
        assert_eq!(glance(&state, true, None, None, Some("Halfway.")).mood, Mood::Working);
        let quiet = glance(&state, true, Some(12), None, Some("Halfway."));
        assert_eq!(quiet.mood, Mood::Working, "quiet is worth knowing, not trouble");
        assert!(quiet.line.contains("nothing new from it for 12 minutes"), "{}", quiet.line);
        let asks = glance(&state, false, None, Some("Approve the plan?"), Some("Halfway."));
        assert_eq!((asks.mood, asks.line.as_str()), (Mood::Waiting, "Asks you: Approve the plan?"));
        state.start_failures = 3;
        let trouble = glance(&state, false, None, None, Some("Halfway."));
        assert_eq!(trouble.mood, Mood::Trouble);
        assert!(trouble.line.contains("3 times in a row"), "{}", trouble.line);
        state.start_failures = 0;
        state.paused = Some(crate::state::Pause { at: Timestamp::now(), until: None, reason: None });
        assert_eq!(glance(&state, false, None, None, None).line, "Paused");
        state.paused = Some(crate::state::Pause { at: Timestamp::now(), until: Some(Timestamp::in_minutes(-1)), reason: None });
        assert_eq!(glance(&state, false, None, None, None).mood, Mood::Idle, "a pause that has run out holds nothing");
        state.status = Status::Done;
        assert_eq!(glance(&state, false, None, None, Some("Merged.")).line, "Merged.");
    }

    #[test]
    fn the_week_marks_each_wake_by_what_it_did() {
        let _h = TempHome::new();
        test_init("t-week", "a goal").unwrap();
        let dir = crate::paths::run_dir("t-week").unwrap();
        journal(
            &dir,
            &[
                serde_json::json!({"ts": ago(600), "event": "wake-started", "wake": 1}),
                serde_json::json!({"ts": ago(599), "event": "noop-tick"}),
                serde_json::json!({"ts": ago(598), "event": "wake-complete", "status": "sleeping"}),
                serde_json::json!({"ts": ago(300), "event": "wake-started", "wake": 2}),
                serde_json::json!({"ts": ago(299), "event": "curated", "message": "added 2 talks"}),
                serde_json::json!({"ts": ago(298), "event": "wake-complete", "status": "sleeping"}),
                serde_json::json!({"ts": ago(200), "event": "wake-started", "wake": 3}),
                serde_json::json!({"ts": ago(200), "event": "start-failed", "reason": "exited 1", "consecutive": 1}),
                serde_json::json!({"ts": ago(100), "event": "gate-opened", "gate": "001", "slug": "go-on"}),
                serde_json::json!({"ts": ago(50), "event": "gate-closed", "gate": "001", "slug": "go-on", "answer": "Continue"}),
                serde_json::json!({"ts": ago(30000), "event": "wake-started", "wake": 0}),
                serde_json::json!({"ts": ago(29999), "event": "wake-complete", "status": "sleeping"}),
            ],
        );
        let state = read_run("t-week").unwrap();
        let week = timeline(&dir, &state);
        let marks: Vec<WakeMark> = week.wakes.iter().map(|m| m.mark).collect();
        assert_eq!(marks, vec![WakeMark::Ran, WakeMark::Did, WakeMark::CouldntStart], "and nothing from before the week");
        assert_eq!(week.wakes[1].text.as_deref(), Some("added 2 talks"));
        assert_eq!(week.waiting.len(), 1);
    }

    #[test]
    fn lately_is_the_story_without_the_rhythm() {
        let _h = TempHome::new();
        test_init("t-lately", "a goal").unwrap();
        let dir = crate::paths::run_dir("t-lately").unwrap();
        crate::messages::send(
            "t-lately",
            &crate::messages::Outgoing {
                kind: crate::messages::MessageKind::Report,
                title: "Why it fails",
                body: "Stale index.",
                draft: None,
                attachments: &[],
                output: None,
            },
        )
        .unwrap();
        crate::messages::done("t-lately", Some("1"), Some("dig into the grader")).unwrap();
        let story: Vec<String> = lately(&dir, 10).into_iter().map(|h| h.text).collect();
        assert_eq!(
            story,
            vec![
                "You replied to “Why it fails”: dig into the grader".to_string(),
                "Sent you a report: Why it fails".to_string(),
                "You started it: a goal".to_string(),
            ],
            "the reply's note is told once, as the reply"
        );
    }

    /// What it made reads as made, updated or linked, a person's mark on one as theirs, and a note
    /// about one says which.
    #[test]
    fn lately_tells_what_it_made() {
        let _h = TempHome::new();
        test_init("t-made", "a goal").unwrap();
        let dir = crate::paths::run_dir("t-made").unwrap();
        let doc = |text| crate::outputs::Publish { key: "design", title: "Design: tiers", text: Some((text, "md")), ..Default::default() };
        crate::outputs::publish("t-made", &doc("v1")).unwrap();
        crate::outputs::publish("t-made", &doc("v2")).unwrap();
        let link = crate::outputs::Publish { key: "proj-1", title: "Add rate_tier", link: Some("https://x.example/PROJ-1"), ..Default::default() };
        crate::outputs::publish("t-made", &link).unwrap();
        crate::outputs::set_stages("t-made", &["Design".into(), "Build".into()]).unwrap();
        crate::outputs::set_state("t-made", "proj-1", crate::outputs::OutputState::Done, None, "person").unwrap();
        let text = crate::core::note_about("t-made", Some("design"), "shorter").unwrap();
        crate::notes::add("t-made", &text, false).unwrap();

        let told: Vec<(String, Option<String>)> = lately(&dir, 20).into_iter().map(|h| (h.text, h.output)).collect();
        let design = Some("design".to_string());
        assert_eq!(
            told,
            vec![
                ("You said about “Design: tiers”: shorter".to_string(), design.clone()),
                ("You marked “Add rate_tier” done".to_string(), Some("proj-1".to_string())),
                ("Laid out its steps: Design → Build".to_string(), None),
                ("Linked: Add rate_tier".to_string(), Some("proj-1".to_string())),
                ("Updated: Design: tiers (v2)".to_string(), design.clone()),
                ("Made: Design: tiers".to_string(), design),
                ("You started it: a goal".to_string(), None),
            ]
        );
        assert_eq!(timeline(&dir, &read_run("t-made").unwrap()).made.len(), 3, "each version is a mark on the week");
    }
}
