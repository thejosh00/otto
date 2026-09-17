//! One function per `otto state` subcommand

use super::{
    dig, emit, journal, parse_kv, read_all_runs, read_run, read_text_arg, transaction, write_atomic, Budget, Check,
    CheckResult, Detach, Launcher, LauncherKind, Permission, PermissionMode, Policy, RunLock, RunState, Status,
    WrapKind, Wraps, SCHEMA_VERSION,
};
use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::event::Event;
use serde_json::{json, Map, Value};
use time::macros::format_description;

/// Replaces v1's `brief.md`. One file, not two: a rolling human summary and a
/// next-wake handoff are the same thing written twice, and two accounts of one event mean
/// nobody can tell later which was true.
pub const HANDOFF_FILE: &str = "handoff.md";
/// DESIGN.md §6. Enforced, not advisory — see `handoff()`.
pub const HANDOFF_MAX_BYTES: usize = 8192;

pub fn kind_str(kind: WrapKind) -> &'static str {
    match kind {
        WrapKind::Skill => "skill",
        WrapKind::Instructions => "instructions",
        WrapKind::GoalOnly => "goal-only",
    }
}

/// The first handoff. Written at init so that wake 1 reads the same file wake 40 will,
/// rather than having to special-case its own first run.
fn initial_handoff(run_id: &str, args: &InitArgs, wraps_ref: &Option<String>, created: Timestamp) -> String {
    let wraps = match wraps_ref {
        Some(reference) => reference.clone(),
        None => "nothing — the goal is the whole instruction".to_string(),
    };
    let needs = if args.until.is_some() || args.perpetual {
        "nothing".to_string()
    } else {
        "a done-condition: propose one and gate it before doing any work".to_string()
    };
    format!(
        "# {run_id} · {phase} · wake 0\n\n\
         ## Goal\n{goal}\n\n\
         ## Where this is\nCreated {created}. No wake has run yet.\n\n\
         ## Decided\nNothing yet.\n\n\
         ## This wake did\nNothing — this is the handoff `init` wrote.\n\n\
         ## Next wake must\nRead the wrapped instructions ({wraps}) and orient.\n\n\
         ## Needs\n{needs}\n\n\
         ## Don't re-derive\nNothing recorded yet.\n",
        phase = args.phase,
        goal = args.goal,
    )
}

#[derive(clap::Args, Debug)]
pub struct InitArgs {
    /// What this run is trying to achieve. Required: it is the only thing that survives
    /// every wake unaltered, and the only thing that can say when to stop.
    #[arg(long, required = true)]
    pub goal: String,
    /// Wrap a Claude skill by name
    #[arg(long, conflicts_with = "instructions")]
    pub skill: Option<String>,
    /// Wrap any prose file — a runbook, a workflow table, a ticket
    #[arg(long)]
    pub instructions: Option<String>,
    /// State the done-condition outright; otherwise the first wake proposes one and gates it
    #[arg(long, conflicts_with = "perpetual")]
    pub until: Option<String>,
    /// There is no done-condition: this run is retired, never finished
    #[arg(long)]
    pub perpetual: bool,
    /// What the run is about, e.g. a Jira key
    #[arg(long)]
    pub target: Option<String>,
    /// Explicit run id (default: <date>-<slug>)
    #[arg(long)]
    pub id: Option<String>,
    /// Slug for the generated id (default: from target, else the goal)
    #[arg(long)]
    pub slug: Option<String>,
    #[arg(long, default_value = "start")]
    pub phase: String,
    #[arg(long, value_enum, default_value = "claude")]
    pub launcher: LauncherKind,
    /// Extra repo the wake may touch (repeatable)
    #[arg(long = "repo")]
    pub repos: Vec<String>,
    /// Directory to load skills from (repeatable). Required under yolo for --skill to
    /// resolve, because the sandbox cannot see ~/.claude
    #[arg(long = "skills-dir")]
    pub skill_dirs: Vec<String>,
    /// Defaults to the permissive mode, and not casually. A wake is unattended, so every other
    /// mode relies on a prompt for some tool class, and `--permission-prompts none` turns every
    /// unanswerable prompt into a denial. Measured: `accept-edits` denies Bash, and `dont-ask`
    /// denies Write, Edit and Bash — so the stricter modes do not merely restrict a wake, they
    /// break it silently while still charging for it. The real guardrail is the launcher (run
    /// under `--launcher yolo` for a kernel-enforced sandbox) plus `authorize` for anything
    /// outward-facing.
    #[arg(long = "permission-mode", value_enum, default_value = "bypass-permissions")]
    pub permission_mode: PermissionMode,
    #[arg(long = "allow-tool", value_name = "TOOL")]
    pub allowed_tools: Vec<String>,
    #[arg(long = "deny-tool", value_name = "TOOL")]
    pub disallowed_tools: Vec<String>,
    #[arg(long, value_enum, default_value = "tmux")]
    pub detach: Detach,
    /// 0 means unlimited
    #[arg(long = "budget-wakes", default_value_t = 0)]
    pub budget_wakes: u32,
    #[arg(long = "budget-hours", default_value_t = 0)]
    pub budget_hours: u32,
    /// Unenforceable, and refused if set — kept declared so saying so is possible. See `Budget`.
    #[arg(long = "budget-usd", default_value_t = 0.0)]
    pub budget_usd: f64,
    #[arg(long = "fact", value_name = "K=V")]
    pub fact: Vec<String>,
    #[arg(long = "policy", value_name = "K=V")]
    pub policy: Vec<String>,
}

