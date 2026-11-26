//! `otto sandbox observe <run>`, `otto sandbox enforce <run>` and `otto sandbox off <run>` (plans/v3.md, 1.7).
//!
//! Moving a run into the sandbox is two things that must agree: its record at `$OTTO_HOME/sandbox/runs/<id>.json`
//! and the launcher its `run.json` names. A run where they differ is refused at its next wake, so these write
//! both, and undo the first if the second fails. They are a person's alone: a wake that could run them could
//! open its own sandbox, so they refuse when started from a wake or a chat, and the profile closes
//! `$OTTO_HOME/sandbox/` to writes besides.
//!
//! Nothing new is kept anywhere an older binary would trip on: the sandboxed launcher is an ordinary entry in
//! `config.json` named `sandbox-<the launcher the run had>`, and that name is how `off` knows what to put back.

use super::{Mode, Record};
use crate::config::LauncherDef;
use crate::error::OttoError;
use crate::liveness::Liveness as _;
use crate::state::RunState;
use std::path::{Path, PathBuf};

/// In front of a launcher's name, it names that launcher's sandboxed twin.
const PREFIX: &str = "sandbox-";

#[derive(clap::Args, Debug)]
pub struct OnArgs {
    /// The run: its number or name (a unique prefix of the name will do)
    pub id: String,
    /// Also open this path to reads (repeatable). Added to what the record already lists
    #[arg(long, value_name = "PATH")]
    pub read: Vec<String>,
    /// Also open this path to writes, and so reads (repeatable). Added to what the record already lists
    #[arg(long, value_name = "PATH")]
    pub write: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct OffArgs {
    /// The run: its number or name (a unique prefix of the name will do)
    pub id: String,
}

/// Refused from a wake or a chat, before anything is read or written.
fn person_only(what: &str) -> Result<(), OttoError> {
    if std::env::var_os("OTTO_RUN_ID").is_some() || std::env::var_os(crate::chat::CHAT_ENV).is_some() {
        return Err(OttoError::usage(format!(
            "`otto sandbox {what}` is the person's to run: turning a run's sandbox on or off is refused from a wake or a chat, and nothing was changed. Ask them to run it"
        )));
    }
    Ok(())
}

fn mode_word(mode: Mode) -> &'static str {
    match mode {
        Mode::Report => "report",
        Mode::Enforce => "enforce",
    }
}

fn status_word(state: &RunState) -> &'static str {
    crate::state::commands::status_str(state.status)
}

/// A launcher of exactly this name (ignoring case), never a prefix of one: `off` must not put back a
/// launcher that only resembles the one the run had.
fn exact(name: &str) -> Result<Option<LauncherDef>, OttoError> {
    Ok(crate::config::load()?.launchers().into_iter().find(|l| l.name.eq_ignore_ascii_case(name)))
}

/// `command` with the wrapper put in front of its last `claude`: the words before it (an `env VAR=…`) stay
/// where they are, outside the sandbox, and so do any after.
fn wrapped_command(prior: &LauncherDef, wrapper: &Path) -> Result<String, OttoError> {
    let wrapper = wrapper.display().to_string();
    if wrapper.split_whitespace().count() != 1 {
        return Err(OttoError::usage(format!("the wrapper's path {wrapper:?} has a space in it, and a launcher's command is split on spaces: nothing was changed")));
    }
    let mut words = prior.command_words();
    let Some(at) = words.iter().rposition(|word| Path::new(word).file_name().is_some_and(|name| name == "claude")) else {
        return Err(OttoError::usage(format!(
            "the launcher {:?} runs `{}`, which has no word `claude` for the wrapper to go in front of. Nothing was changed. \
             Add a launcher named {:?} to {} whose command goes through {wrapper}, and run this again",
            prior.name,
            prior.command,
            format!("{PREFIX}{}", prior.name),
            crate::config::config_path().display(),
        )));
    };
    words.splice(at..at, [wrapper, "--".to_string()]);
    Ok(words.join(" "))
}

