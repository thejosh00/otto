//! The commands a person uses. `otto state` remains the machine surface a wake writes through;
//! everything here is a thin composition over it plus formatting.
//!
//! This is where v2 differs most visibly from v1. A run used to be driven from *inside* a Claude
//! Code session: you typed `/otto start`, and answering a gate meant attaching to tmux and
//! replying to an `AskUserQuestion` dialog — which is why v1 required every gate to end a turn
//! inside that tool, and why a session manager had to classify a pane to notice a run needed you.
//!
//! Here a gate is a row in `otto ls` and a question `otto show` prints. Nothing reads a terminal
//! to find out a run is waiting, and `otto answer` works from a script, over ssh, or from a phone.
//! tmux becomes somewhere to *look* rather than the interface.

use crate::clock::relative;
use crate::error::OttoError;
use crate::liveness::{Liveness, LockLiveness};
use crate::state::commands::InitArgs;
use crate::state::{read_all_runs, read_run, CheckResult, Detach, RunState, Status};
use serde_json::Value;
use std::io::{IsTerminal, Write as _};

// ---------------------------------------------------------------------------
// otto run
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    #[command(flatten)]
    pub init: InitArgs,
    /// Watch the first wake in this terminal instead of detaching it
    #[arg(long)]
    pub watch: bool,
}

pub fn run(mut args: RunArgs) -> Result<(), OttoError> {
    if args.watch {
        // Persisted through `init`, so every later wake is foreground too — otherwise `--watch`
        // would quietly mean "watch the first one and detach the rest".
        args.init.detach = Detach::None;
    }
    let detach = args.init.detach;
    let id = crate::state::commands::init_run(args.init)?;
    println!("{id}");
    // Fail before the first wake rather than after it: a launcher that cannot resolve what the
    // run wraps produces a wake that spends money and achieves nothing.
    crate::wake::launcher::check_resolvable(&read_run(&id)?)?;
    start_wake(&id, detach, None)
}

