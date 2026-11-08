//! src/why.rs — `otto why`: one ordered account of a wake, from what otto already keeps.
//!
//! Reads only. No lock is taken, no file is made, and no `crate::paths` helper that creates its
//! directory is called, so it is safe against a live wake, however often. Whether a wake is running
//! comes from `liveness::probe_path`, whose shared lock can't look like a holder: poke's probe is
//! shared too, and a wake taking the lock retries past a probe's instant. Only when the lock file is
//! there: the probe would make it, and a run with no lock file has no wake holding it.
//!
//! It reports what was recorded and never why. A source that doesn't go back far enough, or isn't
//! there, says "not recorded", because no line from it doesn't mean nothing happened.
//!
//! Each source gives [`Entry`]s and any words about itself; a new one, like the sandbox's denials,
//! is one more function for [`account`] to call.

use crate::clock::format_iso;
use crate::core::{parse_line, Line, Renderer};
use crate::error::OttoError;
use crate::liveness::{probe_path, read_holder, WakeState, WAKE_LOCK_FILE};
use crate::state::RunState;
use serde_json::Value;
use std::path::Path;
use time::OffsetDateTime;

/// One screen, with the opening above it. Past this, repeats collapse, and then a stretch is cut.
const TIMELINE_MAX: usize = 30;
/// A line longer than this would wrap on a person's screen.
const WIDTH: usize = 110;
/// Past an entry's time and source, where what's further of it goes.
const INDENT: usize = 29;
/// A start's fields the opening says instead, or that say nothing about why.
const IN_THE_OPENING: &[&str] = &["session", "launcher"];
const TOKEN_FIELDS: &[&str] = &["spentTokens", "inputTokens", "outputTokens", "cacheRead", "cacheCreation", "spentUsd"];
const ENDINGS: &[&str] = &["wake-complete", "wake-incomplete", "wake-killed", "usage-limited"];
/// Worth keeping when a stretch has to be cut.
const TURNING_POINTS: &[&str] = &["wake-started", "note-added", "status-changed", "gate-opened", "gate-closed", "gate-expired"];

#[derive(clap::Args, Debug)]
pub struct WhyArgs {
    /// The run: its number or name (a unique prefix of the name will do)
    pub id: String,
    /// Which wake [default: the run's latest]
    #[arg(long, value_name = "N")]
    pub wake: Option<u64>,
}

pub fn why(args: WhyArgs) -> Result<(), OttoError> {
    let id = crate::paths::resolve_run_id(&args.id)?;
    print!("{}", account(&id, args.wake)?);
    Ok(())
}

/// Which source a line came from. In this order on a tie: a check's result leads to poke's decision,
/// and poke's stamp is never earlier than what it did when the two share a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    Check,
    Poke,
    Journal,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::Check => "check",
            Source::Poke => "poke",
            Source::Journal => "journal",
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    at: OffsetDateTime,
    /// Where a collapsed run of them ends, when there is more than one.
    last: OffsetDateTime,
    count: usize,
    source: Source,
    /// The event, poke's action or the check's result.
    kind: String,
    text: String,
    /// Further lines of it, for a kill's fields, which are what the line is for.
    more: Vec<String>,
}

impl Entry {
    fn new(at: OffsetDateTime, source: Source, kind: &str, text: String) -> Self {
        Entry { at, last: at, count: 1, source, kind: kind.to_string(), text, more: Vec::new() }
    }

    fn turning_point(&self) -> bool {
        match self.source {
            Source::Poke => true,
            Source::Check => self.kind != "no-change",
            Source::Journal => TURNING_POINTS.contains(&self.kind.as_str()) || ENDINGS.contains(&self.kind.as_str()),
        }
    }

    fn lines(&self) -> Vec<String> {
        let head = format!("{} {:<7} ", format_iso(self.at), self.source.name());
        let body = if self.count > 1 {
            format!("{} ×{} to {}: {}", self.kind, self.count, format_iso(self.last), self.text)
        } else {
            format!("{:<15} {}", self.kind, self.text)
        };
        let mut lines = vec![fit(&format!("{head}{}", body.trim_end()))];
        lines.extend(self.more.iter().map(|more| fit(&format!("{:INDENT$}{more}", ""))));
        lines
    }
}

