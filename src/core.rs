//! What each person-facing command does, as data.
//!
//! `human.rs` (the terminal) and `server` (the browser) are two renderings of this module, and
//! neither decides anything the other doesn't. Nothing here prints or reads stdin: the one
//! interactive piece — `otto answer`'s numbered menu — stays in `human`, because only a terminal
//! has someone to type into it.
//!
//! Both front ends read and write the run directory directly. The server is a view over
//! `~/.otto`, not an owner of it: `otto ls` and the web page agree because they read the same
//! files, and the CLI never needs the server to be running.

use crate::clock::relative;
use crate::error::OttoError;
use crate::liveness::{Liveness, LockLiveness};
use crate::paths::{short_id, short_id_among};
use crate::spawner::Strategy;
use crate::state::commands::InitArgs;
use crate::state::{read_all_runs, read_run, Blocked, BlockedCause, Detach, RunEntry, RunState, Status};
use serde::Serialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Starting a wake
// ---------------------------------------------------------------------------

/// Who is asking for a wake, which decides what `Detach::None` means. A person in a terminal
/// asked to watch it there; anything else — poke, the web server — has no terminal to put it in
/// and must never block on a wake that can run for most of an hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Terminal,
    Background,
}

impl Caller {
    pub fn strategy(self, detach: Detach) -> Strategy {
        match (self, detach) {
            (Caller::Terminal, Detach::None) => Strategy::Foreground,
            (Caller::Background, detach) => Strategy::for_poke(detach),
            (Caller::Terminal, Detach::Tmux) => Strategy::Tmux,
        }
    }
}

/// What starting a wake produced. `note` is `None` for a foreground wake, which already said how
/// it went.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeStart {
    pub session: Option<String>,
    pub note: Option<String>,
}

/// Start a wake under `strategy` and, when it is backgrounded, confirm it actually started. A
/// detached wake that dies immediately — a bad PATH, a lost $OTTO_HOME, a missing binary — takes
/// its tmux session with it and writes nothing anywhere, so "wake started" would be a lie nobody
/// could check. Waiting for the wake number to move is definitive: `otto wake` records it before
/// spawning.
pub fn start_wake(id: &str, strategy: Strategy, answer: Option<&str>) -> Result<WakeStart, OttoError> {
    let mut exec = crate::exec::RealExec;
    if strategy == Strategy::Foreground {
        crate::spawner::start(id, strategy, answer, &mut exec)?;
        return Ok(WakeStart::default());
    }
    let before = crate::spawner::wake_number(id);
    let handle = crate::spawner::start(id, strategy, answer, &mut exec)?;
    if !crate::spawner::wait_for_hold(id, before) {
        let container = match &handle.session {
            Some(session) => format!("started tmux session {session}"),
            None => "started a detached wake".to_string(),
        };
        return Err(OttoError::usage(format!(
            "{container} but no wake took hold within {}s — the wake process exited immediately. \
             Run `otto wake {} --watch` in a terminal to see why.",
            crate::spawner::CONFIRM_SECONDS,
            short_id(id)
        )));
    }
    Ok(WakeStart { session: handle.session, note: handle.description })
}

/// Refuse to background a wake onto a run that already has one. The foreground path gets this
/// from the wake lock. The background path would not: the child would die on the lock while
/// `spawner::wait_for_hold` saw that very lock and reported success.
pub fn refuse_if_busy(id: &str) -> Result<(), OttoError> {
    if LockLiveness.probe(id).is_busy() {
        return Err(OttoError::conflict(format!(
            "a wake is already running for {} — `otto attach {}` to watch it, or wait for it to finish",
            id,
            short_id(id)
        )));
    }
    Ok(())
}

/// Wait for an in-flight wake to finish, bounded.
///
/// A wake writes its gate to disk and then spends a few more seconds on its handoff and exit, so
/// `otto ls` can show a question as open while the wake that asked it is still running. Answering
/// in that window used to fail: `close_gate` succeeded, then the spawn hit the wake lock and
/// reported "a wake is already running", which looks like the answer was rejected when it was
/// safely recorded. Waiting is the honest fix — the work is nearly done, and the alternative
/// (spawning anyway) is two wakes on one run.
pub fn wait_for_wake_to_finish(id: &str) -> bool {
    for _ in 0..(crate::spawner::CONFIRM_SECONDS * 4) {
        if !LockLiveness.probe(id).is_busy() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    !LockLiveness.probe(id).is_busy()
}

/// How this run's wakes are backgrounded, as configured when the run was created.
pub fn detach_of(id: &str) -> Detach {
    read_run(id).ok().map(|s| s.launcher.detach).unwrap_or(Detach::Tmux)
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Planned {
    pub id: String,
    pub name: String,
    pub first_wake: String,
}

/// Everything `init_run` would decide, and the same refusals, with nothing on disk — so a flag
/// can be checked before it costs a run directory and a wake.
pub fn plan_run(init: &InitArgs) -> Result<Planned, OttoError> {
    let planned = crate::state::commands::plan_run(init)?;
    Ok(Planned {
        id: planned.state.id.clone(),
        name: planned.state.name.clone(),
        first_wake: crate::wake::first_wake_dry_run_line(&planned.state, &planned.path)?,
    })
}

/// Create the run. The first wake is the caller's to start, so the terminal can print the id
/// before a foreground wake takes over the screen.
pub fn create_run(init: InitArgs) -> Result<String, OttoError> {
    // An unknown launcher is refused by `init_run` itself, before any run directory exists.
    crate::state::commands::init_run(init)
}

// ---------------------------------------------------------------------------
// ls
// ---------------------------------------------------------------------------

/// Whatever the run is waiting on, in a few words. This is the column that answers "which of my
/// runs needs me?", so a gate names its question rather than just its id.
pub fn blocking(state: &RunState, wake_running: bool) -> String {
    if let Some(gate) = &state.gate {
        // The wake that opened this gate does not always exit right away — a model that keeps
        // working past "stop there" leaves the lock held. Without this, `otto ls` reads as
        // "waiting on you" when a wake is in fact still alive and could race an answer.
        return if wake_running {
            format!("you: gate {} {} (wake {} still running)", gate.id, gate.slug, state.wake.as_ref().map(|w| w.n).unwrap_or(0))
        } else {
            format!("you: gate {} {}", gate.id, gate.slug)
        };
    }
    if wake_running {
        let n = state.wake.as_ref().map(|w| w.n).unwrap_or(0);
        return format!("working (wake {n})");
    }
    if state.is_paused() && !state.status.is_terminal() {
        return match state.paused.as_ref().and_then(|p| p.until) {
            Some(until) => format!("paused until {}", crate::clock::local_clock(until)),
            None => "paused until resumed".to_string(),
        };
    }
    if state.done_claim.is_some() && !state.status.is_terminal() {
        return "a check that it's done".to_string();
    }
    match state.status {
        // When is its own column (`next_wake`); this one only says what the run waits on.
        Status::Sleeping => match state.next_wake_at {
            Some(_) if state.usage_limited() => "usage limit".to_string(),
            Some(_) => "timer".to_string(),
            None => "nothing — sleeping with no wake time".to_string(),
        },
        Status::Done | Status::Failed | Status::Stopped => "—".to_string(),
        // Not running and nothing pending. The validator turns this into a retry, so seeing it
        // here means a wake is between attempts, or something went wrong outside a wake.
        _ => "nothing scheduled".to_string(),
    }
}

/// When the next wake is due, for a list: relative first (what a person scans for), then the
/// local clock time. A running wake has cleared its timer, so nothing is due until it decides.
pub fn next_wake(state: &RunState, wake_running: bool) -> String {
    match state.next_wake_at {
        Some(at) if !wake_running && !state.status.is_terminal() && !state.is_paused() => {
            format!("{} ({})", crate::clock::due(at), crate::clock::local_clock(at))
        }
        _ => "—".to_string(),
    }
}

/// A run that says it is `done` while its todo still has items to do or under way: work it
/// declared and never accounted for — the gap DESIGN.md §19.1 found, when `dev-flow` went
/// `land → done` and its cleanup never happened. Not a failure, and nothing refuses it; the list and
/// the page say so, so a person can tell. Only `done` claims the work is finished: a stopped or
/// failed run is expected to leave items open.
pub fn todo_left_open(state: &RunState, todo: Option<&crate::todo::Counts>) -> bool {
    state.status == Status::Done && todo.is_some_and(|t| t.left() > 0)
}

/// The status as a person should read it. `blocked` on its own sends them to the logs; the
/// cause says whether that is even the right place to look.
pub fn status_label(state: &RunState) -> String {
    if state.status == Status::Blocked {
        format!("blocked ({})", blocked_record(state).0.cause.label())
    } else {
        crate::state::commands::status_str(state.status).to_string()
    }
}

/// The lines of one handoff section, `## <name>` up to the next heading, trimmed and with blank
/// ones dropped. `None` when the section is missing or empty.
fn handoff_section<'a>(handoff: &'a str, name: &str) -> Option<Vec<&'a str>> {
    // A markdown heading is `#`s then a space; `#123 merged` at the start of a line is not one.
    fn heading(line: &str) -> Option<&str> {
        let line = line.trim_start();
        let rest = line.trim_start_matches('#');
        (rest.len() < line.len() && (rest.is_empty() || rest.starts_with(' '))).then(|| rest.trim())
    }
    let mut lines = handoff.lines().skip_while(|line| !heading(line).is_some_and(|text| text.to_lowercase().starts_with(name)));
    lines.next()?;
    let body: Vec<&str> =
        lines.take_while(|line| heading(line).is_none()).map(str::trim).filter(|line| !line.is_empty()).collect();
    (!body.is_empty()).then_some(body)
}

/// The handoff's `## Where this is` section — written for the person (`harness/wake.md`): one
/// sentence on where the run stands, then a few lines — joined onto one line for a list and a
/// summary that have room for a paragraph, not a layout. `None` when it is missing or empty.
pub fn where_this_is(handoff: &str) -> Option<String> {
    handoff_section(handoff, "where this is").map(|lines| lines.join(" "))
}

/// The handoff's `## Needs`: what the run says it needs from someone — the open question, the
/// blocker, something that would help — with its lines kept, since it is often a list. `None` when
/// it says nothing, which is most of the time, and mostly in those words.
pub fn needs(handoff: &str) -> Option<String> {
    let text = handoff_section(handoff, "needs")?.join("\n");
    let bare = text.trim().trim_end_matches('.').to_lowercase();
    (!matches!(bare.as_str(), "nothing" | "none" | "n/a" | "-" | "—")).then_some(text)
}

/// One line of a run's activity feed: something a wake journaled about its own work.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityLine {
    pub at: crate::clock::Timestamp,
    /// The name the wake gave it: `cycle`, `curated`, `error`, ….
    pub event: String,
    /// Its `message`, then any other fields as `key=value`.
    pub text: String,
}

/// How the run's last finished wake ended — or its last check, when that came later — in a few
/// words, for a person glancing at the run. A run with a check script mostly runs the check, and a
/// check that finds nothing is never journaled, so for "the last time it ran" the check counts too.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    /// When it ended.
    pub at: crate::clock::Timestamp,
    /// `wake` or `check`.
    pub kind: &'static str,
    pub outcome: String,
}

pub fn last_run(dir: &std::path::Path, state: &RunState) -> Option<LastRun> {
    let wake = last_wake(dir);
    let check = state.check.as_ref().and_then(|c| Some((c.last_at?, c.last_result?, c.last_note.clone())));
    match (wake, check) {
        (wake, Some((at, result, note))) if wake.as_ref().is_none_or(|w| at > w.at) => {
            let what = match result {
                crate::state::CheckResult::NoChange => "checked, nothing new",
                crate::state::CheckResult::Changed => "checked, found something",
                crate::state::CheckResult::Error => "the check failed",
            };
            let outcome = match note.filter(|n| !n.trim().is_empty()) {
                Some(note) => format!("{what} — {}", note.trim()),
                None => what.to_string(),
            };
            Some(LastRun { at, kind: "check", outcome })
        }
        (wake, _) => wake,
    }
}

/// The last wake that has finished, read from the journal: the events from its `wake-started` to
/// the line that ended it. What it says it did wins over what otto saw, except when it did not
/// finish at all.
fn last_wake(dir: &std::path::Path) -> Option<LastRun> {
    const ENDS: &[&str] = &["wake-complete", "wake-incomplete", "usage-limited", "wake-killed"];
    let text = std::fs::read_to_string(dir.join("journal.jsonl")).ok()?;
    let lines: Vec<Value> = text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
    let event = |v: &Value| v.get("event").and_then(Value::as_str).unwrap_or_default().to_string();
    let end = lines.iter().rposition(|v| ENDS.contains(&event(v).as_str()))?;
    let start = lines[..end].iter().rposition(|v| event(v) == "wake-started").map_or(0, |i| i + 1);
    let wake = &lines[start..=end];
    let at = crate::clock::Timestamp::parse(lines[end].get("ts")?.as_str()?).ok()?;
    let has = |name: &str| wake.iter().any(|v| event(v) == name);
    let said = wake.iter().rev().find(|v| !crate::event::OTTO_EVENTS.contains(&event(v).as_str()));
    let outcome = if has("usage-limited") {
        "stopped by the usage limit".to_string()
    } else if has("wake-incomplete") || has("wake-killed") {
        "didn't finish".to_string()
    } else if wake.iter().any(|v| event(v) == "status-changed" && v.get("to").and_then(Value::as_str) == Some("done")) {
        "finished the run".to_string()
    } else if let Some(said) = said {
        let message = said.get("message").and_then(Value::as_str).unwrap_or_default();
        if message.is_empty() { event(said) } else { message.to_string() }
    } else if let Some(gate) = wake.iter().rev().find(|v| event(v) == "gate-opened") {
        format!("asked you: {}", gate.get("slug").and_then(Value::as_str).unwrap_or("a question"))
    } else if has("tick") {
        "made progress".to_string()
    } else if has("noop-tick") || lines[end].get("progress").and_then(Value::as_bool) == Some(false) {
        "no changes".to_string()
    } else {
        "finished".to_string()
    };
    Some(LastRun { at, kind: "wake", outcome })
}

/// How much of a run's activity its page shows. The journal has the rest (`otto logs`).
pub const ACTIVITY_SHOWN: usize = 20;

