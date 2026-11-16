//! `otto sandbox report <run>` (plans/v3.md, 1.7): what a run under observation would have been denied, and
//! what an enforced one was, read from the machine's log.
//!
//! Every rule a run's profile writes carries the message `otto-run-<id>`, and the kernel writes that message
//! on the line after the record it belongs to. This reads those lines, keeps the ones whose message is exactly
//! this run's, and folds them into a few lines a person can decide from. It only reads: no record, `run.json`
//! or journal is written. And it proves nothing, because a sandboxed process can write to the log itself; the
//! report says so every time.

use crate::error::OttoError;
use std::collections::BTreeMap;

/// `log` in zsh is a builtin that prints nothing, so it is always called by its path.
const LOG: &str = "/usr/bin/log";
/// More files than this under one directory, for one operation, become one line.
const HANDFUL: usize = 4;
/// The report is to decide from, not to scroll: at most this many lines of findings.
const MOST: usize = 30;

pub const CAVEAT: &str = "This guides a decision and proves nothing: a sandboxed process can write lines to the log itself, so read it as a hint of what to open, never as evidence of what a run did.";

#[derive(clap::Args, Debug)]
pub struct ReportArgs {
    /// The run: its number or name (a unique prefix of the name will do)
    pub id: String,
    /// From when: an age (45m, 2h, 3d), a date or a timestamp. Without it, from when the run was last put under observation or enforced
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,
    /// Until when, in the same forms. Without it, until now
    #[arg(long, value_name = "WHEN")]
    pub until: Option<String>,
    /// The same report as JSON
    #[arg(long)]
    pub json: bool,
    /// Read log lines from this file, as `/usr/bin/log show` printed them, and don't read the machine's log
    #[arg(long, value_name = "FILE")]
    pub log_file: Option<std::path::PathBuf>,
}

/// Whether the line was a real denial, or something allowed and logged because the run is only observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Blocked: the run is enforced.
    Denied,
    /// Not blocked, and logged: the run is under observation, and enforcing would block it.
    WouldDeny,
}

/// One record from the log that carries this run's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// As the log wrote it, to the millisecond, in the machine's own time zone.
    pub at: String,
    pub process: String,
    pub verdict: Verdict,
    pub operation: String,
    /// A path, or a service's name for `mach-lookup` and the like.
    pub target: String,
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Finding {
    pub verdict: Verdict,
    pub operation: String,
    pub target: String,
    pub count: usize,
    #[serde(rename = "firstSeen")]
    pub first_seen: String,
    #[serde(rename = "lastSeen")]
    pub last_seen: String,
    /// How many distinct paths under `target` this line stands for; 0 when it is one path or service.
    pub grouped: usize,
    pub processes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Findings left off the end because there were more than a person would read.
    pub more: usize,
    /// Records with this run's message that couldn't be read as `process(pid) allow|deny operation target`.
    pub unread: usize,
}

/// The log's lines as (time, message) pairs, the message with its following lines attached. Takes either
/// style: `--style ndjson`, one JSON object a line with the message's line break inside `eventMessage`, or
/// `--style compact`, where what follows a record's line, up to the next line that starts with a time, is its.
fn messages(log: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in log.lines() {
        if line.starts_with('{') {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(message) = value.get("eventMessage").and_then(|m| m.as_str()) {
                    let at = value.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
                    out.push((at.chars().take(23).collect(), message.to_string()));
                    continue;
                }
            }
        }
        if starts_with_time(line) {
            let at: String = line.chars().take(23).collect();
            let message = line.find("Sandbox: ").map(|i| &line[i..]).unwrap_or("");
            out.push((at, message.to_string()));
        } else if let Some(last) = out.last_mut() {
            last.1.push('\n');
            last.1.push_str(line);
        }
    }
    out
}

/// `2026-11-15 22:45:20.631`: how a record's line starts in the compact style.
fn starts_with_time(line: &str) -> bool {
    let b = line.as_bytes();
    b.len() >= 23 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' && b[7] == b'-' && b[10] == b' ' && b[13] == b':' && b[16] == b':' && b[19] == b'.'
}