/// Fields as lines that fit under an entry, broken between fields and never in one.
fn wrapped(fields: &[String]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for field in fields {
        match lines.last_mut() {
            Some(line) if INDENT + line.chars().count() + 1 + field.chars().count() <= WIDTH => {
                line.push(' ');
                line.push_str(field);
            }
            _ => lines.push(field.clone()),
        }
    }
    lines
}

fn fit(line: &str) -> String {
    if line.chars().count() <= WIDTH {
        return line.to_string();
    }
    format!("{}…", line.chars().take(WIDTH - 1).collect::<String>())
}

/// The span a wake's account covers. `to` is the next wake's start, or nothing while it is the last.
#[derive(Debug, Clone, Copy)]
struct Window {
    from: OffsetDateTime,
    to: Option<OffsetDateTime>,
}

impl Window {
    fn holds(&self, at: OffsetDateTime) -> bool {
        at >= self.from && self.to.is_none_or(|to| at <= to)
    }
}

/// What a source found: its lines in the window, what it has to say about itself, and the lines it
/// couldn't read there.
#[derive(Debug, Default)]
struct Found {
    entries: Vec<Entry>,
    said: Vec<String>,
    garbled: usize,
}

fn event_of(line: &Line) -> &str {
    line.value.get("event").and_then(Value::as_str).unwrap_or("?")
}

/// A journal line, in the same words as `otto logs`, less what would run off the screen.
fn journal_entry(line: &Line, at: OffsetDateTime) -> Entry {
    let event = event_of(line);
    let fields = Renderer::new(time::UtcOffset::UTC).fields(line);
    let named = |field: &str, names: &[&str]| names.iter().any(|name| field.starts_with(&format!("{name}=")));
    let shown = |field: &&String| match event {
        "wake-spent" => !named(field, TOKEN_FIELDS),
        "wake-started" => !named(field, IN_THE_OPENING),
        _ => true,
    };
    let fields: Vec<&String> = fields.iter().filter(shown).collect();
    let mut entry = Entry::new(at, Source::Journal, event, String::new());
    if event == "wake-incomplete" {
        entry.text = fields.iter().map(|f| f.strip_prefix("reason=").unwrap_or(f)).collect::<Vec<_>>().join(" ");
    } else if event == "wake-killed" {
        // The reason on the line itself; what the kill recorded under it, and it mustn't be cut off.
        let (reason, rest): (Vec<&String>, Vec<&String>) = fields.into_iter().partition(|f| f.starts_with("reason="));
        entry.text = reason.iter().map(|f| f.trim_start_matches("reason=")).collect();
        let (recorded, missing): (Vec<&String>, Vec<&String>) = rest.into_iter().partition(|f| !f.ends_with("=not recorded"));
        let mut more: Vec<String> = recorded.iter().map(|f| f.to_string()).collect();
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|f| f.trim_end_matches("=not recorded")).collect();
            more.push(format!("not recorded: {}", names.join(", ")));
        }
        entry.more = wrapped(&more);
    } else {
        entry.text = fields.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(" ");
    }
    entry
}

/// `<stamp> <action> <run> <reason>`, for this run: the run's id as a whole word, and an action is
/// one lowercase word. That rules out poke's note about a gap, whose second word is its length.
fn poke_decision(words: &[&str], ids: &[&str]) -> bool {
    let [_, action, run, ..] = words else { return false };
    ids.contains(run) && action.chars().all(|c| c.is_ascii_lowercase())
}

/// What's left of a decision past its stamp, action and run, however they were padded.
fn after_three_words(line: &str) -> &str {
    let mut rest = line;
    for _ in 0..3 {
        rest = rest.trim_start();
        rest = rest.find(char::is_whitespace).map_or("", |i| &rest[i..]);
    }
    rest.trim_start()
}

fn is_gap_note(words: &[&str]) -> bool {
    matches!(words, [_, length, "since", "the", "last", "pass,", ..] if length.starts_with(|c: char| c.is_ascii_digit()))
}