/// The sandboxed launcher for a run now under `current`, and whether it has yet to be added to `config.json`.
/// One already through the wrapper is kept: observe then enforce, or a launcher the person made by hand.
fn sandboxed_for(current: &LauncherDef) -> Result<(LauncherDef, bool), OttoError> {
    if super::through_wrapper(current) {
        return Ok((current.clone(), false));
    }
    let name = format!("{PREFIX}{}", current.name);
    if let Some(have) = exact(&name)? {
        if !super::through_wrapper(&have) {
            return Err(OttoError::conflict(format!(
                "the launcher {name:?} in {} runs `{}`, which doesn't go through {}: nothing was changed. Fix or remove it, and run this again",
                crate::config::config_path().display(),
                have.command,
                super::dir().join(super::WRAPPER).display(),
            )));
        }
        return Ok((have, false));
    }
    let command = wrapped_command(current, &super::dir().join(super::WRAPPER))?;
    // The wrapper ignores the grants otto passes (the record decides), and takes them only ahead of its `--`.
    // A command with a `--` of its own would hand them to whatever is in front, so it gets none it didn't have.
    let grant_flag = current.grant_flag.clone().or_else(|| (!current.command_words().iter().any(|w| w == "--")).then(|| "--allow".to_string()));
    Ok((LauncherDef { name, command, grant_flag }, true))
}

/// Add `given` to `list`, leaving out what it already has (compared as the paths they come to).
fn add_paths(list: &mut Vec<String>, given: &[String], home: &Path) -> Result<(), OttoError> {
    for path in given {
        let wanted = super::normalize(path, home).map_err(|e| OttoError::usage(format!("{e}. Nothing was changed")))?;
        if !list.iter().any(|have| super::normalize(have, home).is_ok_and(|have| have == wanted)) {
            list.push(path.clone());
        }
    }
    Ok(())
}

fn listed(paths: &[String]) -> String {
    if paths.is_empty() {
        return "    (nothing)\n".to_string();
    }
    paths.iter().map(|p| format!("    {p}\n")).collect()
}

/// Put the record file back as it was before this command touched it.
fn restore(path: &Path, before: Option<&str>) -> Result<(), OttoError> {
    match before {
        Some(text) => crate::state::write_atomic(path, text),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        },
    }
}

/// A run refused for disagreeing with its record, which agrees now (or has none): back to how it was, by the
/// same path `otto wake` takes. A run waiting on Claude's sign-in is left waiting: that isn't settled here.
fn lift_if_agreeing(id: &str) -> Result<Option<String>, OttoError> {
    let state = crate::state::read_run(id)?;
    if !state.blocked.as_ref().is_some_and(|b| b.sandbox) || state.gate.as_ref().is_some_and(|g| g.slug == super::SIGN_IN_GATE) {
        return Ok(None);
    }
    let dir = crate::paths::run_dir(id)?;
    let resolved = crate::wake::launcher::resolve(&state);
    if let Some(why) = super::disagreement(&dir, &state, resolved.as_ref()) {
        return Ok(Some(format!("It is still blocked, because it still disagrees with its record: {why}")));
    }
    super::lift_refusal(id, &state)?;
    Ok(Some("It was blocked for disagreeing with its sandbox record; they agree now, so it is no longer blocked.".to_string()))
}

fn when_it_applies(id: &str) -> &'static str {
    if crate::liveness::LockLiveness.probe(id).is_busy() {
        "A wake is running now and is not touched: this applies from the next wake."
    } else {
        "It applies from the next wake."
    }
}

fn refuse_if_over(id: &str, state: &RunState, what: &str) -> Result<(), OttoError> {
    if state.status.is_terminal() {
        return Err(OttoError::conflict(format!(
            "{} is {}, so it has no next wake for `otto sandbox {what}` to apply to: nothing was changed. `otto resume {}` first if it is to run again",
            crate::paths::label_of(id),
            status_word(state),
            crate::paths::short_id(id),
        )));
    }
    Ok(())
}