/// `Sandbox: ls(17353) allow file-read-data /Users/x/.ssh`, or `… deny(1) mach-lookup com.apple.pasteboard.1`.
fn record(first: &str) -> Option<(String, Verdict, String, String)> {
    let rest = first.trim().strip_prefix("Sandbox: ")?;
    let (allow, deny) = (rest.find(") allow "), rest.find(") deny("));
    let (at, verdict) = match (allow, deny) {
        (Some(a), Some(d)) if a < d => (a, Verdict::WouldDeny),
        (Some(a), None) => (a, Verdict::WouldDeny),
        (_, Some(d)) => (d, Verdict::Denied),
        (None, None) => return None,
    };
    let process = rest[..at].rsplit_once('(').map(|(name, _pid)| name).unwrap_or(&rest[..at]).to_string();
    let after = &rest[at + 2..];
    let (_verdict_word, after) = after.split_once(' ')?;
    let (operation, target) = after.split_once(' ').unwrap_or((after, ""));
    (!operation.is_empty()).then(|| (process, verdict, operation.to_string(), target.trim().to_string()))
}

/// The records in `log` that carry exactly this run's message, and how many of those couldn't be read. A
/// message is a line of its own after the record, and must be the whole of that line: run 7's report takes
/// nothing of run 77's.
pub fn hits(log: &str, run_id: &str) -> (Vec<Hit>, usize) {
    let message = format!("otto-run-{run_id}");
    let (mut found, mut unread) = (Vec::new(), 0);
    for (at, text) in messages(log) {
        let mut lines = text.lines();
        let first = lines.next().unwrap_or("");
        if !lines.any(|line| line.trim() == message) {
            continue;
        }
        match record(first) {
            Some((process, verdict, operation, target)) => found.push(Hit { at, process, verdict, operation, target }),
            None => unread += 1,
        }
    }
    (found, unread)
}

fn parent(target: &str) -> Option<&str> {
    let (dir, name) = target.rsplit_once('/')?;
    (target.starts_with('/') && !dir.is_empty() && !name.is_empty()).then_some(dir)
}

/// The report for one run from the log's text: the same operation on the same target is one line with a
/// count; more than a handful of files under one directory, for one operation, are one line that says so;
/// most frequent first.
pub fn report(log: &str, run_id: &str) -> Report {
    let (found, unread) = hits(log, run_id);
    let mut by_target: BTreeMap<(Verdict, String, String), Finding> = BTreeMap::new();
    for hit in found {
        let entry = by_target.entry((hit.verdict, hit.operation.clone(), hit.target.clone())).or_insert_with(|| Finding {
            verdict: hit.verdict,
            operation: hit.operation.clone(),
            target: hit.target.clone(),
            count: 0,
            first_seen: hit.at.clone(),
            last_seen: hit.at.clone(),
            grouped: 0,
            processes: Vec::new(),
        });
        entry.count += 1;
        if hit.at < entry.first_seen {
            entry.first_seen = hit.at.clone();
        }
        if hit.at > entry.last_seen {
            entry.last_seen = hit.at.clone();
        }
        if !entry.processes.contains(&hit.process) {
            entry.processes.push(hit.process);
        }
    }
    // How many distinct paths each directory holds, for one verdict and operation.
    let mut siblings: BTreeMap<(Verdict, String, String), usize> = BTreeMap::new();
    for (verdict, operation, target) in by_target.keys() {
        if let Some(dir) = parent(target) {
            *siblings.entry((*verdict, operation.clone(), dir.to_string())).or_default() += 1;
        }
    }
    let mut folded: BTreeMap<(Verdict, String, String), Finding> = BTreeMap::new();
    for ((verdict, operation, target), finding) in by_target {
        let dir = parent(&target).filter(|dir| siblings.get(&(verdict, operation.clone(), dir.to_string())).is_some_and(|n| *n > HANDFUL));
        let Some(dir) = dir else {
            folded.insert((verdict, operation, target), finding);
            continue;
        };
        let key = (verdict, operation.clone(), format!("{dir}/"));
        match folded.get_mut(&key) {
            None => {
                folded.insert(key.clone(), Finding { target: key.2.clone(), grouped: 1, ..finding });
            }
            Some(group) => {
                group.count += finding.count;
                group.grouped += 1;
                group.first_seen = group.first_seen.clone().min(finding.first_seen);
                group.last_seen = group.last_seen.clone().max(finding.last_seen);
                for process in finding.processes {
                    if !group.processes.contains(&process) {
                        group.processes.push(process);
                    }
                }
            }
        }
    }
    let mut findings: Vec<Finding> = folded.into_values().collect();
    findings.sort_by(|a, b| b.count.cmp(&a.count).then(a.verdict.cmp(&b.verdict)).then(a.target.cmp(&b.target)).then(a.operation.cmp(&b.operation)));
    let more = findings.len().saturating_sub(MOST);
    findings.truncate(MOST);
    Report { findings, more, unread }
}