/// What a run's wakes have said they did, newest first, at most `limit` lines: every journal
/// entry otto did not write itself (`event::OTTO_EVENTS`), which is to say everything a wake
/// recorded with `otto state log`. A run that watches or repeats has no todo to tick off, and this
/// is its progress instead (`harness/wake.md`, "Activity").
pub fn activity(dir: &std::path::Path, limit: usize) -> Vec<ActivityLine> {
    let Ok(text) = std::fs::read_to_string(dir.join("journal.jsonl")) else { return Vec::new() };
    text.lines()
        .rev()
        .filter_map(|line| {
            let Value::Object(mut fields) = serde_json::from_str::<Value>(line).ok()? else { return None };
            let event = fields.remove("event")?.as_str()?.to_string();
            if crate::event::OTTO_EVENTS.contains(&event.as_str()) {
                return None;
            }
            let at = crate::clock::Timestamp::parse(fields.remove("ts")?.as_str()?).ok()?;
            let message = fields.remove("message").and_then(|m| m.as_str().map(str::to_string));
            let extra = fields.iter().map(|(k, v)| format!("{k}={}", v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())));
            let text = message.into_iter().chain(extra).collect::<Vec<_>>().join(" ");
            Some(ActivityLine { at, event, text })
        })
        .take(limit)
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LsRow {
    pub id: String,
    pub short: String,
    pub name: String,
    pub status: String,
    pub phase: String,
    pub blocking: String,
    /// Where the run stands, in the last wake's words (`where_this_is`). The web list shows it
    /// under each run; `otto ls` stays one line per run.
    pub where_this_is: Option<String>,
    /// How far along its todo is, when it keeps one (`crate::todo`).
    pub todo: Option<crate::todo::Counts>,
    /// See `todo_left_open`.
    pub todo_left_open: bool,
    /// The newest line of its activity feed (`activity`).
    pub last_activity: Option<ActivityLine>,
    pub wakes: String,
    /// `1h`, `30m`; `—` for a finished run, which will never wake again.
    pub period: String,
    /// For the web page, which renders it relative to the viewer's clock on each refresh.
    pub next_wake_at: Option<crate::clock::Timestamp>,
    /// For `otto ls`: `in 43m (14:05)`, or `—` with nothing scheduled.
    pub next_wake: String,
    pub running: bool,
    pub terminal: bool,
    /// The run in one line, as a person reads it (`story::glance`).
    pub mood: crate::story::Mood,
    pub line: String,
    /// Messages it has sent that are still in the inbox.
    pub messages: usize,
    /// When it last ran or checked (`last_run`).
    pub last_run_at: Option<crate::clock::Timestamp>,
}

/// A gate a person can act on right now.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeedsYou {
    pub id: String,
    pub short: String,
    pub name: String,
    pub label: String,
    /// The default leads, so the first one shown is the one the wake recommended.
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LsView {
    pub rows: Vec<LsRow>,
    pub needs_you: Vec<NeedsYou>,
    /// How many runs are sleeping on a timer while the reviver is known not to be loaded — the
    /// runs that will silently never come back.
    pub stranded_sleepers: Option<usize>,
}

pub fn list_runs(all: bool) -> Result<LsView, OttoError> {
    let liveness = LockLiveness;
    let all_ids = crate::paths::all_run_ids();
    let mut rows: Vec<LsRow> = Vec::new();
    let mut needs_you: Vec<NeedsYou> = Vec::new();
    let mut sleeping_count = 0usize;
    for entry in read_all_runs()? {
        let state = match entry {
            RunEntry::Readable(state) => state,
            RunEntry::Unreadable { id } => {
                if all {
                    let short = short_id_among(&id, &all_ids);
                    rows.push(LsRow {
                        name: crate::paths::slug_of(&id).to_string(),
                        id,
                        short,
                        status: "unreadable".to_string(),
                        phase: "?".to_string(),
                        blocking: "a person needs to look".to_string(),
                        where_this_is: None,
                        todo: None,
                        todo_left_open: false,
                        last_activity: None,
                        wakes: "?".to_string(),
                        period: "?".to_string(),
                        next_wake_at: None,
                        next_wake: "?".to_string(),
                        running: false,
                        terminal: false,
                        mood: crate::story::Mood::Trouble,
                        line: "Its run.json can't be read — a person needs to look".to_string(),
                        messages: 0,
                        last_run_at: None,
                    });
                }
                continue;
            }
        };
        if state.status.is_terminal() && !all {
            continue;
        }
        let running = liveness.probe(&state.id).is_busy();
        let dir = crate::paths::run_dir(&state.id).ok();
        let mut question_title = None;
        if let Some(gate) = &state.gate {
            let question = dir.as_ref().and_then(|d| crate::gate::read_question(d, gate).ok()).unwrap_or_default();
            question_title = Some(question_title_of(&question, &gate.slug));
            needs_you.push(NeedsYou {
                id: state.id.clone(),
                short: short_id_among(&state.id, &all_ids),
                name: state.display_name(),
                label: format!("gate {} {}", gate.id, gate.slug),
                options: crate::gate::options_default_first(&question),
            });
        } else if !running && state.status == Status::Sleeping {
            sleeping_count += 1;
        }
        // Wakes, not dollars. There is no per-wake cost to show since otto stopped passing `-p`
        // (see `wake::launcher`), and a `$0.00` column that can never change is worse than no
        // column — it reads as a run that has cost nothing. Wakes against their budget is the
        // number that still means something at a glance.
        let spent = state.budget.spent_wakes;
        let limit = state.budget.wakes;
        let wakes = if limit > 0 { format!("{spent}/{limit}") } else { spent.to_string() };
        let todo = dir.as_deref().and_then(crate::todo::read).map(|t| t.counts);
        let said = dir
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join(crate::state::commands::HANDOFF_FILE)).ok())
            .and_then(|handoff| where_this_is(&handoff));
        let quiet = running.then(|| quiet_minutes(&state)).flatten();
        let glance = crate::story::glance(&state, running, quiet, question_title.as_deref(), said.as_deref());
        rows.push(LsRow {
            todo_left_open: todo_left_open(&state, todo.as_ref()),
            todo,
            last_activity: dir.as_deref().and_then(|d| activity(d, 1).into_iter().next()),
            id: state.id.clone(),
            short: short_id_among(&state.id, &all_ids),
            name: state.display_name(),
            status: status_label(&state),
            phase: state.phase.clone(),
            blocking: blocking(&state, running),
            where_this_is: said,
            mood: glance.mood,
            line: glance.line,
            messages: state.messages.len(),
            last_run_at: dir.as_deref().and_then(|d| last_run(d, &state)).map(|l| l.at),
            wakes,
            period: if state.status.is_terminal() {
                "—".to_string()
            } else {
                crate::clock::format_minutes(state.policy.period_minutes)
            },
            next_wake_at: state.next_wake_at,
            next_wake: next_wake(&state, running),
            running,
            terminal: state.status.is_terminal(),
        });
    }
    let stranded_sleepers = if sleeping_count > 0 && crate::launchd::is_loaded() == Some(false) {
        Some(sleeping_count)
    } else {
        None
    };
    Ok(LsView { rows, needs_you, stranded_sleepers })
}

// ---------------------------------------------------------------------------
// show
// ---------------------------------------------------------------------------

/// The run a command means when none is named. With one gate open anywhere, it is that run —
/// `otto ls` said "needs you", and this is the reply. Otherwise, only when `or_only_live` and
/// exactly one run is live, that one. Anything else is a question back, naming what to type.
pub fn implied_run(verb: &str, or_only_live: bool) -> Result<String, OttoError> {
    let mut waiting: Vec<String> = Vec::new();
    let mut live: Vec<String> = Vec::new();
    for entry in read_all_runs()? {
        let RunEntry::Readable(state) = entry else { continue };
        if state.status.is_terminal() {
            continue;
        }
        if state.gate.is_some() {
            waiting.push(state.id.clone());
        }
        live.push(state.id);
    }
    let ids = crate::paths::all_run_ids();
    let name_each = |runs: &[String]| -> String {
        runs.iter().map(|id| format!("`otto {verb} {}`", short_id_among(id, &ids))).collect::<Vec<_>>().join(", ")
    };
    match (waiting.as_slice(), live.as_slice()) {
        ([only], _) => Ok(only.clone()),
        ([], [only]) if or_only_live => Ok(only.clone()),
        ([], []) => Err(OttoError::conflict("no run is live — `otto ls --all` to see finished ones")),
        ([], _) => Err(OttoError::conflict(format!("no run is waiting on you — say which: {}", name_each(&live)))),
        (several, _) => Err(OttoError::usage(format!(
            "{} runs are waiting on you — say which: {}",
            several.len(),
            name_each(several)
        ))),
    }
}

/// An open gate, as a person answers it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateView {
    pub id: String,
    pub slug: String,
    pub file: String,
    /// The whole gate file; `None` when it is missing.
    pub text: Option<String>,
    /// Just the question, for a caller that shows it without the file's header.
    pub question: String,
    /// In the order the question lists them.
    pub options: Vec<String>,
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub state: RunState,
    pub short: String,
    /// `state.name`, or the old id's slug for a run from before names.
    pub name: String,
    pub status_label: String,
    pub wraps_kind: &'static str,
    pub running: bool,
    pub blocked_explanation: Option<String>,
    pub gate: Option<GateView>,
    /// What the last wake left for the next one, whole. The page keeps it a click away.
    pub handoff: Option<String>,
    /// How the last wake or check ended (`last_run`).
    pub last_run: Option<LastRun>,
    /// The handoff's `Needs`, when it says something (`needs`).
    pub needs: Option<String>,
    /// What the run is waiting on, in a few words — the list's column (`blocking`).
    pub blocking: String,
    /// The run's checklist, when it keeps one (`crate::todo`).
    pub todo: Option<crate::todo::Todo>,
    /// See `todo_left_open`.
    pub todo_left_open: bool,
    /// What its wakes have said they did, newest first (`activity`).
    pub activity: Vec<ActivityLine>,
    /// Standing notes, and one-off notes not yet delivered.
    pub notes: Vec<NoteView>,
    /// `None` means every wake is a full model session — worth saying, since a check is the
    /// cheapest wake there is.
    pub check: Option<CheckView>,
    pub wake_cost: Option<WakeCost>,
    /// Where this run's wakes start — only when it's worth saying: a directory other than the
    /// configured default, or no directory at all. `None` means "the default", which goes unsaid.
    pub workdir: Option<WorkdirView>,
    /// Everything this run's wakes have used.
    pub usage: TokenTotals,
    /// The run in one line, as the list shows it (`story::glance`).
    pub glance: crate::story::Glance,
    /// The open question in one line, when there is one.
    pub question_title: Option<String>,
    /// What it has sent, newest first, open or done.
    pub messages: Vec<MessageLine>,
    /// What it has made, kept: its stages and its outputs, newest change first (`outputs::view`).
    pub outputs: crate::outputs::OutputsView,
    /// Its last week, for a picture.
    pub timeline: crate::story::Timeline,
    /// What has happened lately, in words, newest first.
    pub lately: Vec<crate::story::Happening>,
}

/// One message, as a list of them shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageLine {
    #[serde(flatten)]
    pub meta: crate::messages::MessageMeta,
    pub open: bool,
}

/// How many messages a run's page lists.
pub const MESSAGES_SHOWN: usize = 20;
/// How many things a run's page tells in `lately`.
pub const LATELY_SHOWN: usize = 30;