/// Mirrors every `#[arg(default_value...)]` above, so an internal caller — `test_init`, mainly —
/// can say only the fields it cares about (`InitArgs { goal: ..., id: Some(...), ..Default::default() }`)
/// instead of all twenty. `goal` has no real default (clap requires it); an empty string is a
/// deliberately obvious placeholder for a caller that forgets to set it.
impl Default for InitArgs {
    fn default() -> Self {
        Self {
            goal: String::new(),
            skill: None,
            instructions: None,
            until: None,
            perpetual: false,
            target: None,
            id: None,
            slug: None,
            phase: "start".to_string(),
            launcher: LauncherKind::Claude,
            repos: vec![],
            skill_dirs: vec![],
            permission_mode: PermissionMode::BypassPermissions,
            allowed_tools: vec![],
            disallowed_tools: vec![],
            detach: Detach::Tmux,
            budget_wakes: 0,
            budget_hours: 0,
            budget_usd: 0.0,
            fact: vec![],
            policy: vec![],
        }
    }
}

pub fn init(args: InitArgs) -> Result<(), OttoError> {
    let run_id = init_run(args)?;
    println!("{run_id}");
    Ok(())
}

/// Create the run and hand back its id, so `otto run` can go straight on to waking it instead
/// of parsing the id back out of stdout.
pub fn init_run(args: InitArgs) -> Result<String, OttoError> {
    let wraps = match (&args.skill, &args.instructions) {
        (Some(skill), None) => Wraps {
            kind: WrapKind::Skill,
            reference: Some(skill.clone()),
        },
        (None, Some(file)) => {
            // Fail now, not on the first wake: a wrapped instructions file that isn't
            // there produces a run that can never do anything, and finding that out an
            // hour later costs a wake.
            let path = std::path::Path::new(file);
            if !path.is_file() {
                return Err(OttoError::usage(format!("--instructions {file} is not a file")));
            }
            Wraps {
                kind: WrapKind::Instructions,
                reference: Some(
                    std::fs::canonicalize(path)
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| file.clone()),
                ),
            }
        }
        (None, None) => Wraps {
            kind: WrapKind::GoalOnly,
            reference: None,
        },
        // clap's conflicts_with already rejects this; belt and braces.
        (Some(_), Some(_)) => return Err(OttoError::usage("pass --skill or --instructions, not both")),
    };

    let fallback_slug = match &wraps.reference {
        Some(reference) => reference.rsplit('/').next().unwrap_or("run").to_string(),
        None => "run".to_string(),
    };
    let slug_source = args
        .slug
        .as_deref()
        .or(args.target.as_deref())
        .unwrap_or_else(|| args.goal.as_str());
    let slug = crate::clock::slugify(slug_source, &fallback_slug);
    let mut run_id = match args.id.clone() {
        Some(id) => id,
        None => {
            let date = crate::clock::now().date();
            let fmt = format_description!("[year]-[month]-[day]");
            format!("{}-{slug}", date.format(&fmt).expect("a valid date always formats"))
        }
    };
    let mut path = crate::paths::runs_dir().join(&run_id);
    if path.exists() {
        if args.id.is_some() {
            // An explicit --id is a deliberate choice, so this refuses rather than silently
            // picking a different one — but the old message pointed at a `resume` command that
            // has never existed. Name the actual recovery instead.
            return Err(OttoError::conflict(format!(
                "run already exists: {run_id} — `otto stop {run_id}` or remove {} yourself, \
                 then retry, or pass a different --id",
                path.display()
            )));
        }
        // No --id means the id is `<date>-<slug>`, which collides on the very common case this
        // is meant to smooth over: retrying a same-day run that failed before it did anything.
        // Suffix a counter rather than dead-ending on an id nobody actually asked for.
        let base = run_id.clone();
        let mut n = 2u32;
        loop {
            run_id = format!("{base}-{n}");
            path = crate::paths::runs_dir().join(&run_id);
            if !path.exists() {
                break;
            }
            n += 1;
            if n > 1000 {
                return Err(OttoError::conflict(format!("could not find a free id near {base} after 1000 tries")));
            }
        }
    }
    std::fs::create_dir_all(path.join("gates"))?;
    std::fs::create_dir_all(path.join("artifacts"))?;
    let created = Timestamp::now();

    // Every field below has its default in exactly one place: `Policy::default()`. See its doc
    // for why that is the point.
    let mut policy = Policy::default();
    if args.perpetual {
        // Recorded as policy rather than a top-level field so that "no done-condition
        // because it is perpetual" is distinguishable from "none proposed yet".
        policy.perpetual = true;
    }
    for (k, v) in parse_kv(&args.policy, "--policy")? {
        policy.set(k, v);
    }

    let mut facts = Map::new();
    facts.insert(
        "target".into(),
        args.target.clone().map(Value::String).unwrap_or(Value::Null),
    );
    for (k, v) in parse_kv(&args.fact, "--fact")? {
        facts.insert(k, v);
    }

    // Refused rather than silently ignored. Nothing can price a wake since otto stopped passing
    // `-p` (print mode bills SDK credits — see `wake::launcher`), so a dollar budget would be a
    // ceiling that never fires: `spentUsd` stays at 0 and the run looks healthy right up to the
    // point the real limit was blown past. Better to say so at the only moment a person is
    // watching.
    if args.budget_usd > 0.0 {
        return Err(OttoError::usage(
            "--budget-usd cannot be enforced: measuring per-wake cost needed `claude -p`, which \
             otto no longer runs because print mode bills SDK credits rather than the \
             subscription. Use --budget-wakes or --budget-hours, or the maxWakeMinutes policy to \
             cap a single wake."
                .to_string(),
        ));
    }

    let wraps_ref = wraps.reference.clone();
    let wraps_kind = wraps.kind;
    let mut state = RunState {
        schema_version: SCHEMA_VERSION,
        id: run_id.clone(),
        wraps,
        goal: args.goal.clone(),
        done_condition: args.until.clone(),
        status: Status::Running,
        phase: args.phase.clone(),
        gate: None,
        next_wake_at: None,
        wake: None,
        check: None,
        incomplete_wakes: 0,
        spawn_attempts: 0,
        last_spawned_at: None,
        ticks_without_progress: 0,
        launcher: Launcher {
            kind: args.launcher,
            detach: args.detach,
            repos: args.repos.clone(),
            skill_dirs: args.skill_dirs.clone(),
        },
        permission: Permission {
            mode: args.permission_mode,
            allowed_tools: args.allowed_tools.clone(),
            disallowed_tools: args.disallowed_tools.clone(),
        },
        budget: Budget {
            wakes: args.budget_wakes,
            hours: args.budget_hours,
            usd: args.budget_usd,
            spent_wakes: 0,
            spent_usd: 0.0,
        },
        policy,
        facts,
        created_at: created,
        updated_at: created,
        authorizations: Map::new(),
    };

    let _lock = RunLock::acquire(&path)?;
    super::write_state(&path, &mut state)?;
    write_atomic(&path.join(HANDOFF_FILE), &initial_handoff(&run_id, &args, &wraps_ref, created))?;
    crate::event::record(
        &path,
        &Event::RunCreated {
            wraps: kind_str(wraps_kind).to_string(),
            reference: wraps_ref,
            goal: args.goal,
            done_condition: args.until,
            perpetual: args.perpetual,
            phase: args.phase,
            target: args.target,
            launcher: format!("{:?}", args.launcher).to_lowercase(),
        },
    )?;
    Ok(run_id)
}