/// `22:45:20` when first and last are the same day, the whole stamp to the second otherwise.
fn seen(finding: &Finding) -> String {
    let (first, last) = (finding.first_seen.get(..19).unwrap_or(&finding.first_seen), finding.last_seen.get(..19).unwrap_or(&finding.last_seen));
    if first == last {
        format!("at {first}")
    } else if first.get(..10) == last.get(..10) {
        format!("first {first}, last {}", last.get(11..).unwrap_or(last))
    } else {
        format!("first {first}, last {last}")
    }
}

fn finding_line(finding: &Finding) -> String {
    let mut processes = finding.processes.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
    if finding.processes.len() > 4 {
        processes.push_str(&format!(" and {} more", finding.processes.len() - 4));
    }
    let target = if finding.grouped > 0 { format!("{} ({} paths under it, grouped)", finding.target, finding.grouped) } else { finding.target.clone() };
    format!("  {:>5}×  {:<24} {target}   {}, by {processes}", finding.count, finding.operation, seen(finding))
}

/// The report as a person reads it. `window` and `about` are sentences of their own, written by the caller.
pub fn render(label: &str, short: &str, window: &str, notes: &[String], report: &Report) -> String {
    let mut out = vec![format!("Sandbox report for {label}"), format!("  window    {window}")];
    out.extend(notes.iter().map(|note| format!("  note      {note}")));
    let denied: Vec<&Finding> = report.findings.iter().filter(|f| f.verdict == Verdict::Denied).collect();
    let would: Vec<&Finding> = report.findings.iter().filter(|f| f.verdict == Verdict::WouldDeny).collect();
    if report.findings.is_empty() {
        out.push(String::new());
        out.push("Nothing in the log for this run in that window: nothing denied, and nothing that would have been.".to_string());
    }
    if !denied.is_empty() {
        out.push(String::new());
        out.push(format!("Denied (blocked, the run is enforced): {}", denied.len()));
        out.extend(denied.iter().map(|f| finding_line(f)));
    }
    if !would.is_empty() {
        out.push(String::new());
        out.push(format!("Would be denied (allowed and logged, the run is under observation): {}", would.len()));
        out.extend(would.iter().map(|f| finding_line(f)));
    }
    if report.more > 0 {
        out.push(format!("  … and {} less frequent lines left out; narrow the window with --since and --until, or see them all with --json", report.more));
    }
    if report.unread > 0 {
        out.push(format!("  ({} log records carried this run's message and couldn't be read)", report.unread));
    }
    out.push(String::new());
    out.push(CAVEAT.to_string());
    out.push(format!(
        "To open a path, add it to \"read\" or \"write\" in the run's record by hand, or with `otto sandbox observe {short} --read <path>` / `--write <path>` (docs/guides/sandboxing.md, \"Moving a run in and out\"). This report changes nothing."
    ));
    out.join("\n") + "\n"
}

/// When the run was last put under observation or enforced, by its journal.
fn last_switch(id: &str) -> Option<time::OffsetDateTime> {
    let journal = std::fs::read_to_string(crate::paths::run_dir(id).ok()?.join("journal.jsonl")).ok()?;
    journal
        .lines()
        .filter_map(crate::core::parse_line)
        .filter(|line| matches!(line.value.get("event").and_then(|e| e.as_str()), Some("sandbox-observed" | "sandbox-enforced")))
        .filter_map(|line| line.at)
        .next_back()
}

/// How `log show --start` and `--end` take a time: the machine's own zone, to the second.
fn for_log(at: time::OffsetDateTime) -> String {
    let format = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    at.to_offset(crate::clock::local_offset()).format(&format).expect("a date and time format")
}