/// A question's title for a list: its own heading or first sentence, else its slug in words.
pub fn question_title_of(question: &str, slug: &str) -> String {
    let title = crate::gate::title(question, crate::story::QUESTION_TITLE_CHARS);
    if title.is_empty() {
        slug.replace('-', " ")
    } else {
        title
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum WorkdirView {
    /// The run's own directory, which isn't the default.
    Own { path: String },
    /// Nothing recorded and no default: a wake starts wherever whatever started it happens to be.
    Unset,
}

/// What `otto show` and the run page say about a run's working directory: nothing when it is the
/// default, since that is what a person expects without being told.
pub fn workdir_view(state: &RunState) -> Option<WorkdirView> {
    let default = crate::config::default_workdir().ok().flatten();
    match (&state.launcher.workdir, default) {
        (Some(own), Some(default)) if std::path::Path::new(own) == default => None,
        (Some(own), _) => Some(WorkdirView::Own { path: own.clone() }),
        (None, Some(_)) => None,
        (None, None) => Some(WorkdirView::Unset),
    }
}

/// Everything needed to answer a gate cold, in one screen: where the run stands, what it last
/// did, and the question in full. Someone arriving eight hours later has no transcript, because
/// the session that asked is gone.
pub fn run_detail(id: Option<&str>) -> Result<RunDetail, OttoError> {
    let id = match id {
        Some(id) => crate::paths::resolve_run_id(id)?,
        None => implied_run("show", true)?,
    };
    let state = read_run(&id)?;
    let dir = crate::paths::run_dir(&id)?;
    let running = LockLiveness.probe(&id).is_busy();
    let short = short_id(&id);
    let blocked_explanation = (state.status == Status::Blocked).then(|| blocked_explanation(&state, &short));
    let gate = state.gate.as_ref().map(|gate| {
        let question = crate::gate::read_question(&dir, gate).unwrap_or_default();
        let options = crate::gate::parse_options(&question);
        let default = crate::gate::parse_default(&question, &options);
        GateView {
            id: gate.id.clone(),
            slug: gate.slug.clone(),
            file: gate.file.clone(),
            text: std::fs::read_to_string(dir.join(&gate.file)).ok(),
            question,
            options,
            default,
        }
    });
    let handoff = std::fs::read_to_string(dir.join(crate::state::commands::HANDOFF_FILE)).ok();
    let notes = note_views(&dir, &state);
    let check = check_view(&dir, &state);
    let wake_cost = wake_cost(&dir);
    let todo = crate::todo::read(&dir);
    let question_title = gate.as_ref().map(|g| question_title_of(&g.question, &g.slug));
    let glance =
        crate::story::glance(
            &state,
            running,
            running.then(|| quiet_minutes(&state)).flatten(),
            question_title.as_deref(),
            handoff.as_deref().and_then(where_this_is).as_deref(),
        );
    let messages = crate::messages::history(&dir)
        .into_iter()
        .take(MESSAGES_SHOWN)
        .map(|meta| MessageLine { open: state.messages.iter().any(|m| m.id == meta.id), meta })
        .collect();
    Ok(RunDetail {
        glance,
        question_title,
        messages,
        timeline: crate::story::timeline(&dir, &state),
        lately: crate::story::lately(&dir, LATELY_SHOWN),
        outputs: crate::outputs::view(&dir, &state),
        todo_left_open: todo_left_open(&state, todo.as_ref().map(|t| &t.counts)),
        todo,
        activity: activity(&dir, ACTIVITY_SHOWN),
        needs: handoff.as_deref().and_then(needs),
        last_run: last_run(&dir, &state),
        blocking: blocking(&state, running),
        workdir: workdir_view(&state),
        usage: run_usage(&dir),
        short,
        name: state.display_name(),
        notes,
        check,
        wake_cost,
        status_label: status_label(&state),
        wraps_kind: crate::state::commands::kind_str(state.wraps.kind),
        running,
        blocked_explanation,
        gate,
        handoff,
        state,
    })
}

/// Why the run is blocked and what to do about it, in one line, matched to the cause. This used
/// to say "blocked after repeated wake failures — see the logs, then retry the wake" for every
/// blocked run, and was seen saying it to a run whose wakes had all completed: it had stalled,
/// nothing had failed, and retrying was the one thing that could not help.
pub fn blocked_explanation(state: &RunState, short: &str) -> String {
    let gate = match &state.gate {
        Some(gate) => format!("answer gate {} below", gate.id),
        // The contract forbids this state; if it is seen anyway, say what to do rather than nothing.
        None => format!("no gate is open, so nothing can unblock it — `otto wake {short} --watch` or `otto stop {short}`"),
    };
    let (blocked, inferred) = blocked_record(state);
    let detail = blocked.detail.as_deref();
    let line = match blocked.cause {
        BlockedCause::WakeFailures => format!(
            "blocked: {} wake(s) in a row did not finish{} — `otto logs {short}` for what happened, \
             `otto wake {short} --watch` to retry one in this terminal, or {gate}",
            state.incomplete_wakes,
            detail.map(|d| format!(" (last: {d})")).unwrap_or_default()
        ),
        BlockedCause::Budget => format!(
            "blocked: {} — nothing is wrong with the work; {gate}",
            detail.unwrap_or("a budget ceiling was reached")
        ),
        BlockedCause::Stall => format!(
            "blocked: {} wakes in a row changed nothing (policy maxTicksWithoutProgress = {}) — nothing \
             failed, the run is asking whether to keep going; {gate}",
            state.ticks_without_progress, state.policy.max_ticks_without_progress
        ),
        BlockedCause::Instructions => format!(
            "blocked by its instructions{} — {gate}",
            detail.map(|d| format!(": {d}")).unwrap_or_default()
        ),
        BlockedCause::Unverified => format!(
            "blocked: it kept saying it was done, and each fresh look found it wasn't{} — {gate}",
            detail.map(|d| format!(" (last: {d})")).unwrap_or_default()
        ),
    };
    if inferred {
        format!("{line}\n(cause inferred from the run's counters: it was blocked before otto recorded reasons)")
    } else {
        line
    }
}

/// The blocked record, or — for a run blocked before otto kept one — the same inference
/// `set-status` makes today, flagged as such. The `bool` is "inferred".
pub fn blocked_record(state: &RunState) -> (Blocked, bool) {
    match &state.blocked {
        Some(blocked) => (blocked.clone(), false),
        None => (Blocked { cause: state.inferred_block_cause(), detail: None, at: state.updated_at }, true),
    }
}

// ---------------------------------------------------------------------------
// answer
// ---------------------------------------------------------------------------

/// The gate an answer is for, read once so that what is validated, prompted for and recorded
/// all come from the same question.
#[derive(Debug, Clone)]
pub struct PendingGate {
    pub id: String,
    pub short: String,
    pub gate_id: String,
    pub slug: String,
    pub question: String,
    pub options: Vec<String>,
}

pub fn pending_gate(id: Option<&str>) -> Result<PendingGate, OttoError> {
    let id = match id {
        Some(id) => crate::paths::resolve_run_id(id)?,
        None => implied_run("answer", false)?,
    };
    let short = short_id(&id);
    let state = read_run(&id)?;
    let gate = state.gate.as_ref().ok_or_else(|| {
        OttoError::conflict(format!("{id} has no gate open — nothing is being asked. `otto show {short}` for where it stands"))
    })?;
    let dir = crate::paths::run_dir(&id)?;
    let question = crate::gate::read_question(&dir, gate).unwrap_or_default();
    let options = crate::gate::parse_options(&question);
    Ok(PendingGate { gate_id: gate.id.clone(), slug: gate.slug.clone(), id, short, question, options })
}

/// Match `choice` against the gate's own options, case-insensitively, and record the gate's
/// casing rather than the user's — so the journal always shows the option exactly as named in
/// the question, whichever case someone typed. An empty `options` (a question with no parseable
/// list) skips validation entirely: most gates are free-form prose from an arbitrary wrapped
/// skill, and refusing those would break far more than it catches.
pub fn resolve_choice(choice: &str, options: &[String]) -> Result<String, OttoError> {
    if options.is_empty() {
        return Ok(choice.to_string());
    }
    match options.iter().find(|o| o.eq_ignore_ascii_case(choice.trim())) {
        Some(matched) => Ok(matched.clone()),
        None => Err(OttoError::usage(format!(
            "\"{choice}\" is not one of this gate's options: {}",
            options.join(", ")
        ))),
    }
}

/// Record `answer` against the gate. `ops::close_gate` records it verbatim and returns
/// `running`, so the wake that follows sees an answered gate. It takes the run lock inside its
/// own transaction and releases it on return, so a wake started afterwards is sequential rather
/// than nested — `RunLock` is not reentrant.
pub fn record_answer(pending: &PendingGate, answer: &str) -> Result<(), OttoError> {
    crate::state::ops::close_gate(&pending.id, Some(answer), Status::Running)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerOutcome {
    pub id: String,
    pub gate_id: String,
    pub slug: String,
    pub answer: String,
    pub wake: Option<WakeStart>,
    /// Why no wake was started, when none was.
    pub note: Option<String>,
}

/// Record an answer and continue the run in the background — the whole of `otto answer` for a
/// caller with no terminal. `choice` is checked against the gate's options; `text` is not.
pub fn answer_in_background(id: &str, choice: Option<&str>, text: Option<&str>, no_wake: bool) -> Result<AnswerOutcome, OttoError> {
    let pending = pending_gate(Some(id))?;
    let answer = match (choice, text) {
        (Some(choice), _) => resolve_choice(choice, &pending.options)?,
        (None, Some(text)) if !text.trim().is_empty() => text.to_string(),
        _ => return Err(OttoError::usage("give the answer: a choice or some text")),
    };
    record_answer(&pending, &answer)?;
    let mut outcome = AnswerOutcome {
        id: pending.id.clone(),
        gate_id: pending.gate_id.clone(),
        slug: pending.slug.clone(),
        answer: answer.clone(),
        wake: None,
        note: None,
    };
    if no_wake {
        outcome.note = Some("recorded; not waking it yet".to_string());
        return Ok(outcome);
    }
    if !wait_for_wake_to_finish(&pending.id) {
        outcome.note = Some(format!(
            "it's still busy after {}s — your answer is recorded, and it will act on it",
            crate::spawner::CONFIRM_SECONDS
        ));
        return Ok(outcome);
    }
    // Hand the answer to the wake as well as recording it: the wake must see the person's own
    // words, not a summary of them.
    let strategy = Caller::Background.strategy(detach_of(&pending.id));
    outcome.wake = Some(start_wake(&pending.id, strategy, Some(&answer))?);
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// check scripts
// ---------------------------------------------------------------------------

/// Where `otto check` keeps a person's script: the run directory's top level, beside `run.json`,
/// rather than `artifacts/`, which is the wake's.
pub const CHECK_FILE: &str = "check.sh";
/// Enough to read the script on the page; a check that long is doing too much anyway.
const CHECK_SHOW_MAX_BYTES: usize = 4096;
/// How many recent wakes the cost summary averages over.
const WAKE_COST_WINDOW: usize = 10;

/// A run's check script as a person reads it: what it runs, how often, and what it has saved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckView {
    pub script: String,
    /// The script itself; `None` when the file is missing.
    pub text: Option<String>,
    /// How long it sleeps again after "nothing new"; `None` is the run's period.
    pub retry_seconds: Option<i64>,
    /// Wake anyway after this many "nothing new" results in a row; 0 is off.
    pub wake_after: u32,
    pub last_result: Option<crate::state::CheckResult>,
    pub last_at: Option<crate::clock::Timestamp>,
    pub last_note: Option<String>,
    pub consecutive_no_change: u32,
    /// No-change results since it was set: each one a wake that did not happen.
    pub no_change_total: u64,
    /// Standing — it stays with the run — rather than armed by a wake for one sleep.
    pub pinned: bool,
    /// Set by a wake rather than a person.
    pub set_by_wake: bool,
}

/// What recent wakes have used, averaged. The honest price of a wake on a subscription is tokens,
/// and this is what a check script saves each time it finds nothing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeCost {
    pub wakes: usize,
    pub avg_turns: u64,
    pub avg_output_tokens: u64,
    pub avg_cache_creation: u64,
    pub avg_cache_read: u64,
}

pub fn check_view(dir: &std::path::Path, state: &RunState) -> Option<CheckView> {
    let check = state.check.as_ref()?;
    let text = std::fs::read_to_string(dir.join(&check.script)).ok().map(|t| {
        if t.len() > CHECK_SHOW_MAX_BYTES {
            let mut cut = CHECK_SHOW_MAX_BYTES;
            while !t.is_char_boundary(cut) {
                cut -= 1;
            }
            format!("{}\n… ({} bytes; see {})", &t[..cut], t.len(), check.script)
        } else {
            t
        }
    });
    Some(CheckView {
        script: check.script.clone(),
        text,
        retry_seconds: check.retry_seconds,
        wake_after: check.wake_after,
        last_result: check.last_result,
        last_at: check.last_at,
        last_note: check.last_note.clone(),
        consecutive_no_change: check.consecutive_no_change,
        no_change_total: check.no_change_total,
        pinned: check.pinned,
        set_by_wake: check.set_by_wake,
    })
}

/// Averages over the last few `wake-spent` lines, or `None` before the first wake.
pub fn wake_cost(dir: &std::path::Path) -> Option<WakeCost> {
    let journal = std::fs::read_to_string(dir.join("journal.jsonl")).ok()?;
    let spent: Vec<Value> = journal
        .lines()
        .filter(|l| l.contains("\"wake-spent\""))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();
    let recent = &spent[spent.len().saturating_sub(WAKE_COST_WINDOW)..];
    if recent.is_empty() {
        return None;
    }
    let avg = |key: &str| recent.iter().map(|v| v[key].as_u64().unwrap_or(0)).sum::<u64>() / recent.len() as u64;
    Some(WakeCost {
        wakes: recent.len(),
        avg_turns: avg("turns"),
        avg_output_tokens: avg("outputTokens"),
        avg_cache_creation: avg("cacheCreation"),
        avg_cache_read: avg("cacheRead"),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckSetOutcome {
    pub id: String,
    pub short: String,
    pub check: CheckView,
    pub period_minutes: u64,
    pub next_wake_at: Option<crate::clock::Timestamp>,
}

/// Give a run a check script of its own (`otto check --script`). From here on, whenever the run's
/// period comes due, poke runs the script instead of waking the run: "nothing new" sleeps it for
/// another period, anything else wakes it, and `wake_after` no-change results in a row wake it
/// anyway (0: never) — the safety net under a script that is wrong without failing.
pub fn set_check(id: &str, script: &str, wake_after: u32) -> Result<CheckSetOutcome, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    if !script.starts_with("#!") {
        return Err(OttoError::usage(
            "the check script must start with a #! line (e.g. #!/bin/sh) — poke runs it directly",
        ));
    }
    let dir = crate::paths::run_dir(&id)?;
    crate::state::transaction(&id, |path, state| {
        if state.status.is_terminal() {
            return Err(OttoError::conflict(format!(
                "{id} is {} — nothing wakes it to check for",
                crate::state::commands::status_str(state.status)
            )));
        }
        let file = path.join(CHECK_FILE);
        crate::state::write_atomic(&file, script)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))?;
        state.check = Some(crate::state::Check {
            script: CHECK_FILE.to_string(),
            retry_seconds: None,
            wake_after,
            consecutive_no_change: 0,
            last_result: None,
            last_at: None,
            last_note: None,
            no_change_total: 0,
            pinned: true,
            set_by_wake: false,
        });
        crate::event::record(
            path,
            &crate::event::Event::CheckSet { script: CHECK_FILE.to_string(), wake_after, by: "person".to_string() },
        )
    })?;
    let state = read_run(&id)?;
    Ok(CheckSetOutcome {
        short: short_id(&id),
        check: check_view(&dir, &state).expect("just set"),
        period_minutes: state.policy.period_minutes,
        next_wake_at: state.next_wake_at,
        id,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodSetOutcome {
    pub id: String,
    pub short: String,
    pub period_minutes: u64,
    pub previous_minutes: u64,
    pub next_wake_at: Option<crate::clock::Timestamp>,
    /// Whether the sleep already scheduled was moved onto the new period. False when the run is
    /// not sleeping, or is sleeping on a timer a wake armed for its own reasons.
    pub rescheduled: bool,
}

/// Change how often a run wakes (`otto period`). The new period applies to every sleep from here
/// on, and to the one already scheduled too when that sleep is the period's own — re-anchored on
/// when that sleep began, sooner or later. A timer a wake armed itself (`arm-timer --in 600`
/// because CI takes ten minutes) is left alone: that wake knew something the period does not.
/// A re-anchored wake whose time has already passed is due now, and the next poke starts it.
pub fn set_period(id: &str, minutes: u64) -> Result<PeriodSetOutcome, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    if minutes == 0 {
        return Err(OttoError::usage("the period must be positive"));
    }
    let mut previous = 0;
    let mut rescheduled = false;
    crate::state::transaction(&id, |path, state| {
        if state.status.is_terminal() {
            return Err(OttoError::conflict(format!(
                "{id} is {} — it never wakes again, so it has no period to change",
                crate::state::commands::status_str(state.status)
            )));
        }
        previous = state.policy.period_minutes;
        state.policy.period_minutes = minutes;
        if state.status == Status::Sleeping && !state.wake_armed_timer() {
            if let Some(next) = state.next_wake_at {
                // The period's sleep began one old period before it ends — at the last wake's
                // start, or at the check that last put it off.
                let began = next.dt() - time::Duration::minutes(previous as i64);
                let now = crate::clock::Timestamp::now();
                let anchored = crate::clock::Timestamp::at(began + time::Duration::minutes(minutes as i64));
                state.next_wake_at = Some(if anchored.dt() > now.dt() { anchored } else { now });
                rescheduled = true;
            }
        }
        crate::event::record(
            path,
            &crate::event::Event::PeriodSet { period_minutes: minutes, previous_minutes: previous },
        )
    })?;
    let state = read_run(&id)?;
    Ok(PeriodSetOutcome {
        short: short_id(&id),
        period_minutes: minutes,
        previous_minutes: previous,
        next_wake_at: state.next_wake_at,
        rescheduled,
        id,
    })
}

// ---------------------------------------------------------------------------
// rename / renumber
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Renamed {
    pub id: String,
    pub from: String,
    pub to: String,
}

/// Give a run a new name. Nothing on disk is keyed by the name, so this is one field in
/// `run.json` — the reason the id and the name are two things.
pub fn rename_run(id: &str, new_name: &str) -> Result<Renamed, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let to = crate::paths::valid_name(new_name)?;
    if let Some(other) = crate::paths::run_named(&to, Some(&id)) {
        return Err(OttoError::conflict(format!("{} already has that name", crate::paths::label(&other, &to))));
    }
    let mut from = String::new();
    crate::state::transaction(&id, |path, state| {
        from = state.display_name();
        if from == to {
            return Ok(());
        }
        state.name = to.clone();
        crate::event::record(path, &crate::event::Event::Renamed { from: from.clone(), to: to.clone() })
    })?;
    Ok(Renamed { id, from, to })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Renumbered {
    pub from: String,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenumberReport {
    pub renumbered: Vec<Renumbered>,
    /// Runs left as they were, and why — a wake running in one, most likely.
    pub skipped: Vec<(String, String)>,
}

/// Give every run from before numbering a run number, oldest first, and a short name if it has
/// none. The run directory is renamed; the old id is kept as `formerId`, which still resolves.
///
/// Holds the poke lock for the whole pass, so the reviver can't start a wake in a directory that
/// is about to move, and each run's wake lock across its own move, so nothing else can either. A
/// run with a wake in flight is skipped, not waited for — run it again once the wake is over.
pub fn renumber_runs() -> Result<RenumberReport, OttoError> {
    let lock_path = crate::paths::poke_lock_path()?;
    let lock_file = std::fs::OpenOptions::new().create(true).append(true).open(&lock_path)?;
    if fs4::fs_std::FileExt::try_lock_exclusive(&lock_file).is_err() {
        return Err(OttoError::conflict("a poke pass is running — try again in a moment"));
    }
    let mut legacy: Vec<RunState> = read_all_runs()?
        .into_iter()
        .filter_map(|entry| match entry {
            RunEntry::Readable(state) if state.id.parse::<u64>().is_err() => Some(state),
            _ => None,
        })
        .collect();
    legacy.sort_by_key(|state| state.created_at);

    let mut report = RenumberReport::default();
    for state in legacy {
        let from = state.id.clone();
        let Ok(wake_lock) = crate::liveness::WakeLock::acquire(&from) else {
            report.skipped.push((from, "a wake is running".to_string()));
            continue;
        };
        let (id, claimed) = crate::paths::claim_next_run_id()?;
        // Onto the empty directory just claimed: `rename` replaces an empty directory, and the
        // claim is what keeps a run started meanwhile from taking the same number.
        if let Err(e) = std::fs::rename(crate::paths::runs_dir().join(&from), &claimed) {
            let _ = std::fs::remove_dir(&claimed);
            return Err(e.into());
        }
        drop(wake_lock);
        let name = if state.name.is_empty() {
            let slug = crate::paths::slug_of(&from);
            crate::paths::unique_name(&crate::paths::name_from(slug).unwrap_or_else(|| "run".to_string()))
        } else {
            state.name.clone()
        };
        crate::state::transaction(&id, |path, st| {
            st.id = id.clone();
            st.name = name.clone();
            st.former_id = Some(from.clone());
            crate::event::record(path, &crate::event::Event::Renumbered { from: from.clone() })
        })?;
        crate::state::locks::repoint(&from, &id);
        report.renumbered.push(Renumbered { from, id, name });
    }
    Ok(report)
}

/// Run a run's check script once, now, exactly as poke would, and report what it said — without
/// recording it or acting on it. `otto check` does this when a script is set, so a broken one
/// shows at once rather than when the run is next due.
pub fn try_check(id: &str) -> Result<crate::poke::CheckRun, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let state = read_run(&id)?;
    let check = state.check.as_ref().ok_or_else(|| OttoError::conflict(format!("{id} has no check script")))?;
    let script = crate::paths::run_dir(&id)?.join(&check.script);
    let script = script.to_str().ok_or_else(|| OttoError::usage("check script path is not valid UTF-8"))?.to_string();
    Ok(crate::poke::execute_check(&id, &state, &script, &mut crate::exec::RealExec))
}

/// Take a run's check script away. Its file stays, like a dropped note's.
pub fn clear_check(id: &str) -> Result<String, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    crate::state::transaction(&id, |path, state| {
        let Some(check) = state.check.take() else {
            return Err(OttoError::conflict(format!("{id} has no check script")));
        };
        crate::event::record(
            path,
            &crate::event::Event::CheckCleared { script: check.script.clone(), by: "person".to_string() },
        )?;
        Ok(check.script)
    })
}

// ---------------------------------------------------------------------------
// notes
// ---------------------------------------------------------------------------

/// A note as a person reads it back: its text, and whether a wake has seen it yet.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteView {
    pub id: String,
    pub standing: bool,
    pub added_at: crate::clock::Timestamp,
    pub text: String,
    /// The wake whose prompt last carried it. For a one-off note still listed, that wake has not
    /// completed — it is running now, or it failed and the note will be given again.
    pub given_to_wake: Option<u32>,
}

pub fn note_views(dir: &std::path::Path, state: &RunState) -> Vec<NoteView> {
    state
        .notes
        .iter()
        .map(|note| NoteView {
            id: note.id.clone(),
            standing: note.standing,
            added_at: note.added_at,
            text: crate::notes::read_text(dir, note),
            given_to_wake: note.given_to_wake,
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteOutcome {
    pub id: String,
    pub short: String,
    pub note: NoteView,
    /// When a wake will read it, in words.
    pub delivery: String,
    /// The same for the web page, in its words (`note_delivery_plain`).
    pub delivery_plain: String,
    pub wake: Option<WakeStart>,
    /// Why `now` did not start a wake, when it was asked to and didn't.
    pub not_woken: Option<String>,
}

/// Why a wake cannot be started for a note right now, or `None` when one can.
pub fn why_not_wake_for_note(state: &RunState, running: bool) -> Option<String> {
    if running {
        return Some("a wake is already running".to_string());
    }
    if let Some(gate) = &state.gate {
        return Some(format!("gate {} is open — answer it and the wake that follows reads the note", gate.id));
    }
    None
}

/// When the wakes will see a note just added, said plainly — a note is easy to mistake for an
/// answer, and one sitting unread behind an open gate should not be a surprise.
pub fn note_delivery(state: &RunState, running: bool, standing: bool) -> String {
    let next = if running {
        "the wake running now won't see it; the next one will".to_string()
    } else if let Some(gate) = &state.gate {
        format!("gate {} is open, so it reaches the wake after you answer it — a note is not an answer", gate.id)
    } else {
        match state.next_wake_at {
            Some(at) if at.is_past() => "the next wake reads it, at the next poke".to_string(),
            Some(at) => format!("the next wake reads it, {}", relative(at)),
            None => "the next wake reads it".to_string(),
        }
    };
    if standing {
        format!("standing, until you drop it; {next}")
    } else {
        next
    }
}

/// `note_delivery` for the web page, where a wake is "a run" and a gate "its question" — the
/// terminal keeps otto's words, which its other commands use.
pub fn note_delivery_plain(state: &RunState, running: bool, standing: bool) -> String {
    let next = if running {
        "it's busy now, so the next run reads it".to_string()
    } else if state.gate.is_some() {
        "it reads it after you answer its question — a note isn't an answer".to_string()
    } else {
        match state.next_wake_at {
            Some(at) if at.is_past() => "it reads it the next time it runs, within a few minutes".to_string(),
            Some(at) => format!("it reads it the next time it runs, {}", relative(at)),
            None => "it reads it the next time it runs".to_string(),
        }
    };
    if standing {
        format!("pinned until you unpin it; {next}")
    } else {
        next
    }
}

/// Record a note, and say when it will be read. Waking for it is the caller's business, because
/// a terminal and the web page start wakes differently.
pub fn add_note(id: &str, text: &str, standing: bool) -> Result<NoteOutcome, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let note = crate::notes::add(&id, text, standing)?;
    let state = read_run(&id)?;
    let dir = crate::paths::run_dir(&id)?;
    let running = LockLiveness.probe(&id).is_busy();
    Ok(NoteOutcome {
        short: short_id(&id),
        delivery: note_delivery(&state, running, standing),
        delivery_plain: note_delivery_plain(&state, running, standing),
        note: NoteView {
            text: crate::notes::read_text(&dir, &note),
            id: note.id,
            standing,
            added_at: note.added_at,
            given_to_wake: None,
        },
        id,
        wake: None,
        not_woken: None,
    })
}

/// A note's text, opening with the output it is about when it is about one (`outputs::note_text`).
pub fn note_about(id: &str, about: Option<&str>, text: &str) -> Result<String, OttoError> {
    match about.map(str::trim).filter(|a| !a.is_empty()) {
        Some(about) => {
            let id = crate::paths::resolve_run_id(id)?;
            crate::outputs::note_text(&crate::paths::run_dir(&id)?, about, text)
        }
        None => Ok(text.to_string()),
    }
}

/// `add_note`, then — when `now` — a backgrounded wake to read it.
pub fn note_in_background(id: &str, text: &str, standing: bool, now: bool) -> Result<NoteOutcome, OttoError> {
    let mut outcome = add_note(id, text, standing)?;
    if !now {
        return Ok(outcome);
    }
    let state = read_run(&outcome.id)?;
    if let Some(why) = why_not_wake_for_note(&state, LockLiveness.probe(&outcome.id).is_busy()) {
        outcome.not_woken = Some(why);
        return Ok(outcome);
    }
    match start_wake(&outcome.id, Caller::Background.strategy(state.launcher.detach), None) {
        Ok(wake) => outcome.wake = Some(wake),
        // The note stands either way; it is on disk for whichever wake comes next.
        Err(err) => outcome.not_woken = Some(format!("the wake did not start: {}", err.message)),
    }
    Ok(outcome)
}

pub fn drop_note(id: &str, note: &str) -> Result<NoteView, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let dir = crate::paths::run_dir(&id)?;
    let note = crate::notes::drop_note(&id, note)?;
    Ok(NoteView {
        text: crate::notes::read_text(&dir, &note),
        id: note.id,
        standing: note.standing,
        added_at: note.added_at,
        given_to_wake: note.given_to_wake,
    })
}

// ---------------------------------------------------------------------------
// wake
// ---------------------------------------------------------------------------

/// Force one wake now, in the background, wherever the run's wakes go — or wherever `detach`
/// says, for this wake only.
pub fn wake_in_background(id: &str, detach: Option<Detach>) -> Result<WakeStart, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let state = read_run(&id)?;
    if state.status.is_terminal() {
        return Err(OttoError::conflict(format!(
            "{id} is {} — there is nothing left to wake",
            crate::state::commands::status_str(state.status)
        )));
    }
    refuse_if_busy(&id)?;
    let strategy = Caller::Background.strategy(detach.unwrap_or(state.launcher.detach));
    start_wake(&id, strategy, None)
}

// ---------------------------------------------------------------------------
// stop
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopOutcome {
    pub id: String,
    pub status: Status,
    pub reason: String,
    pub killed_wake: bool,
}

/// Retiring a healthy run is `stopped`, not `failed` — `failed` in the audit trail would be a
/// lie about a run that worked and simply is not wanted any more.
pub fn stop_run(id: &str, reason: Option<String>, failed: bool, exec: &mut dyn crate::exec::Exec) -> Result<StopOutcome, OttoError> {
    // Resolve before anything else: `spawner::kill` names a tmux session directly from the id,
    // never through `paths::run_dir`, so an unresolved prefix would go looking for a session that
    // was never named that.
    let id = crate::paths::resolve_run_id(id)?;
    let reason = reason.unwrap_or_else(|| "stopped by hand".to_string());
    let status = if failed { Status::Failed } else { Status::Stopped };
    crate::state::commands::set_status(crate::state::commands::SetStatusArgs {
        id: id.clone(),
        status,
        reason: Some(reason.clone()),
        because: None,
    })?;
    // Release locks on the way out, or the next run against that repo waits on a corpse.
    let _ = crate::state::locks::unlock(crate::state::locks::UnlockArgs { id: id.clone(), repo: None });
    // A wake still running would keep working on a run nobody wants — kill it however it was
    // started, by pid as well as by tmux session.
    let killed_wake = match read_run(&id) {
        Ok(state) => crate::spawner::kill(&state, &id, exec).did_anything(),
        Err(_) => false,
    };
    Ok(StopOutcome { id, status, reason, killed_wake })
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deleted {
    pub id: String,
    /// How a person knew it — `#7 legenda` — read before it went.
    pub label: String,
    /// Its status when it went, or `unreadable` for a run whose `run.json` couldn't be read.
    pub was: String,
    pub killed_wake: bool,
}

/// How long a delete waits for a wake it has just killed to let go of its lock.
const DELETE_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Delete a run: its directory, and with it everything otto kept for it — the journal, its
/// questions and their answers, notes, messages and outputs, and what it used. Nothing outside
/// it: the working directory, the wakes' transcripts and whatever the run did in the world stay
/// where they are. Its number is never given to another run (`paths::claim_next_run_id`).
///
/// A run still going is stopped first, as `otto stop` would: its wake killed, its repo locks
/// released. The directory goes only once the wake lock is free, and the lock is held until the
/// directory is out of `runs/`, so no wake can start in a run on its way out.
pub fn delete_run(id: &str, exec: &mut dyn crate::exec::Exec) -> Result<Deleted, OttoError> {
    delete_run_waiting(id, exec, DELETE_WAIT)
}

fn delete_run_waiting(id: &str, exec: &mut dyn crate::exec::Exec, wait: std::time::Duration) -> Result<Deleted, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let label = crate::paths::label_of(&id);
    // A run whose `run.json` can't be read is one a person may well want gone, so it isn't
    // needed: without it there is no status to stop and no pid to kill, only a tmux session.
    let state = read_run(&id).ok();
    let was = state.as_ref().map_or("unreadable", |s| crate::state::commands::status_str(s.status)).to_string();
    let stopping = state.as_ref().is_some_and(|s| !s.status.is_terminal());
    let killed_wake = if stopping {
        stop_run(&id, Some("deleted".to_string()), false, exec)?.killed_wake
    } else {
        let _ = crate::state::locks::unlock(crate::state::locks::UnlockArgs { id: id.clone(), repo: None });
        match &state {
            Some(state) => crate::spawner::kill(state, &id, exec).did_anything(),
            None => {
                let session = crate::detach::session_name(&id);
                crate::detach::session_exists(exec, &session) && crate::detach::kill_session(exec, &session)
            }
        }
    };
    // `otto wake` holds its lock until it has written its last word, so a wake killed above is
    // only gone once the lock is free.
    let deadline = std::time::Instant::now() + wait;
    let lock = loop {
        match crate::liveness::WakeLock::acquire(&id) {
            Ok(lock) => break lock,
            Err(e) if e.code != 2 => return Err(e),
            Err(_) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(_) => {
                return Err(OttoError::conflict(format!(
                    "{label}{} its wake hasn't exited yet — try again in a moment",
                    if stopping { " is stopped, but" } else { ":" }
                )))
            }
        }
    };
    crate::paths::retire_run_number(&id)?;
    let gone = crate::paths::deleting_dir()?.join(format!("{id}-{}", crate::clock::now().unix_timestamp()));
    std::fs::rename(crate::paths::run_dir(&id)?, &gone)?;
    drop(lock);
    std::fs::remove_dir_all(&gone).map_err(|e| {
        OttoError::new(format!("{label} is gone, but not all of its files could be removed ({e}): they're in {}", gone.display()), 4)
    })?;
    Ok(Deleted { id, label, was, killed_wake })
}

// ---------------------------------------------------------------------------
// resume
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeOutcome {
    pub id: String,
    pub status: Status,
    pub reason: String,
    pub wake: Option<WakeStart>,
    /// Why no wake was started, when none was.
    pub note: Option<String>,
}

/// Undo `stop`: bring a stopped or failed run back. `done` stays done — its goal was met, and
/// more work is a new run with a goal of its own.
///
/// The run comes back `sleeping` and due, never `running`: that is a state poke already knows how
/// to revive, so a wake that fails to start here still gets taken on the next poke rather than
/// leaving the run stranded. A gate that was open when it stopped is still the question, so the
/// run goes back to waiting on it instead.
pub fn resume_run(id: &str, reason: Option<String>, no_wake: bool) -> Result<ResumeOutcome, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    // A paused run is not stopped: resuming it is lifting the pause, and a wake already running
    // while it was paused is no reason to refuse.
    if read_run(&id)?.paused.is_some() && !read_run(&id)?.status.is_terminal() {
        return unpause_run(&id);
    }
    let reason = reason.unwrap_or_else(|| "resumed by hand".to_string());
    // A wake that `stop` failed to kill must not get a second one alongside it.
    refuse_if_busy(&id)?;
    let status = crate::state::transaction(&id, |path, state| {
        let previous = crate::state::commands::status_str(state.status);
        match state.status {
            Status::Stopped | Status::Failed => {}
            Status::Done => {
                return Err(OttoError::conflict(format!(
                    "{id} is done — its goal was met; start a new run for more work"
                )))
            }
            _ => {
                return Err(OttoError::conflict(format!(
                    "{id} is {previous} — only a stopped or failed run can be resumed"
                )))
            }
        }
        // A fresh start: whatever counted towards giving up last time should not count again.
        state.incomplete_wakes = 0;
        state.clear_start_failures();
        state.spawn_attempts = 0;
        state.ticks_without_progress = 0;
        state.paused = None;
        if state.gate.is_some() {
            state.status = Status::AwaitingHuman;
            state.next_wake_at = None;
        } else {
            state.status = Status::Sleeping;
            state.next_wake_at = Some(if no_wake { state.period_wake_at() } else { crate::clock::Timestamp::now() });
        }
        crate::event::record(
            path,
            &crate::event::Event::StatusChanged {
                from: previous.to_string(),
                to: crate::state::commands::status_str(state.status).to_string(),
                reason: Some(reason.clone()),
                because: None,
                left_open: vec![],
            },
        )?;
        Ok(state.status)
    })?;
    let mut outcome = ResumeOutcome { id: id.clone(), status, reason, wake: None, note: None };
    let state = read_run(&id)?;
    if let Some(gate) = &state.gate {
        outcome.note = Some(format!("it's still waiting on its question ({}) — answer it to continue", gate.slug));
        return Ok(outcome);
    }
    if no_wake {
        let at = state.next_wake_at.map(relative).unwrap_or_default();
        outcome.note = Some(format!("not waking it now — next wake {at}"));
        return Ok(outcome);
    }
    match start_wake(&id, Caller::Background.strategy(state.launcher.detach), None) {
        Ok(wake) => outcome.wake = Some(wake),
        // The resume itself stands: the run is due, so the next poke tries again.
        Err(err) => outcome.note = Some(format!("resumed, but it didn't start: {} — it's tried again within a few minutes", err.message)),
    }
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// pause
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseOutcome {
    pub id: String,
    pub until: Option<crate::clock::Timestamp>,
    /// A wake running now finishes; the pause holds back the scheduled ones after it.
    pub running: bool,
}

/// Hold back a run's scheduled wakes until `until`, or until it is resumed (`otto resume`). Nothing
/// is killed and nothing is lost: questions stay open, and a person can still answer, note or wake
/// it by hand. A later pause replaces an earlier one.
pub fn pause_run(id: &str, until: Option<crate::clock::Timestamp>, reason: Option<String>) -> Result<PauseOutcome, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    if until.is_some_and(|u| u.is_past()) {
        return Err(OttoError::usage("that time has already passed — pause until a time still to come"));
    }
    crate::state::transaction(&id, |path, state| {
        if state.status.is_terminal() {
            return Err(OttoError::conflict(format!(
                "{} is {} — it never wakes again, so there is nothing to pause",
                state.label(),
                crate::state::commands::status_str(state.status)
            )));
        }
        state.paused = Some(crate::state::Pause { at: crate::clock::Timestamp::now(), until, reason: reason.clone() });
        crate::event::record(path, &crate::event::Event::Paused { until, reason: reason.clone() })
    })?;
    Ok(PauseOutcome { running: LockLiveness.probe(&id).is_busy(), id, until })
}

/// Lift a pause. The run goes back on its schedule; a wake it missed while paused comes at the
/// next poke — one, never a backlog.
pub fn unpause_run(id: &str) -> Result<ResumeOutcome, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let status = crate::state::transaction(&id, |path, state| {
        if state.paused.take().is_none() {
            return Err(OttoError::conflict(format!("{} is not paused", state.label())));
        }
        crate::event::record(path, &crate::event::Event::Unpaused { by: "person".to_string() })?;
        Ok(state.status)
    })?;
    let state = read_run(&id)?;
    let note = match (&state.gate, state.next_wake_at) {
        (Some(gate), _) => format!("back on its schedule, and still waiting on question {}", gate.id),
        (None, Some(at)) if at.is_past() => "back on its schedule — the run it missed comes within a few minutes".to_string(),
        (None, Some(at)) => format!("back on its schedule — next run {}", relative(at)),
        (None, None) => "back on its schedule".to_string(),
    };
    Ok(ResumeOutcome { id, status, reason: "pause lifted".to_string(), wake: None, note: Some(note) })
}