/// `otto sandbox observe` and `otto sandbox enforce`.
pub fn on(mode: Mode, args: OnArgs) -> Result<(), OttoError> {
    let what = if mode == Mode::Report { "observe" } else { "enforce" };
    person_only(what)?;
    let id = crate::paths::resolve_run_id(&args.id)?;
    let state = crate::state::read_run(&id)?;
    refuse_if_over(&id, &state, what)?;
    let home = crate::paths::home_dir();
    let path: PathBuf = super::record_path(&id);

    let current = crate::config::launcher(&state.launcher.kind)?;
    let (sandboxed, missing) = sandboxed_for(&current)?;

    let before = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !super::has_record(&id) => None,
        Err(e) => return Err(OttoError::conflict(format!("the sandbox record {} can't be read: {e}. Nothing was changed", path.display()))),
    };
    let first = before.is_none();
    let mut record = match &before {
        // What it lists is kept, hand edits and all: only the mode and the launcher are this command's to say.
        Some(text) => serde_json::from_str::<Record>(text).map_err(|e| {
            OttoError::conflict(format!(
                "the sandbox record {} can't be used: {e}. Nothing was changed. Fix it, or `otto sandbox off {}` to remove it and start again",
                path.display(),
                crate::paths::short_id(&id)
            ))
        })?,
        // The run's own working directory and repos, and nothing more: observing is how what's missing is found.
        None => Record {
            launcher: sandboxed.name.clone(),
            workdir: crate::config::workdir_for(&state).map(|dir| dir.display().to_string()),
            read: Vec::new(),
            write: state.launcher.repos.clone(),
            mode,
        },
    };
    record.launcher = sandboxed.name.clone();
    record.mode = mode;
    add_paths(&mut record.read, &args.read, &home)?;
    add_paths(&mut record.write, &args.write, &home)?;

    // The launcher first: defined and unused, it changes nothing. Then the record, then run.json; if run.json
    // can't be written the record goes back, so the two are never left disagreeing by this.
    // Inside a sandbox neither can be written (the profile closes both), and the bare error names a temporary
    // file: say what it was, that nothing changed, and where to run this.
    let outside = format!("Inside a sandbox otto's sandbox records and config.json are closed to writing: run `otto sandbox {what} {}` outside a sandbox, in a terminal of your own", crate::paths::short_id(&id));
    if missing {
        crate::config::add_launcher(&sandboxed).map_err(|e| {
            OttoError::conflict(format!("the launcher {} couldn't be added to {} ({}). Nothing was changed. {outside}", sandboxed.name, crate::config::config_path().display(), e.message))
        })?;
    }
    let text = serde_json::to_string_pretty(&record)? + "\n";
    let written = match path.parent() {
        Some(parent) => std::fs::create_dir_all(parent).map_err(|e| e.to_string()),
        None => Ok(()),
    }
    .and_then(|()| crate::state::write_atomic(&path, &text).map_err(|e| e.message));
    if let Err(e) = written {
        let added = if missing { format!("The launcher {} was added to {} and nothing uses it; the run's record and run.json are as they were", sandboxed.name, crate::config::config_path().display()) } else { "Nothing was changed".to_string() };
        return Err(OttoError::conflict(format!("the sandbox record {} couldn't be written ({e}). {added}. {outside}", path.display())));
    }
    let event = |previous_launcher: String| {
        let (mode, launcher) = (mode_word(mode).to_string(), sandboxed.name.clone());
        match mode.as_str() {
            "report" => crate::event::Event::SandboxObserved { mode, launcher, previous_launcher },
            _ => crate::event::Event::SandboxEnforced { mode, launcher, previous_launcher },
        }
    };
    let switched = crate::state::transaction(&id, |run_path, state| {
        let previous = std::mem::replace(&mut state.launcher.kind, sandboxed.name.clone());
        crate::event::record(run_path, &event(previous))
    });
    if let Err(e) = switched {
        let undone = match restore(&path, before.as_deref()) {
            Ok(()) => "the record was put back as it was, so nothing changed".to_string(),
            Err(undo) => format!("and the record at {} couldn't be put back ({undo}): remove or fix it by hand, or the run is refused at its next wake", path.display()),
        };
        return Err(OttoError::conflict(format!("{}'s run.json couldn't be updated ({}); {undone}", crate::paths::label_of(&id), e.message)));
    }

    let label = crate::paths::label_of(&id);
    let short = crate::paths::short_id(&id);
    match (mode, first) {
        (Mode::Report, _) => println!("{label} is under observation: nothing is blocked, and what the record doesn't allow is logged."),
        (Mode::Enforce, false) => println!("{label}'s sandbox is enforced: what the record doesn't allow is blocked."),
        (Mode::Enforce, true) => println!("{label} had no sandbox record, so a first one was written, and it is enforced: what it doesn't allow is blocked, without having been observed first."),
    }
    println!("  record    {} ({})", path.display(), if first { "written" } else { "kept, with what it listed" });
    println!("  mode      {}", mode_word(mode));
    println!("  launcher  {} ({}{})", sandboxed.name, if current.name == sandboxed.name { "as before".to_string() } else { format!("was {}", current.name) }, if missing { format!(", added to {}", crate::config::config_path().display()) } else { String::new() });
    println!("  command   {}", sandboxed.command);
    println!("  starts in {} (which opens nothing)", record.workdir.as_deref().unwrap_or("(nowhere set)"));
    print!("  open to read, beyond what every sandboxed run can read:\n{}", listed(&record.read));
    print!("  open to write, and so read:\n{}", listed(&record.write));
    println!("{}", when_it_applies(&id));
    if let Some(said) = lift_if_agreeing(&id)? {
        println!("{said}");
    }
    println!("To open more, run this again with --read <path> or --write <path>, or edit the record. To undo: otto sandbox off {short}");
    Ok(())
}

