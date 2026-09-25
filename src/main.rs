mod cli;
mod clock;
mod config;
mod core;
mod detach;
mod error;
mod event;
mod exec;
mod gate;
mod human;
mod install;
mod launchd;
mod liveness;
mod notes;
mod notify;
mod paths;
mod poke;
mod server;
mod spawner;
mod state;
mod wake;

use cli::{AgentCommand, Cli, Command, StateCommand};
use clap::Parser;
use error::OttoError;

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli.command) {
        eprintln!("otto: {err}");
        std::process::exit(err.code);
    }
}

fn run(command: Command) -> Result<(), OttoError> {
    match command {
        Command::State { command } => dispatch_state(command),
        Command::Poke(args) => std::process::exit(poke::poke_run(args)),
        Command::Wake(args) => human::wake_command(args),
        Command::Run(args) => human::run(args),
        Command::Ls(args) => human::ls(args),
        Command::Show(args) => human::show(args),
        Command::Answer(args) => human::answer(args),
        Command::Note(args) => human::note(args),
        Command::Check(args) => human::check(args),
        Command::Logs(args) => human::logs(args),
        Command::Attach(args) => human::attach(args),
        Command::Stop(args) => human::stop(args),
        Command::Resume(args) => human::resume(args),
        Command::Install(args) => install::install(args),
        Command::Serve(args) => server::serve(args),
        Command::Agent { command } => match command {
            AgentCommand::Start => launchd::start(),
            AgentCommand::Stop => launchd::stop(),
            AgentCommand::Status => launchd::status(),
        },
    }
}

fn dispatch_state(command: StateCommand) -> Result<(), OttoError> {
    use state::{authorize, commands, locks};
    match command {
        StateCommand::Init(a) => commands::init(a),
        StateCommand::Get(a) => commands::get(a),
        StateCommand::List(a) => commands::list(a),
        StateCommand::SetPhase(a) => commands::set_phase(a),
        StateCommand::SetStatus(a) => commands::set_status(a),
        StateCommand::RecordFact(a) => commands::record_fact(a),
        StateCommand::OpenGate(a) => commands::open_gate(a),
        StateCommand::CloseGate(a) => commands::close_gate(a),
        StateCommand::ArmTimer(a) => commands::arm_timer(a),
        StateCommand::Tick(a) => commands::tick(a),
        StateCommand::Due => commands::due(),
        StateCommand::Authorize(a) => authorize::authorize(a),
        StateCommand::CheckAuthorized(a) => authorize::check_authorized(a),
        StateCommand::Lock(a) => locks::lock(a),
        StateCommand::Unlock(a) => locks::unlock(a),
        StateCommand::Locks(a) => locks::locks(a),
        StateCommand::Handoff(a) => commands::handoff(a),
        StateCommand::Log(a) => commands::log(a),
        StateCommand::Tail(a) => commands::tail(a),
    }
}