// ---------------------------------------------------------------------------
// messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDoneOutcome {
    pub id: String,
    pub done: crate::messages::Done,
    /// When the run will read the reply, in words — only with a reply.
    pub delivery: Option<String>,
    pub delivery_plain: Option<String>,
    pub wake: Option<WakeStart>,
    pub not_woken: Option<String>,
}

/// Mark a message done, and with a reply, pass it to the run — and, with `now`, wake it to read it.
pub fn message_done(id: &str, which: Option<&str>, reply: Option<&str>, now: bool) -> Result<MessageDoneOutcome, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let done = crate::messages::done(&id, which, reply)?;
    let state = read_run(&id)?;
    let running = LockLiveness.probe(&id).is_busy();
    let mut outcome = MessageDoneOutcome {
        delivery: done.note.as_ref().map(|_| note_delivery(&state, running, false)),
        delivery_plain: done.note.as_ref().map(|_| note_delivery_plain(&state, running, false)),
        done,
        id: id.clone(),
        wake: None,
        not_woken: None,
    };
    if now && outcome.done.note.is_some() {
        match why_not_wake_for_note(&state, running) {
            Some(why) => outcome.not_woken = Some(why),
            None => match start_wake(&id, Caller::Background.strategy(state.launcher.detach), None) {
                Ok(wake) => outcome.wake = Some(wake),
                Err(err) => outcome.not_woken = Some(format!("the run did not start: {}", err.message)),
            },
        }
    }
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// health
// ---------------------------------------------------------------------------

