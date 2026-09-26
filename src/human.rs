//! The commands a person uses in a terminal. `otto state` remains the machine surface a wake
//! writes through; what each command *does* lives in `core`, shared with the web server, and
//! everything here is how it reads in a terminal: formatting, and the one interactive prompt.
//!
//! This is where v2 differs most visibly from v1. A run used to be driven from *inside* a Claude
//! Code session: you typed `/otto start`, and answering a gate meant attaching to tmux and
//! replying to an `AskUserQuestion` dialog — which is why v1 required every gate to end a turn
//! inside that tool, and why a session manager had to classify a pane to notice a run needed you.
//!
//! Here a gate is a row in `otto ls` and a question `otto show` prints. Nothing reads a terminal
//! to find out a run is waiting, and `otto answer` works from a script, over ssh, or from a phone.
//! tmux becomes somewhere to *look* rather than the interface.

use crate::core::{Caller, LogQuery};
use crate::error::OttoError;
use crate::liveness::{Liveness, LockLiveness};
use crate::state::commands::InitArgs;
use crate::state::{read_run, CheckResult, Detach};
use std::io::{IsTerminal, Write as _};

#[cfg(test)]
use crate::core::{
    blocked_explanation, blocking, humanise, implied_run, logs_header, parse_line, parse_since, Filter, Line, Renderer,
    DECISION_EVENTS,
};
#[cfg(test)]
use crate::state::{BlockedCause, Status};

pub(crate) use crate::core::detach_of;

// ---------------------------------------------------------------------------
// otto run
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    #[command(flatten)]
    pub init: InitArgs,
    /// Watch every wake in this terminal instead of detaching it (persists for the run)
    #[arg(long, help_heading = "Where it runs")]
    pub watch: bool,
    /// Print the id the run would get and the command its first wake would run, and create nothing
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(mut args: RunArgs) -> Result<(), OttoError> {
    if args.watch {
        // Persisted through `init`, so every later wake is foreground too — otherwise `--watch`
        // would quietly mean "watch the first one and detach the rest".
        args.init.detach = Detach::None;
    }
    let detach = args.init.detach;
    if args.dry_run {
        let planned = crate::core::plan_run(&args.init)?;
        println!("{}", planned.id);
        println!("{}", planned.first_wake);
        return Ok(());
    }
    let id = crate::core::create_run(args.init)?;
    println!("{id}");
    start_wake(&id, detach, None)
}

/// `otto wake <id>` — force one wake now, backgrounded however the run asks for.
///
/// Same detach behaviour as `otto run` and `otto answer`, and for the same reason: where a run's
/// wakes go is a property of the run, so every way of starting one should agree. A bare
/// `otto wake` therefore honours the run's recorded `detach` and returns, `--watch` keeps it in
/// this terminal, and `--detach` overrides the run's setting for this wake only — deliberately
/// without persisting it, unlike `otto run --watch`, because forcing one wake somewhere is not a
/// statement about the next twenty.
pub fn wake_command(mut args: crate::wake::WakeArgs) -> Result<(), OttoError> {
    // Resolve a prefix/slug to the real id first, and use that from here on — `spawner::start`
    // and `LockLiveness` name a tmux session directly from whatever string they're handed, never
    // through `paths::run_dir`, so an unresolved abbreviation would silently name the wrong
    // session.
    args.id = crate::paths::resolve_run_id(&args.id)?;
    // Read the run before deciding, so an unknown id says so straight away rather than starting a
    // tmux session whose child dies and reporting it fifteen seconds later as a timeout.
    let state = read_run(&args.id)?;
    match wake_destination(&args, &state) {
        // Pass the original args through rather than going via `start_wake`, which builds fresh
        // ones — `--dry-run` and `--answer` both have to survive.
        Detach::None => crate::wake::wake(args),
        Detach::Tmux => {
            crate::core::refuse_if_busy(&args.id)?;
            start_wake(&args.id, Detach::Tmux, args.answer)
        }
    }
}

/// Where an `otto wake` invocation should put the wake. Pure, because the precedence is the part
/// worth testing and everything around it is process plumbing.
///
/// `Detach::None` means "in this process". Four things force it:
///
/// - `--foreground`, because the caller is already the container (see `WakeArgs`);
/// - `--watch`, because a person asked to watch it;
/// - `--dry-run`, which prints a command and changes nothing;
/// - a terminal run, which prints "nothing to do" and changes nothing.
///
/// The last two are the same point: a wake that is never going to happen should explain itself to
/// the person who asked, not to a tmux session that exits before they can attach — and
/// `spawner::wait_for_hold` would report that silence as a timeout, which is a worse answer than
/// the real one.
fn wake_destination(args: &crate::wake::WakeArgs, state: &crate::state::RunState) -> Detach {
    if args.foreground || args.watch || args.dry_run || state.status.is_terminal() {
        return Detach::None;
    }
    // A per-wake override beats the run's standing preference, and does not overwrite it: forcing
    // one wake somewhere is not a statement about the next twenty.
    args.detach.unwrap_or(state.launcher.detach)
}