fn read_log(id: &str, since: time::OffsetDateTime, until: Option<time::OffsetDateTime>) -> Result<String, OttoError> {
    let mut command = std::process::Command::new(LOG);
    command.args(["show", "--style", "ndjson", "--start", &for_log(since)]);
    if let Some(until) = until {
        command.args(["--end", &for_log(until)]);
    }
    // CONTAINS also brings run 77's lines for run 7: the exact match is made on the message's own line, in `hits`.
    command.args(["--predicate", &format!("sender == \"Sandbox\" AND eventMessage CONTAINS \"otto-run-{id}\"")]);
    let out = command.output().map_err(|e| OttoError::conflict(format!("{LOG} couldn't be started ({e}), so there is no report. Inside a sandbox the log may not be readable: run this outside one")))?;
    if !out.status.success() {
        return Err(OttoError::conflict(format!(
            "{LOG} show failed ({}): {}. There is no report. Inside a sandbox the log may not be readable: run this outside one",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `otto sandbox report <run>`. Reads the run's journal, its record's age and the log; writes nothing.
pub fn run(args: ReportArgs) -> Result<(), OttoError> {
    let id = crate::paths::resolve_run_id(&args.id)?;
    crate::state::read_run(&id)?;
    let (label, short) = (crate::paths::label_of(&id), crate::paths::short_id(&id));
    let offset = crate::clock::local_offset();
    let until = args.until.as_deref().map(|text| crate::core::parse_since(text, offset).map_err(|e| OttoError::usage(e.message.replace("--since", "--until")))).transpose()?;
    let record = super::record_path(&id);
    let mut notes = Vec::new();
    let (since, from) = match &args.since {
        Some(text) => (crate::core::parse_since(text, offset)?, "as --since said".to_string()),
        None => match (last_switch(&id), record.metadata().and_then(|m| m.modified())) {
            (Some(at), _) => (at, "when the run was last put under observation or enforced".to_string()),
            (None, Ok(modified)) => (modified.into(), "when its sandbox record was last written, as its journal doesn't say when it was put under observation".to_string()),
            (None, Err(_)) => (crate::clock::now() - time::Duration::hours(24), "the last 24 hours, for want of anything better".to_string()),
        },
    };
    match super::read_record(&id) {
        Ok(record) => notes.push(format!("the record's mode is {} now", if record.mode == super::Mode::Report { "report: nothing is blocked, and what it doesn't allow is logged" } else { "enforce: what it doesn't allow is blocked" })),
        Err(_) if !super::has_record(&id) => notes.push(format!("{label} has no sandbox record, so it isn't sandboxed now and its wakes log nothing. `otto sandbox observe {short}` puts it under observation; --since <when> looks further back")),
        Err(e) => notes.push(format!("its sandbox record can't be used ({e})")),
    }
    let window = format!("{} to {} ({from})", for_log(since), until.map(for_log).unwrap_or_else(|| "now".to_string()));
    let log = match &args.log_file {
        Some(path) => {
            notes.push(format!("read from {}, not the machine's log, so the window wasn't applied", path.display()));
            std::fs::read_to_string(path).map_err(|e| OttoError::usage(format!("{} can't be read: {e}", path.display())))?
        }
        None => read_log(&id, since, until)?,
    };
    let report = report(&log, &id);
    if args.json {
        let value = serde_json::json!({
            "run": id,
            "since": crate::clock::format_iso(since),
            "until": until.map(crate::clock::format_iso),
            "notes": notes,
            "findings": report.findings,
            "more": report.more,
            "unread": report.unread,
            "caveat": CAVEAT,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        print!("{}", render(&label, &short, &window, &notes, &report));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPACT: &str = include_str!("../tests/fixtures/sandbox-report/compact.txt");
    const NDJSON: &str = include_str!("../tests/fixtures/sandbox-report/ndjson.txt");

    #[test]
    fn real_lines_are_read_in_either_style_with_the_message_on_the_line_after() {
        for log in [COMPACT, NDJSON] {
            let (found, unread) = hits(log, "9007");
            assert_eq!(unread, 0);
            assert_eq!(found.len(), 5, "{found:?}");
            assert_eq!(found[0], Hit { at: "2026-11-15 22:45:20.631".into(), process: "ls".into(), verdict: Verdict::WouldDeny, operation: "file-read-metadata".into(), target: "/Users/joshuahill/.ssh".into() });
            assert!(found.iter().all(|hit| hit.verdict == Verdict::WouldDeny));
        }
    }

    #[test]
    fn another_runs_lines_are_left_out_even_when_its_id_starts_the_same() {
        // The fixture was captured with a predicate for otto-run-9007, which brought run 90077's lines too.
        assert!(COMPACT.contains("otto-run-90077") && NDJSON.contains("otto-run-90077"));
        for log in [COMPACT, NDJSON] {
            let ours = report(log, "9007");
            assert!(ours.findings.iter().all(|f| !f.target.contains("closed") && f.operation != "mach-lookup"), "{ours:?}");
            let theirs = report(log, "90077");
            assert_eq!(theirs.findings.len(), 2);
            assert!(theirs.findings.iter().all(|f| f.verdict == Verdict::Denied && f.count == 1));
            assert!(theirs.findings.iter().any(|f| f.operation == "mach-lookup" && f.target == "com.apple.pasteboard.1" && f.processes == ["pbpaste"]));
            assert!(theirs.findings.iter().any(|f| f.operation == "file-write-create" && f.target == "/private/tmp/sbrep/closed/a"));
            assert!(report(log, "900").findings.is_empty() && report(log, "7").findings.is_empty());
        }
    }

    #[test]
    fn the_same_thing_twice_is_one_line_with_a_count_and_when() {
        let report = report(COMPACT, "9007");
        assert_eq!(report.findings.len(), 3);
        let read = report.findings.iter().find(|f| f.operation == "file-read-data").unwrap();
        assert_eq!((read.count, read.first_seen.as_str(), read.last_seen.as_str()), (2, "2026-11-15 22:45:20.631", "2026-11-15 22:45:20.634"));
        assert_eq!(read.target, "/Users/joshuahill/.ssh");
        // Most frequent first.
        assert_eq!(report.findings.iter().map(|f| f.count).collect::<Vec<_>>(), [2, 2, 1]);
    }

    fn line(second: u32, process: &str, verdict: &str, rest: &str, id: &str) -> String {
        format!("2026-11-15 10:00:{second:02}.000 Df kernel[0:1] (Sandbox) Sandbox: {process}(1) {verdict} {rest}\notto-run-{id}\n")
    }

    #[test]
    fn a_noisy_directory_is_one_line_that_says_it_is_grouped() {
        let mut log = String::from("Timestamp               Ty Process[PID:TID]\n");
        for n in 0..12 {
            log += &line(n, "node", "allow", &format!("file-read-data /Users/x/Library/Caches/thing/{n}.bin"), "7");
            log += &line(n, "node", "allow", &format!("file-read-data /Users/x/Library/Caches/thing/{n}.bin"), "7");
        }
        log += &line(20, "git helper", "deny(1)", "file-write-create /Users/x/notes/a b.txt", "7");
        log += &line(21, "cat", "allow", "file-read-data /Users/x/.ssh/config", "77");
        let report = report(&log, "7");
        assert_eq!(report.findings.len(), 2, "{report:?}");
        let group = &report.findings[0];
        assert_eq!((group.target.as_str(), group.count, group.grouped), ("/Users/x/Library/Caches/thing/", 24, 12));
        assert_eq!((group.first_seen.as_str(), group.last_seen.as_str()), ("2026-11-15 10:00:00.000", "2026-11-15 10:00:11.000"));
        let denied = &report.findings[1];
        assert_eq!((denied.verdict, denied.target.as_str(), denied.processes[0].as_str()), (Verdict::Denied, "/Users/x/notes/a b.txt", "git helper"));
        let text = render("#7 seven", "7", "then to now", &[], &report);
        assert!(text.contains("(12 paths under it, grouped)"), "{text}");
        assert!(text.contains("Denied (blocked") && text.contains("Would be denied (allowed and logged"), "{text}");
        assert!(text.find("Denied (blocked").unwrap() < text.find("Would be denied").unwrap());
        assert!(!text.contains(".ssh"), "run 77's line is not run 7's: {text}");
        assert!(text.contains("guides a decision and proves nothing"), "{text}");
        assert!(text.lines().count() < 20, "{text}");
    }

    #[test]
    fn hundreds_of_distinct_things_are_still_a_few_lines() {
        let mut log = String::new();
        for n in 0..300 {
            log += &line(n % 60, "p", "allow", &format!("file-read-data /d{n}/f"), "7");
        }
        let report = report(&log, "7");
        assert_eq!((report.findings.len(), report.more), (MOST, 300 - MOST));
        assert!(render("#7", "7", "w", &[], &report).lines().count() < 40);
    }

    #[test]
    fn nothing_found_says_so_and_still_says_it_proves_nothing() {
        let text = render("#7", "7", "w", &[], &report("", "7"));
        assert!(text.contains("Nothing in the log for this run") && text.contains("proves nothing"), "{text}");
    }
}