/// A run otto cannot get going, for the health line.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Struggling {
    pub id: String,
    pub short: String,
    pub name: String,
    pub start_failures: u32,
    /// What claude said the last time.
    pub said: Option<String>,
}

/// Whether the machinery that keeps runs going is working — the one thing about otto itself a
/// person needs to know. Everything here is otto's problem, not any run's, so it is said once, as
/// one line, rather than as a question on each run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub ok: bool,
    /// What is wrong, in sentences; empty when nothing is.
    pub problems: Vec<String>,
    pub reviver_loaded: Option<bool>,
    pub last_poke: Option<crate::clock::Timestamp>,
    pub struggling: Vec<Struggling>,
    pub stranded_sleepers: usize,
}

/// A reviver that should poke every five minutes and hasn't for this long has stopped.
const POKE_SILENT_MINUTES: i64 = 20;

pub fn health() -> Result<Health, OttoError> {
    let ids = crate::paths::all_run_ids();
    let mut struggling = Vec::new();
    let mut sleeping = 0usize;
    for entry in read_all_runs()? {
        let RunEntry::Readable(state) = entry else { continue };
        if state.status.is_terminal() {
            continue;
        }
        if state.start_failures >= crate::notify::START_FAILURES_NOTIFIED {
            struggling.push(Struggling {
                short: short_id_among(&state.id, &ids),
                name: state.display_name(),
                id: state.id.clone(),
                start_failures: state.start_failures,
                said: state.start_failure.clone(),
            });
        }
        if state.status == Status::Sleeping && state.gate.is_none() {
            sleeping += 1;
        }
    }
    let agent = crate::launchd::agent_status()?;
    let mut problems = Vec::new();
    let stranded_sleepers = if agent.loaded == Some(false) { sleeping } else { 0 };
    if stranded_sleepers > 0 {
        problems.push(format!(
            "The reviver isn't running, so {stranded_sleepers} sleeping run(s) will never wake on their own."
        ));
    } else if agent.loaded == Some(true) {
        if let Some(last) = agent.last_poke {
            let silent = (crate::clock::now() - last.dt()).whole_minutes();
            if silent > POKE_SILENT_MINUTES && sleeping > 0 {
                problems.push(format!("The reviver hasn't run for {} — runs aren't being woken.", relative(last).trim_end_matches(" ago")));
            }
        }
    }
    if !struggling.is_empty() {
        let names: Vec<String> = struggling.iter().map(|s| s.name.clone()).collect();
        let said = struggling.iter().find_map(|s| s.said.as_deref());
        problems.push(format!(
            "{} couldn't start{}. {}",
            names.join(", "),
            crate::story::claude_says(said),
            crate::story::start_failure_hint(said)
        ));
    }
    Ok(Health {
        ok: problems.is_empty(),
        problems,
        reviver_loaded: agent.loaded,
        last_poke: agent.last_poke,
        struggling,
        stranded_sleepers,
    })
}

// ---------------------------------------------------------------------------
// logs
// ---------------------------------------------------------------------------

/// What `--decisions` keeps: the lines that changed what the run is doing, as opposed to the
/// wake-started/spent/complete rhythm that surrounds every one of them.
pub const DECISION_EVENTS: &[&str] = &[
    "run-created",
    "phase-changed",
    "status-changed",
    "done-claimed",
    "done-verified",
    "done-reopened",
    "gate-opened",
    "gate-closed",
    "gate-expired",
    "note-added",
    "note-dropped",
    "check-set",
    "check-cleared",
    "period-set",
    "budget-warning",
    "budget-exhausted",
    "wake-incomplete",
    "usage-limited",
    "wake-killed",
    "spawn-abandoned",
    "authorized",
    "lock-broken",
    "lock-lost",
];

/// The `wake-spent` counters. Six-digit cache reads are the norm and nobody compares them to
/// the token; `359k` is what a person actually reads off the line.
const TOKEN_KEYS: &[&str] = &["inputTokens", "outputTokens", "cacheRead", "cacheCreation"];

pub fn humanise(n: u64) -> String {
    match n {
        n if n < 10_000 => n.to_string(),
        n if n < 1_000_000 => format!("{}k", (n as f64 / 1_000.0).round() as u64),
        n => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

// ---------------------------------------------------------------------------
// usage
// ---------------------------------------------------------------------------

/// Tokens used by some set of wakes, as journaled in each wake's `wake-spent` line — and by the
/// chats a person had about them (`chat-ended`), which are the run's cost too, though no budget's.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    pub wakes: u64,
    pub chats: u64,
    /// Wakes and chats whose transcript couldn't be read (`usage-unavailable`, or a `chat-ended`
    /// with no tokens): counted, but their tokens are unknown rather than zero.
    pub unmeasured: u64,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

impl TokenTotals {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_creation + self.cache_read
    }

    fn add_spent(&mut self, v: &Value) {
        let n = |key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);
        self.wakes += 1;
        self.turns += n("turns");
        self.input_tokens += n("inputTokens");
        self.output_tokens += n("outputTokens");
        self.cache_creation += n("cacheCreation");
        self.cache_read += n("cacheRead");
    }

    /// A `chat-ended` line: its tokens when its transcript could be read, and unmeasured when not.
    fn add_chat(&mut self, v: &Value) {
        self.chats += 1;
        if v.get("inputTokens").is_none() {
            self.unmeasured += 1;
            return;
        }
        let n = |key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);
        self.turns += n("turns");
        self.input_tokens += n("inputTokens");
        self.output_tokens += n("outputTokens");
        self.cache_creation += n("cacheCreation");
        self.cache_read += n("cacheRead");
    }

    fn add(&mut self, other: &TokenTotals) {
        self.wakes += other.wakes;
        self.chats += other.chats;
        self.unmeasured += other.unmeasured;
        self.turns += other.turns;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation += other.cache_creation;
        self.cache_read += other.cache_read;
    }

    /// One line a person reads: the total, then where it went.
    pub fn summary(&self) -> String {
        let chats = if self.chats > 0 { format!(" and {} chat(s)", self.chats) } else { String::new() };
        let mut line = format!(
            "{} tokens over {} wake(s){chats} — {} in, {} out, {} cache write, {} cache read",
            humanise(self.total()),
            self.wakes,
            humanise(self.input_tokens),
            humanise(self.output_tokens),
            humanise(self.cache_creation),
            humanise(self.cache_read),
        );
        if self.unmeasured > 0 {
            line.push_str(&format!(" ({} not measured)", self.unmeasured));
        }
        line
    }
}

/// Every `wake-spent`, `usage-unavailable` and `chat-ended` line in one run's journal, with its
/// instant.
fn usage_lines(dir: &std::path::Path) -> Vec<Line> {
    let Ok(journal) = std::fs::read_to_string(dir.join("journal.jsonl")) else { return Vec::new() };
    journal
        .lines()
        .filter(|l| l.contains("\"wake-spent\"") || l.contains("\"usage-unavailable\"") || l.contains("\"chat-ended\""))
        .filter_map(parse_line)
        .collect()
}

fn tally(totals: &mut TokenTotals, line: &Line) {
    match line.value.get("event").and_then(Value::as_str) {
        Some("wake-spent") => totals.add_spent(&line.value),
        Some("usage-unavailable") => totals.unmeasured += 1,
        Some("chat-ended") => totals.add_chat(&line.value),
        _ => {}
    }
}

/// Everything a run's wakes, and chats about it, have used over its whole life.
pub fn run_usage(dir: &std::path::Path) -> TokenTotals {
    let mut totals = TokenTotals::default();
    for line in usage_lines(dir) {
        tally(&mut totals, &line);
    }
    totals
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "camelCase")]
pub enum UsageBy {
    #[default]
    Run,
    Day,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    /// The run's id, or the local date (`2026-09-26`).
    pub key: String,
    /// What a person reads: the run's short id, or the date.
    pub label: String,
    /// The run's name, by run — what the web page lists it as, as everywhere else on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub totals: TokenTotals,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    /// The window's start; `None` is all time.
    pub since: Option<crate::clock::Timestamp>,
    pub by: UsageBy,
    /// Biggest first when by run; oldest first when by day.
    pub rows: Vec<UsageRow>,
    pub totals: TokenTotals,
    pub total: u64,
}

/// Token usage across every run, finished ones included, from `since` on (`7d`, a date, …; `None`
/// is all time), grouped by run or by local day.
pub fn usage(since: Option<&str>, by: UsageBy, offset: time::UtcOffset) -> Result<UsageReport, OttoError> {
    let since = since.map(|s| parse_since(s, offset)).transpose()?;
    let ids = crate::paths::all_run_ids();
    let mut groups: std::collections::BTreeMap<String, (String, TokenTotals)> = std::collections::BTreeMap::new();
    for id in &ids {
        let dir = crate::paths::runs_dir().join(id);
        for line in usage_lines(&dir) {
            let Some(at) = line.at else { continue };
            if since.is_some_and(|s| at < s) {
                continue;
            }
            let (key, label) = match by {
                UsageBy::Run => (id.clone(), crate::paths::short_id_among(id, &ids)),
                UsageBy::Day => {
                    let day = at.to_offset(offset).date().to_string();
                    (day.clone(), day)
                }
            };
            tally(&mut groups.entry(key).or_insert_with(|| (label, TokenTotals::default())).1, &line);
        }
    }
    let mut totals = TokenTotals::default();
    let mut rows: Vec<UsageRow> = groups
        .into_iter()
        .map(|(key, (label, t))| {
            totals.add(&t);
            let name = (by == UsageBy::Run).then(|| crate::state::read_run(&key).ok().map(|s| s.display_name())).flatten();
            UsageRow { key, label, name, total: t.total(), totals: t }
        })
        .collect();
    if by == UsageBy::Run {
        rows.sort_by(|a, b| b.total.cmp(&a.total).then(a.key.cmp(&b.key)));
    }
    Ok(UsageReport { since: since.map(crate::clock::Timestamp::at), by, rows, total: totals.total(), totals })
}

/// `--since`: an age (`45m`, `2h`, `3d`), a date (midnight, local), or a full timestamp.
pub fn parse_since(text: &str, offset: time::UtcOffset) -> Result<time::OffsetDateTime, OttoError> {
    let text = text.trim();
    if let Some(ago) = crate::clock::parse_age(text) {
        return Ok(crate::clock::now() - ago);
    }
    if let Ok(dt) = crate::clock::parse_iso(text) {
        return Ok(dt);
    }
    let day = time::macros::format_description!("[year]-[month]-[day]");
    if let Ok(date) = time::Date::parse(text, &day) {
        return Ok(date.midnight().assume_offset(offset));
    }
    Err(OttoError::usage(format!(
        "--since takes an age (45m, 2h, 3d), a date (2026-09-15) or a timestamp, not {text:?}"
    )))
}

