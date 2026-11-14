mod agents;
mod changes;
mod chat;
mod check_history;
mod cleanup;
mod cli;
mod clock;
mod config;
mod core;
mod detach;
#[cfg(test)]
mod doc_examples;
mod error;
mod event;
mod exec;
mod feedback;
mod fnv;
mod gate;
mod human;
mod inbox;
mod install;
mod launchd;
mod liveness;
mod messages;
mod logs;
mod notes;
mod notify;
mod outputs;
mod paths;
mod plain;
mod poke;
mod progress;
mod sandbox;
mod server;
mod spawner;
mod state;
mod story;
mod todo;
mod wake;
mod why;

use cli::{AgentCommand, Cli, Command, ConfigCommand, SandboxCommand, ServiceCommand, StateCommand};
use clap::Parser;
use error::OttoError;

/// What a shell reports for a command that SIGPIPE ended (128 + 13), which is how every other
/// tool in a pipeline ends when its reader leaves. Not 0: the listing was cut short, and a caller
/// who asked to know (`set -o pipefail`) should be able to tell that from a whole one.
const READER_LEFT: i32 = 128 + libc::SIGPIPE;

/// `otto feedback --all | head -1`: once `head` has gone, the next `println!` fails and std
/// panics with "failed printing to stdout: Broken pipe (os error 32)". That is no fault of otto's
/// and nothing to say, so that one panic ends the process quietly; any other is shown as ever.
///
/// SIGPIPE stays ignored, as Rust starts with it, rather than being put back to its default:
/// otto writes into pipes of its own (a token to a sandboxed child, tmux, launchers), where a
/// reader that died has to stay an error otto can report rather than the silent end of otto, and
/// the children it starts would inherit a changed disposition.
fn quiet_when_reader_left() {
    let shown = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let said = payload.downcast_ref::<String>().map(String::as_str).or_else(|| payload.downcast_ref::<&str>().copied());
        let broken_pipe = std::io::Error::from_raw_os_error(libc::EPIPE).to_string();
        if said.is_some_and(|said| said.strip_prefix("failed printing to stdout: ") == Some(broken_pipe.as_str())) {
            logs::finish_stamping();
            std::process::exit(READER_LEFT);
        }
        shown(info);
    }));
}

fn main() {
    quiet_when_reader_left();
    // Before any thread starts: `time` will only read the local offset in a single-threaded
    // process, and a run's time of day (`periodAt`) is on the local clock wherever it's computed.
    clock::local_offset();
    let cli = Cli::parse();
    if let Err(err) = run(cli.command) {
        eprintln!("otto: {err}");
        logs::finish_stamping();
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
        Command::Inbox(args) => human::inbox(args),
        Command::Outputs(args) => human::outputs(args),
        Command::Changes(args) => human::changes(args),
        Command::Answer(args) => human::answer(args),
        Command::Pause(args) => human::pause(args),
        Command::Note(args) => human::note(args),
        Command::Chat(args) => chat::chat(args),
        Command::Check(args) => human::check(args),
        Command::Logs(args) => human::logs(args),
        Command::Why(args) => why::why(args),
        Command::Usage(args) => human::usage(args),
        Command::Feedback(args) => human::feedback(args),
        Command::Attach(args) => human::attach(args),
        Command::Period(args) => human::period(args),
        Command::Policy(args) => human::policy(args),
        Command::Rename(args) => human::rename(args),
        Command::Renumber => human::renumber(),
        Command::Stop(args) => human::stop(args),
        Command::Resume(args) => human::resume(args),
        Command::Delete(args) => human::delete(args),
        Command::Cleanup(args) => human::cleanup(args),
        Command::Install(args) => install::install(args),
        Command::Sandbox { command } => match command {
            // As they were given, not as clap read them: it takes the first `--` away, and what's past that is claude's.
            SandboxCommand::Wrap { .. } => {
                let raw: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
                sandbox::wrap(raw.get(2..).unwrap_or_default())
            }
        },
        Command::Serve(args) => server::serve(args),
        Command::Agent { command } => match command {
            AgentCommand::Start => launchd::start(),
            AgentCommand::Stop => launchd::stop(),
            AgentCommand::Status => launchd::status(),
        },
        Command::Config { command } => match command {
            ConfigCommand::Workdir { dir } => human::config_workdir(dir),
            ConfigCommand::Lander { run, off } => human::config_lander(run, off),
            ConfigCommand::Process(args) => human::config_process(args),
            ConfigCommand::Project(args) => human::config_project(args),
        },
        Command::Service { command } => match command {
            ServiceCommand::Start { port } => launchd::service_start(port),
            ServiceCommand::Stop => launchd::service_stop(),
            ServiceCommand::Restart => launchd::service_restart(),
            ServiceCommand::Status => launchd::service_status(),
        },
    }
}

fn dispatch_state(command: StateCommand) -> Result<(), OttoError> {
    use state::{authorize, commands, locks};
    chat::refuse_state_write(&command, std::env::var(chat::CHAT_ENV).ok().as_deref())?;
    match command {
        StateCommand::Init(a) => commands::init(a),
        StateCommand::Get(a) => commands::get(a),
        StateCommand::List(a) => commands::list(a),
        StateCommand::SetPhase(a) => commands::set_phase(a),
        StateCommand::SetStatus(a) => commands::set_status(a),
        StateCommand::Verify(a) => commands::verify(a),
        StateCommand::RecordFact(a) => commands::record_fact(a),
        StateCommand::OpenGate(a) => commands::open_gate(a),
        StateCommand::CloseGate(a) => commands::close_gate(a),
        StateCommand::Message(a) => commands::message(a),
        StateCommand::Publish(a) => commands::publish(a),
        StateCommand::Stages(a) => commands::stages(a),
        StateCommand::OutputState(a) => commands::output_state(a),
        StateCommand::Change { command } => state::change::run(command),
        StateCommand::SetCheck(a) => commands::set_check(a),
        StateCommand::ArmTimer(a) => commands::arm_timer(a),
        StateCommand::Tick(a) => commands::tick(a),
        StateCommand::Feedback(a) => commands::feedback(a),
        StateCommand::Assign(a) => commands::assign(a),
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
