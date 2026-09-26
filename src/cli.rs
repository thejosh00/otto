use clap::{Parser, Subcommand};

use crate::state::authorize::{AuthorizeArgs, CheckAuthorizedArgs};
use crate::state::commands::{
    ArmTimerArgs, CloseGateArgs, SetCheckArgs, GetArgs, HandoffArgs, InitArgs, ListArgs, LogArgs, OpenGateArgs,
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
    /// Give the run a standing check script, or remove the one a wake gave it
    SetCheck(SetCheckArgs),
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

/// `docs/reference/cli.md` is generated from the help text above, so the reference cannot drift
/// from the binary: this test fails when a flag changes and the file was not regenerated.
///
/// ```text
/// OTTO_UPDATE_DOCS=1 cargo test cli_reference
/// ```
#[cfg(test)]
mod reference {
    use super::Cli;
    use clap::CommandFactory;

    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/reference/cli.md");

    fn section(out: &mut String, cmd: &clap::Command, path: &str, depth: usize) {
        if cmd.is_hide_set() || cmd.get_name() == "help" {
            return;
        }
        out.push_str(&format!("{} `{path}`\n\n", "#".repeat(depth)));
        let help = cmd.clone().bin_name(path).render_long_help().to_string();
        out.push_str("```text\n");
        out.push_str(help.trim_end());
        out.push_str("\n```\n\n");
        for sub in cmd.get_subcommands() {
            section(out, sub, &format!("{path} {}", sub.get_name()), (depth + 1).min(3));
        }
    }

    fn render() -> String {
        let mut cmd = Cli::command();
        cmd.build();
        let mut out = String::from(
            "# CLI reference\n\n\
             <!-- Generated from otto's own --help by `OTTO_UPDATE_DOCS=1 cargo test cli_reference`. \
             Do not edit by hand. -->\n\n\
             Every command and flag, exactly as `--help` prints it. For what the commands are *for*, \
             start with [concepts](../concepts.md) and the [guides](../README.md#guides).\n\n\
             Every `<ID>` takes the full run id, a unique prefix of it, or a unique prefix of its slug \
             (`otto show verify`). Exit codes: `0` ok, `1` usage, `2` state conflict, `3` no such run.\n\n",
        );
        for sub in cmd.get_subcommands() {
            section(&mut out, sub, &format!("otto {}", sub.get_name()), 2);
        }
        out.trim_end().to_string() + "\n"
    }

    /// Every argument a person can see says what it is for — the reference is only as good as
    /// the help it is generated from.
    #[test]
    fn every_visible_argument_has_help() {
        fn walk(cmd: &clap::Command, path: &str, missing: &mut Vec<String>) {
            if cmd.is_hide_set() {
                return;
            }
            for arg in cmd.get_arguments() {
                let id = arg.get_id().as_str();
                if arg.is_hide_set() || id == "help" || id == "version" {
                    continue;
                }
                if arg.get_help().is_none() && arg.get_long_help().is_none() {
                    missing.push(format!("{path} {id}"));
                }
            }
            for sub in cmd.get_subcommands() {
                walk(sub, &format!("{path} {}", sub.get_name()), missing);
            }
        }
        let mut missing = Vec::new();
        walk(&Cli::command(), "otto", &mut missing);
        assert!(missing.is_empty(), "arguments with no help text: {missing:?}");
    }

    #[test]
    fn cli_reference_is_up_to_date() {
        let fresh = render();
        if std::env::var_os("OTTO_UPDATE_DOCS").is_some() {
            std::fs::write(PATH, &fresh).expect("write docs/reference/cli.md");
            return;
        }
        let on_disk = std::fs::read_to_string(PATH).unwrap_or_default();
        assert!(
            on_disk == fresh,
            "docs/reference/cli.md is out of date with the CLI — regenerate it with \
             `OTTO_UPDATE_DOCS=1 cargo test cli_reference`"
        );
    }
}
