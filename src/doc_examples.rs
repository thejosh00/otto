//! Every example in the documentation is executed, in the spirit of rustdoc's doctests — which
//! don't apply here: otto is a binary crate, and its docs show shell commands and JSON rather than
//! Rust. So these are ordinary tests over the markdown:
//!
//! 1. every `otto …` command in a code block parses against the real CLI,
//! 2. every `config.json` example loads through the real config loader,
//! 3. every journal event otto writes, with every field, is documented in `journal.md`.
//!
//! The CLI reference itself is generated (`cli::reference`); these cover everything written by
//! hand, including the harness and workflows a wake is given — where a stale flag costs a wake.

use clap::Parser;
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Every hand-written markdown file whose examples a person or a wake will copy. DESIGN.md is left
/// out on purpose: it is a record of reasoning, and keeps commands from designs since changed.
fn documents() -> Vec<PathBuf> {
    let root = Path::new(ROOT);
    let mut files = vec![root.join("README.md"), root.join("harness/wake.md")];
    for dir in ["docs", "workflows", "skills", "test/fixtures"] {
        collect(&root.join(dir), &mut files);
    }
    // Generated from the CLI; checked by `cli::reference`.
    files.retain(|f| !f.ends_with("docs/reference/cli.md"));
    files
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// A fenced code block: its language tag (possibly empty), its body, and where it starts.
struct Block {
    file: String,
    line: usize,
    lang: String,
    body: String,
}

fn blocks(path: &Path) -> Vec<Block> {
    let text = std::fs::read_to_string(path).unwrap();
    let file = path.strip_prefix(ROOT).unwrap_or(path).display().to_string();
    let mut out = Vec::new();
    let mut open: Option<(usize, String, String)> = None;
    for (n, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(fence) = trimmed.strip_prefix("```") {
            match open.take() {
                Some((start, lang, body)) => out.push(Block { file: file.clone(), line: start, lang, body }),
                None => open = Some((n + 1, fence.trim().to_string(), String::new())),
            }
        } else if let Some((_, _, body)) = open.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

/// Split a documented command line the way a shell would, near enough for docs: quotes group,
/// an unquoted `#` starts a comment, and an unquoted `|` (`|| stop`) ends the command.
fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    in_word = true;
                }
                '#' if !in_word => break,
                '|' => break,
                c if c.is_whitespace() => {
                    if in_word {
                        words.push(std::mem::take(&mut word));
                        in_word = false;
                    }
                }
                c => {
                    word.push(c);
                    in_word = true;
                }
            },
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// The `otto` commands in a block, with continuation lines joined and the docs' own notation made
/// concrete: `<placeholder>` becomes a value, and `[--optional flag]` is taken as given.
fn otto_commands(block: &Block) -> Vec<(usize, String, Vec<String>)> {
    let mut out = Vec::new();
    let mut lines = block.body.lines().enumerate().peekable();
    while let Some((n, line)) = lines.next() {
        let trimmed = line.trim_start();
        // ASCII diagrams (`otto run ──> wake 1`) are pictures, not commands.
        if !trimmed.starts_with("otto ") || trimmed.contains('─') {
            continue;
        }
        let mut command = trimmed.to_string();
        while command.trim_end().ends_with('\\') {
            let cut = command.trim_end().len() - 1;
            command.truncate(cut);
            match lines.next() {
                Some((_, next)) => command.push_str(next),
                None => break,
            }
        }
        let concrete = placeholders(&command).replace(['[', ']'], "");
        out.push((block.line + 1 + n, command, shell_words(&concrete)));
    }
    out
}

/// `<id>`, `<that file>`, `<n+1>` → a value any argument accepts, numbers included.
fn placeholders(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let Some(len) = rest[start..].find('>') else { break };
        out.push_str(&rest[..start]);
        out.push('1');
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

/// A removed or renamed flag in any documented command fails here, naming the file and line.
#[test]
fn every_documented_otto_command_parses() {
    let mut checked = 0;
    let mut failures = Vec::new();
    for doc in documents() {
        for block in blocks(&doc) {
            if !matches!(block.lang.as_str(), "" | "bash" | "sh" | "shell" | "console" | "text") {
                continue;
            }
            for (line, original, words) in otto_commands(&block) {
                checked += 1;
                if let Err(e) = crate::cli::Cli::try_parse_from(&words) {
                    let why = e.to_string().lines().next().unwrap_or_default().to_string();
                    failures.push(format!("{}:{line}: `{}`\n    {why}", block.file, original.trim()));
                }
            }
        }
    }
    assert!(checked > 50, "found only {checked} documented commands — is the extraction broken?");
    assert!(failures.is_empty(), "documented commands the CLI rejects:\n{}", failures.join("\n"));
}

/// Every `config.json` shown in the docs loads, through the same parser otto uses — which refuses
/// unknown keys, so a renamed field fails here too.
#[test]
fn every_documented_config_loads() {
    let mut checked = 0;
    for doc in documents() {
        for block in blocks(&doc).into_iter().filter(|b| b.lang == "json") {
            let value: serde_json::Value = serde_json::from_str(&block.body)
                .unwrap_or_else(|e| panic!("{}:{}: not valid JSON: {e}", block.file, block.line));
            if value.get("launchers").is_some() {
                checked += 1;
                let config = crate::config::parse(&block.body)
                    .unwrap_or_else(|e| panic!("{}:{}: not a valid config.json: {e}", block.file, block.line));
                for launcher in &config.launchers {
                    assert!(
                        config.launcher(&launcher.name).is_ok(),
                        "{}:{}: launcher {:?} does not resolve by its own name",
                        block.file,
                        block.line,
                        launcher.name
                    );
                }
            }
        }
    }
    assert!(checked >= 2, "expected config examples in the docs, found {checked}");
}

/// One of every event otto writes, with every optional field present so every field is checked.
/// The match below has no wildcard, so adding an event without a sample here does not compile.
fn one_of_each_event() -> Vec<crate::event::Event> {
    use crate::event::Event::*;
    use crate::state::CheckResult;
    let at = crate::clock::Timestamp::now();
    let s = || "x".to_string();
    let some = || Some("x".to_string());
    let events = vec![
        RunCreated { wraps: s(), reference: some(), goal: s(), done_condition: some(), perpetual: false, phase: s(), target: some(), launcher: s() },
        PhaseChanged { from: s(), to: s(), status: s(), note: some() },
        StatusChanged { from: s(), to: s(), reason: some(), because: some() },
        FactsRecorded { facts: serde_json::Map::new() },
        GateOpened { gate: s(), slug: s(), phase: s(), expires_at: Some(at) },
        GateClosed { gate: s(), slug: s(), asked_at: at, answered_at: at, answer: s() },
        GateExpired { gate: s(), slug: s(), asked_at: at, answered_at: at, answer: s() },
        TimerArmed { next_wake_at: at, note: some() },
        Tick { ticks_without_progress: 0, note: some() },
        NoopTick { ticks_without_progress: 0, note: some() },
        WakeStarted { wake: 1, deadline_at: at, launcher: s() },
        WakeSpent { turns: 0, spent_wakes: 0, input_tokens: 0, output_tokens: 0, cache_read: 0, cache_creation: 0, tool_errors: 0 },
        UsageUnavailable { note: s() },
        WakeReportedError { exit_code: 1, timed_out: false, turns: 0 },
        BudgetWarning { fraction_spent: 0.8, budget: crate::state::test_run_state("x").budget },
        WakeComplete { status: s() },
        WakeIncomplete { reason: s(), consecutive: 1 },
        BudgetExhausted { reason: s() },
        LockAcquired { repo: s() },
        LockReleased { repo: s() },
        LockBroken { repo: s(), previous_owner: s(), reason: s() },
        LockLost { repo: s(), taken_by: s(), reason: s() },
        Authorized { action: s(), item: some(), head: s(), by: s(), source: s() },
        WakeKilled { reason: s() },
        SpawnAbandoned { reason: s() },
        CheckRan { result: CheckResult::Changed, consecutive_no_change: 0, note: some() },
        NoteAdded { note: s(), file: s(), standing: false },
        NotesDelivered { notes: vec![], wake: 1 },
        NoteDropped { note: s(), standing: false },
        CheckSet { script: s(), wake_after: 24, by: s() },
        PeriodSet { period_minutes: 60, previous_minutes: 60 },
        CheckCleared { script: s(), by: s() },
        Notified { key: s(), title: s(), message: s() },
    ];
    for event in &events {
        match event {
            RunCreated { .. } | PhaseChanged { .. } | StatusChanged { .. } | FactsRecorded { .. }
            | GateOpened { .. } | GateClosed { .. } | GateExpired { .. } | TimerArmed { .. }
            | Tick { .. } | NoopTick { .. } | WakeStarted { .. } | WakeSpent { .. }
            | UsageUnavailable { .. } | WakeReportedError { .. } | BudgetWarning { .. }
            | WakeComplete { .. } | WakeIncomplete { .. } | BudgetExhausted { .. }
            | LockAcquired { .. } | LockReleased { .. } | LockBroken { .. } | LockLost { .. }
            | Authorized { .. } | WakeKilled { .. } | SpawnAbandoned { .. } | CheckRan { .. }
            | NoteAdded { .. } | NotesDelivered { .. } | NoteDropped { .. } | CheckSet { .. }
            | PeriodSet { .. } | CheckCleared { .. } | Notified { .. } => {}
        }
    }
    events
}

/// `journal.md` lists every event otto writes and every field it carries — in the table row that
/// names the event, or a row that says it has the `same` fields as the one above.
#[test]
fn every_journal_event_is_documented() {
    let doc = std::fs::read_to_string(Path::new(ROOT).join("docs/reference/journal.md")).unwrap();
    let rows: Vec<&str> = doc.lines().filter(|l| l.starts_with('|')).collect();
    let mut missing = Vec::new();
    for event in one_of_each_event() {
        let value = serde_json::to_value(&event).unwrap();
        let name = value["event"].as_str().unwrap().to_string();
        let Some(row) = rows.iter().find(|r| r.contains(&format!("`{name}`"))) else {
            missing.push(format!("event `{name}`"));
            continue;
        };
        if row.contains("| same |") {
            continue;
        }
        for field in value.as_object().unwrap().keys().filter(|k| *k != "event") {
            if !row.contains(&format!("`{field}`")) {
                missing.push(format!("`{name}`'s field `{field}`"));
            }
        }
    }
    assert!(missing.is_empty(), "docs/reference/journal.md is missing: {}", missing.join(", "));
}