/// Poke's spawn bookkeeping. Its own function rather than `record_fact` because these live at
/// the top level of `run.json`, deliberately outside `facts` — see `RunState::spawn_attempts`.
pub fn record_spawn(id: &str, attempts: u32, stamp: bool) -> Result<(), OttoError> {
    transaction(id, |_path, state| {
        state.spawn_attempts = attempts;
        if stamp {
            state.last_spawned_at = Some(Timestamp::now());
        }
        Ok(())
    })
}

#[derive(clap::Args, Debug)]
pub struct GetArgs {
    pub id: String,
    /// e.g. status, facts.branch, gate.file
    #[arg(long)]
    pub field: Option<String>,
}

pub fn get(args: GetArgs) -> Result<(), OttoError> {
    let state = read_run(&args.id)?;
    let value = serde_json::to_value(&state)?;
    match args.field {
        Some(field) => emit(&dig(&value, &field)?),
        None => emit(&value),
    }
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct ListArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long, value_enum)]
    pub status: Option<Status>,
}

pub fn list(args: ListArgs) -> Result<(), OttoError> {
    let mut rows = Vec::new();
    for entry in read_all_runs()? {
        if let Some(filter) = args.status {
            let matches = matches!(&entry, super::RunEntry::Readable(state) if state.status == filter);
            if !matches {
                continue;
            }
        }
        rows.push(entry);
    }
    if args.json {
        let values: Vec<Value> = rows.iter().map(super::RunEntry::to_value).collect();
        println!("{}", serde_json::to_string_pretty(&values)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no runs");
        return Ok(());
    }
    for entry in &rows {
        let state = match entry {
            super::RunEntry::Readable(state) => state,
            super::RunEntry::Unreadable { id } => {
                println!("{id:<34} unreadable — a person needs to look");
                continue;
            }
        };
        let blocking = match &state.gate {
            // Every gate is a human gate now, so the useful thing to show is which
            // question is open, not what kind it is.
            Some(gate) => format!("{}:{}", gate.id, gate.slug),
            None => "—".to_string(),
        };
        println!(
            "{:<34} {:<12} {:<15} phase={:<12} gate={:<10} wake={}",
            state.id,
            kind_str(state.wraps.kind),
            status_str(state.status),
            state.phase,
            blocking,
            state.next_wake_at.map(|t| t.to_string()).unwrap_or_else(|| "—".to_string()),
        );
    }
    Ok(())
}

pub fn status_str(status: Status) -> &'static str {
    match status {
        Status::Running => "running",
        Status::AwaitingHuman => "awaiting_human",
        Status::Sleeping => "sleeping",
        Status::Blocked => "blocked",
        Status::Done => "done",
        Status::Failed => "failed",
        Status::Stopped => "stopped",
    }
}