/// Wait for an in-flight wake to finish, bounded.
///
/// A wake writes its gate to disk and then spends a few more seconds on its handoff and exit, so
/// `otto ls` can show a question as open while the wake that asked it is still running. Answering
/// in that window used to fail: `close_gate` succeeded, then the spawn hit the wake lock and
/// reported "a wake is already running", which looks like the answer was rejected when it was
/// safely recorded. Waiting is the honest fix — the work is nearly done, and the alternative
/// (spawning anyway) is two wakes on one run.
fn wait_for_wake_to_finish(id: &str) -> bool {
    if !LockLiveness.probe(id).is_busy() {
        return true;
    }
    println!("a wake is still finishing — waiting for it before continuing the run");
    for _ in 0..(crate::spawner::CONFIRM_SECONDS * 4) {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if !LockLiveness.probe(id).is_busy() {
            return true;
        }
    }
    false
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
            if LockLiveness.probe(&args.id).is_busy() {
                // The foreground path gets this from the wake lock. The background path would not:
                // the child would die on the lock while `spawner::wait_for_hold` saw that very lock
                // and reported success, so a busy run has to be refused out here instead.
                return Err(OttoError::conflict(format!(
                    "a wake is already running for {} — `otto attach {}` to watch it, or wait for \
                     it to finish",
                    args.id, args.id
                )));
            }
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
    let mut exec = crate::exec::RealExec;
    match detach {
        Detach::None => crate::spawner::start(id, crate::spawner::Strategy::Foreground, answer.as_deref(), &mut exec)
            .map(|_| ()),
        Detach::Tmux => {
            let before = crate::spawner::wake_number(id);
            let handle = crate::spawner::start(id, crate::spawner::Strategy::Tmux, answer.as_deref(), &mut exec)?;
            // Confirm it actually started. A detached wake that dies immediately — a bad PATH,
            // a lost $OTTO_HOME, a missing binary — takes its tmux session with it and writes
            // nothing anywhere, so "wake started" would be a lie nobody could check. Waiting for
            // the wake number to move is definitive: `otto wake` records it before spawning.
            if !crate::spawner::wait_for_hold(id, before) {
                return Err(OttoError::usage(format!(
                    "started tmux session {} but no wake took hold within {}s — the wake \
                     process exited immediately. Run `otto wake {id} --watch` in this terminal to \
                     see why.",
                    handle.session.unwrap_or_default(),
                    crate::spawner::CONFIRM_SECONDS
                )));
            }
            if let Some(note) = handle.description {
                println!("{note}");
            }
            Ok(())
        }
    }
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

/// Whatever the run is waiting on, in a few words. This is the column that answers "which of my
/// runs needs me?", so a gate names its question rather than just its id.
fn blocking(state: &RunState, wake_running: bool) -> String {
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
    match state.status {
        Status::Sleeping => match state.next_wake_at {
            Some(at) => format!("timer: {}", relative(at)),
            None => "nothing — sleeping with no wake time".to_string(),
        },
        Status::Done | Status::Failed | Status::Stopped => "—".to_string(),
        // Not running and nothing pending. The validator turns this into a retry, so seeing it
        // here means a wake is between attempts, or something went wrong outside a wake.
        _ => "nothing scheduled".to_string(),
    }
}

/// The shortest leading substring of `id` that no other id in `all_ids` also starts with — what
/// `otto ls` shows so a person has something shorter than the full id to type back at
/// `otto answer`/`otto show`/etc. (see `paths::resolve_run_id`).
fn shortest_unique_prefix(id: &str, all_ids: &[String]) -> String {
    let chars: Vec<char> = id.chars().collect();
    for len in 1..chars.len() {
        let candidate: String = chars[..len].iter().collect();
        if all_ids.iter().filter(|other| other.starts_with(&candidate)).count() == 1 {
            return candidate;
        }
    }
    id.to_string()
}

struct LsRow {
    id: String,
    prefix: String,
    status: String,
    phase: String,
    blocking: String,
    wakes: String,
}

pub fn ls(args: LsArgs) -> Result<(), OttoError> {
    let liveness = LockLiveness;
    let all_ids = crate::paths::all_run_ids();
    let mut rows: Vec<LsRow> = Vec::new();
    // Gates a person can act on right now, and sleeping runs — the two things `otto ls` exists
    // to surface: what needs you, and (paired with the reviver check below) what might silently
    // never come back to you at all.
    let mut needs_you: Vec<(String, String, Vec<String>)> = Vec::new();
    let mut sleeping_count = 0usize;
    for entry in read_all_runs()? {
        let state = match entry {
            crate::state::RunEntry::Readable(state) => state,
            crate::state::RunEntry::Unreadable { id } => {
                if args.all {
                    let prefix = shortest_unique_prefix(&id, &all_ids);
                    rows.push(LsRow {
                        id,
                        prefix,
                        status: "unreadable".to_string(),
                        phase: "?".to_string(),
                        blocking: "a person needs to look".to_string(),
                        wakes: "?".to_string(),
                    });
                }
                continue;
            }
        };
        if state.status.is_terminal() && !args.all {
            continue;
        }
        let running = liveness.probe(&state.id).is_busy();
        if let Some(gate) = &state.gate {
            let dir = crate::paths::run_dir(&state.id).ok();
            let options = dir
                .and_then(|d| crate::gate::read_question(&d, gate).ok())
                .map(|q| crate::gate::parse_options(&q))
                .unwrap_or_default();
            needs_you.push((state.id.clone(), format!("gate {} {}", gate.id, gate.slug), options));
        } else if !running && state.status == Status::Sleeping {
            sleeping_count += 1;
        }
        // Wakes, not dollars. There is no per-wake cost to show since otto stopped passing `-p`
        // (see `wake::launcher`), and a `$0.00` column that can never change is worse than no
        // column — it reads as a run that has cost nothing. Wakes against their budget is the
        // number that still means something at a glance; tokens are per-wake and live in the
        // journal, which is where a question about one wake belongs.
        let spent = state.budget.spent_wakes;
        let limit = state.budget.wakes;
        let wakes = if limit > 0 {
            format!("{spent}/{limit}")
        } else {
            spent.to_string()
        };
        let prefix = shortest_unique_prefix(&state.id, &all_ids);
        rows.push(LsRow {
            id: state.id.clone(),
            prefix,
            status: crate::state::commands::status_str(state.status).to_string(),
            phase: state.phase.clone(),
            blocking: blocking(&state, running),
            wakes,
        });
    }
    if rows.is_empty() {
        println!("no runs{}", if args.all { "" } else { " (--all includes finished ones)" });
        return Ok(());
    }
    let width = rows.iter().map(|r| r.id.len()).max().unwrap_or(2).max(2);
    let prefix_width = rows.iter().map(|r| r.prefix.len()).max().unwrap_or(6).max("PREFIX".len());
    let waiting = rows.iter().map(|r| r.blocking.len()).max().unwrap_or(10).max("WAITING ON".len());
    println!(
        "{:<width$}  {:<prefix_width$}  {:<14}  {:<12}  {:<waiting$}  WAKES",
        "ID", "PREFIX", "STATUS", "PHASE", "WAITING ON"
    );
    for row in &rows {
        println!(
            "{:<width$}  {:<prefix_width$}  {:<14}  {:<12}  {:<waiting$}  {}",
            row.id, row.prefix, row.status, row.phase, row.blocking, row.wakes
        );
    }
    if sleeping_count > 0 {
        if let Some(false) = crate::launchd::is_loaded() {
            println!(
                "\nwarning: {sleeping_count} run(s) are sleeping on a timer, but the reviver is \
                 not registered — `otto agent start` to fix, or they will never wake on their own"
            );
        }
    }
    if !needs_you.is_empty() {
        println!("\nneeds you:");
        for (id, label, options) in &needs_you {
            match options.as_slice() {
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
    pub id: String,
}

/// Everything needed to answer a gate cold, in one screen: where the run stands, what it last
/// did, and the question in full. Someone arriving eight hours later has no transcript, because
/// the session that asked is gone.
pub fn show(mut args: ShowArgs) -> Result<(), OttoError> {
    args.id = crate::paths::resolve_run_id(&args.id)?;
    let state = read_run(&args.id)?;
    let dir = crate::paths::run_dir(&args.id)?;
    let running = LockLiveness.probe(&args.id).is_busy();

    println!("{}  ({:?})", state.id, state.status);
    println!("  goal      {}", state.goal);
    match &state.done_condition {
        Some(done) => println!("  done when {done}"),
        None if state.policy.perpetual => {
            println!("  done when never — perpetual; retire it with `otto stop {}`", state.id)
        }
        None => println!("  done when (not yet decided — the next wake proposes one and asks)"),
    }
    println!(
        "  wraps     {} {}",
        crate::state::commands::kind_str(state.wraps.kind),
        state.wraps.reference.as_deref().unwrap_or("")
    );
    println!("  phase     {}", state.phase);
    if let Some(wake) = &state.wake {
        println!(
            "  wake      {} — {}{}",
            wake.n,
            if running { "running now" } else { "finished" },
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
    if let Some(at) = state.next_wake_at {
        println!("  next wake {} ({at})", relative(at));
    }
    if let Some(check) = &state.check {
        // Opt-in only (DESIGN.md §8) — kept cold-readable like everything else here, since a
        // check script's whole point is to run where nobody is watching.
        let last = match check.last_result {
            Some(CheckResult::NoChange) => "no change".to_string(),
            Some(CheckResult::Changed) => "changed".to_string(),
            Some(CheckResult::Error) => "errored".to_string(),
            None => "not run yet".to_string(),
        };
        println!(
            "  check     {} every {}s, next {} — last: {}",
            check.script,
            check.every_seconds,
            relative(check.next_check_at),
            last
        );
    }
    if state.status == Status::Blocked {
        // The same two commands `wake::open_stuck_gate`'s own gate text recommends — printed
        // here too, so a person does not have to read the gate file to find the diagnostic path.
        println!(
            "\nblocked after repeated wake failures — see `otto logs {}` for what happened, \
             then `otto wake {} --watch` to retry it in this terminal",
            state.id, state.id
        );
    }

    if let Some(gate) = &state.gate {
        println!("\n─── open gate {} — {} ───\n", gate.id, gate.slug);
        match std::fs::read_to_string(dir.join(&gate.file)) {
            Ok(text) => println!("{}", text.trim_end()),
            Err(_) => println!("(gate file {} is missing)", gate.file),
        }
        let options = crate::gate::read_question(&dir, gate).map(|q| crate::gate::parse_options(&q)).unwrap_or_default();
        println!("\nAnswer it with:");
        if options.is_empty() {
            println!("  otto answer {} --choice <option>", state.id);
        } else {
            for option in &options {
                println!("  otto answer {} --choice \"{option}\"", state.id);
            }
        }
        println!("  otto answer {} --text \"…\"", state.id);
    } else {
        let handoff = dir.join(crate::state::commands::HANDOFF_FILE);
        match std::fs::read_to_string(&handoff) {
            Ok(text) => println!("\n─── handoff ───\n{}", text.trim_end()),
            Err(_) => println!("\n(no handoff yet)"),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// otto answer
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct AnswerArgs {
    pub id: String,
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

/// Match `choice` against the gate's own options, case-insensitively, and record the gate's
/// casing rather than the user's — so the journal always shows the option exactly as named in
/// the question, whichever case someone typed. An empty `options` (a question with no parseable
/// list) skips validation entirely: most gates are free-form prose from an arbitrary wrapped
/// skill, and refusing those would break far more than it catches.
fn resolve_choice(choice: &str, options: &[String]) -> Result<String, OttoError> {
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

/// Print the question and, when the gate lists options, a numbered menu; read one line from
/// stdin and resolve it to an answer. A number picks that option, text matching an option's name
/// picks it too (both case-insensitively), and anything else is taken as free text — so someone
/// who wants to write more than an option name still can. Only reached on a TTY (see `answer`);
/// otherwise there is nobody to read a menu.
fn interactive_prompt(question: &str, options: &[String]) -> Result<String, OttoError> {
    println!("{}\n", question.trim());
    if options.is_empty() {
        print!("your answer: ");
    } else {
        for (i, option) in options.iter().enumerate() {
            println!("  {}) {option}", i + 1);
        }
        print!("choose 1-{}, or type an answer: ", options.len());
    }
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim();
    if line.is_empty() {
        return Err(OttoError::usage("no answer given"));
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

pub fn answer(mut args: AnswerArgs) -> Result<(), OttoError> {
    args.id = crate::paths::resolve_run_id(&args.id)?;
    let state = read_run(&args.id)?;
    let gate = state.gate.as_ref().ok_or_else(|| {
        OttoError::conflict(format!(
            "{} has no gate open — nothing is being asked. `otto show {}` for where it stands",
            args.id, args.id
        ))
    })?;
    let slug = gate.slug.clone();
    let dir = crate::paths::run_dir(&args.id)?;
    let question = crate::gate::read_question(&dir, gate).unwrap_or_default();
    let options = crate::gate::parse_options(&question);

    // Resolve the answer once, whichever way it arrived, so there is exactly one reading of it:
    // what gets recorded verbatim and what the wake sees are the same string.
    let answer = match (&args.choice, &args.text, &args.file) {
        (Some(choice), _, _) => resolve_choice(choice, &options)?,
        (_, Some(text), _) => text.clone(),
        (_, _, Some(path)) => {
            std::fs::read_to_string(path).map_err(|e| OttoError::usage(format!("cannot read {path}: {e}")))?
        }
        (None, None, None) => {
            if stdin_is_terminal() {
                interactive_prompt(&question, &options)?
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

    // `ops::close_gate` records the answer verbatim and returns `running`, so the wake that
    // follows sees an answered gate. It takes the run lock inside its own transaction and
    // releases it on return, so the wake below is sequential rather than nested — `RunLock` is
    // not reentrant and holding it across the wake would deadlock the wake's own first write.
    crate::state::ops::close_gate(&args.id, Some(&answer), Status::Running)?;
    println!("recorded against gate {} ({slug})", gate.id);

    if args.no_wake {
        println!("not waking it — `otto wake {}` when you want it to continue", args.id);
        return Ok(());
    }
    // The answer is already on disk, so if a wake is somehow still running after the wait, saying
    // so is better than failing: poke will carry the run on from here either way.
    if !wait_for_wake_to_finish(&args.id) {
        println!(
            "a wake is still running after {}s — your answer is recorded, and the \
             run will act on it. `otto ls` to watch, or `otto wake {}` once it is idle.",
            crate::spawner::CONFIRM_SECONDS,
            args.id
        );
        return Ok(());
    }
    // Hand the answer to the wake as well as recording it: the wake must see the person's own
    // words, not a summary of them.
    start_wake(&args.id, detach_of(&args.id), Some(answer))
}

/// How this run's wakes are backgrounded, as configured when the run was created.
fn detach_of(id: &str) -> Detach {
    read_run(id).ok().map(|s| s.launcher.detach).unwrap_or(Detach::Tmux)
}

// ---------------------------------------------------------------------------
// otto logs
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct LogsArgs {
    pub id: String,
    /// Trailing lines to show, like `tail -n`
    #[arg(long, short = 'n', default_value_t = 40)]
    pub lines: usize,
    /// Keep printing as the run writes more, like `tail -f`
    #[arg(long, short)]
    pub follow: bool,
}

/// One journal line, readable. The journal is JSON because a wake writes it; this is for reading.
fn format_event(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    let ts = value.get("ts").and_then(Value::as_str).unwrap_or("");
    let time = ts.get(11..19).unwrap_or(ts);
    let event = value.get("event").and_then(Value::as_str).unwrap_or("?");
    let mut rest: Vec<String> = Vec::new();
    if let Value::Object(map) = &value {
        for (key, val) in map {
            if key == "ts" || key == "event" {
                continue;
            }
            let shown = match val {
                Value::String(s) => {
                    // Long prose (a goal, a verbatim answer) makes the log unreadable as a
                    // sequence; `otto show` is where full text belongs.
                    if s.len() > 80 {
                        format!("{}…", &s[..77])
                    } else {
                        s.clone()
                    }
                }
                other => other.to_string(),
            };
            rest.push(format!("{key}={shown}"));
        }
    }
    Some(format!("{time}  {event:<18}  {}", rest.join(" ")))
}

pub fn logs(mut args: LogsArgs) -> Result<(), OttoError> {
    args.id = crate::paths::resolve_run_id(&args.id)?;
    let path = crate::paths::run_dir(&args.id)?.join("journal.jsonl");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let all: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = all.len().saturating_sub(args.lines);
    for line in &all[start..] {
        if let Some(shown) = format_event(line) {
            println!("{shown}");
        }
    }
    if !args.follow {
        return Ok(());
    }
    // Poll rather than notify: a journal is append-only and a run writes to it a few times a
    // minute at most, so this costs nothing and needs no platform-specific watching.
    let mut seen = text.len();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if text.len() > seen {
            for line in text[seen..].lines().filter(|l| !l.trim().is_empty()) {
                if let Some(shown) = format_event(line) {
                    println!("{shown}");
                }
            }
            seen = text.len();
        }
    }
}

// ---------------------------------------------------------------------------
// otto stop / attach
// ---------------------------------------------------------------------------

#[derive(clap::Args, Debug)]
pub struct StopArgs {
    pub id: String,
    #[arg(long)]
    pub reason: Option<String>,
    /// The run ended because it could not do its job, rather than simply no longer being wanted
    #[arg(long)]
    pub failed: bool,
}

/// Retiring a healthy run is `stopped`, not `failed` — `failed` in the audit trail would be a
/// lie about a run that worked and simply is not wanted any more.
pub fn stop(args: StopArgs) -> Result<(), OttoError> {
    let mut exec = crate::exec::RealExec;
    stop_with(args, &mut exec)
}

/// The body, with the killer's `Exec` injected so the kill path is testable without shelling out.
fn stop_with(mut args: StopArgs, exec: &mut dyn crate::exec::Exec) -> Result<(), OttoError> {
    // Resolve before anything else: `spawner::kill` names a tmux session directly from `args.id`,
    // never through `paths::run_dir`, so an unresolved prefix would go looking for a session that
    // was never named that.
    args.id = crate::paths::resolve_run_id(&args.id)?;
    let reason = args.reason.unwrap_or_else(|| "stopped by hand".to_string());
    let status = if args.failed { Status::Failed } else { Status::Stopped };
    crate::state::commands::set_status(crate::state::commands::SetStatusArgs {
        id: args.id.clone(),
        status,
        reason: Some(reason.clone()),
    })?;
    // Release locks on the way out, or the next run against that repo waits on a corpse.
    let _ = crate::state::locks::unlock(crate::state::locks::UnlockArgs {
        id: args.id.clone(),
        repo: None,
    });
    // A wake still running would keep working on a run nobody wants — kill it however it was
    // started. This used to check only for a tmux session, which missed a `--detach none` run's
    // detached process entirely: it kept going, unsupervised, until it hit its own deadline.
    if let Ok(state) = read_run(&args.id) {
        let outcome = crate::spawner::kill(&state, &args.id, exec);
        if outcome.did_anything() {
            println!("killed the live wake for {}", args.id);
        }
    }
    println!("{} is {:?} — {reason}", args.id, status);
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct AttachArgs {
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
            id: "h-answer".into(),
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
            id: "h-typo".into(),
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
            id: "h-case".into(),
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
            id: "h-prose".into(),
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
            id: "h-nogate".into(),
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
            id: "h-empty".into(),
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
                launcher: crate::state::LauncherKind::Claude,
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
                launcher: crate::state::LauncherKind::Claude,
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
                launcher: crate::state::LauncherKind::Claude,
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
        let shown = blocking(&state, false);
        // Same one-minute tolerance as `relative_times_read_forwards_and_backwards`.
        assert!(shown == "timer: in 24m" || shown == "timer: in 23m", "got {shown}");
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

    #[test]
    fn journal_lines_render_readably_and_long_prose_is_trimmed() {
        let line = serde_json::json!({
            "ts": "2026-09-12T14:31:07Z", "event": "wake-complete", "status": "sleeping"
        })
        .to_string();
        let shown = format_event(&line).unwrap();
        assert!(shown.starts_with("14:31:07"));
        assert!(shown.contains("wake-complete"));
        assert!(shown.contains("status=sleeping"));

        let long = serde_json::json!({"ts": "2026-09-12T14:31:07Z", "event": "run-created", "goal": "x".repeat(200)})
            .to_string();
        assert!(format_event(&long).unwrap().contains('…'));
    }
}