/// Poke's lines about this run and its notes about long gaps. A line with no time can't be placed,
/// so it counts where the lines around it say it was. `nothing due` isn't per run and isn't listed.
fn poke_lines(window: Window, ids: &[&str]) -> Found {
    let mut found = Found::default();
    let mut oldest: Option<OffsetDateTime> = None;
    let mut any_file = false;
    let mut last_stamp: Option<OffsetDateTime> = None;
    for path in crate::logs::poke_logs_oldest_first() {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        any_file = true;
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let words: Vec<&str> = line.split_whitespace().collect();
            let Some(at) = words.first().and_then(|word| crate::clock::parse_iso(word).ok()) else {
                if !line.trim().is_empty() && last_stamp.is_some_and(|at| window.holds(at)) {
                    found.garbled += 1;
                }
                continue;
            };
            last_stamp = Some(at);
            oldest = oldest.or(Some(at));
            if !window.holds(at) {
                continue;
            }
            if poke_decision(&words, ids) {
                found.entries.push(Entry::new(at, Source::Poke, words[1], after_three_words(line).to_string()));
            } else if is_gap_note(&words) {
                let note = words[1..].join(" ");
                found.entries.push(Entry::new(at, Source::Poke, "gap", note));
            }
        }
    }
    match oldest {
        None if any_file => found.said.push("poke.log        not recorded: no line in it has a time".to_string()),
        None => found.said.push("poke.log        not recorded: there is no poke.log".to_string()),
        Some(oldest) if oldest > window.from => {
            found.said.push(format!("poke.log        not recorded before {}, its oldest line", format_iso(oldest)));
        }
        Some(_) => {}
    }
    found
}

/// What the run's check said, from the capped history. Only poke writes it, so anything else in it
/// is counted, not read.
fn check_lines(dir: &Path, window: Window) -> Found {
    let mut found = Found::default();
    let Ok(bytes) = crate::state::links::read(dir.join(crate::check_history::FILE)) else {
        found.said.push("check results   not recorded: no history for this run".to_string());
        return found;
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut oldest: Option<OffsetDateTime> = None;
    for line in text.lines() {
        let record: Option<Value> = serde_json::from_str(line).ok();
        let at = record.as_ref().and_then(|r| crate::clock::parse_iso(r["at"].as_str()?).ok());
        let (Some(record), Some(at)) = (record, at) else {
            found.garbled += 1;
            continue;
        };
        oldest = oldest.or(Some(at));
        if !window.holds(at) {
            continue;
        }
        let kind = if record["timedOut"] == true { "timed-out" } else { record["result"].as_str().unwrap_or("?") };
        let mut text = format!("exit {}", record["code"]);
        if let Some(note) = record["note"].as_str() {
            text = format!("{text}: {note}");
        }
        found.entries.push(Entry::new(at, Source::Check, kind, text));
    }
    match oldest {
        None => found.said.push("check results   not recorded: the history is empty".to_string()),
        Some(oldest) if window.to.is_some_and(|to| oldest > to) => {
            found.said.push(format!("check results   not recorded: the history begins {}, after this", format_iso(oldest)));
        }
        Some(oldest) if oldest > window.from => {
            found.said.push(format!("check results   not recorded before {}, where the history begins", format_iso(oldest)));
        }
        Some(_) => {}
    }
    found
}

/// Adjacent entries of one kind become one line, with the count and the span.
fn collapse(entries: Vec<Entry>, same: impl Fn(&Entry) -> bool) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for entry in entries {
        match out.last_mut() {
            Some(prev) if prev.source == entry.source && prev.kind == entry.kind && same(&entry) => {
                prev.count += entry.count;
                prev.last = entry.last;
                prev.text = entry.text;
            }
            _ => out.push(entry),
        }
    }
    out
}

/// Down to `max` lines by cutting one stretch, chosen to hold the fewest turning points and, of those,
/// nearest the middle, which keeps what led to the wake and what came after it. What was cut is
/// said where it was, with how to see it.
fn cut_to_fit(entries: Vec<Entry>, max: usize, run: &str) -> Vec<String> {
    let render = |entries: &[Entry]| entries.iter().flat_map(Entry::lines).collect::<Vec<_>>();
    if entries.len() <= max {
        return render(&entries);
    }
    let cut = entries.len() - max + 1;
    let middle = (entries.len() - cut) / 2;
    let start = (0..=entries.len() - cut)
        .min_by_key(|&i| (entries[i..i + cut].iter().filter(|e| e.turning_point()).count(), i.abs_diff(middle)))
        .unwrap_or(0);
    let gone = &entries[start..start + cut];
    let count = |source: Source| gone.iter().filter(|e| e.source == source).map(|e| e.count).sum::<usize>();
    let mut said: Vec<String> = Vec::new();
    for (source, see) in [
        (Source::Journal, format!("`otto logs {run} --since {}`", format_iso(gone[0].at))),
        (Source::Poke, "logs/poke.log".to_string()),
        (Source::Check, crate::check_history::FILE.to_string()),
    ] {
        let n = count(source);
        if n > 0 {
            said.push(format!("{n} {} ({see})", source.name()));
        }
    }
    let mut lines = render(&entries[..start]);
    lines.push(fit(&format!("… cut to fit a screen: {}", said.join(", "))));
    lines.extend(render(&entries[start + cut..]));
    lines
}

