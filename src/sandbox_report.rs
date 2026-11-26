//! `otto sandbox report <run>` (plans/v3.md, 1.7): what a run under observation would have been denied, and
//! what an enforced one was, read from the machine's log.
//!
//! Every rule a run's profile writes carries the message `otto-run-<id>@<hash of the otto home>`
//! (`sandbox::tag`), and the kernel writes that message on the line after the record it belongs to. This reads
//! those lines, keeps the ones whose message is exactly this run's in this otto home, and folds them into a few
//! lines a person can decide from. Lines with the older message, `otto-run-<id>` and no home, could be any
//! home's run of that number: they are counted and left out. It only reads: no record, `run.json`
//! or journal is written. And it proves nothing, because a sandboxed process can write to the log itself; the
//! report says so every time.

use crate::error::OttoError;
use std::collections::BTreeMap;
use std::path::Path;

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
    /// Records whose message is the older `otto-run-<id>`, with no otto home: not known to be this run's, so
    /// counted and left out of the findings.
    #[serde(rename = "olderUntagged")]
    pub older_untagged: usize,
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
            // After the sender, so that `1 duplicate report for Sandbox: …` keeps its count.
            let message = line.find("(Sandbox) ").map(|i| &line[i + 10..]).or_else(|| line.find("Sandbox: ").map(|i| &line[i..])).unwrap_or("");
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

/// The records in `log` that carry exactly this run's message for this otto home, how many of those couldn't
/// be read, and how many carried the older message with no home in it. A message is a line of its own after
/// the record, and must be the whole of that line: run 7's report takes nothing of run 77's, and one otto
/// home's nothing of another's.
pub fn hits(log: &str, run_id: &str, otto_home: &Path) -> (Vec<Hit>, usize, usize) {
    let message = super::tag(run_id, otto_home);
    let older_message = format!("otto-run-{run_id}");
    let (mut found, mut unread, mut older) = (Vec::new(), 0, 0);
    for (at, text) in messages(log) {
        let mut lines = text.lines();
        let first = lines.next().unwrap_or("");
        let after: Vec<&str> = lines.map(str::trim).collect();
        if !after.contains(&message.as_str()) {
            if after.contains(&older_message.as_str()) {
                older += 1;
            }
            continue;
        }
        // `1 duplicate report for Sandbox: …` is the kernel saying the record before it happened that many times more.
        let (times, first) = match first.split_once(" duplicate report") {
            Some((n, rest)) if n.trim().parse::<usize>().is_ok() => (n.trim().parse().unwrap_or(1), rest.find("Sandbox: ").map(|i| &rest[i..]).unwrap_or(rest)),
            _ => (1, first),
        };
        match record(first) {
            Some((process, verdict, operation, target)) => {
                let hit = Hit { at, process, verdict, operation, target };
                found.extend(std::iter::repeat_n(hit, times.clamp(1, 100_000)));
            }
            None => unread += 1,
        }
    }
    (found, unread, older)
}

fn parent(target: &str) -> Option<&str> {
    let (dir, name) = target.rsplit_once('/')?;
    (target.starts_with('/') && !dir.is_empty() && !name.is_empty()).then_some(dir)
}

