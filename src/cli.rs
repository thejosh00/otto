use clap::{Parser, Subcommand};

use crate::state::authorize::{AuthorizeArgs, CheckAuthorizedArgs};
use crate::state::commands::{
    ArmTimerArgs, CloseGateArgs, GetArgs, HandoffArgs, InitArgs, ListArgs, LogArgs, OpenGateArgs,
    RecordFactArgs, SetPhaseArgs, SetStatusArgs, TailArgs, TickArgs,
};
use crate::state::locks::{LockArgs, LocksArgs, UnlockArgs};

#[derive(Parser, Debug)]
#[command(name = "otto", version, about = "Durable long-horizon agent runs: wrap a skill, a runbook, or a goal")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start a run: wrap a skill, a runbook, or a bare goal, and take the first wake.
    Run(crate::human::RunArgs),
    /// Every live run, and what each is waiting on.
    Ls(crate::human::LsArgs),
    /// Where one run stands, and the open question in full.
    Show(crate::human::ShowArgs),
    /// Answer the open gate, then continue the run.
    Answer(crate::human::AnswerArgs),
    /// Leave a note for the run, gated or not; its next wake reads it, verbatim.
    Note(crate::human::NoteArgs),
    /// Give a run a check script poke runs instead of a wake, see it, or remove it.
    Check(crate::human::CheckArgs),
    /// The run's journal, readably.
    Logs(crate::human::LogsArgs),
    /// Watch the wake that is running right now.
    Attach(crate::human::AttachArgs),
    /// See or change how often a run wakes.
    Period(crate::human::PeriodArgs),
    /// Retire a run.
    Stop(crate::human::StopArgs),
    /// Bring a stopped or failed run back, and wake it.
    Resume(crate::human::ResumeArgs),
    /// Run one wake: spawn the model, then validate what it left behind.
    Wake(crate::wake::WakeArgs),
    /// Start the wakes that are due, and clean up after the ones that are over. Run this from
    /// launchd (see `otto agent start`) every ~5 minutes, or by hand.
    Poke(crate::poke::PokeArgs),
    /// The background reviver that wakes runs whose timer has passed.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// The web UI: everything above, in a browser, on 127.0.0.1.
    Serve(crate::server::ServeArgs),
    /// The web UI as a launchd service: running from login, restarted if it exits.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Symlink otto's Claude Code skills into ~/.claude/skills/.
    Install(crate::install::InstallArgs),
    /// The only writer of a run's durable state.
    State {
        #[command(subcommand)]
        command: StateCommand,
    },
}

/// The launchd agent lives under its own noun so that `otto stop <run>` can mean the obvious
/// thing. v1 spelled these `otto start` / `otto stop`, which collided with retiring a run — a
/// collision DESIGN.md §16 had in it too.
#[derive(Subcommand, Debug)]
pub enum AgentCommand {
    /// Install (if missing) and start it
    Start,
    /// Stop it
    Stop,
    /// Is it loaded, when did it last poke, and where is its plist
    Status,
}

#[derive(Subcommand, Debug)]
pub enum ServiceCommand {
    /// Install (if missing) and start `otto serve` under launchd
    Start {
        /// Port on 127.0.0.1 to listen on
        #[arg(long, default_value_t = crate::server::DEFAULT_PORT)]
        port: u16,
    },
    /// Stop it; it stays stopped until the next `otto service start`
    Stop,
    /// Is it loaded, on which port, and where are its plist and log
    Status,
}

#[derive(Subcommand, Debug)]
pub enum StateCommand {
    /// Create a run directory
    Init(InitArgs),
    /// Print run.json, or one dotted field
    Get(GetArgs),
    /// One line per run
    List(ListArgs),
    /// Enter a phase
    SetPhase(SetPhaseArgs),
    /// Change status
    SetStatus(SetStatusArgs),
    /// Write re-derived facts
    RecordFact(RecordFactArgs),
    /// Write the gate file and stop the run on it
    OpenGate(OpenGateArgs),
    /// Record the answer verbatim and clear the gate
    CloseGate(CloseGateArgs),
    /// Record nextWakeAt. This is the whole timer: poke reads it, nothing else is needed
    ArmTimer(ArmTimerArgs),
    /// Account for one timer tick
    Tick(TickArgs),
    /// Runs whose nextWakeAt has passed
    Due,
    /// Record that a specific commit may go outward
    Authorize(AuthorizeArgs),
    /// Refuse an outward action that nobody approved
    CheckAuthorized(CheckAuthorizedArgs),
    /// Claim a repo for this run
    Lock(LockArgs),
    /// Release this run's repo lock(s)
    Unlock(UnlockArgs),
    /// Who holds which repo
    Locks(LocksArgs),
    /// Rewrite handoff.md — the only thing the next wake gets for free (capped)
    Handoff(HandoffArgs),
    /// Append one journal line
    Log(LogArgs),
    /// Last N journal lines
    Tail(TailArgs),
}