/// How a wake ended, as the journal has it, every ending in order: a wake can complete and be
/// "killed" a day later, and the account shows both without saying why.
fn ending_lines(endings: &[&Line]) -> Vec<String> {
    endings
        .iter()
        .map(|line| {
            let at = line.at.map(format_iso).unwrap_or_default();
            let text = |key: &str| line.value.get(key).and_then(Value::as_str).unwrap_or("not recorded").to_string();
            match event_of(line) {
                "wake-complete" => format!("completed {at}, status {}", text("status")),
                "wake-incomplete" => format!("incomplete {at}: {}", text("reason")),
                "wake-killed" => format!("killed {at}: {}", text("reason")),
                _ => format!("hit the usage limit {at}"),
            }
        })
        .collect()
}

/// With no ending on record: running, if it's the last wake and the lock is held, and otherwise
/// exactly that. The record beside the lock says whose it was last, so it's only quoted if it's this
/// wake's.
fn no_ending(dir: &Path, id: &str, wake: u64, is_latest: bool, deadline: Option<&str>) -> String {
    let lock = dir.join(WAKE_LOCK_FILE);
    let state = if lock.exists() { probe_path(&lock) } else { WakeState::Idle };
    match state {
        WakeState::Running if is_latest => {
            let pid = read_holder(id).filter(|h| h.wake == Some(wake as u32)).map(|h| format!(", pid {}", h.pid));
            let until = deadline.and_then(|d| crate::clock::parse_iso(d).ok()).map(|d| format!(", deadline {}", format_iso(d)));
            format!("still running{}{}", pid.unwrap_or_default(), until.unwrap_or_default())
        }
        WakeState::Unknown(why) if is_latest => format!("no ending on record; couldn't tell if it's running: {why}"),
        _ if is_latest => "no ending on record, and no wake is running now".to_string(),
        _ => "no ending on record".to_string(),
    }
}

fn transcript_line(session: Option<&str>) -> String {
    let Some(session) = session else {
        return "session not recorded".to_string();
    };
    match crate::wake::transcript::find_transcript(session) {
        Some(path) => path.display().to_string(),
        None => format!("session {session}: no longer on disk"),
    }
}