/// The report for one run from the log's text: the same operation on the same target is one line with a
/// count; more than a handful of files under one directory, for one operation, are one line that says so;
/// most frequent first.
pub fn report(log: &str, run_id: &str, otto_home: &Path) -> Report {
    let (found, unread, older_untagged) = hits(log, run_id, otto_home);
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
    Report { findings, more, unread, older_untagged }
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

/// What the report says of lines with the older message, when there are any.
pub fn older_note(count: usize, run_id: &str) -> String {
    format!(
        "{count} older line{} for a run numbered {run_id} carr{} no otto home (written before otto's reports kept to one home, or by an older otto): {} not known to be this run's and {} left out",
        if count == 1 { "" } else { "s" },
        if count == 1 { "ies" } else { "y" },
        if count == 1 { "it is" } else { "they are" },
        if count == 1 { "is" } else { "are" },
    )
}

/// The report as a person reads it. `window` and `about` are sentences of their own, written by the caller.
pub fn render(label: &str, short: &str, run_id: &str, window: &str, notes: &[String], report: &Report) -> String {
    let mut out = vec![format!("Sandbox report for {label}"), format!("  window    {window}")];
    out.extend(notes.iter().map(|note| format!("  note      {note}")));
    let denied: Vec<&Finding> = report.findings.iter().filter(|f| f.verdict == Verdict::Denied).collect();
    let would: Vec<&Finding> = report.findings.iter().filter(|f| f.verdict == Verdict::WouldDeny).collect();
    if report.findings.is_empty() {
        out.push(String::new());
        out.push("Nothing in the log for this run, in this otto home, in that window: nothing denied, and nothing that would have been.".to_string());
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
    if report.older_untagged > 0 {
        out.push(String::new());
        out.push(format!("{}.", older_note(report.older_untagged, run_id)));
    }
    out.push(String::new());
    out.push(CAVEAT.to_string());
    out.push(format!(
        "To open a path, add it to \"read\" or \"write\" in the run's record by hand, or with `--read <path>` / `--write <path>` on `otto sandbox observe {short}` or `otto sandbox enforce {short}`, whichever the run is under now (docs/guides/sandboxing.md, \"Moving a run in and out\"). This report changes nothing."
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
    // CONTAINS also brings run 77's lines for run 7, other otto homes' and the older lines with no home: the
    // exact match is made on the message's own line, in `hits`.
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
        Ok(record) => notes.push(format!("the record's mode is now {}", if record.mode == super::Mode::Report { "report: nothing is blocked, and what it doesn't allow is logged" } else { "enforce: what it doesn't allow is blocked" })),
        Err(_) if !super::has_record(&id) => notes.push(format!("{label} has no sandbox record, so it isn't sandboxed now and its wakes log nothing. `otto sandbox observe {short}` puts it under observation; --since <when> looks further back")),
        Err(e) => notes.push(format!("its sandbox record can't be used ({e})")),
    }
    if let Some(until) = until {
        if until <= since {
            return Err(OttoError::usage(format!(
                "the window would end before it starts: it starts {} ({from}) and --until puts its end at {}. Give both: --since <when> --until <when>, with --since the earlier",
                for_log(since),
                for_log(until)
            )));
        }
    }
    let window = format!("{} to {} ({from})", for_log(since), until.map(for_log).unwrap_or_else(|| "now".to_string()));
    let log = match &args.log_file {
        Some(path) => {
            notes.push(format!("read from {}, not the machine's log, so the window wasn't applied", path.display()));
            std::fs::read_to_string(path).map_err(|e| OttoError::usage(format!("{} can't be read: {e}", path.display())))?
        }
        None => read_log(&id, since, until)?,
    };
    let report = report(&log, &id, &crate::paths::otto_home());
    if args.json {
        let value = serde_json::json!({
            "run": id,
            "since": crate::clock::format_iso(since),
            "until": until.map(crate::clock::format_iso),
            "notes": notes,
            "findings": report.findings,
            "more": report.more,
            "unread": report.unread,
            "olderUntagged": report.older_untagged,
            "olderUntaggedNote": (report.older_untagged > 0).then(|| older_note(report.older_untagged, &id)),
            "caveat": CAVEAT,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        print!("{}", render(&label, &short, &id, &window, &notes, &report));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPACT: &str = include_str!("../tests/fixtures/sandbox-report/compact.txt");
    const NDJSON: &str = include_str!("../tests/fixtures/sandbox-report/ndjson.txt");
    /// The otto home the fixtures' lines are tagged for. It doesn't exist, so its hash is of the path as written.
    const FIX: &str = "/fixture/otto-home";
    const OTHER: &str = "/another/otto-home";

    #[test]
    fn real_lines_are_read_in_either_style_with_the_message_on_the_line_after() {
        for log in [COMPACT, NDJSON] {
            let (found, unread, _older) = hits(log, "9007", Path::new(FIX));
            assert_eq!(unread, 0);
            assert_eq!(found.len(), 5, "{found:?}");
            assert_eq!(found[0], Hit { at: "2026-11-15 22:45:20.631".into(), process: "ls".into(), verdict: Verdict::WouldDeny, operation: "file-read-metadata".into(), target: "/Users/joshuahill/.ssh".into() });
            assert!(found.iter().all(|hit| hit.verdict == Verdict::WouldDeny));
        }
    }

    #[test]
    fn another_runs_lines_are_left_out_even_when_its_id_starts_the_same() {
        // The fixture was captured with a predicate for otto-run-9007, which brought run 90077's lines too.
        assert!(COMPACT.contains("otto-run-90077@") && NDJSON.contains("otto-run-90077@"));
        for log in [COMPACT, NDJSON] {
            let ours = report(log, "9007", Path::new(FIX));
            assert!(ours.findings.iter().all(|f| !f.target.contains("closed") && f.operation != "mach-lookup"), "{ours:?}");
            let theirs = report(log, "90077", Path::new(FIX));
            assert_eq!(theirs.findings.len(), 2);
            assert!(theirs.findings.iter().all(|f| f.verdict == Verdict::Denied && f.count == 1));
            assert!(theirs.findings.iter().any(|f| f.operation == "mach-lookup" && f.target == "com.apple.pasteboard.1" && f.processes == ["pbpaste"]));
            assert!(theirs.findings.iter().any(|f| f.operation == "file-write-create" && f.target == "/private/tmp/sbrep/closed/a"));
            assert!(report(log, "900", Path::new(FIX)).findings.is_empty() && report(log, "7", Path::new(FIX)).findings.is_empty());
        }
    }

    #[test]
    fn real_denials_are_told_from_what_was_only_logged_and_a_duplicate_report_counts() {
        for log in [include_str!("../tests/fixtures/sandbox-report/enforced-compact.txt"), include_str!("../tests/fixtures/sandbox-report/enforced-ndjson.txt")] {
            assert!(log.contains("1 duplicate report for Sandbox: ls("));
            let report = report(log, "3", Path::new(FIX));
            assert_eq!(report.unread, 0);
            assert_eq!(report.findings.len(), 2, "{report:?}");
            assert!(report.findings.iter().all(|f| f.verdict == Verdict::Denied));
            assert_eq!((report.findings[0].operation.as_str(), report.findings[0].target.as_str(), report.findings[0].count), ("file-read-metadata", "/Users/joshuahill/.ssh", 2));
            assert!(report.findings[1].target.ends_with("/outside/made-enforced") && report.findings[1].count == 1);
        }
    }

    #[test]
    fn the_same_thing_twice_is_one_line_with_a_count_and_when() {
        let report = report(COMPACT, "9007", Path::new(FIX));
        assert_eq!(report.findings.len(), 3);
        let read = report.findings.iter().find(|f| f.operation == "file-read-data").unwrap();
        assert_eq!((read.count, read.first_seen.as_str(), read.last_seen.as_str()), (2, "2026-11-15 22:45:20.631", "2026-11-15 22:45:20.634"));
        assert_eq!(read.target, "/Users/joshuahill/.ssh");
        // Most frequent first.
        assert_eq!(report.findings.iter().map(|f| f.count).collect::<Vec<_>>(), [2, 2, 1]);
    }

    fn line(second: u32, process: &str, verdict: &str, rest: &str, id: &str) -> String {
        line_of(second, process, verdict, rest, &crate::sandbox::tag(id, Path::new(FIX)))
    }

    /// A record as the compact style prints it, with whatever message on the line after.
    fn line_of(second: u32, process: &str, verdict: &str, rest: &str, message: &str) -> String {
        format!("2026-11-15 10:00:{second:02}.000 Df kernel[0:1] (Sandbox) Sandbox: {process}(1) {verdict} {rest}\n{message}\n")
    }

    #[test]
    fn the_same_run_id_from_two_otto_homes_keeps_only_this_homes_lines() {
        let theirs = crate::sandbox::tag("7", Path::new(OTHER));
        let mut log = line(1, "ls", "allow", "file-read-data /Users/x/mine", "7");
        log += &line_of(2, "ls", "allow", "file-read-data /Users/x/.ssh", &theirs);
        log += &line_of(3, "bash", "deny(1)", "file-write-create /Users/x/theirs", &theirs);
        // The same as ndjson, the message's line break inside eventMessage.
        log += &format!("{{\"timestamp\":\"2026-11-15 10:00:04.000000-0600\",\"eventMessage\":\"Sandbox: ls(9) allow file-read-data /Users/x/.ssh/config\\n{theirs}\"}}\n");
        let ours = report(&log, "7", Path::new(FIX));
        assert_eq!(ours.findings.iter().map(|f| f.target.as_str()).collect::<Vec<_>>(), ["/Users/x/mine"], "{ours:?}");
        assert_eq!((ours.unread, ours.older_untagged), (0, 0));
        let text = render("#7", "7", "7", "w", &[], &ours);
        assert!(!text.contains(".ssh") && !text.contains("theirs") && !text.contains("older line"), "{text}");
        let other = report(&log, "7", Path::new(OTHER));
        assert_eq!(other.findings.len(), 3, "{other:?}");
        assert!(other.findings.iter().all(|f| f.target != "/Users/x/mine"));
    }

    #[test]
    fn a_second_run_in_the_same_home_is_not_shown_and_run_7_takes_nothing_of_run_77s() {
        let mut log = line(1, "ls", "allow", "file-read-data /Users/x/seven", "7");
        log += &line(2, "ls", "allow", "file-read-data /Users/x/eight", "8");
        log += &line(3, "ls", "deny(1)", "file-read-data /Users/x/seventy-seven", "77");
        log += &line_of(4, "ls", "allow", "file-read-data /Users/x/elsewhere-seventy-seven", &crate::sandbox::tag("77", Path::new(OTHER)));
        let targets = |id: &str| report(&log, id, Path::new(FIX)).findings.into_iter().map(|f| f.target).collect::<Vec<_>>();
        assert_eq!(targets("7"), ["/Users/x/seven"]);
        assert_eq!(targets("8"), ["/Users/x/eight"]);
        assert_eq!(targets("77"), ["/Users/x/seventy-seven"]);
        assert_eq!(report(&log, "7", Path::new(FIX)).older_untagged, 0, "run 77's tag is not run 7's older line");
        // A run id can't have `@` in it, but a tag used as one still matches nothing.
        assert!(report(&log, &crate::sandbox::tag("7", Path::new(FIX))["otto-run-".len()..], Path::new(FIX)).findings.is_empty());
    }

    #[test]
    fn lines_with_the_older_message_are_counted_said_and_left_out() {
        let mut log = line(1, "ls", "allow", "file-read-data /Users/x/mine", "7");
        log += &line_of(2, "ls", "allow", "file-read-data /Users/x/.ssh", "otto-run-7");
        log += &line_of(3, "bash", "deny(1)", "file-write-create /Users/x/old", "otto-run-7");
        log += &line_of(4, "bash", "deny(1)", "file-write-create /Users/x/old77", "otto-run-77");
        log += "{\"timestamp\":\"2026-11-15 10:00:05.000000-0600\",\"eventMessage\":\"Sandbox: ls(9) allow file-read-data /Users/x/.ssh/config\\notto-run-7\"}\n";
        let report = report(&log, "7", Path::new(FIX));
        assert_eq!(report.findings.iter().map(|f| f.target.as_str()).collect::<Vec<_>>(), ["/Users/x/mine"], "{report:?}");
        assert_eq!(report.older_untagged, 3);
        let text = render("#7", "7", "7", "w", &[], &report);
        assert!(text.contains("3 older lines for a run numbered 7 carry no otto home") && text.contains("they are not known to be this run's and are left out"), "{text}");
        assert!(!text.contains(".ssh") && !text.contains("/Users/x/old"), "{text}");
        assert!(text.contains("guides a decision and proves nothing") && text.contains("This report changes nothing"), "{text}");
        assert_eq!(serde_json::to_value(&report).unwrap()["olderUntagged"], 3);
        // Only older lines: nothing is listed, and it still says why.
        let only = super::report(&line_of(2, "ls", "allow", "file-read-data /Users/x/.ssh", "otto-run-7"), "7", Path::new(FIX));
        let text = render("#7", "7", "7", "w", &[], &only);
        assert!(only.findings.is_empty() && text.contains("Nothing in the log for this run") && text.contains("1 older line for a run numbered 7 carries no otto home"), "{text}");
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
        let report = report(&log, "7", Path::new(FIX));
        assert_eq!(report.findings.len(), 2, "{report:?}");
        let group = &report.findings[0];
        assert_eq!((group.target.as_str(), group.count, group.grouped), ("/Users/x/Library/Caches/thing/", 24, 12));
        assert_eq!((group.first_seen.as_str(), group.last_seen.as_str()), ("2026-11-15 10:00:00.000", "2026-11-15 10:00:11.000"));
        let denied = &report.findings[1];
        assert_eq!((denied.verdict, denied.target.as_str(), denied.processes[0].as_str()), (Verdict::Denied, "/Users/x/notes/a b.txt", "git helper"));
        let text = render("#7 seven", "7", "7", "then to now", &[], &report);
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
        let report = report(&log, "7", Path::new(FIX));
        assert_eq!((report.findings.len(), report.more), (MOST, 300 - MOST));
        assert!(render("#7", "7", "7", "w", &[], &report).lines().count() < 40);
    }

    #[test]
    fn nothing_found_says_so_and_still_says_it_proves_nothing() {
        let text = render("#7", "7", "7", "w", &[], &report("", "7", Path::new(FIX)));
        assert!(text.contains("Nothing in the log for this run") && text.contains("proves nothing"), "{text}");
    }
}