#[derive(clap::Args, Debug)]
pub struct SetPhaseArgs {
    pub id: String,
    #[arg(long, required = true)]
    pub phase: String,
    #[arg(long, value_enum, default_value = "running")]
    pub status: Status,
    #[arg(long)]
    pub note: Option<String>,
}

pub fn set_phase(args: SetPhaseArgs) -> Result<(), OttoError> {
    let id = args.id.clone();
    transaction(&id, |path, state| {
        let previous = state.phase.clone();
        state.phase = args.phase.clone();
        state.status = args.status;
        if matches!(args.status, Status::Running) {
            state.next_wake_at = None;
        }
        crate::event::record(
            path,
            &Event::PhaseChanged {
                from: previous,
                to: args.phase.clone(),
                status: status_str(args.status).to_string(),
                note: args.note.clone(),
            },
        )
    })
}

#[derive(clap::Args, Debug)]
pub struct SetStatusArgs {
    pub id: String,
    #[arg(long, value_enum, required = true)]
    pub status: Status,
    #[arg(long)]
    pub reason: Option<String>,
}

pub fn set_status(args: SetStatusArgs) -> Result<(), OttoError> {
    let id = args.id.clone();
    transaction(&id, |path, state| {
        let previous = status_str(state.status).to_string();
        state.status = args.status;
        if args.status.is_terminal() {
            state.next_wake_at = None;
        }
        crate::event::record(
            path,
            &Event::StatusChanged {
                from: previous,
                to: status_str(args.status).to_string(),
                reason: args.reason.clone(),
            },
        )
    })
}

#[derive(clap::Args, Debug)]
pub struct RecordFactArgs {
    pub id: String,
    #[arg(value_name = "K=V", required = true)]
    pub pairs: Vec<String>,
}

pub fn record_fact(args: RecordFactArgs) -> Result<(), OttoError> {
    let facts = parse_kv(&args.pairs, "fact")?;
    if facts.is_empty() {
        return Err(OttoError::usage("record-fact needs at least one key=value"));
    }
    let id = args.id.clone();
    transaction(&id, |path, state| {
        for (k, v) in facts.clone() {
            state.facts.insert(k, v);
        }
        crate::event::record(path, &Event::FactsRecorded { facts: facts.clone() })
    })
}

#[derive(clap::Args, Debug)]
pub struct OpenGateArgs {
    pub id: String,
    #[arg(long, required = true)]
    pub slug: String,
    #[arg(long)]
    pub question: Option<String>,
    #[arg(long = "question-file")]
    pub question_file: Option<String>,
    #[arg(long)]
    pub stdin: bool,
    /// ISO time after which no answer counts as none. Only use an expiry where silence has
    /// a sane meaning: "no answer, so don't do it" is sane; "no answer, so push it" is not
    #[arg(long = "expires-at")]
    pub expires_at: Option<String>,
    /// same, relative to now
    #[arg(long = "expires-in", value_name = "SECONDS")]
    pub expires_in: Option<i64>,
}