fn account(id: &str, wanted: Option<u64>) -> Result<String, OttoError> {
    let dir = crate::paths::run_dir(id)?;
    let state: Option<RunState> = crate::state::read_run(id).ok();
    let label = state.as_ref().map(RunState::label).unwrap_or_else(|| id.to_string());
    let journal = crate::state::links::read(dir.join("journal.jsonl")).ok();
    let journal_text = journal.as_deref().map(String::from_utf8_lossy).unwrap_or_default();
    // In the file's order, so the window is positions, and what a garbled line sits in is known.
    let lines: Vec<Option<Line>> = journal_text.lines().map(|raw| parse_line(raw).filter(|line| line.at.is_some())).collect();
    let is_start = |line: &Line| event_of(line) == "wake-started";
    let wakes: Vec<(usize, u64)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| Some((i, line.as_ref().filter(|l| is_start(l))?.value.get("wake")?.as_u64()?)))
        .collect();

    let (Some(lowest), Some(highest)) = (wakes.iter().map(|w| w.1).min(), wakes.iter().map(|w| w.1).max()) else {
        if let Some(n) = wanted {
            return Err(OttoError::usage(format!("{label} has no wakes on record, so no wake {n}")));
        }
        let why = if journal.is_none() { "it has no journal" } else { "its journal has none" };
        return Ok(format!("# {label}\nno wake on record: {why}\n"));
    };
    let which = match wanted {
        // The last with that number, should one ever repeat.
        Some(n) => wakes.iter().rposition(|w| w.1 == n).ok_or_else(|| {
            OttoError::usage(format!("{label} never had a wake {n}: its wakes on record run from {lowest} to {highest}"))
        })?,
        None => wakes.len() - 1,
    };
    let (start, n) = wakes[which];
    let next = wakes.get(which + 1).map(|w| w.0);
    let is_ending = |line: &&Line| ENDINGS.contains(&event_of(line));
    // From the end of the wake before, or its start if it has no ending on record.
    let first = match which.checked_sub(1).map(|p| wakes[p].0) {
        None => 0,
        Some(prev) => (prev + 1..start).find(|&i| lines[i].as_ref().is_some_and(|l| is_ending(&l))).unwrap_or(prev),
    };
    let last = next.unwrap_or(lines.len() - 1);
    let timed = |i: usize| lines[i].as_ref().and_then(|l| l.at);
    let window = Window {
        from: (first..=start).find_map(timed).expect("a wake-started line has a time"),
        to: next.and_then(timed),
    };

    let started = lines[start].as_ref().expect("a wake-started line parsed");
    let endings: Vec<&Line> = lines[start..=last].iter().flatten().filter(is_ending).collect();
    let session = started.value.get("session").and_then(Value::as_str).map(str::to_string).or_else(|| {
        // `run.json` names the last wake's: all there is for one from before the journal did.
        let wake = state.as_ref()?.wake.as_ref()?;
        (wake.n as u64 == n).then(|| wake.session.clone()).flatten()
    });

    let mut out = format!("# {label}\nwake {n} · wakes on record {lowest} to {highest} · times are UTC\n");
    let cause = started.value.get("cause").and_then(Value::as_str).unwrap_or("not recorded");
    out.push_str(&format!("started         {}, cause: {cause}\n", format_iso(window_at(started))));
    if endings.is_empty() {
        let deadline = started.value.get("deadlineAt").and_then(Value::as_str);
        out.push_str(&format!("ended           {}\n", no_ending(&dir, id, n, next.is_none(), deadline)));
    }
    for (i, line) in ending_lines(&endings).iter().enumerate() {
        out.push_str(&fit(&format!("{:<15} {line}", if i == 0 { "ended" } else { "then" })));
        out.push('\n');
    }
    out.push_str(&format!("transcript      {}\n", transcript_line(session.as_deref())));
    let output = dir.join(format!("wake-{n}-output.txt"));
    if output.is_file() {
        out.push_str(&format!("output          {}\n", output.display()));
    }

    let former = state.as_ref().and_then(|s| s.former_id.as_deref());
    let ids: Vec<&str> = std::iter::once(id).chain(former).collect();
    let mut journal_found = Found::default();
    for line in &lines[first..=last] {
        match line {
            Some(line) => journal_found.entries.push(journal_entry(line, window_at(line))),
            None => journal_found.garbled += 1,
        }
    }
    let sources = [
        ("journal.jsonl", journal_found),
        ("poke.log", poke_lines(window, &ids)),
        (crate::check_history::FILE, check_lines(&dir, window)),
    ];
    for (_, found) in &sources {
        for said in &found.said {
            out.push_str(&fit(said));
            out.push('\n');
        }
    }
    for (name, found) in &sources {
        if found.garbled > 0 {
            out.push_str(&format!("skipped         {} unreadable or undated line(s) in {name}\n", found.garbled));
        }
    }

    let has_poke = sources[1].1.entries.iter().any(|e| e.kind != "gap");
    let mut entries: Vec<Entry> = sources.into_iter().flat_map(|(_, found)| found.entries).collect();
    // Stable: lines from one source keep their order, whatever the clock did.
    entries.sort_by_key(|e| (e.at, e.source));
    let mut entries = collapse(entries, |e| e.source == Source::Check && e.kind == "no-change");
    if entries.len() > TIMELINE_MAX {
        entries = collapse(entries, |e| !e.turning_point());
    }
    let timeline = cut_to_fit(entries, TIMELINE_MAX, id);
    out.push('\n');
    for line in &timeline {
        out.push_str(line);
        out.push('\n');
    }
    let mut legend = Vec::new();
    if has_poke {
        legend.push("poke's times are when its pass finished: its line can follow what it did.");
    }
    if timeline.iter().any(|line| line.ends_with('…')) {
        legend.push("a line ending … is shortened: the journal, poke.log or the check history has it whole.");
    }
    if !legend.is_empty() {
        out.push('\n');
        out.push_str(&legend.join("\n"));
        out.push('\n');
    }
    Ok(out)
}

/// Every line kept has a time: those without are counted as unreadable when the journal is read.
fn window_at(line: &Line) -> OffsetDateTime {
    line.at.expect("kept lines have a time")
}