/// One parsed journal line: the JSON, and its instant, when it has one.
pub struct Line {
    pub value: Value,
    pub at: Option<time::OffsetDateTime>,
}

pub fn parse_line(raw: &str) -> Option<Line> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let at = value.get("ts").and_then(Value::as_str).and_then(|ts| crate::clock::parse_iso(ts).ok());
    Some(Line { value, at })
}

/// What `logs` keeps, from `--since`, `--event` and `--decisions`. Filters compose as "and".
pub struct Filter {
    pub since: Option<time::OffsetDateTime>,
    pub events: Vec<String>,
}

impl Filter {
    pub fn keeps(&self, line: &Line) -> bool {
        if let Some(since) = self.since {
            match line.at {
                Some(at) if at >= since => {}
                _ => return false,
            }
        }
        if !self.events.is_empty() {
            let event = line.value.get("event").and_then(Value::as_str).unwrap_or("");
            if !self.events.iter().any(|e| e == event) {
                return false;
            }
        }
        true
    }
}

/// Turns journal lines into what a person reads: local times, a rule wherever the date changes
/// (forty lines can span days, and a bare `05:51` on each does not say which), timestamps inside
/// a line shown in the same clock, and token counts rounded to what the eye takes in. The journal
/// is JSON because a wake writes it; this is for reading.
pub struct Renderer {
    offset: time::UtcOffset,
    last_day: Option<time::Date>,
}

impl Renderer {
    pub fn new(offset: time::UtcOffset) -> Self {
        Renderer { offset, last_day: None }
    }

    pub fn render(&mut self, line: &Line) -> String {
        let mut out = String::new();
        let local = line.at.map(|at| at.to_offset(self.offset));
        let day = local.map(|dt| dt.date());
        if let Some(d) = day.filter(|_| day != self.last_day) {
            let weekday = d.weekday().to_string();
            out.push_str(&format!("── {} {d} ──\n", &weekday[..3]));
            self.last_day = day;
        }
        let hms = time::macros::format_description!("[hour]:[minute]:[second]");
        let time = match local {
            Some(dt) => dt.format(&hms).unwrap_or_default(),
            None => line.value.get("ts").and_then(Value::as_str).unwrap_or("").to_string(),
        };
        let event = line.value.get("event").and_then(Value::as_str).unwrap_or("?");
        let mut rest: Vec<String> = Vec::new();
        if let Value::Object(map) = &line.value {
            for (key, val) in map {
                if key == "ts" || key == "event" {
                    continue;
                }
                rest.push(format!("{key}={}", self.shown(key, val, day)));
            }
        }
        out.push_str(&format!("{time}  {event:<18}  {}", rest.join(" ")));
        out
    }

    /// One field's value. `day` is the line's own date, so a timestamp on the same day is just a
    /// time and one on another day says which.
    fn shown(&self, key: &str, val: &Value, day: Option<time::Date>) -> String {
        match val {
            Value::String(s) => {
                if let Ok(at) = crate::clock::parse_iso(s) {
                    let local = at.to_offset(self.offset);
                    let fmt = if Some(local.date()) == day {
                        time::macros::format_description!("[hour]:[minute]:[second]")
                    } else {
                        time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]")
                    };
                    return local.format(&fmt).unwrap_or_else(|_| s.clone());
                }
                // Long prose (a goal, a verbatim answer) makes the log unreadable as a sequence;
                // `otto show` is where full text belongs.
                if s.chars().count() > 80 {
                    format!("{}…", s.chars().take(77).collect::<String>())
                } else {
                    s.clone()
                }
            }
            Value::Number(n) if TOKEN_KEYS.contains(&key) => n.as_u64().map(humanise).unwrap_or_else(|| n.to_string()),
            other => other.to_string(),
        }
    }
}

/// The line above the log: where the run stands, so the tail below it has a frame. Someone
/// arriving cold otherwise reads forty lines of wake rhythm and still has to ask `otto show`.
pub fn logs_header(state: &RunState, short: &str, offset: time::UtcOffset) -> String {
    let since = state.created_at.dt().to_offset(offset).date();
    let days = (crate::clock::now() - state.created_at.dt()).whole_days();
    let age = if days == 0 { "today".to_string() } else { format!("{days}d") };
    let last = match &state.wake {
        Some(wake) => format!("last wake {}", relative(wake.started_at)),
        None => "no wake yet".to_string(),
    };
    let gate = match &state.gate {
        Some(gate) => format!(", gate {} {} open", gate.id, gate.slug),
        None => String::new(),
    };
    format!(
        "# {short} · {} wake(s) since {since} ({age}) · {last} · {}{gate} · times {}",
        state.budget.spent_wakes,
        status_label(state),
        crate::clock::offset_label(offset)
    )
}

/// Which journal lines to show.
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Trailing lines [default: 40, or everything with `since`]
    pub lines: Option<usize>,
    pub since: Option<String>,
    pub events: Vec<String>,
    pub decisions: bool,
}

/// One rendered journal line. `text` may start with a day rule on a line of its own.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub text: String,
    pub event: String,
    pub raw: Value,
}

/// A journal being read, and where to pick it up again. The renderer is kept so that following
/// on only prints a day rule when the day actually changes.
pub struct LogCursor {
    path: std::path::PathBuf,
    filter: Filter,
    renderer: Renderer,
    seen: usize,
}

impl LogCursor {
    /// Lines written since the last read.
    pub fn poll(&mut self) -> Vec<LogLine> {
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        if text.len() <= self.seen {
            return Vec::new();
        }
        let fresh: Vec<Line> = text[self.seen..].lines().filter_map(parse_line).filter(|l| self.filter.keeps(l)).collect();
        self.seen = text.len();
        fresh.iter().map(|l| self.log_line(l)).collect()
    }

    fn log_line(&mut self, line: &Line) -> LogLine {
        LogLine {
            text: self.renderer.render(line),
            event: line.value.get("event").and_then(Value::as_str).unwrap_or("?").to_string(),
            raw: line.value.clone(),
        }
    }
}

pub struct LogPage {
    pub header: String,
    pub lines: Vec<LogLine>,
    /// Continue from here to follow.
    pub cursor: LogCursor,
}

/// The tail of a run's journal, readably. `offset` is the clock times are shown in; read it
/// with `clock::local_offset` before any thread starts (see there).
pub fn read_logs(id: &str, query: &LogQuery, offset: time::UtcOffset) -> Result<LogPage, OttoError> {
    let id = crate::paths::resolve_run_id(id)?;
    let state = read_run(&id)?;
    let path = crate::paths::run_dir(&id)?.join("journal.jsonl");

    let mut events: Vec<String> = query.events.iter().map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).collect();
    if query.decisions {
        events.extend(DECISION_EVENTS.iter().map(|e| e.to_string()));
    }
    let filter = Filter { since: query.since.as_deref().map(|s| parse_since(s, offset)).transpose()?, events };
    // `since` says where to start, so it shows everything from there unless `lines` says otherwise.
    let limit = query.lines.unwrap_or(if filter.since.is_some() { usize::MAX } else { 40 });

    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let kept: Vec<Line> = text.lines().filter_map(parse_line).filter(|l| filter.keeps(l)).collect();
    let start = kept.len().saturating_sub(limit);
    let mut cursor = LogCursor { path, filter, renderer: Renderer::new(offset), seen: text.len() };
    let lines = kept[start..].iter().map(|l| cursor.log_line(l)).collect();
    Ok(LogPage { header: logs_header(&state, &state.label(), offset), lines, cursor })
}

// ---------------------------------------------------------------------------
// Watching a live wake
// ---------------------------------------------------------------------------

/// What a running wake looks like right now, for a caller that cannot `tmux attach`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveView {
    /// What the running wake has said and done, from its session transcript — the view that
    /// actually shows something: a wake's own stdout goes to a pipe, not to its tmux pane.
    Transcript(Vec<crate::wake::transcript::Activity>),
    /// The tmux pane's contents, with its colour escapes.
    Pane(String),
    /// The tail of `wake.log`, for a wake backgrounded without tmux.
    Log(String),
    /// No wake is running; the message says where to look instead.
    Ended(String),
}

/// How many trailing lines of `wake.log` stand in for a pane.
const LIVE_LOG_LINES: usize = 200;
/// How much of the running wake's activity the live view carries.
const LIVE_ACTIVITY: usize = 150;

/// How long a running wake has to say nothing before a person is told so.
pub const QUIET_AFTER_MINUTES: u64 = 5;

/// Minutes since the wake in progress last wrote to its session transcript, once that is at least
/// `QUIET_AFTER_MINUTES`. Paperclip shows the same for a run gone silent, without killing it: a
/// long build says nothing too, so it is a thing to know, not a failure. `None` when there is no
/// wake in progress, it spoke recently, or its transcript can't be found — a sandbox that hides
/// `~/.claude` must not make every wake look silent.
pub fn quiet_minutes(state: &RunState) -> Option<u64> {
    let session = state.wake.as_ref().filter(|w| w.outcome.is_none())?.session.as_deref()?;
    let path = crate::wake::transcript::find_transcript(session)?;
    let silent = std::fs::metadata(path).ok()?.modified().ok()?.elapsed().ok()?;
    let minutes = silent.as_secs() / 60;
    (minutes >= QUIET_AFTER_MINUTES).then_some(minutes)
}

/// The running wake's activity so far, from its transcript — `None` when no wake is running, or
/// its transcript can't be read (a sandbox that hides `~/.claude`).
pub fn live_activity(id: &str) -> Option<Vec<crate::wake::transcript::Activity>> {
    if !LockLiveness.probe(id).is_busy() {
        return None;
    }
    let session = read_run(id).ok()?.wake?.session?;
    let path = crate::wake::transcript::find_transcript(&session)?;
    crate::wake::transcript::activity(&path).ok()
}