/// `otto sandbox off`.
pub fn off(args: OffArgs) -> Result<(), OttoError> {
    person_only("off")?;
    let id = crate::paths::resolve_run_id(&args.id)?;
    let state = crate::state::read_run(&id)?;
    refuse_if_over(&id, &state, "off")?;
    let label = crate::paths::label_of(&id);
    let path: PathBuf = super::record_path(&id);
    let has_record = super::has_record(&id);

    // A launcher that no longer exists can't be asked whether it is the wrapper's: its name is all there is.
    let current = crate::config::launcher(&state.launcher.kind);
    let current_name = current.as_ref().map_or(state.launcher.kind.clone(), |l| l.name.clone());
    let sandboxed = current.as_ref().map_or(current_name.starts_with(PREFIX), super::through_wrapper);
    if !has_record && !sandboxed {
        println!("{label} isn't sandboxed: it has no record at {} and its launcher is {current_name}. Nothing was changed.", path.display());
        return Ok(());
    }
    let back = if sandboxed {
        let Some(name) = current_name.strip_prefix(PREFIX) else {
            return Err(OttoError::conflict(format!(
                "{label} is under the launcher {current_name:?}, which wasn't made by `otto sandbox observe` or `enforce`, so which launcher it had before isn't known: nothing was changed. \
                 Put it back by hand: remove {} and set launcher.kind in its run.json",
                path.display()
            )));
        };
        match exact(name)? {
            Some(def) if !super::through_wrapper(&def) => def.name,
            Some(_) => return Err(OttoError::conflict(format!("the launcher {name:?} goes through the sandbox wrapper too, so putting {label} back under it wouldn't take it out: nothing was changed"))),
            None => {
                return Err(OttoError::conflict(format!(
                    "{label} was under the launcher {name:?} before, and {} no longer has one of that name: nothing was changed. Add it back, or set launcher.kind in the run's run.json by hand and remove {}",
                    crate::config::config_path().display(),
                    path.display()
                )))
            }
        }
    } else {
        current_name.clone()
    };

    // The record first: without it, a run still under the wrapper is refused there, so neither order can
    // leave a wake running unsandboxed by mistake or sandboxed against no record for long.
    let before = std::fs::read_to_string(&path).ok();
    if has_record {
        std::fs::remove_file(&path).map_err(|e| OttoError::conflict(format!("the sandbox record {} couldn't be removed: {e}. Nothing was changed", path.display())))?;
    }
    let switched = crate::state::transaction(&id, |run_path, state| {
        let previous = std::mem::replace(&mut state.launcher.kind, back.clone());
        crate::event::record(run_path, &crate::event::Event::SandboxOff { launcher: back.clone(), previous_launcher: previous })
    });
    if let Err(e) = switched {
        let undone = match (&before, has_record) {
            (Some(text), true) => match restore(&path, Some(text)) {
                Ok(()) => "the record was put back as it was, so nothing changed".to_string(),
                Err(undo) => format!("and the record at {} couldn't be put back ({undo})", path.display()),
            },
            (None, true) => format!("and the record at {} is gone", path.display()),
            (_, false) => "nothing changed".to_string(),
        };
        return Err(OttoError::conflict(format!("{label}'s run.json couldn't be updated ({}); {undone}", e.message)));
    }

    println!("{label} is out of the sandbox.");
    println!("  record    {} ({})", path.display(), if has_record { "removed" } else { "there was none" });
    println!("  launcher  {back} ({})", if back == current_name { "as before".to_string() } else { format!("was {current_name}") });
    if let Ok(def) = crate::config::launcher(&back) {
        println!("  command   {}", def.command);
    }
    if sandboxed && back != current_name {
        println!("  The launcher {current_name} stays in {}: other runs may be under it.", crate::config::config_path().display());
    }
    println!("{}", when_it_applies(&id));
    if let Some(said) = lift_if_agreeing(&id)? {
        println!("{said}");
    }
    let short = crate::paths::short_id(&id);
    println!("To put it back: otto sandbox observe {short} (or enforce). What the record listed is not kept.");
    Ok(())
}