/// A thin CLI adapter: resolve `--question`/`--question-file`/`--stdin` and `--expires-at`/
/// `--expires-in` down to plain values, then hand off to `ops::open_gate`. The engine logic
/// lives there — an internal caller with a question already in memory (`wake::open_stuck_gate`,
/// `wake::block_on_budget`) calls it directly rather than going through this CLI shape.
pub fn open_gate(args: OpenGateArgs) -> Result<(), OttoError> {
    let question = read_text_arg(args.question.as_deref(), args.question_file.as_deref(), args.stdin, "question")?;
    if args.expires_at.is_some() && args.expires_in.is_some() {
        return Err(OttoError::usage("give at most one of --expires-at or --expires-in"));
    }
    let expires: Option<Timestamp> = match (&args.expires_at, args.expires_in) {
        (Some(at), _) => Some(Timestamp::parse(at)?),
        (None, Some(seconds)) => Some(Timestamp::in_seconds(seconds)),
        (None, None) => None,
    };
    let gate_file_path = super::ops::open_gate(&args.id, &args.slug, &question, expires)?;
    println!("{gate_file_path}");
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct CloseGateArgs {
    pub id: String,
    #[arg(long)]
    pub answer: Option<String>,
    #[arg(long = "answer-file")]
    pub answer_file: Option<String>,
    #[arg(long)]
    pub stdin: bool,
    /// close an expired gate as unanswered, instead of with an answer
    #[arg(long)]
    pub expired: bool,
    #[arg(long, value_enum, default_value = "running")]
    pub status: Status,
}

/// A thin CLI adapter: resolve `--answer`/`--answer-file`/`--stdin`/`--expired` down to a plain
/// `Option<&str>`, then hand off to `ops::close_gate`.
pub fn close_gate(args: CloseGateArgs) -> Result<(), OttoError> {
    let answer: Option<String> = if args.expired {
        if args.answer.is_some() || args.answer_file.is_some() || args.stdin {
            return Err(OttoError::usage("--expired records that nobody answered; do not also pass an answer"));
        }
        None
    } else {
        Some(read_text_arg(args.answer.as_deref(), args.answer_file.as_deref(), args.stdin, "answer")?)
    };
    super::ops::close_gate(&args.id, answer.as_deref(), args.status)
}

#[derive(clap::Args, Debug)]
pub struct ArmTimerArgs {
    pub id: String,
    /// ISO-8601 wake time
    #[arg(long)]
    pub at: Option<String>,
    /// seconds from now
    #[arg(long = "in", value_name = "SECONDS")]
    pub seconds: Option<i64>,
    #[arg(long, value_enum, default_value = "sleeping")]
    pub status: Status,
    #[arg(long)]
    pub note: Option<String>,
    /// Path, relative to the run dir, to an opt-in script poke runs directly — DESIGN.md §8.
    /// Requires `--check-every`; give neither to arm a plain timer.
    #[arg(long = "check-script")]
    pub check_script: Option<String>,
    /// How often poke runs the check script, in seconds. Must be less than the time until the
    /// wake itself, or the check would never run before the real wake does.
    #[arg(long = "check-every", value_name = "SECONDS")]
    pub check_every: Option<i64>,
}

pub fn arm_timer(args: ArmTimerArgs) -> Result<(), OttoError> {
    if args.at.is_none() == args.seconds.is_none() {
        return Err(OttoError::usage("give exactly one of --at or --in"));
    }
    if args.check_script.is_some() != args.check_every.is_some() {
        return Err(OttoError::usage("give both --check-script and --check-every, or neither"));
    }
    let wake = match &args.at {
        Some(at) => Timestamp::parse(at)?,
        None => Timestamp::in_seconds(args.seconds.unwrap()),
    };
    let check = match (&args.check_script, args.check_every) {
        (Some(script), Some(every)) => {
            if every <= 0 {
                return Err(OttoError::usage("--check-every must be positive"));
            }
            let seconds_until_wake = (wake.dt() - crate::clock::now()).whole_seconds();
            if every >= seconds_until_wake {
                return Err(OttoError::usage(format!(
                    "--check-every {every}s must be less than the {seconds_until_wake}s until the wake itself"
                )));
            }
            Some(Check {
                script: script.clone(),
                every_seconds: every,
                next_check_at: Timestamp::in_seconds(every),
                consecutive_no_change: 0,
                last_result: None,
            })
        }
        _ => None,
    };
    let id = args.id.clone();
    transaction(&id, |path, state| {
        state.next_wake_at = Some(wake);
        state.status = args.status;
        state.check = check.clone();
        crate::event::record(path, &Event::TimerArmed { next_wake_at: wake, note: args.note.clone() })
    })?;
    println!("{wake}");
    Ok(())
}

/// Poke's own bookkeeping for an opt-in check script, distinct from `record_spawn` for the same
/// reason that one is (`RunState::spawn_attempts`): a wake rewriting `check` while re-arming its
/// own timer would otherwise clobber poke's no-change streak. A no-change result never reaches
/// the journal — only `changed`/`error` do — so a tight cadence over days doesn't spam it.
pub fn record_check(id: &str, result: CheckResult, note: Option<String>) -> Result<(), OttoError> {
    transaction(id, |path, state| {
        let Some(check) = state.check.as_mut() else { return Ok(()) };
        match result {
            CheckResult::NoChange => {
                check.consecutive_no_change += 1;
                check.next_check_at = Timestamp::in_seconds(check.every_seconds);
            }
            CheckResult::Changed | CheckResult::Error => {
                check.consecutive_no_change = 0;
            }
        }
        check.last_result = Some(result);
        let consecutive_no_change = check.consecutive_no_change;
        if !matches!(result, CheckResult::NoChange) {
            crate::event::record(
                path,
                &Event::CheckRan { result, consecutive_no_change, note: note.clone() },
            )?;
        }
        Ok(())
    })
}

#[derive(clap::Args, Debug)]
pub struct TickArgs {
    pub id: String,
    /// something happened: reset the counter
    #[arg(long)]
    pub progress: bool,
    #[arg(long)]
    pub note: Option<String>,
}

pub fn tick(args: TickArgs) -> Result<(), OttoError> {
    let mut count = 0u32;
    let mut limit = 24i64;
    let id = args.id.clone();
    transaction(&id, |path, state| {
        count = if args.progress { 0 } else { state.ticks_without_progress + 1 };
        state.ticks_without_progress = count;
        // A configured `0` must stay 0, distinct from "absent → default 24" — Python's
        // `or 24` bug conflated the two. `Policy`'s own default supplies the 24; this just
        // reads it back.
        limit = state.policy.max_ticks_without_progress;
        let note = args.note.clone();
        crate::event::record(
            path,
            &if args.progress {
                Event::Tick { ticks_without_progress: count, note }
            } else {
                Event::NoopTick { ticks_without_progress: count, note }
            },
        )
    })?;
    let exhausted = limit > 0 && (count as i64) >= limit;
    println!(
        "{}",
        serde_json::to_string(&json!({"ticksWithoutProgress": count, "limit": limit, "exhausted": exhausted}))?
    );
    Ok(())
}

pub fn due() -> Result<(), OttoError> {
    for entry in read_all_runs()? {
        let super::RunEntry::Readable(state) = entry else { continue };
        if state.status.is_terminal() {
            continue;
        }
        let Some(wake) = state.next_wake_at else { continue };
        if wake.is_past() {
            println!("{}\t{wake}\t{}", state.id, status_str(state.status));
        }
    }
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct HandoffArgs {
    pub id: String,
    #[arg(long)]
    pub file: Option<String>,
    #[arg(long)]
    pub stdin: bool,
}

/// Rewrite `handoff.md` — the only thing the next wake gets for free.
///
/// The cap is **enforced here and refused loudly**, which v1's `brief` never did. With no
/// context reuse between wakes this file is re-read on every single wake, so unchecked
/// growth is multiplied by the number of wakes and quietly turns a long run quadratic. A
/// handoff that no longer fits is a signal that the run needs a ledger artifact, not a
/// bigger cap — so failing is the useful behaviour, and truncating would be the harmful one
/// (it would silently drop whatever the wake thought mattered most, which is usually last).
pub fn handoff(args: HandoffArgs) -> Result<(), OttoError> {
    let text = read_text_arg(None, args.file.as_deref(), args.stdin, "handoff")?;
    let path = crate::paths::run_dir(&args.id)?;
    let cap = read_run(&args.id)
        .ok()
        .map(|state| state.policy.handoff_max_bytes)
        .unwrap_or(HANDOFF_MAX_BYTES as u64) as usize;
    let text = if text.ends_with('\n') { text } else { format!("{text}\n") };
    if cap > 0 && text.len() > cap {
        return Err(OttoError::usage(format!(
            "handoff is {} bytes, cap is {cap} — put what accumulates in an artifact ledger and \
             keep the handoff to what the next wake needs to start",
            text.len()
        )));
    }
    let _lock = RunLock::acquire(&path)?;
    write_atomic(&path.join(HANDOFF_FILE), &text)
}

#[derive(clap::Args, Debug)]
pub struct LogArgs {
    pub id: String,
    #[arg(long, required = true)]
    pub event: String,
    #[arg(long)]
    pub message: Option<String>,
    #[arg(long = "data", value_name = "K=V")]
    pub data: Vec<String>,
}

pub fn log(args: LogArgs) -> Result<(), OttoError> {
    let data = parse_kv(&args.data, "--data")?;
    let path = crate::paths::run_dir(&args.id)?;
    let _lock = RunLock::acquire(&path)?;
    let mut fields = Map::new();
    fields.insert("message".into(), args.message.map(Value::String).unwrap_or(Value::Null));
    for (k, v) in data {
        fields.insert(k, v);
    }
    journal(&path, &args.event, Value::Object(fields))
}

#[derive(clap::Args, Debug)]
pub struct TailArgs {
    pub id: String,
    #[arg(long, default_value_t = 20)]
    pub lines: usize,
}

pub fn tail(args: TailArgs) -> Result<(), OttoError> {
    let path = crate::paths::run_dir(&args.id)?;
    let text = std::fs::read_to_string(path.join("journal.jsonl"))?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(args.lines);
    for line in &lines[start..] {
        println!("{line}");
    }
    Ok(())
}

/// Creates a run with a known, explicit id — so tests never have to capture stdout to
/// learn what `init` picked. Every field but `id` and `goal` is `InitArgs::default()`; a test
/// that needs something else builds its own `InitArgs { ..Default::default() }` instead.
#[cfg(test)]
pub(crate) fn test_init(id: &str, goal: &str) -> Result<(), OttoError> {
    init(InitArgs {
        goal: goal.to_string(),
        id: Some(id.to_string()),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;

    /// The case this exists for: retrying a same-day run that failed before doing anything. The
    /// default id is `<date>-<slug>`, so a bare retry collides every time — and used to dead-end
    /// on a `resume` command that never existed.
    #[test]
    fn a_colliding_auto_generated_id_gets_suffixed_rather_than_failing() {
        let _home = TempHome::new();
        let id1 = init_run(InitArgs {
            goal: "a goal".to_string(),
            slug: Some("dup-goal".to_string()),
            ..Default::default()
        })
        .unwrap();
        let id2 = init_run(InitArgs {
            goal: "a goal".to_string(),
            slug: Some("dup-goal".to_string()),
            ..Default::default()
        })
        .unwrap();
        assert_ne!(id1, id2);
        assert_eq!(id2, format!("{id1}-2"));
        assert!(read_run(&id1).is_ok());
        assert!(read_run(&id2).is_ok());
    }

    /// An explicit `--id` is a deliberate choice, so a collision still refuses — but must name
    /// real recovery, not a `resume` subcommand that has never existed.
    #[test]
    fn an_explicit_id_collision_names_real_recovery_not_resume() {
        let _home = TempHome::new();
        test_init("dup-explicit", "a goal").unwrap();
        let err = init(InitArgs {
            goal: "a goal".to_string(),
            id: Some("dup-explicit".to_string()),
            ..Default::default()
        })
        .expect_err("must refuse");
        assert!(!err.to_string().contains("resume"), "got: {err}");
        assert!(err.to_string().contains("otto stop dup-explicit"), "got: {err}");
        assert!(err.to_string().contains("--id"), "got: {err}");
    }

    #[test]
    fn gate_ids_are_sequential() {
        let _home = TempHome::new();
        test_init("run-a", "hello-flow").unwrap();
        for (n, expected) in [(1, "001"), (2, "002"), (3, "003")] {
            let file = open_gate(OpenGateArgs {
                id: "run-a".to_string(),
                slug: format!("gate-{n}"),
                question: Some("well?".to_string()),
                question_file: None,
                stdin: false,
                expires_at: None,
                expires_in: None,
            });
            assert!(file.is_ok(), "{file:?}");
            close_gate(CloseGateArgs {
                id: "run-a".to_string(),
                answer: Some("yep".to_string()),
                answer_file: None,
                stdin: false,
                expired: false,
                status: Status::Running,
            })
            .unwrap();
            let gates_dir = crate::paths::run_dir("run-a").unwrap().join("gates");
            assert!(
                gates_dir.join(format!("{expected}-gate-{n}.md")).exists(),
                "expected gate file {expected}-gate-{n}.md to exist"
            );
        }
    }

    #[test]
    fn gate_id_does_not_wrap_past_999() {
        let _home = TempHome::new();
        test_init("run-b", "hello-flow").unwrap();
        let gates_dir = crate::paths::run_dir("run-b").unwrap().join("gates");
        std::fs::write(gates_dir.join("999-old.md"), "stub").unwrap();
        std::fs::write(gates_dir.join("1000-newer.md"), "## Answer\n\n_unanswered_\n").unwrap();
        // A stray, non-numeric-prefixed file must be ignored by the scan.
        std::fs::write(gates_dir.join("not-a-gate.md"), "stub").unwrap();

        open_gate(OpenGateArgs {
            id: "run-b".to_string(),
            slug: "next".to_string(),
            question: Some("well?".to_string()),
            question_file: None,
            stdin: false,
            expires_at: None,
            expires_in: None,
        })
        .unwrap();

        assert!(gates_dir.join("1001-next.md").exists(), "next gate id must be 1001, not wrapped to 001");
        // The pre-existing 1000-newer.md must be untouched, not overwritten.
        let body = std::fs::read_to_string(gates_dir.join("1000-newer.md")).unwrap();
        assert!(body.contains("_unanswered_"));
    }

    #[test]
    fn arm_timer_rejects_a_check_every_that_wont_fit_before_the_wake() {
        let _home = TempHome::new();
        test_init("run-check", "hello-flow").unwrap();
        let err = arm_timer(ArmTimerArgs {
            id: "run-check".to_string(),
            at: None,
            seconds: Some(60),
            status: Status::Sleeping,
            note: None,
            check_script: Some("artifacts/check.sh".to_string()),
            check_every: Some(60),
        })
        .unwrap_err();
        assert!(err.to_string().contains("must be less than"), "got: {err}");
    }

    #[test]
    fn arm_timer_rejects_one_of_check_script_or_check_every_without_the_other() {
        let _home = TempHome::new();
        test_init("run-check-2", "hello-flow").unwrap();
        let err = arm_timer(ArmTimerArgs {
            id: "run-check-2".to_string(),
            at: None,
            seconds: Some(3600),
            status: Status::Sleeping,
            note: None,
            check_script: Some("artifacts/check.sh".to_string()),
            check_every: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("both --check-script and --check-every"), "got: {err}");
    }

    #[test]
    fn arm_timer_with_a_valid_check_persists_it() {
        let _home = TempHome::new();
        test_init("run-check-3", "hello-flow").unwrap();
        arm_timer(ArmTimerArgs {
            id: "run-check-3".to_string(),
            at: None,
            seconds: Some(3600),
            status: Status::Sleeping,
            note: None,
            check_script: Some("artifacts/check.sh".to_string()),
            check_every: Some(900),
        })
        .unwrap();
        let check = read_run("run-check-3").unwrap().check.expect("check must be persisted");
        assert_eq!(check.script, "artifacts/check.sh");
        assert_eq!(check.every_seconds, 900);
        assert_eq!(check.consecutive_no_change, 0);
        assert!(check.last_result.is_none());
    }

    #[test]
    fn arm_timer_without_a_check_script_clears_a_previous_one() {
        let _home = TempHome::new();
        test_init("run-check-4", "hello-flow").unwrap();
        arm_timer(ArmTimerArgs {
            id: "run-check-4".to_string(),
            at: None,
            seconds: Some(3600),
            status: Status::Sleeping,
            note: None,
            check_script: Some("artifacts/check.sh".to_string()),
            check_every: Some(900),
        })
        .unwrap();
        arm_timer(ArmTimerArgs {
            id: "run-check-4".to_string(),
            at: None,
            seconds: Some(3600),
            status: Status::Sleeping,
            note: None,
            check_script: None,
            check_every: None,
        })
        .unwrap();
        assert!(read_run("run-check-4").unwrap().check.is_none(), "a plain re-arm must not leave a stale check");
    }

    #[test]
    fn record_check_no_change_advances_the_streak_and_does_not_journal() {
        let _home = TempHome::new();
        test_init("run-check-5", "hello-flow").unwrap();
        arm_timer(ArmTimerArgs {
            id: "run-check-5".to_string(),
            at: None,
            seconds: Some(3600),
            status: Status::Sleeping,
            note: None,
            check_script: Some("artifacts/check.sh".to_string()),
            check_every: Some(900),
        })
        .unwrap();
        record_check("run-check-5", CheckResult::NoChange, Some("quiet".to_string())).unwrap();
        let check = read_run("run-check-5").unwrap().check.unwrap();
        assert_eq!(check.consecutive_no_change, 1);
        assert_eq!(check.last_result, Some(CheckResult::NoChange));
        let journal =
            std::fs::read_to_string(crate::paths::run_dir("run-check-5").unwrap().join("journal.jsonl")).unwrap();
        assert!(!journal.contains("check-ran"));
    }

    #[test]
    fn record_check_changed_resets_the_streak_and_journals() {
        let _home = TempHome::new();
        test_init("run-check-6", "hello-flow").unwrap();
        arm_timer(ArmTimerArgs {
            id: "run-check-6".to_string(),
            at: None,
            seconds: Some(3600),
            status: Status::Sleeping,
            note: None,
            check_script: Some("artifacts/check.sh".to_string()),
            check_every: Some(900),
        })
        .unwrap();
        record_check("run-check-6", CheckResult::NoChange, None).unwrap();
        record_check("run-check-6", CheckResult::Changed, Some("3 new comments".to_string())).unwrap();
        let check = read_run("run-check-6").unwrap().check.unwrap();
        assert_eq!(check.consecutive_no_change, 0, "a real change resets the no-change streak");
        assert_eq!(check.last_result, Some(CheckResult::Changed));
        let journal =
            std::fs::read_to_string(crate::paths::run_dir("run-check-6").unwrap().join("journal.jsonl")).unwrap();
        assert!(journal.contains("check-ran"));
        assert!(journal.contains("3 new comments"));
    }

    #[test]
    fn tick_default_limit_is_24() {
        let _home = TempHome::new();
        test_init("run-c", "hello-flow").unwrap();
        for _ in 0..23 {
            tick(TickArgs { id: "run-c".to_string(), progress: false, note: None }).unwrap();
        }
        let state = read_run("run-c").unwrap();
        assert_eq!(state.ticks_without_progress, 23);
        tick(TickArgs { id: "run-c".to_string(), progress: false, note: None }).unwrap();
        let state = read_run("run-c").unwrap();
        assert_eq!(state.ticks_without_progress, 24);
    }

    #[test]
    fn tick_explicit_zero_limit_never_exhausts() {
        let _home = TempHome::new();
        test_init("run-d", "improve-flow").unwrap();
        transaction("run-d", |_p, state| {
            state.policy.max_ticks_without_progress = 0;
            Ok(())
        })
        .unwrap();
        for _ in 0..30 {
            tick(TickArgs { id: "run-d".to_string(), progress: false, note: None }).unwrap();
        }
        let state = read_run("run-d").unwrap();
        assert_eq!(state.ticks_without_progress, 30);
        // exhausted is only observable via tick's own stdout in the CLI; re-derive the
        // same rule tick() uses to confirm the explicit-zero policy stays "never exhausted".
        let limit = state.policy.max_ticks_without_progress;
        assert_eq!(limit, 0);
        let exhausted = limit > 0 && (state.ticks_without_progress as i64) >= limit;
        assert!(!exhausted, "an explicit policy of 0 must never be treated as exhausted");
    }

    #[test]
    fn tick_explicit_five_exhausts_at_five() {
        let _home = TempHome::new();
        test_init("run-e", "improve-flow").unwrap();
        transaction("run-e", |_p, state| {
            state.policy.max_ticks_without_progress = 5;
            Ok(())
        })
        .unwrap();
        for _ in 0..4 {
            tick(TickArgs { id: "run-e".to_string(), progress: false, note: None }).unwrap();
        }
        let state = read_run("run-e").unwrap();
        assert_eq!(state.ticks_without_progress, 4);
        let limit = state.policy.max_ticks_without_progress;
        assert!(!(limit > 0 && 4 >= limit));
        tick(TickArgs { id: "run-e".to_string(), progress: false, note: None }).unwrap();
        let state = read_run("run-e").unwrap();
        assert_eq!(state.ticks_without_progress, 5);
        assert!(limit > 0 && 5 >= limit);
    }

    #[test]
    fn tick_progress_resets_counter() {
        let _home = TempHome::new();
        test_init("run-f", "hello-flow").unwrap();
        for _ in 0..5 {
            tick(TickArgs { id: "run-f".to_string(), progress: false, note: None }).unwrap();
        }
        tick(TickArgs { id: "run-f".to_string(), progress: true, note: None }).unwrap();
        let state = read_run("run-f").unwrap();
        assert_eq!(state.ticks_without_progress, 0);
    }
}