/// Start a wake, in the foreground or detached. The only difference is where it runs; the run
/// cannot tell which was used. `Detach::None` from a human command means *watch it here* —
/// `spawner::Strategy::Foreground` — never the detached-background strategy poke uses for the
/// same preference; see `spawner`'s module doc for why those used to be conflated.
fn start_wake(id: &str, detach: Detach, answer: Option<String>) -> Result<(), OttoError> {
    let started = crate::core::start_wake(id, Caller::Terminal.strategy(detach), answer.as_deref())?;
    if let Some(note) = started.note {
        println!("{note}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// otto ls
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct LsArgs {
    /// Include runs that have finished
    #[arg(long)]
    pub all: bool,
}

pub fn ls(args: LsArgs) -> Result<(), OttoError> {
    let view = crate::core::list_runs(args.all)?;
    let rows = &view.rows;
    if rows.is_empty() {
        println!("no runs{}", if args.all { "" } else { " (--all includes finished ones)" });
        return Ok(());
    }
    let width = rows.iter().map(|r| r.id.len()).max().unwrap_or(2).max(2);
    // SHORT is what to type back: every command otto prints uses it, and every `<id>` accepts it.
    let short_width = rows.iter().map(|r| r.short.len()).max().unwrap_or(5).max("SHORT".len());
    let status_width = rows.iter().map(|r| r.status.len()).max().unwrap_or(6).max("awaiting_human".len());
    let waiting = rows.iter().map(|r| r.blocking.len()).max().unwrap_or(10).max("WAITING ON".len());
    let next_width = rows.iter().map(|r| r.next_wake.chars().count()).max().unwrap_or(9).max("NEXT WAKE".len());
    let period_width = rows.iter().map(|r| r.period.chars().count()).max().unwrap_or(6).max("PERIOD".len());
    println!(
        "{:<width$}  {:<short_width$}  {:<status_width$}  {:<12}  {:<waiting$}  {:<next_width$}  {:<period_width$}  WAKES",
        "ID", "SHORT", "STATUS", "PHASE", "WAITING ON", "NEXT WAKE", "PERIOD"
    );
    for row in rows {
        // `—` is one column wide but three bytes, and `{:<n}` pads by chars, so this lines up.
        println!(
            "{:<width$}  {:<short_width$}  {:<status_width$}  {:<12}  {:<waiting$}  {:<next_width$}  {:<period_width$}  {}",
            row.id, row.short, row.status, row.phase, row.blocking, row.next_wake, row.period, row.wakes
        );
    }
    if let Some(sleeping_count) = view.stranded_sleepers {
        println!(
            "\nwarning: {sleeping_count} run(s) are sleeping on a timer, but the reviver is \
             not registered — `otto agent start` to fix, or they will never wake on their own"
        );
    }
    if !view.needs_you.is_empty() {
        println!("\nneeds you:");
        for needs in &view.needs_you {
            let (id, label) = (&needs.short, &needs.label);
            match needs.options.as_slice() {
                [] => println!("  otto answer {id} --choice <option>   # {label} — `otto show {id}` for the question"),
                [only] => println!("  otto answer {id} --choice \"{only}\"   # {label}"),
                [first, rest @ ..] => {
                    let alts: Vec<String> = rest.iter().map(|o| format!("\"{o}\"")).collect();
                    println!("  otto answer {id} --choice \"{first}\"   # {label} (or {})", alts.join(" / "));
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// otto show
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct ShowArgs {
    /// The run: its id, a prefix of it, or its slug. Omitted, the one run waiting on you — or the
    /// only live run, if there is just one
    pub id: Option<String>,
}

/// Everything needed to answer a gate cold, in one screen: where the run stands, what it last
/// did, and the question in full.
pub fn show(args: ShowArgs) -> Result<(), OttoError> {
    let detail = crate::core::run_detail(args.id.as_deref())?;
    let state = &detail.state;
    // The full id leads the screen; the commands below it use the short form `otto ls` shows.
    let short = &detail.short;

    println!("{}  ({:?})", state.id, state.status);
    println!("  goal      {}", state.goal);
    match &state.done_condition {
        Some(done) => println!("  done when {done}"),
        None if state.policy.perpetual => {
            println!("  done when never — perpetual; retire it with `otto stop {short}`")
        }
        None => println!("  done when (not yet decided — the next wake proposes one and asks)"),
    }
    println!("  wraps     {} {}", detail.wraps_kind, state.wraps.reference.as_deref().unwrap_or(""));
    println!("  phase     {}", state.phase);
    if let Some(wake) = &state.wake {
        println!(
            "  wake      {} — {}{}",
            wake.n,
            if detail.running { "running now" } else { "finished" },
            wake.outcome
                .map(|o| format!(", {}", format!("{o:?}").to_lowercase()))
                .unwrap_or_default()
        );
    }
    // No dollar figure: nothing measures one without `-p`. Per-wake tokens are journaled, so
    // `otto logs` is where a question about what a wake used gets answered.
    print!("  used      {} wake(s)", state.budget.spent_wakes);
    if state.budget.wakes > 0 {
        print!(" of {}", state.budget.wakes);
    }
    if state.budget.hours > 0 {
        print!(", {}h budget", state.budget.hours);
    }
    println!();
    if state.incomplete_wakes > 0 {
        println!("  failed    {} wake(s) in a row did not finish", state.incomplete_wakes);
    }
    if !state.status.is_terminal() {
        match state.next_wake_at {
            Some(at) if !detail.running => {
                println!("  next wake {} ({})", crate::clock::due(at), crate::clock::local_clock(at))
            }
            _ if detail.running => println!("  next wake set when this wake finishes"),
            _ if state.gate.is_some() => println!("  next wake after the gate is answered"),
            _ => println!("  next wake nothing scheduled"),
        }
        println!("  period    every {}", crate::clock::format_minutes(state.policy.period_minutes));
    }
    if !state.status.is_terminal() {
        print_check_summary(&detail);
    }
    if let Some(explanation) = &detail.blocked_explanation {
        println!("\n{explanation}");
    }
    if !detail.notes.is_empty() {
        println!();
        print_notes(&detail.notes);
    }

    if let Some(gate) = &detail.gate {
        println!("\n─── open gate {} — {} ───\n", gate.id, gate.slug);
        match &gate.text {
            Some(text) => println!("{}", text.trim_end()),
            None => println!("(gate file {} is missing)", gate.file),
        }
        println!("\nAnswer it with:");
        if gate.options.is_empty() {
            println!("  otto answer {short} --choice <option>");
        } else {
            for option in &gate.options {
                let mark = if gate.default.as_deref() == Some(option) { "   # default" } else { "" };
                println!("  otto answer {short} --choice \"{option}\"{mark}");
            }
        }
        println!("  otto answer {short} --text \"…\"");
    } else {
        match &detail.handoff {
            Some(text) => println!("\n─── handoff ───\n{}", text.trim_end()),
            None => println!("\n(no handoff yet)"),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// otto answer
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct AnswerArgs {
    /// The run: its id, a prefix of it, or its slug. Omitted, the one run waiting on you
    pub id: Option<String>,
    /// The option you are choosing, by name — checked against the gate's own options when it
    /// lists any, and rejected (naming the valid ones) if it matches none
    #[arg(long, conflicts_with_all = ["text", "file"])]
    pub choice: Option<String>,
    /// Prose, when the decision needs more than an option name
    #[arg(long, conflicts_with = "file")]
    pub text: Option<String>,
    /// Read the answer from a file
    #[arg(long)]
    pub file: Option<String>,
    /// Record the answer but don't wake the run yet
    #[arg(long = "no-wake")]
    pub no_wake: bool,
}

/// Print the question and, when the gate lists options, a numbered menu; read one line from
/// stdin and resolve it with `resolve_typed`. Only reached on a TTY (see `answer`); otherwise
/// there is nobody to read a menu.
fn interactive_prompt(question: &str, options: &[String], default: Option<&str>) -> Result<String, OttoError> {
    println!("{}\n", question.trim());
    if options.is_empty() {
        print!("your answer: ");
    } else {
        for (i, option) in options.iter().enumerate() {
            let mark = if default == Some(option.as_str()) { "  (default)" } else { "" };
            println!("  {}) {option}{mark}", i + 1);
        }
        match default {
            Some(name) => print!("choose 1-{}, Enter for {name}, or type an answer: ", options.len()),
            None => print!("choose 1-{}, or type an answer: ", options.len()),
        }
    }
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    resolve_typed(line.trim(), options, default)
}

/// What one typed line at the prompt means. A number picks that option, text matching an
/// option's name picks it too (both case-insensitively), an empty line takes the default when the
/// gate names one, and anything else is taken as free text — so someone who wants to write more
/// than an option name still can.
fn resolve_typed(line: &str, options: &[String], default: Option<&str>) -> Result<String, OttoError> {
    if line.is_empty() {
        return match default {
            Some(name) => Ok(name.to_string()),
            None => Err(OttoError::usage("no answer given")),
        };
    }
    if let Ok(n) = line.parse::<usize>() {
        if n >= 1 && n <= options.len() {
            return Ok(options[n - 1].clone());
        }
    }
    if let Some(matched) = options.iter().find(|o| o.eq_ignore_ascii_case(line)) {
        return Ok(matched.clone());
    }
    Ok(line.to_string())
}

/// `stdin().is_terminal()` reflects the *real* process's TTY, which a test binary inherits from
/// whatever shell ran `cargo test` — so under test this is always false, or an interactive
/// terminal makes `answer` block on `interactive_prompt` reading input nothing is typing.
fn stdin_is_terminal() -> bool {
    !cfg!(test) && std::io::stdin().is_terminal()
}

pub fn answer(args: AnswerArgs) -> Result<(), OttoError> {
    let pending = crate::core::pending_gate(args.id.as_deref())?;
    let (id, short) = (&pending.id, &pending.short);
    let options = &pending.options;

    // Resolve the answer once, whichever way it arrived, so there is exactly one reading of it:
    // what gets recorded verbatim and what the wake sees are the same string.
    let answer = match (&args.choice, &args.text, &args.file) {
        (Some(choice), _, _) => crate::core::resolve_choice(choice, options)?,
        (_, Some(text), _) => text.clone(),
        (_, _, Some(path)) => {
            std::fs::read_to_string(path).map_err(|e| OttoError::usage(format!("cannot read {path}: {e}")))?
        }
        (None, None, None) => {
            if stdin_is_terminal() {
                let default = crate::gate::parse_default(&pending.question, options);
                interactive_prompt(&pending.question, options, default.as_deref())?
            } else if options.is_empty() {
                return Err(OttoError::usage(
                    "give the answer: --choice <option>, --text \"…\", or --file <path>".to_string(),
                ));
            } else {
                return Err(OttoError::usage(format!(
                    "give the answer: --choice <option> ({}), --text \"…\", or --file <path>",
                    options.join(", ")
                )));
            }
        }
    };

    crate::core::record_answer(&pending, &answer)?;
    println!("recorded against gate {} ({})", pending.gate_id, pending.slug);

    if args.no_wake {
        println!("not waking it — `otto wake {short}` when you want it to continue");
        return Ok(());
    }
    if LockLiveness.probe(id).is_busy() {
        println!("a wake is still finishing — waiting for it before continuing the run");
    }
    // The answer is already on disk, so if a wake is somehow still running after the wait, saying
    // so is better than failing: poke will carry the run on from here either way.
    if !crate::core::wait_for_wake_to_finish(id) {
        println!(
            "a wake is still running after {}s — your answer is recorded, and the \
             run will act on it. `otto ls` to watch, or `otto wake {short}` once it is idle.",
            crate::spawner::CONFIRM_SECONDS
        );
        return Ok(());
    }
    // Hand the answer to the wake as well as recording it: the wake must see the person's own
    // words, not a summary of them.
    start_wake(id, detach_of(id), Some(answer))
}

// ---------------------------------------------------------------------------
// otto note
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct NoteArgs {
    /// The run: its id, a prefix of it, or its slug
    pub id: String,
    /// What to tell it, verbatim. Guidance on how to do the work — not an answer to a gate
    #[arg(conflicts_with = "file")]
    pub text: Option<String>,
    /// Read the note from a file
    #[arg(long)]
    pub file: Option<String>,
    /// Give it to every wake until dropped, not just the next one to complete
    #[arg(long)]
    pub standing: bool,
    /// Wake the run now to read it, rather than waiting for its next wake
    #[arg(long)]
    pub now: bool,
    /// Show the run's standing notes and the ones not yet delivered
    #[arg(long, conflicts_with_all = ["text", "file", "standing", "now", "drop"])]
    pub list: bool,
    /// Withdraw an undelivered note, or retire a standing one, by number
    #[arg(long, value_name = "NOTE", conflicts_with_all = ["text", "file", "standing", "now"])]
    pub drop: Option<String>,
}

pub fn note(args: NoteArgs) -> Result<(), OttoError> {
    if let Some(which) = &args.drop {
        let dropped = crate::core::drop_note(&args.id, which)?;
        let kind = if dropped.standing { "standing note" } else { "note" };
        println!("dropped {kind} {} — no wake will be given it again", dropped.id);
        return Ok(());
    }
    if args.list {
        let detail = crate::core::run_detail(Some(&args.id))?;
        if detail.notes.is_empty() {
            println!("no standing or undelivered notes — `otto logs {} --event note-added` for past ones", detail.short);
        }
        print_notes(&detail.notes);
        return Ok(());
    }
    let text = match (&args.text, &args.file) {
        (Some(text), _) => text.clone(),
        (_, Some(path)) => std::fs::read_to_string(path).map_err(|e| OttoError::usage(format!("cannot read {path}: {e}")))?,
        (None, None) => return Err(OttoError::usage("give the note: `otto note <run> \"…\"`, or --file <path>")),
    };
    let outcome = crate::core::add_note(&args.id, &text, args.standing)?;
    println!("note {} recorded — {}", outcome.note.id, outcome.delivery);
    if !args.now {
        return Ok(());
    }
    let state = read_run(&outcome.id)?;
    match crate::core::why_not_wake_for_note(&state, LockLiveness.probe(&outcome.id).is_busy()) {
        Some(why) => {
            println!("not waking it: {why}");
            Ok(())
        }
        None => start_wake(&outcome.id, state.launcher.detach, None),
    }
}

/// The notes section `otto show` and `otto note --list` share.
fn print_notes(notes: &[crate::core::NoteView]) {
    for (i, note) in notes.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let kind = if note.standing {
            "standing".to_string()
        } else {
            match note.given_to_wake {
                Some(n) => format!("given to wake {n}, not yet delivered"),
                None => "not yet read".to_string(),
            }
        };
        println!("── note {} · {kind} · added {} ──", note.id, crate::clock::relative(note.added_at));
        println!("{}", note.text.trim_end());
    }
}

// ---------------------------------------------------------------------------
// otto check
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct CheckArgs {
    /// The run: its id, a prefix of it, or its slug
    pub id: String,
    /// A script poke runs directly, no model involved, whenever the run's period comes due:
    /// exit 0 = nothing new (no wake is spent; it sleeps another period), 1 = changed (a wake),
    /// anything else = a wake too. It starts with a #! line, and runs under launchd's PATH — use
    /// absolute paths for anything outside /usr/bin and Homebrew
    #[arg(long, value_name = "FILE")]
    pub script: Option<String>,
    /// The safety net: wake anyway after this many "nothing new" results in a row, in case the
    /// script is wrong without failing. 0 turns it off
    #[arg(long = "wake-after", requires = "script", value_name = "N", default_value_t = crate::state::DEFAULT_CHECK_WAKE_AFTER)]
    pub wake_after: u32,
    /// Remove the run's check script; every wake is a full one again
    #[arg(long, conflicts_with = "script")]
    pub off: bool,
}

pub fn check(args: CheckArgs) -> Result<(), OttoError> {
    if args.off {
        let script = crate::core::clear_check(&args.id)?;
        println!("removed the check script ({script} is left on disk) — every wake is a full one again");
        return Ok(());
    }
    let Some(path) = &args.script else {
        let detail = crate::core::run_detail(Some(&args.id))?;
        print_check_summary(&detail);
        if let Some(text) = detail.check.as_ref().and_then(|c| c.text.as_ref()) {
            println!("\n{}", text.trim_end());
        }
        return Ok(());
    };
    let script = std::fs::read_to_string(path).map_err(|e| OttoError::usage(format!("cannot read {path}: {e}")))?;
    let outcome = crate::core::set_check(&args.id, &script, args.wake_after)?;
    let period = crate::clock::format_minutes(outcome.period_minutes);
    println!("check set: every {period}, poke runs {} before waking the run", outcome.check.script);
    match outcome.check.wake_after {
        0 => println!("no safety net: only a change or an error wakes it"),
        n => println!("safety net: after {n} \"nothing new\" results in a row it wakes anyway"),
    }
    // Once now, so a broken script shows here rather than when the run is next due.
    let tried = crate::core::try_check(&outcome.id)?;
    let said = tried.note.as_deref().map(|n| format!(" — {n}")).unwrap_or_default();
    match tried.result {
        CheckResult::NoChange => println!("tried it now: exit 0, nothing new{said}"),
        CheckResult::Changed => println!("tried it now: exit 1, changed{said} (not acted on — the run wakes when it is due)"),
        CheckResult::Error => println!(
            "tried it now: it FAILED (exit {}{}){said} — poke would wake the run every time; fix the script",
            tried.code,
            if tried.timed_out { ", timed out" } else { "" }
        ),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// otto period
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct PeriodArgs {
    /// The run: its id, a prefix of it, or its slug
    pub id: String,
    /// How often it wakes when a wake doesn't ask for something else: `30m`, `4h`, `1d`. Measured
    /// from the start of one wake to the start of the next. Omit it to see the current period
    #[arg(value_name = "DURATION")]
    pub period: Option<String>,
}

pub fn period(args: PeriodArgs) -> Result<(), OttoError> {
    let Some(text) = &args.period else {
        let state = crate::state::read_run(&args.id)?;
        println!("{} wakes every {}", state.id, crate::clock::format_minutes(state.policy.period_minutes));
        return Ok(());
    };
    let minutes = parse_duration("the period", text)?.whole_minutes();
    if minutes < 1 {
        return Err(OttoError::usage("the period must be at least 1m"));
    }
    let outcome = crate::core::set_period(&args.id, minutes as u64)?;
    println!(
        "{} now wakes every {} (was {})",
        outcome.id,
        crate::clock::format_minutes(outcome.period_minutes),
        crate::clock::format_minutes(outcome.previous_minutes)
    );
    match (outcome.rescheduled, outcome.next_wake_at) {
        (true, Some(at)) if at.is_past() => println!("its period has already passed — the next poke wakes it"),
        (true, Some(at)) => println!("the next wake moved to {} ({})", crate::clock::due(at), crate::clock::local_clock(at)),
        (false, Some(at)) => println!(
            "the next wake stays {} ({}) — a wake armed that timer itself; the new period starts after it",
            crate::clock::due(at),
            crate::clock::local_clock(at)
        ),
        _ => println!("it applies from the end of the next wake"),
    }
    Ok(())
}

fn parse_duration(flag: &str, text: &str) -> Result<time::Duration, OttoError> {
    crate::clock::parse_age(text)
        .ok_or_else(|| OttoError::usage(format!("{flag} takes a duration like 15m, 1h or 1d, not \"{text}\"")))
}

/// The check line `otto show` and `otto check` share. With no check it says so, and what that is
/// costing: every wake a full model session, when a script could answer most of them.
fn print_check_summary(detail: &crate::core::RunDetail) {
    let cost = detail.wake_cost.as_ref().map(|c| {
        format!(
            "~{} turns, {} cache-creation tokens a wake over the last {}",
            c.avg_turns,
            crate::core::humanise(c.avg_cache_creation),
            c.wakes
        )
    });
    let Some(check) = &detail.check else {
        println!(
            "  check     none — every wake is a full model session{}",
            cost.map(|c| format!(" ({c})")).unwrap_or_default()
        );
        println!(
            "            a script poke runs before each wake, spending one only when something changed:\n            \
             `otto check {0} --script <file>`",
            detail.short
        );
        return;
    };
    let last = match (check.last_result, check.last_at) {
        (Some(result), Some(at)) => {
            let what = match result {
                CheckResult::NoChange => "no change",
                CheckResult::Changed => "changed",
                CheckResult::Error => "errored",
            };
            let said = check.last_note.as_deref().map(|n| format!(" — {n}")).unwrap_or_default();
            let when = match crate::clock::relative(at) {
                now if now == "now" => "just now".to_string(),
                ago => ago,
            };
            format!("{what} {when}{said}")
        }
        (Some(CheckResult::NoChange), None) => "no change".to_string(),
        (Some(CheckResult::Changed), None) => "changed".to_string(),
        (Some(CheckResult::Error), None) => "errored".to_string(),
        (None, _) => "not run yet".to_string(),
    };
    println!(
        "  check     {} before each wake{} — last: {last}",
        check.script,
        match (check.pinned, check.set_by_wake) {
            (true, false) => ", set by you",
            (true, true) => ", set by the run (`otto check --script` to replace it with your own)",
            (false, _) => ", set by a wake for this sleep",
        },
    );
    println!(
        "            {} check(s) found nothing, each a wake not spent{}",
        check.no_change_total,
        cost.map(|c| format!(" ({c})")).unwrap_or_default()
    );
    match check.wake_after {
        0 => println!("            no safety net"),
        n => println!(
            "            safety net: wakes anyway after {n} in a row ({} so far)",
            check.consecutive_no_change
        ),
    }
}

// ---------------------------------------------------------------------------
// otto logs
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct LogsArgs {
    /// The run: its id, a prefix of it, or its slug
    pub id: String,
    /// Trailing lines to show, like `tail -n` [default: 40, or everything with --since]
    #[arg(long, short = 'n')]
    pub lines: Option<usize>,
    /// Only lines from this long ago (`45m`, `2h`, `3d`) or since a date/time (`2026-09-15`,
    /// `2026-09-15T07:00Z`)
    #[arg(long, value_name = "AGE|WHEN")]
    pub since: Option<String>,
    /// Only these events, comma-separated (`gate-opened,gate-closed`)
    #[arg(long, value_name = "NAMES", value_delimiter = ',')]
    pub event: Vec<String>,
    /// Only the turning points: gates, status and phase changes, budgets, wakes that failed
    #[arg(long)]
    pub decisions: bool,
    /// Keep printing as the run writes more, like `tail -f`
    #[arg(long, short)]
    pub follow: bool,
}

pub fn logs(args: LogsArgs) -> Result<(), OttoError> {
    // First, before anything could start a thread: see `clock::local_offset`.
    let offset = crate::clock::local_offset();
    let query = LogQuery { lines: args.lines, since: args.since, events: args.event, decisions: args.decisions };
    let mut page = crate::core::read_logs(&args.id, &query, offset)?;
    println!("{}", page.header);
    for line in &page.lines {
        println!("{}", line.text);
    }
    if !args.follow {
        return Ok(());
    }
    // Poll rather than notify: a journal is append-only and a run writes to it a few times a
    // minute at most, so this costs nothing and needs no platform-specific watching.
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        for line in page.cursor.poll() {
            println!("{}", line.text);
        }
    }
}

// ---------------------------------------------------------------------------
// otto stop / resume / attach
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct StopArgs {
    /// The run: its id, a prefix of it, or its slug
    pub id: String,
    /// Why, recorded in the journal
    #[arg(long)]
    pub reason: Option<String>,
    /// The run ended because it could not do its job, rather than simply no longer being wanted
    #[arg(long)]
    pub failed: bool,
}

pub fn stop(args: StopArgs) -> Result<(), OttoError> {
    let mut exec = crate::exec::RealExec;
    stop_with(args, &mut exec)
}

/// The body, with the killer's `Exec` injected so the kill path is testable without shelling out.
fn stop_with(args: StopArgs, exec: &mut dyn crate::exec::Exec) -> Result<(), OttoError> {
    let outcome = crate::core::stop_run(&args.id, args.reason, args.failed, exec)?;
    if outcome.killed_wake {
        println!("killed the live wake for {}", outcome.id);
    }
    println!("{} is {:?} — {}", outcome.id, outcome.status, outcome.reason);
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct ResumeArgs {
    /// The run: its id, a prefix of it, or its slug
    pub id: String,
    /// Why, recorded in the journal
    #[arg(long)]
    pub reason: Option<String>,
    /// Put it back on its schedule without waking it now
    #[arg(long)]
    pub no_wake: bool,
}

/// Undo `otto stop`. The wake goes wherever the run's wakes go — `otto attach` to watch it.
pub fn resume(args: ResumeArgs) -> Result<(), OttoError> {
    let outcome = crate::core::resume_run(&args.id, args.reason, args.no_wake)?;
    println!("{} is {:?} — {}", outcome.id, outcome.status, outcome.reason);
    match (&outcome.wake, &outcome.note) {
        (Some(wake), _) => println!("{}", wake.note.as_deref().unwrap_or("wake started")),
        (None, Some(note)) => println!("{note}"),
        (None, None) => {}
    }
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct AttachArgs {
    /// The run: its id, a prefix of it, or its slug
    pub id: String,
}

pub fn attach(args: AttachArgs) -> Result<(), OttoError> {
    let id = crate::paths::resolve_run_id(&args.id)?;
    crate::detach::attach(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;
    use crate::state::commands::{open_gate, test_init, OpenGateArgs};
    use crate::state::transaction;

    fn gate_open(id: &str, slug: &str) {
        open_gate(OpenGateArgs {
            id: id.to_string(),
            slug: slug.to_string(),
            question: Some("Approve the plan?\n\n- Approve\n- Revise".to_string()),
            question_file: None,
            stdin: false,
            expires_at: None,
            expires_in: None,
        })
        .unwrap();
    }

    fn wake_args(id: &str) -> crate::wake::WakeArgs {
        crate::wake::WakeArgs {
            id: id.to_string(),
            answer: None,
            dry_run: false,
            watch: false,
            detach: None,
            foreground: false,
        }
    }

    /// The point of the change: every way of starting a wake agrees on where it goes, so a bare
    /// `otto wake` lands wherever `otto run` and `otto answer` would have put it.
    #[test]
    fn a_bare_wake_goes_where_the_run_asks() {
        let _h = TempHome::new();
        test_init("w-dest", "a goal").unwrap();
        let mut state = read_run("w-dest").unwrap();

        state.launcher.detach = Detach::Tmux;
        assert_eq!(wake_destination(&wake_args("w-dest"), &state), Detach::Tmux);
        state.launcher.detach = Detach::None;
        assert_eq!(wake_destination(&wake_args("w-dest"), &state), Detach::None);
    }

    #[test]
    fn an_explicit_detach_beats_the_runs_preference() {
        let _h = TempHome::new();
        test_init("w-over", "a goal").unwrap();
        let mut state = read_run("w-over").unwrap();
        state.launcher.detach = Detach::None;

        let mut args = wake_args("w-over");
        args.detach = Some(Detach::Tmux);
        assert_eq!(wake_destination(&args, &state), Detach::Tmux);

        state.launcher.detach = Detach::Tmux;
        args.detach = Some(Detach::None);
        assert_eq!(wake_destination(&args, &state), Detach::None);
    }

    /// Each of these would otherwise print something a person needs into a tmux session that exits
    /// before they can attach — or, for `--foreground`, spawn a container inside a container.
    #[test]
    fn anything_that_must_be_seen_stays_in_this_process() {
        let _h = TempHome::new();
        test_init("w-here", "a goal").unwrap();
        let mut state = read_run("w-here").unwrap();
        state.launcher.detach = Detach::Tmux;

        for (what, mutate) in [
            ("--watch", (|a: &mut crate::wake::WakeArgs| a.watch = true) as fn(&mut crate::wake::WakeArgs)),
            ("--dry-run", |a| a.dry_run = true),
            ("--foreground", |a| a.foreground = true),
        ] {
            let mut args = wake_args("w-here");
            mutate(&mut args);
            assert_eq!(wake_destination(&args, &state), Detach::None, "{what} must run here");
        }

        // A finished run explains itself instead of waking, and that explanation is for the person
        // who just typed the command.
        state.status = Status::Done;
        assert_eq!(wake_destination(&wake_args("w-here"), &state), Detach::None);
    }

    #[test]
    fn answering_records_the_choice_verbatim_and_clears_the_gate() {
        let _h = TempHome::new();
        test_init("h-answer", "a goal").unwrap();
        gate_open("h-answer", "plan-review");
        answer(AnswerArgs {
            id: Some("h-answer".into()),
            choice: Some("Approve".into()),
            text: None,
            file: None,
            no_wake: true,
        })
        .unwrap();
        let state = read_run("h-answer").unwrap();
        assert!(state.gate.is_none(), "an answered gate must be closed");
        let journal = std::fs::read_to_string(crate::paths::run_dir("h-answer").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("gate-closed"));
        assert!(journal.contains("Approve"));
        let gate_file =
            std::fs::read_to_string(crate::paths::run_dir("h-answer").unwrap().join("gates/001-plan-review.md")).unwrap();
        assert!(gate_file.contains("Approve"), "the gate file keeps the answer verbatim");
    }

    /// The whole point of parsing a gate's options: a typo should not burn a wake before anyone
    /// finds out it was not what was meant.
    #[test]
    fn a_choice_that_matches_no_option_is_rejected_naming_the_real_ones() {
        let _h = TempHome::new();
        test_init("h-typo", "a goal").unwrap();
        gate_open("h-typo", "plan-review");
        let err = answer(AnswerArgs {
            id: Some("h-typo".into()),
            choice: Some("aprove".into()),
            text: None,
            file: None,
            no_wake: true,
        })
        .expect_err("must refuse");
        assert_eq!(err.code, 1);
        assert!(err.to_string().contains("Approve"), "got: {err}");
        assert!(err.to_string().contains("Revise"), "got: {err}");
        // The gate must survive a rejected choice, same as a malformed answer.
        assert!(read_run("h-typo").unwrap().gate.is_some());
    }

    /// `--choice approve` should work even though the gate names it `Approve` — and the journal
    /// should show the gate's own casing, not whatever a person typed.
    #[test]
    fn a_choice_matches_case_insensitively_and_records_the_gates_own_casing() {
        let _h = TempHome::new();
        test_init("h-case", "a goal").unwrap();
        gate_open("h-case", "plan-review");
        answer(AnswerArgs {
            id: Some("h-case".into()),
            choice: Some("approve".into()),
            text: None,
            file: None,
            no_wake: true,
        })
        .unwrap();
        let gate_file =
            std::fs::read_to_string(crate::paths::run_dir("h-case").unwrap().join("gates/001-plan-review.md")).unwrap();
        assert!(gate_file.contains("(verbatim)\n\nApprove"), "got: {gate_file}");
    }

    /// A gate whose question is free prose with no parseable option list must still accept any
    /// choice — most gates, from an arbitrary wrapped skill, look like this.
    #[test]
    fn a_gate_with_no_parseable_options_accepts_any_choice() {
        let _h = TempHome::new();
        test_init("h-prose", "a goal").unwrap();
        crate::state::ops::open_gate("h-prose", "review", "Does this look right? Say yes or no.", None).unwrap();
        answer(AnswerArgs {
            id: Some("h-prose".into()),
            choice: Some("yes".into()),
            text: None,
            file: None,
            no_wake: true,
        })
        .unwrap();
        assert!(read_run("h-prose").unwrap().gate.is_none());
    }

    /// Improvement 1's concrete failure mode: a short id must resolve to the *real* run before
    /// anything that names a tmux session directly from the string it's given (never through
    /// `paths::run_dir`) gets to see it — otherwise it goes looking for a session that was never
    /// named that.
    #[test]
    fn stopping_a_run_by_a_short_prefix_kills_the_real_sessions_name() {
        let _h = TempHome::new();
        test_init("2026-09-11-verify-1789130664", "a goal").unwrap();
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 1, ..Default::default() }); // has-session: no
        stop_with(StopArgs { id: "verify".into(), reason: None, failed: false }, &mut exec).unwrap();
        assert_eq!(read_run("2026-09-11-verify-1789130664").unwrap().status, Status::Stopped);
        let calls: Vec<String> = exec.calls.borrow().iter().map(|c| c.join(" ")).collect();
        assert!(
            calls.iter().any(|c| c.contains("otto-2026-09-11-verify-1789130664")),
            "must probe the real session name, not one named after the prefix: {calls:?}"
        );
    }

    #[test]
    fn answering_with_no_gate_open_is_refused() {
        let _h = TempHome::new();
        test_init("h-nogate", "a goal").unwrap();
        let err = answer(AnswerArgs {
            id: Some("h-nogate".into()),
            choice: Some("Approve".into()),
            text: None,
            file: None,
            no_wake: true,
        })
        .expect_err("must refuse");
        assert_eq!(err.code, 2);
        assert!(err.to_string().contains("no gate open"));
    }

    #[test]
    fn answering_with_nothing_to_say_is_a_usage_error() {
        let _h = TempHome::new();
        test_init("h-empty", "a goal").unwrap();
        gate_open("h-empty", "review");
        let err = answer(AnswerArgs {
            id: Some("h-empty".into()),
            choice: None,
            text: None,
            file: None,
            no_wake: true,
        })
        .expect_err("must refuse");
        assert_eq!(err.code, 1);
        // The gate must survive a malformed answer, or the question is lost.
        assert!(read_run("h-empty").unwrap().gate.is_some());
    }

    /// Regression: `--detach` used to be handed to the first wake and never stored, so the
    /// second wake — from `otto answer` or from poke — silently fell back to the foreground.
    #[test]
    fn the_detach_preference_survives_the_first_wake() {
        let _h = TempHome::new();
        test_init("h-detach", "a goal").unwrap();
        // `test_init` asks for tmux, and every later wake must still see that.
        assert_eq!(read_run("h-detach").unwrap().launcher.detach, Detach::Tmux);
        assert_eq!(detach_of("h-detach"), Detach::Tmux);
        // Recording a wake must not change it — that circularity was the bug.
        transaction("h-detach", |_p, state| {
            state.wake = Some(crate::state::Wake {
                n: 1,
                started_at: crate::clock::Timestamp::now(),
                deadline_at: crate::clock::Timestamp::now(),
                launcher: "claude".into(),
                pid: None,
                session: None,
                outcome: None,
            });
            Ok(())
        })
        .unwrap();
        assert_eq!(detach_of("h-detach"), Detach::Tmux);
    }

    /// A run with no done-condition yet must still answer the question when asked, rather than
    /// erroring as though the field did not exist.
    #[test]
    fn an_undecided_done_condition_reads_as_null_not_missing() {
        let _h = TempHome::new();
        test_init("h-dc", "a goal").unwrap();
        let value = serde_json::to_value(read_run("h-dc").unwrap()).unwrap();
        assert!(value.get("doneCondition").is_some(), "the field must be present");
        assert!(value["doneCondition"].is_null(), "and null until something decides it");
    }

    #[test]
    fn stopping_a_run_is_stopped_not_failed() {
        let _h = TempHome::new();
        test_init("h-stop", "a goal").unwrap();
        stop(StopArgs {
            id: "h-stop".into(),
            reason: Some("changed my mind".into()),
            failed: false,
        })
        .unwrap();
        assert_eq!(read_run("h-stop").unwrap().status, Status::Stopped);
    }

    /// The bug this fixes: `otto stop` used to know only how to kill a tmux session. A run
    /// created with `--detach none` whose wake was backgrounded by poke (`spawner::Strategy::
    /// Detached`, not tmux) kept running — unsupervised, for up to `maxWakeMinutes` — after
    /// being retired, because nothing ever looked for its pid.
    #[test]
    fn stopping_a_run_kills_a_detached_wake_by_pid_not_just_a_tmux_session() {
        let _h = TempHome::new();
        test_init("h-stop-detached", "a goal").unwrap();
        transaction("h-stop-detached", |_p, state| {
            state.wake = Some(crate::state::Wake {
                n: 1,
                started_at: crate::clock::Timestamp::now(),
                deadline_at: crate::clock::Timestamp::in_minutes(30),
                launcher: "claude".into(),
                pid: Some(313131),
                session: None,
                outcome: None,
            });
            Ok(())
        })
        .unwrap();
        let mut exec = crate::exec::fake::FakeExec::new();
        exec.queue(crate::exec::Output { code: 0, ..Default::default() }); // the `kill -TERM` call
        exec.queue(crate::exec::Output { code: 1, ..Default::default() }); // has-session: no tmux at all
        stop_with(
            StopArgs { id: "h-stop-detached".into(), reason: None, failed: false },
            &mut exec,
        )
        .unwrap();
        assert_eq!(read_run("h-stop-detached").unwrap().status, Status::Stopped);
        let calls: Vec<String> = exec.calls.borrow().iter().map(|c| c.join(" ")).collect();
        assert!(calls.iter().any(|c| c.contains("kill -TERM -313131")), "got {calls:?}");
    }

    #[test]
    fn stopping_with_failed_says_so() {
        let _h = TempHome::new();
        test_init("h-fail", "a goal").unwrap();
        stop(StopArgs {
            id: "h-fail".into(),
            reason: None,
            failed: true,
        })
        .unwrap();
        assert_eq!(read_run("h-fail").unwrap().status, Status::Failed);
    }

    /// The column that answers "which of my runs needs me?".
    #[test]
    fn an_open_gate_names_its_question_in_the_waiting_column() {
        let _h = TempHome::new();
        test_init("h-ls", "a goal").unwrap();
        gate_open("h-ls", "plan-review");
        let state = read_run("h-ls").unwrap();
        assert_eq!(blocking(&state, false), "you: gate 001 plan-review");
    }

    /// A gate does not prove the wake that opened it has exited — a model that keeps working
    /// past its "stop there" instruction leaves the lock held, and someone watching `otto ls`
    /// needs to see that, or they'll trust a wake is idle when it could still race their answer.
    #[test]
    fn a_gate_still_says_so_when_its_wake_has_not_exited() {
        let _h = TempHome::new();
        test_init("h-ls-busy", "a goal").unwrap();
        gate_open("h-ls-busy", "plan-review");
        transaction("h-ls-busy", |_p, state| {
            state.wake = Some(crate::state::Wake {
                n: 1,
                started_at: crate::clock::Timestamp::now(),
                deadline_at: crate::clock::Timestamp::now(),
                launcher: "claude".into(),
                pid: Some(1),
                session: None,
                outcome: None,
            });
            Ok(())
        })
        .unwrap();
        let state = read_run("h-ls-busy").unwrap();
        assert_eq!(blocking(&state, true), "you: gate 001 plan-review (wake 1 still running)");
    }

    #[test]
    fn a_sleeping_run_names_its_timer_and_a_running_one_says_working() {
        let _h = TempHome::new();
        test_init("h-sleep", "a goal").unwrap();
        transaction("h-sleep", |_p, state| {
            state.status = Status::Sleeping;
            state.next_wake_at = Some(crate::clock::Timestamp::in_minutes(24));
            Ok(())
        })
        .unwrap();
        let state = read_run("h-sleep").unwrap();
        assert_eq!(blocking(&state, false), "timer");
        let when = crate::core::next_wake(&state, false);
        // Same one-minute tolerance as `relative_times_read_forwards_and_backwards`.
        assert!(when.starts_with("in 24m (") || when.starts_with("in 23m ("), "got {when}");
        assert_eq!(crate::core::next_wake(&state, true), "—", "a running wake has nothing due yet");
        // A live wake outranks the recorded status: the run is working, whatever it last wrote.
        assert!(blocking(&state, true).starts_with("working"));
    }

    #[test]
    fn a_run_with_nothing_pending_says_so_rather_than_looking_fine() {
        let _h = TempHome::new();
        test_init("h-strand", "a goal").unwrap();
        let state = read_run("h-strand").unwrap();
        assert_eq!(blocking(&state, false), "nothing scheduled");
    }

    /// `otto run --dry-run` is how a flag gets checked before it costs a run: nothing may exist
    /// afterwards, and what it prints is the real first-wake command.
    #[test]
    fn a_dry_run_prints_the_first_wake_and_creates_nothing() {
        let _h = TempHome::new();
        run(RunArgs {
            init: InitArgs { goal: "a goal".into(), slug: Some("dry".into()), repos: vec!["/w/a".into()], ..Default::default() },
            watch: false,
            dry_run: true,
        })
        .unwrap();
        assert!(crate::paths::all_run_ids().is_empty(), "a dry run must create no run");
        // The same refusals apply: a launcher nobody has defined.
        let err = run(RunArgs {
            init: InitArgs { goal: "a goal".into(), launcher: "no-such-launcher".into(), ..Default::default() },
            watch: false,
            dry_run: true,
        })
        .expect_err("must refuse");
        assert!(err.to_string().contains("no launcher"));
        assert!(crate::paths::all_run_ids().is_empty());
    }

    /// Every flag a person sees on `otto run --help` says what it is for. Nine used to say
    /// nothing at all.
    #[test]
    fn every_visible_run_flag_has_help() {
        use clap::CommandFactory;
        let cli = crate::cli::Cli::command();
        let run = cli.find_subcommand("run").expect("otto run exists");
        for arg in run.get_arguments() {
            if arg.is_hide_set() || arg.get_id() == "help" {
                continue;
            }
            assert!(arg.get_help().is_some(), "--{} has no help text", arg.get_id());
        }
        assert!(run.get_arguments().any(|a| a.get_id() == "budget_usd" && a.is_hide_set()), "--budget-usd is hidden");
    }

    /// `otto ls` said one run needs you; `otto answer --choice X` with no id is the reply to that.
    #[test]
    fn answering_with_no_id_means_the_one_run_waiting_on_you() {
        let _h = TempHome::new();
        test_init("2026-09-13-polling", "a goal").unwrap();
        test_init("2026-09-13-other", "a goal").unwrap();
        gate_open("2026-09-13-polling", "keep-going");
        answer(AnswerArgs { id: None, choice: Some("Approve".into()), text: None, file: None, no_wake: true }).unwrap();
        assert!(read_run("2026-09-13-polling").unwrap().gate.is_none());
        assert!(read_run("2026-09-13-other").unwrap().gate.is_none(), "untouched");
    }

    /// With nothing or several waiting there is no one obvious run, so it asks — naming the short
    /// forms, which is what a person would type next.
    #[test]
    fn an_omitted_id_is_refused_unless_exactly_one_run_is_waiting() {
        let _h = TempHome::new();
        test_init("2026-09-13-polling", "a goal").unwrap();
        test_init("2026-09-13-other", "a goal").unwrap();
        let err = implied_run("answer", false).expect_err("nothing is waiting");
        assert_eq!(err.code, 2);
        assert!(err.to_string().contains("`otto answer polling`"), "got: {err}");
        assert!(err.to_string().contains("`otto answer other`"), "got: {err}");

        gate_open("2026-09-13-polling", "a");
        gate_open("2026-09-13-other", "b");
        let err = implied_run("answer", false).expect_err("two are waiting");
        assert_eq!(err.code, 1);
        assert!(err.to_string().contains("2 runs are waiting"), "got: {err}");
        assert!(err.to_string().contains("`otto answer other`"), "got: {err}");
    }

    /// `otto show` on its own is also useful when there is only one run at all, gate or not.
    #[test]
    fn show_with_no_id_falls_back_to_the_only_live_run() {
        let _h = TempHome::new();
        test_init("2026-09-13-solo", "a goal").unwrap();
        assert_eq!(implied_run("show", true).unwrap(), "2026-09-13-solo");
        // `answer` does not take that fallback: with no gate there is nothing to answer.
        assert_eq!(implied_run("answer", false).expect_err("no gate").code, 2);
        // A finished run is not live, so it is never implied.
        stop(StopArgs { id: "2026-09-13-solo".into(), reason: None, failed: false }).unwrap();
        assert!(implied_run("show", true).is_err());
    }

    /// The line under `otto show` for a blocked run must match the cause: the logs-and-retry
    /// advice is right for failures and wrong for a stall, where every wake completed.
    #[test]
    fn a_blocked_run_is_explained_by_its_cause() {
        let _h = TempHome::new();
        test_init("h-why", "a goal").unwrap();
        gate_open("h-why", "keep-going");
        let mut state = read_run("h-why").unwrap();
        state.ticks_without_progress = 24;
        state.block(BlockedCause::Stall, None);
        let stall = blocked_explanation(&state, "why");
        assert!(stall.contains("24 tick(s) in a row changed nothing"), "got: {stall}");
        assert!(stall.contains("nothing failed"), "got: {stall}");
        assert!(stall.contains("answer gate 001"), "got: {stall}");
        assert!(!stall.contains("otto wake"), "retrying is not the remedy for a stall: {stall}");

        state.incomplete_wakes = 5;
        state.block(BlockedCause::WakeFailures, Some("exited 1; ended still running".into()));
        let failures = blocked_explanation(&state, "why");
        assert!(failures.contains("5 wake(s) in a row did not finish (last: exited 1"), "got: {failures}");
        assert!(failures.contains("`otto logs why`") && failures.contains("`otto wake why --watch`"), "got: {failures}");

        state.block(BlockedCause::Budget, Some("wake budget spent: 35 of 35".into()));
        assert!(blocked_explanation(&state, "why").contains("wake budget spent: 35 of 35"));

        state.block(BlockedCause::Instructions, Some("gh cannot reach the PR host".into()));
        assert!(blocked_explanation(&state, "why").contains("blocked by its instructions: gh cannot reach"));

        // A run blocked before otto recorded causes gets the same inference `set-status` makes,
        // and says that it is one.
        state.blocked = None;
        let legacy = blocked_explanation(&state, "why");
        assert!(legacy.contains("24 tick(s) in a row changed nothing"), "got: {legacy}");
        assert!(legacy.contains("cause inferred"), "got: {legacy}");
        assert!(legacy.contains("answer gate 001"), "got: {legacy}");
    }

    /// Enter at the prompt means the gate's stated default — and nothing, when it has none.
    #[test]
    fn an_empty_line_at_the_prompt_takes_the_default_when_there_is_one() {
        let options: Vec<String> = vec!["keep-polling".into(), "stop-run".into()];
        assert_eq!(resolve_typed("", &options, Some("keep-polling")).unwrap(), "keep-polling");
        assert!(resolve_typed("", &options, None).is_err());
        // The other readings are unchanged by a default being present.
        assert_eq!(resolve_typed("2", &options, Some("keep-polling")).unwrap(), "stop-run");
        assert_eq!(resolve_typed("STOP-RUN", &options, Some("keep-polling")).unwrap(), "stop-run");
        assert_eq!(resolve_typed("something else", &options, Some("keep-polling")).unwrap(), "something else");
    }

    fn line(json: serde_json::Value) -> Line {
        parse_line(&json.to_string()).unwrap()
    }

    #[test]
    fn journal_lines_render_readably_and_long_prose_is_trimmed() {
        let mut r = Renderer::new(time::UtcOffset::UTC);
        let shown = r.render(&line(serde_json::json!({
            "ts": "2026-09-12T14:31:07Z", "event": "wake-complete", "status": "sleeping"
        })));
        assert!(shown.contains("14:31:07  wake-complete"), "got: {shown}");
        assert!(shown.contains("status=sleeping"));

        let long = line(serde_json::json!({"ts": "2026-09-12T14:31:07Z", "event": "run-created", "goal": "x".repeat(200)}));
        assert!(r.render(&long).contains('…'));
    }

    /// Forty lines span days; a bare `05:51` on each does not say which. The rule is printed
    /// once per day, and times — the line's own and any inside it — are in the reader's clock.
    #[test]
    fn a_date_rule_marks_each_new_day_and_times_are_local() {
        let two_east = time::UtcOffset::from_hms(2, 0, 0).unwrap();
        let mut r = Renderer::new(two_east);
        let first = r.render(&line(serde_json::json!({
            "ts": "2026-09-14T23:30:00Z", "event": "timer-armed", "nextWakeAt": "2026-09-15T00:30:00Z"
        })));
        // 23:30Z is 01:30 on the 15th, two hours east; so is the wake time, hence just a clock.
        assert!(first.starts_with("── Tue 2026-09-15 ──\n01:30:00  timer-armed"), "got: {first}");
        assert!(first.contains("nextWakeAt=02:30:00"), "got: {first}");

        let same_day = r.render(&line(serde_json::json!({"ts": "2026-09-15T05:00:00Z", "event": "wake-started"})));
        assert!(!same_day.contains("──"), "no second rule on the same day: {same_day}");

        let next_day = r.render(&line(serde_json::json!({
            "ts": "2026-09-16T05:00:00Z", "event": "gate-opened", "expiresAt": "2026-09-18T05:00:00Z"
        })));
        assert!(next_day.starts_with("── Wed 2026-09-16 ──"), "got: {next_day}");
        // A timestamp on another day says which day.
        assert!(next_day.contains("expiresAt=2026-09-18 07:00"), "got: {next_day}");
    }

    #[test]
    fn token_counts_are_rounded_to_what_the_eye_reads() {
        assert_eq!(humanise(392), "392");
        assert_eq!(humanise(9_999), "9999");
        assert_eq!(humanise(46_209), "46k");
        assert_eq!(humanise(358_986), "359k");
        assert_eq!(humanise(1_260_000), "1.3M");
        let mut r = Renderer::new(time::UtcOffset::UTC);
        let shown = r.render(&line(serde_json::json!({
            "ts": "2026-09-15T07:12:40Z", "event": "wake-spent", "turns": 16, "spentWakes": 35,
            "inputTokens": 392, "outputTokens": 9437, "cacheRead": 358986, "cacheCreation": 50545, "toolErrors": 0
        })));
        assert!(shown.contains("cacheRead=359k cacheCreation=51k"), "got: {shown}");
        assert!(shown.contains("turns=16 spentWakes=35"), "counters that are not tokens stay exact: {shown}");
    }

    #[test]
    fn since_takes_an_age_a_date_or_a_timestamp() {
        let utc = time::UtcOffset::UTC;
        let two_hours = parse_since("2h", utc).unwrap();
        let delta = crate::clock::now() - two_hours;
        assert!((delta.whole_minutes() - 120).abs() <= 1, "got {delta}");
        assert_eq!(parse_since("2026-09-15T07:00:00Z", utc).unwrap(), crate::clock::parse_iso("2026-09-15T07:00:00Z").unwrap());
        // A bare date is local midnight.
        let east = time::UtcOffset::from_hms(2, 0, 0).unwrap();
        assert_eq!(parse_since("2026-09-15", east).unwrap(), crate::clock::parse_iso("2026-09-14T22:00:00Z").unwrap());
        assert!(parse_since("yesterday", utc).is_err());
    }

    #[test]
    fn decisions_keep_the_turning_points_and_drop_the_wake_rhythm() {
        let filter = Filter { since: None, events: DECISION_EVENTS.iter().map(|e| e.to_string()).collect() };
        let kept = |event: &str| filter.keeps(&line(serde_json::json!({"ts": "2026-09-15T07:00:00Z", "event": event})));
        for event in ["gate-opened", "gate-closed", "status-changed", "wake-incomplete", "budget-exhausted"] {
            assert!(kept(event), "{event} is a decision");
        }
        for event in ["wake-started", "wake-spent", "wake-complete", "noop-tick", "timer-armed"] {
            assert!(!kept(event), "{event} is rhythm");
        }
        // `--since` composes with it.
        let recent = Filter { since: Some(crate::clock::parse_iso("2026-09-15T08:00:00Z").unwrap()), events: vec!["gate-opened".into()] };
        assert!(!recent.keeps(&line(serde_json::json!({"ts": "2026-09-15T07:00:00Z", "event": "gate-opened"}))));
        assert!(recent.keeps(&line(serde_json::json!({"ts": "2026-09-15T09:00:00Z", "event": "gate-opened"}))));
    }

    #[test]
    fn the_header_frames_the_tail() {
        let _h = TempHome::new();
        test_init("h-logs", "a goal").unwrap();
        gate_open("h-logs", "plan-review");
        let state = read_run("h-logs").unwrap();
        let header = logs_header(&state, "logs", time::UtcOffset::UTC);
        assert!(header.starts_with("# logs · 0 wake(s) since"), "got: {header}");
        assert!(header.contains("(today) · no wake yet · awaiting_human, gate 001 plan-review open · times UTC"), "got: {header}");
    }
}