pub fn live_view(id: &str, exec: &mut dyn crate::exec::Exec) -> LiveView {
    if let Some(mut activity) = live_activity(id) {
        let start = activity.len().saturating_sub(LIVE_ACTIVITY);
        return LiveView::Transcript(activity.split_off(start));
    }
    let session = crate::detach::session_name(id);
    if let Some(pane) = crate::detach::capture_pane(exec, &session) {
        return LiveView::Pane(pane);
    }
    if LockLiveness.probe(id).is_busy() {
        if let Ok(text) = crate::paths::run_dir(id).and_then(|d| Ok(std::fs::read_to_string(d.join("wake.log"))?)) {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(LIVE_LOG_LINES);
            return LiveView::Log(lines[start..].join("\n"));
        }
    }
    let short = short_id(id);
    LiveView::Ended(format!(
        "no wake is running for {id} right now — `otto show {short}` for where it stands, `otto logs {short}` for what it has done"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::{open_gate, test_init, OpenGateArgs};

    fn gate_open(id: &str) {
        open_gate(OpenGateArgs {
            id: id.to_string(),
            slug: "plan-review".to_string(),
            question: Some("Approve the plan?\n\n- Approve\n- Revise *(default)*".to_string()),
            question_file: None,
            stdin: false,
            expires_at: None,
            expires_in: None,
            about: None,
        })
        .unwrap();
    }

    const SCRIPT: &str = "#!/bin/sh\nexit 0\n";

    /// Quiet is read off the running wake's transcript, and said only once it has lasted.
    #[test]
    fn a_running_wake_is_quiet_once_its_transcript_has_gone_still() {
        let _h = TempHome::new();
        test_init("c-quiet", "a goal").unwrap();
        assert_eq!(quiet_minutes(&read_run("c-quiet").unwrap()), None, "no wake in progress");
        crate::state::transaction("c-quiet", |_p, s| {
            let now = crate::clock::Timestamp::now();
            s.wake = Some(crate::state::Wake {
                n: 1,
                started_at: now,
                deadline_at: now,
                launcher: "claude".into(),
                pid: None,
                session: Some("sess-quiet".into()),
                outcome: None,
            });
            Ok(())
        })
        .unwrap();
        assert_eq!(quiet_minutes(&read_run("c-quiet").unwrap()), None, "no transcript to read, so nothing to say");
        let project = crate::paths::claude_projects_dir().join("-w-work");
        std::fs::create_dir_all(&project).unwrap();
        let transcript = std::fs::File::create(project.join("sess-quiet.jsonl")).unwrap();
        assert_eq!(quiet_minutes(&read_run("c-quiet").unwrap()), None, "it just spoke");
        transcript.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(12 * 60 + 5)).unwrap();
        assert_eq!(quiet_minutes(&read_run("c-quiet").unwrap()), Some(12));
    }

    fn sleeping_after_a_wake(id: &str, started_minutes_ago: i64, next_in_minutes: i64) {
        crate::state::transaction(id, |_p, s| {
            s.status = Status::Sleeping;
            let started = crate::clock::Timestamp::in_minutes(-started_minutes_ago);
            s.wake = Some(crate::state::Wake {
                n: 1,
                started_at: started,
                deadline_at: started,
                launcher: "claude".into(),
                pid: None,
                session: None,
                outcome: Some(crate::state::WakeOutcome::Complete),
            });
            s.next_wake_at = Some(crate::clock::Timestamp::in_minutes(next_in_minutes));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_check_set_by_a_person_is_executable_pinned_and_stands_in_front_of_the_next_wake() {
        let _h = TempHome::new();
        test_init("2026-09-25-chk", "a goal").unwrap();
        sleeping_after_a_wake("2026-09-25-chk", 10, 50);
        let out = set_check("chk", SCRIPT, 24).unwrap();
        let dir = crate::paths::run_dir("2026-09-25-chk").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join(CHECK_FILE)).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "poke runs it directly, so it must be executable");

        let state = read_run("2026-09-25-chk").unwrap();
        let check = state.check.as_ref().unwrap();
        assert!(check.pinned);
        assert_eq!(check.wake_after, 24);
        assert!(state.check_gates_wake());
        assert_eq!(out.check.text.as_deref(), Some(SCRIPT));
    }

    /// A person's check knows nothing about why a wake armed its own timer, so it does not stand
    /// in front of it — only in front of the period's wakes.
    #[test]
    fn a_person_s_check_does_not_gate_a_timer_a_wake_armed() {
        let _h = TempHome::new();
        test_init("2026-09-25-ci", "a goal").unwrap();
        set_check("ci", SCRIPT, 24).unwrap();
        crate::state::commands::arm_timer(crate::state::commands::ArmTimerArgs {
            id: "2026-09-25-ci".into(),
            at: None,
            seconds: Some(600),
            status: Status::Sleeping,
            note: None,
            check_script: None,
            waiting_for: None,
            give_up_after: None,
        })
        .unwrap();
        let state = read_run("2026-09-25-ci").unwrap();
        assert!(state.check.is_some(), "kept");
        assert!(!state.check_gates_wake());
    }

    #[test]
    fn a_longer_period_moves_the_period_sleep_out() {
        let _h = TempHome::new();
        test_init("2026-09-25-long", "a goal").unwrap();
        sleeping_after_a_wake("2026-09-25-long", 10, 50);
        let out = set_period("long", 24 * 60).unwrap();
        assert!(out.rescheduled);
        assert_eq!(out.previous_minutes, 60);
        let state = read_run("2026-09-25-long").unwrap();
        assert_eq!(state.policy.period_minutes, 24 * 60);
        // Anchored on the last wake's start, ten minutes ago.
        let next = state.next_wake_at.unwrap();
        assert!(next.dt() > crate::clock::now() + time::Duration::hours(23), "{next}");
        assert!(next.dt() < crate::clock::now() + time::Duration::hours(24), "{next}");
    }

    #[test]
    fn a_shorter_period_brings_the_period_sleep_in_but_never_into_the_past() {
        let _h = TempHome::new();
        test_init("2026-09-25-short", "a goal").unwrap();
        sleeping_after_a_wake("2026-09-25-short", 10, 50);
        set_period("short", 30).unwrap();
        let next = read_run("2026-09-25-short").unwrap().next_wake_at.unwrap();
        assert!(next.dt() > crate::clock::now() + time::Duration::minutes(19), "{next}");
        assert!(next.dt() < crate::clock::now() + time::Duration::minutes(21), "{next}");

        // Shorter than the time since the last wake: due now, not retroactively.
        set_period("short", 5).unwrap();
        assert!(read_run("2026-09-25-short").unwrap().next_wake_at.unwrap().is_past());
    }

    /// A timer the wake armed for its own reasons is not the period's to move.
    #[test]
    fn a_timer_a_wake_armed_is_left_alone() {
        let _h = TempHome::new();
        test_init("2026-09-25-ci", "a goal").unwrap();
        // Woke 10 minutes ago and armed a 5-minute timer for CI — not the hourly period.
        sleeping_after_a_wake("2026-09-25-ci", 10, 5);
        crate::state::transaction("2026-09-25-ci", |_, s| {
            s.armed_wake_at = s.next_wake_at;
            Ok(())
        })
        .unwrap();
        let before = read_run("2026-09-25-ci").unwrap().next_wake_at;
        let out = set_period("ci", 24 * 60).unwrap();
        assert!(!out.rescheduled);
        let state = read_run("2026-09-25-ci").unwrap();
        assert_eq!(state.next_wake_at, before);
        assert_eq!(state.policy.period_minutes, 24 * 60, "the period still changes for later sleeps");
    }

    #[test]
    fn a_zero_period_or_a_finished_run_is_refused() {
        let _h = TempHome::new();
        test_init("2026-09-25-zero", "a goal").unwrap();
        assert!(set_period("zero", 0).is_err());
        crate::state::transaction("2026-09-25-zero", |_, s| {
            s.status = Status::Done;
            Ok(())
        })
        .unwrap();
        assert!(set_period("zero", 60).unwrap_err().to_string().contains("no period"));
    }

    #[test]
    fn a_bad_check_is_refused_and_one_can_be_removed() {
        let _h = TempHome::new();
        test_init("2026-09-25-bad", "a goal").unwrap();
        assert!(set_check("bad", "exit 0\n", 24).unwrap_err().message.contains("#!"));
        assert!(clear_check("bad").is_err(), "nothing to remove yet");
        set_check("bad", SCRIPT, 24).unwrap();
        assert_eq!(clear_check("bad").unwrap(), CHECK_FILE);
        assert!(read_run("2026-09-25-bad").unwrap().check.is_none());
        let journal = std::fs::read_to_string(crate::paths::run_dir("2026-09-25-bad").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("check-set") && journal.contains("check-cleared"));
    }

    #[test]
    fn a_wake_s_plain_re_arm_keeps_a_person_s_check() {
        let _h = TempHome::new();
        test_init("2026-09-25-keep", "a goal").unwrap();
        set_check("keep", SCRIPT, 12).unwrap();
        crate::state::commands::arm_timer(crate::state::commands::ArmTimerArgs {
            id: "2026-09-25-keep".into(),
            at: None,
            seconds: Some(7200),
            status: Status::Sleeping,
            note: None,
            check_script: Some("artifacts/other.sh".into()),
            waiting_for: None,
            give_up_after: None,
        })
        .unwrap();
        let check = read_run("2026-09-25-keep").unwrap().check.unwrap();
        assert!(check.pinned);
        assert_eq!(check.script, CHECK_FILE);
        assert_eq!(check.wake_after, 12);
    }

    /// Appends a `wake-spent` line stamped `ts` — the shape `run_wake` writes.
    fn spent(id: &str, ts: &str, input: u64, output: u64, write: u64, read: u64) {
        let path = crate::paths::run_dir(id).unwrap().join("journal.jsonl");
        let line = serde_json::json!({
            "event": "wake-spent", "turns": 3, "spentWakes": 1, "inputTokens": input, "outputTokens": output,
            "cacheCreation": write, "cacheRead": read, "toolErrors": 0, "ts": ts,
        });
        let mut f = std::fs::OpenOptions::new().append(true).create(true).open(path).unwrap();
        use std::io::Write;
        writeln!(f, "{line}").unwrap();
    }

    fn unmeasured(id: &str, ts: &str) {
        let path = crate::paths::run_dir(id).unwrap().join("journal.jsonl");
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        use std::io::Write;
        writeln!(f, "{}", serde_json::json!({"event": "usage-unavailable", "note": "x", "ts": ts})).unwrap();
    }

    #[test]
    fn usage_totals_every_run_by_run_and_by_day() {
        let _h = TempHome::new();
        test_init("2026-09-20-big", "a goal").unwrap();
        test_init("2026-09-20-small", "a goal").unwrap();
        let now = crate::clock::Timestamp::now().to_string();
        spent("2026-09-20-big", &now, 100, 200, 1000, 5000);
        spent("2026-09-20-big", &now, 100, 200, 1000, 5000);
        spent("2026-09-20-small", &now, 10, 20, 100, 500);
        spent("2026-09-20-small", &now, 0, 0, 0, 0);
        unmeasured("2026-09-20-small", &now);
        // Outside a 7-day window.
        spent("2026-09-20-small", "2020-01-01T00:00:00Z", 1, 1, 1, 1);

        let week = usage(Some("7d"), UsageBy::Run, time::UtcOffset::UTC).unwrap();
        assert_eq!(week.rows.len(), 2);
        assert_eq!(week.rows[0].label, "big", "biggest first, by its short id");
        assert_eq!(week.rows[0].totals.wakes, 2);
        assert_eq!(week.rows[0].total, 12_600);
        assert_eq!(week.rows[1].totals.wakes, 2);
        assert_eq!(week.rows[1].totals.unmeasured, 1, "an unreadable transcript is counted, not zeroed silently");
        assert_eq!(week.totals.wakes, 4);
        assert_eq!(week.total, 12_600 + 630);

        let ever = usage(None, UsageBy::Run, time::UtcOffset::UTC).unwrap();
        assert_eq!(ever.totals.wakes, 5, "all time includes the old wake");

        let days = usage(None, UsageBy::Day, time::UtcOffset::UTC).unwrap();
        assert_eq!(days.rows.len(), 2);
        assert_eq!(days.rows[0].key, "2020-01-01", "oldest day first");
        assert_eq!(days.rows[1].totals.wakes, 4);
    }

    #[test]
    fn a_run_s_own_usage_is_its_whole_life() {
        let _h = TempHome::new();
        test_init("2026-09-20-life", "a goal").unwrap();
        spent("2026-09-20-life", "2020-01-01T00:00:00Z", 1, 2, 3, 4);
        spent("2026-09-20-life", &crate::clock::Timestamp::now().to_string(), 1, 2, 3, 4);
        let detail = run_detail(Some("life")).unwrap();
        assert_eq!(detail.usage.wakes, 2);
        assert_eq!(detail.usage.total(), 20);
        assert!(detail.usage.summary().starts_with("20 tokens over 2 wake(s)"));
    }

    /// A chat about a run is that run's cost: counted beside its wakes, never as one, and never
    /// against its budget — which `wake-spent` alone carries.
    #[test]
    fn a_chat_is_counted_beside_the_runs_wakes() {
        let _h = TempHome::new();
        test_init("2026-09-20-talk", "a goal").unwrap();
        let now = crate::clock::Timestamp::now().to_string();
        spent("2026-09-20-talk", &now, 1, 2, 3, 4);
        for usage in [Some(10), None] {
            crate::state::log_event(
                "2026-09-20-talk",
                crate::event::Event::ChatEnded {
                    session: "s".into(),
                    turns: usage.map(|_| 2),
                    input_tokens: usage,
                    output_tokens: usage,
                    cache_read: usage,
                    cache_creation: usage,
                    notes: vec![],
                },
            )
            .unwrap();
        }
        let detail = run_detail(Some("talk")).unwrap();
        assert_eq!((detail.usage.wakes, detail.usage.chats), (1, 2));
        assert_eq!(detail.usage.total(), 10 + 40);
        assert_eq!(detail.usage.unmeasured, 1, "a chat whose transcript couldn't be read is counted, not zeroed");
        assert!(detail.usage.summary().starts_with("50 tokens over 1 wake(s) and 2 chat(s)"), "{}", detail.usage.summary());
        assert_eq!(detail.state.budget.spent_wakes, 0, "no budget is spent by a chat");
        let week = usage(Some("7d"), UsageBy::Run, time::UtcOffset::UTC).unwrap();
        assert_eq!(week.totals.chats, 2);
    }

    /// The working directory is only worth a line when it isn't the one a person would assume.
    #[test]
    fn the_workdir_is_shown_only_when_it_is_not_the_default() {
        let h = TempHome::new();
        let work = h.path().join("work");
        let other = h.path().join("other");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let mut state = crate::state::test_run_state("r");
        assert!(matches!(workdir_view(&state), Some(WorkdirView::Unset)), "no directory at all is worth saying");

        crate::config::save_workdir(work.to_str().unwrap()).unwrap();
        assert!(workdir_view(&state).is_none(), "nothing recorded: the default");
        state.launcher.workdir = Some(work.canonicalize().unwrap().display().to_string());
        assert!(workdir_view(&state).is_none(), "the default, recorded");
        state.launcher.workdir = Some(other.canonicalize().unwrap().display().to_string());
        assert!(matches!(workdir_view(&state), Some(WorkdirView::Own { .. })));
    }

    #[test]
    fn a_run_s_wake_cost_averages_its_recent_wakes() {
        let _h = TempHome::new();
        test_init("2026-09-25-cost", "a goal").unwrap();
        let dir = crate::paths::run_dir("2026-09-25-cost").unwrap();
        assert!(wake_cost(&dir).is_none());
        for (turns, create) in [(8, 20_000), (10, 40_000)] {
            crate::state::log_event(
                "2026-09-25-cost",
                crate::event::Event::WakeSpent {
                    turns,
                    spent_wakes: 1,
                    spent_tokens: 0,
                    input_tokens: 0,
                    output_tokens: 100,
                    cache_read: 0,
                    cache_creation: create,
                    tool_errors: 0,
                },
            )
            .unwrap();
        }
        let cost = wake_cost(&dir).unwrap();
        assert_eq!((cost.wakes, cost.avg_turns, cost.avg_cache_creation), (2, 9, 30_000));
    }

    #[test]
    fn a_gate_is_listed_as_needing_you_with_its_default_first() {
        let _h = TempHome::new();
        test_init("2026-09-20-gated", "a goal").unwrap();
        test_init("2026-09-20-quiet", "a goal").unwrap();
        gate_open("2026-09-20-gated");
        let view = list_runs(false).unwrap();
        assert_eq!(view.rows.len(), 2);
        assert_eq!(view.needs_you.len(), 1);
        let needs = &view.needs_you[0];
        assert_eq!(needs.id, "2026-09-20-gated");
        assert_eq!(needs.short, "gated");
        assert_eq!(needs.options, vec!["Revise".to_string(), "Approve".to_string()]);
    }

    #[test]
    fn where_this_is_is_the_section_up_to_the_next_heading_on_one_line() {
        let handoff = "# r · plan · wake 3\n\n## Goal\nShip it\n\n## Where this is\nPlan written,\n\
                       waiting on review.\n\n#123 is the PR.\n## Decided\nNothing yet.\n";
        assert_eq!(where_this_is(handoff).as_deref(), Some("Plan written, waiting on review. #123 is the PR."));
        assert_eq!(where_this_is("## where this is — today\n  Rebasing.  \n").as_deref(), Some("Rebasing."));
        assert_eq!(where_this_is("## Where this is\n\n## Decided\nx\n"), None, "an empty section is no summary");
        assert_eq!(where_this_is("## Goal\nShip it\n"), None);
        assert_eq!(where_this_is("##Where this is\nnot a heading\n"), None);
    }

    #[test]
    fn the_last_run_is_the_last_wake_or_a_later_check() {
        let _h = TempHome::new();
        test_init("2026-09-20-last", "a goal").unwrap();
        let dir = crate::paths::run_dir("2026-09-20-last").unwrap();
        let line = |event: &str, extra: serde_json::Value| crate::state::journal(&dir, event, extra).unwrap();
        let state = || read_run("2026-09-20-last").unwrap();
        assert_eq!(last_run(&dir, &state()), None, "nothing has run yet");

        line("wake-started", serde_json::json!({"wake": 1}));
        line("noop-tick", serde_json::json!({"ticksWithoutProgress": 1}));
        line("wake-complete", serde_json::json!({"status": "sleeping"}));
        assert_eq!(last_run(&dir, &state()).unwrap().outcome, "no changes");

        line("wake-started", serde_json::json!({"wake": 2}));
        line("curated", serde_json::json!({"message": "added 1 talk"}));
        line("wake-complete", serde_json::json!({"status": "sleeping"}));
        line("wake-started", serde_json::json!({"wake": 3}));
        let last = last_run(&dir, &state()).unwrap();
        assert_eq!((last.kind, last.outcome.as_str()), ("wake", "added 1 talk"), "the running wake is not the last run");

        // A check that ran after the last wake is the last thing that ran.
        crate::state::transaction("2026-09-20-last", |_p, s| {
            s.check = Some(crate::state::Check {
                script: "artifacts/check.sh".into(),
                retry_seconds: None,
                wake_after: 24,
                consecutive_no_change: 1,
                last_result: Some(crate::state::CheckResult::NoChange),
                last_at: Some(crate::clock::Timestamp::in_minutes(1)),
                last_note: Some("nothing tagged agent/ai in next".into()),
                no_change_total: 1,
                pinned: true,
                set_by_wake: false,
            });
            Ok(())
        })
        .unwrap();
        let last = last_run(&dir, &state()).unwrap();
        assert_eq!((last.kind, last.outcome.as_str()), ("check", "checked, nothing new — nothing tagged agent/ai in next"));
    }

    #[test]
    fn needs_is_shown_only_when_it_says_something() {
        assert_eq!(needs("## Needs\nnothing\n## Don't re-derive\nx\n"), None);
        assert_eq!(needs("## Needs\nNothing.\n"), None);
        assert_eq!(needs("## Goal\nx\n"), None);
        assert_eq!(
            needs("## Needs\nNothing is blocking. You may want:\n- an API key\n\n## Don't re-derive\n").as_deref(),
            Some("Nothing is blocking. You may want:\n- an API key")
        );
    }

    #[test]
    fn the_list_carries_each_run_s_where_this_is() {
        let _h = TempHome::new();
        test_init("2026-09-20-fresh", "a goal").unwrap();
        test_init("2026-09-20-moved", "a goal").unwrap();
        let dir = crate::paths::run_dir("2026-09-20-moved").unwrap();
        std::fs::write(dir.join(crate::state::commands::HANDOFF_FILE), "## Where this is\nHalfway.\n## Needs\nnothing\n").unwrap();
        let view = list_runs(false).unwrap();
        let summary = |id: &str| view.rows.iter().find(|r| r.id == id).unwrap().where_this_is.clone();
        assert_eq!(summary("2026-09-20-moved").as_deref(), Some("Halfway."));
        // `init`'s own handoff has the section, so a run that has never woken still says something.
        assert!(summary("2026-09-20-fresh").unwrap().contains("nothing has been done yet"));
        let json = serde_json::to_value(&view).unwrap();
        assert!(json["rows"][0].get("whereThisIs").is_some(), "the web page reads it in camelCase");
    }

    /// The feed is what wakes wrote with `otto state log`, newest first — never otto's own lines,
    /// including the ones otto writes untyped.
    #[test]
    fn activity_is_what_wakes_logged_newest_first() {
        let _h = TempHome::new();
        test_init("2026-09-20-act", "a goal").unwrap();
        let dir = crate::paths::run_dir("2026-09-20-act").unwrap();
        let log = |event: &str, message: &str, data: &[&str]| {
            crate::state::commands::log(crate::state::commands::LogArgs {
                id: "2026-09-20-act".into(),
                event: event.into(),
                message: Some(message.into()),
                data: data.iter().map(|d| d.to_string()).collect(),
            })
            .unwrap()
        };
        log("curated", "added 1 talk", &["picked=3"]);
        crate::state::journal(&dir, "lock-acquired", serde_json::json!({"repo": "/r"})).unwrap();
        log("cycle", "nothing new in next", &[]);

        let feed = activity(&dir, 10);
        let said: Vec<(&str, &str)> = feed.iter().map(|a| (a.event.as_str(), a.text.as_str())).collect();
        assert_eq!(said, vec![("cycle", "nothing new in next"), ("curated", "added 1 talk picked=3")]);
        assert_eq!(activity(&dir, 1).len(), 1);

        let row = list_runs(false).unwrap().rows.into_iter().find(|r| r.id == "2026-09-20-act").unwrap();
        assert_eq!(row.last_activity.unwrap().event, "cycle");
        assert_eq!(run_detail(Some("act")).unwrap().activity.len(), 2);
    }

    #[test]
    fn the_list_and_the_detail_carry_the_todo_and_flag_one_left_open_at_done() {
        let _h = TempHome::new();
        test_init("2026-09-20-todo", "a goal").unwrap();
        test_init("2026-09-20-none", "a goal").unwrap();
        let dir = crate::paths::run_dir("2026-09-20-todo").unwrap();
        std::fs::write(dir.join(crate::todo::TODO_FILE), "- [x] land\n- [ ] cleanup\n").unwrap();
        let row = |id: &str| list_runs(true).unwrap().rows.into_iter().find(|r| r.id == id).unwrap();

        assert!(row("2026-09-20-none").todo.is_none());
        let counts = row("2026-09-20-todo").todo.expect("it keeps one");
        assert_eq!((counts.total, counts.done, counts.open), (2, 1, 1));
        assert!(!row("2026-09-20-todo").todo_left_open, "open items are the normal state of a live run");

        crate::state::transaction("2026-09-20-todo", |_p, state| {
            state.status = Status::Done;
            Ok(())
        })
        .unwrap();
        assert!(row("2026-09-20-todo").todo_left_open);
        let detail = run_detail(Some("todo")).unwrap();
        assert!(detail.todo_left_open);
        assert_eq!(detail.todo.unwrap().open_items(), vec!["cleanup"]);

        crate::state::transaction("2026-09-20-todo", |_p, state| {
            state.status = Status::Stopped;
            Ok(())
        })
        .unwrap();
        assert!(!row("2026-09-20-todo").todo_left_open, "a person retired it; nothing was claimed");
    }

    #[test]
    fn run_detail_carries_the_question_and_its_parsed_options() {
        let _h = TempHome::new();
        test_init("2026-09-20-detail", "a goal").unwrap();
        gate_open("2026-09-20-detail");
        let detail = run_detail(Some("detail")).unwrap();
        let gate = detail.gate.expect("a gate is open");
        assert_eq!(gate.options, vec!["Approve".to_string(), "Revise".to_string()]);
        assert_eq!(gate.default.as_deref(), Some("Revise"));
        assert!(gate.text.unwrap().contains("Approve the plan?"));
        assert!(detail.handoff.is_some(), "the handoff stays a click away with a gate open, too");
        // And it serialises, which is what the web page reads.
        let json = serde_json::to_value(run_detail(Some("detail")).unwrap()).unwrap();
        assert_eq!(json["gate"]["default"], "Revise");
        assert_eq!(json["state"]["id"], "2026-09-20-detail");
    }

    #[test]
    fn a_background_answer_with_a_bad_choice_leaves_the_gate_open() {
        let _h = TempHome::new();
        test_init("2026-09-20-bad", "a goal").unwrap();
        gate_open("2026-09-20-bad");
        let err = answer_in_background("bad", Some("aprove"), None, true).expect_err("must refuse");
        assert_eq!(err.code, 1);
        assert!(read_run("2026-09-20-bad").unwrap().gate.is_some());

        let ok = answer_in_background("bad", Some("approve"), None, true).unwrap();
        assert_eq!(ok.answer, "Approve");
        assert!(ok.wake.is_none());
        assert!(read_run("2026-09-20-bad").unwrap().gate.is_none());
    }

    fn stopped(id: &str) {
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() });
        stop_run(id, None, false, &mut exec).unwrap();
    }

    #[test]
    fn a_stopped_run_resumes_sleeping_on_a_clean_slate() {
        let _h = TempHome::new();
        test_init("2026-09-25-back", "a goal").unwrap();
        crate::state::transaction("2026-09-25-back", |_p, state| {
            state.incomplete_wakes = 3;
            state.ticks_without_progress = 7;
            Ok(())
        })
        .unwrap();
        stopped("back");

        let outcome = resume_run("back", None, true).unwrap();
        assert_eq!(outcome.status, Status::Sleeping);
        assert!(outcome.wake.is_none());
        let state = read_run("2026-09-25-back").unwrap();
        assert_eq!(state.status, Status::Sleeping);
        assert!(state.next_wake_at.is_some(), "a resumed run must be revivable by poke");
        assert_eq!((state.incomplete_wakes, state.ticks_without_progress), (0, 0));
        let journal = std::fs::read_to_string(crate::paths::run_dir("2026-09-25-back").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("resumed by hand"), "the resume must be in the audit trail");
    }

    #[test]
    fn a_resumed_run_with_an_open_gate_goes_back_to_waiting_on_it() {
        let _h = TempHome::new();
        test_init("2026-09-25-asked", "a goal").unwrap();
        gate_open("2026-09-25-asked");
        stopped("asked");

        let outcome = resume_run("asked", None, false).unwrap();
        assert_eq!(outcome.status, Status::AwaitingHuman);
        assert!(outcome.wake.is_none(), "the question is still unanswered — nothing to wake for");
        assert!(read_run("2026-09-25-asked").unwrap().next_wake_at.is_none());
    }

    #[test]
    fn only_a_stopped_or_failed_run_can_be_resumed() {
        let _h = TempHome::new();
        test_init("2026-09-25-live", "a goal").unwrap();
        assert_eq!(resume_run("live", None, true).expect_err("still live").code, 2);

        test_init("2026-09-25-met", "a goal").unwrap();
        crate::state::transaction("2026-09-25-met", |_p, state| {
            state.status = Status::Done;
            Ok(())
        })
        .unwrap();
        let err = resume_run("met", None, true).expect_err("done stays done");
        assert!(err.message.contains("new run"), "got: {}", err.message);

        test_init("2026-09-25-broke", "a goal").unwrap();
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() });
        stop_run("broke", None, true, &mut exec).unwrap();
        assert_eq!(resume_run("broke", None, true).unwrap().status, Status::Sleeping);
    }

    #[test]
    fn a_deleted_run_is_gone_and_its_number_never_comes_back() {
        let _h = TempHome::new();
        test_init("4", "a goal").unwrap();
        stopped("4");
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() }); // has-session: no
        let deleted = delete_run("4", &mut exec).unwrap();
        assert_eq!((deleted.was.as_str(), deleted.killed_wake), ("stopped", false));
        assert!(!crate::paths::runs_dir().join("4").exists());
        assert_eq!(crate::paths::resolve_run_id("4").expect_err("gone").code, 3);
        assert_eq!(crate::paths::peek_next_run_id(), 5, "a new run must not answer to a deleted one's number");
        let leftovers = std::fs::read_dir(crate::paths::deleting_dir().unwrap()).unwrap().count();
        assert_eq!(leftovers, 0, "nothing is left on its way out");
    }

    #[test]
    fn a_run_still_going_is_stopped_and_lets_go_of_its_repos_before_it_goes() {
        let _h = TempHome::new();
        test_init("2026-10-19-going", "a goal").unwrap();
        crate::state::locks::lock(crate::state::locks::LockArgs {
            id: "2026-10-19-going".into(),
            repo: "/tmp/otto-delete-repo".into(),
            force: false,
        })
        .unwrap();
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() }); // has-session: no
        let deleted = delete_run("going", &mut exec).unwrap();
        assert_ne!(deleted.was, "stopped", "it was still going");
        let calls: Vec<String> = exec.calls.borrow().iter().map(|c| c.join(" ")).collect();
        assert!(calls.iter().any(|c| c.contains("otto-2026-10-19-going")), "its session must be looked for: {calls:?}");
        let locks = std::fs::read_dir(crate::paths::repo_locks_dir().unwrap()).unwrap().count();
        assert_eq!(locks, 0, "a deleted run must not hold a repo");
        assert!(crate::paths::resolve_run_id("going").is_err());
    }

    #[test]
    fn a_run_whose_wake_will_not_let_go_is_left_stopped_not_deleted() {
        let _h = TempHome::new();
        test_init("2026-10-19-held", "a goal").unwrap();
        let _wake = crate::liveness::WakeLock::acquire("2026-10-19-held").unwrap();
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() });
        let err = delete_run_waiting("held", &mut exec, std::time::Duration::from_millis(300)).expect_err("a wake holds it");
        assert_eq!(err.code, 2);
        assert!(err.message.contains("is stopped"), "got: {}", err.message);
        assert_eq!(read_run("2026-10-19-held").unwrap().status, Status::Stopped);
    }

    #[test]
    fn a_run_whose_run_json_cannot_be_read_can_still_be_deleted() {
        let _h = TempHome::new();
        test_init("2026-10-19-mangled", "a goal").unwrap();
        std::fs::write(crate::paths::runs_dir().join("2026-10-19-mangled").join("run.json"), "{ not json").unwrap();
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() });
        assert_eq!(delete_run("2026-10-19-mangled", &mut exec).unwrap().was, "unreadable");
        assert!(!crate::paths::runs_dir().join("2026-10-19-mangled").exists());
    }

    #[test]
    fn a_background_wake_of_a_finished_run_is_refused() {
        let _h = TempHome::new();
        test_init("2026-09-20-done", "a goal").unwrap();
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() });
        stop_run("done", None, false, &mut exec).unwrap();
        assert_eq!(wake_in_background("done", None).expect_err("finished").code, 2);
    }

    /// The web page never runs a wake inside the server's request: `Detach::None` from anything
    /// but a terminal is a detached process, as it is for poke.
    #[test]
    fn only_a_terminal_ever_gets_a_foreground_wake() {
        assert_eq!(Caller::Terminal.strategy(Detach::None), Strategy::Foreground);
        assert_eq!(Caller::Terminal.strategy(Detach::Tmux), Strategy::Tmux);
        assert_eq!(Caller::Background.strategy(Detach::None), Strategy::Detached);
        assert_eq!(Caller::Background.strategy(Detach::Tmux), Strategy::Tmux);
    }

    #[test]
    fn a_log_cursor_picks_up_only_what_was_written_since() {
        let _h = TempHome::new();
        test_init("2026-09-20-logs", "a goal").unwrap();
        let mut page = read_logs("logs", &LogQuery::default(), time::UtcOffset::UTC).unwrap();
        assert!(page.lines.iter().any(|l| l.event == "run-created"));
        assert!(page.cursor.poll().is_empty());
        gate_open("2026-09-20-logs");
        let fresh = page.cursor.poll();
        assert!(fresh.iter().any(|l| l.event == "gate-opened"), "got {:?}", fresh.iter().map(|l| &l.event).collect::<Vec<_>>());
    }

    /// A run from before names: an old-style id, and no `name` in its `run.json`.
    fn legacy_run(id: &str) {
        test_init(id, "a goal").unwrap();
        crate::state::transaction(id, |_, state| {
            state.name.clear();
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_rename_is_checked_journaled_and_resolves_at_once() {
        let _h = TempHome::new();
        test_init("7", "a goal").unwrap();
        test_init("8", "a goal").unwrap();
        rename_run("7", "Legenda Picks").unwrap();
        assert_eq!(crate::paths::resolve_run_id("legenda").unwrap(), "7");
        assert_eq!(rename_run("8", "legenda-picks").unwrap_err().code, 2, "taken");
        assert_eq!(rename_run("8", "99").unwrap_err().code, 1, "a number is an id");
        let journal = std::fs::read_to_string(crate::paths::run_dir("7").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("\"event\":\"renamed\"") && journal.contains("legenda-picks"), "{journal}");
    }

    /// The migration: oldest first, numbered after anything already numbered, a short name from
    /// the old slug, the old id kept as an alias, and a repo lock following its run.
    #[test]
    fn renumber_moves_old_runs_onto_numbers_and_keeps_their_old_ids() {
        let _h = TempHome::new();
        legacy_run("2026-09-13-once-an-hour-check-for-tasks-to-complete");
        legacy_run("2026-10-04-once-a-day-find-new-things");
        let fresh = crate::state::commands::init_run(InitArgs { goal: "g".into(), name: Some("fresh".into()), ..Default::default() }).unwrap();
        assert_eq!(fresh, "1");
        let lock = crate::paths::repo_locks_dir().unwrap().join("repo-x.json");
        std::fs::write(&lock, r#"{"repo": "/x", "run": "2026-10-04-once-a-day-find-new-things"}"#).unwrap();

        let report = renumber_runs().unwrap();
        let moved: Vec<(&str, &str)> = report.renumbered.iter().map(|r| (r.id.as_str(), r.name.as_str())).collect();
        assert_eq!(moved, [("2", "once-an-hour"), ("3", "once-a-day")]);
        assert_eq!(crate::paths::all_run_ids(), ["1", "2", "3"]);
        let state = read_run("2").unwrap();
        assert_eq!(state.id, "2");
        assert_eq!(state.former_id.as_deref(), Some("2026-09-13-once-an-hour-check-for-tasks-to-complete"));
        assert_eq!(crate::paths::resolve_run_id("2026-10-04-once-a-day-find-new-things").unwrap(), "3");
        assert!(std::fs::read_to_string(&lock).unwrap().contains("\"run\": \"3\""));
        assert!(renumber_runs().unwrap().renumbered.is_empty(), "a second pass has nothing to do");
    }

    #[test]
    fn renumber_skips_a_run_with_a_wake_in_flight() {
        let _h = TempHome::new();
        legacy_run("2026-09-13-busy");
        let _wake = crate::liveness::WakeLock::acquire("2026-09-13-busy").unwrap();
        let report = renumber_runs().unwrap();
        assert!(report.renumbered.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert!(read_run("2026-09-13-busy").is_ok(), "left exactly where it was");
    }
}
