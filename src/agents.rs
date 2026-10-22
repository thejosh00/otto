//! Standard agents: the runs otto itself needs, made for the person rather than by them.
//!
//! Landing changes takes a run of its own — the lander — and looking after the pull requests it
//! opens takes another, the babysitter. Setting up the lander used to be a page of the README: an
//! `otto run` with the right workflow, a check script, `otto config lander`. Nothing in that was the
//! person's to decide, so otto does it. Each standard agent is made once, the first
//! time otto finds it missing, and made **not started**: paused until the person starts it on the
//! Standard agents page (`otto resume`), because a run that pushes code shouldn't begin by itself.
//!
//! - **Listed apart.** A standard agent is marked with its `role`, and the person's own runs list
//!   without it; the Standard agents page is where they are.
//! - **otto's workflow, kept current.** What it wraps is written from the binary into
//!   `$OTTO_HOME/agents/` every time otto checks on them, so a new otto brings its agents' new
//!   instructions with it.
//! - **One each.** One with the role already — in any state — is the one; stopping it retires it
//!   and it stays retired until resumed. A lander named by hand before this existed is adopted, not
//!   doubled.
//! - **The repositories they need.** Each wake of one may work in every repository with a change
//!   in play (`grants`), which is exactly where landing and babysitting happen.

use crate::error::OttoError;
use crate::state::{RunEntry, RunState};
use std::path::PathBuf;

/// Why a standard agent is paused before anyone has started it.
pub const NOT_STARTED: &str = "not started yet — start it on the Standard agents page, or `otto resume`";

pub struct Agent {
    /// What its runs are marked with (`RunState::role`), and the name it is given.
    pub role: &'static str,
    /// What it does, for the page that lists it.
    pub about: &'static str,
    pub goal: &'static str,
    pub period: &'static str,
    /// The file its instructions are written to, under `$OTTO_HOME/agents/`.
    pub file: &'static str,
    pub instructions: &'static str,
    /// Its check script: free when there is nothing for it. `{otto}` is the otto that made it.
    pub check: &'static str,
}

pub const LANDER: Agent = Agent {
    role: "lander",
    about: "Lands the changes you approve — pushed to their base, or opened as a pull request — and cleans up after them. \
            The only agent that pushes.",
    goal: "Land the changes I approve, and clean up after them",
    period: "10m",
    file: "land-flow.md",
    instructions: include_str!("../workflows/land-flow.md"),
    check: "#!/bin/sh\n\
            # Nothing to land or clean up is nothing new; anything else, or otto failing, is a wake.\n\
            if [ \"$('{otto}' changes --due --json)\" = \"[]\" ]; then\n\
            \x20 echo \"nothing to land\"; exit 0\n\
            fi\n\
            echo \"something to land\"; exit 1\n",
};

pub const BABYSITTER: Agent = Agent {
    role: "babysitter",
    about: "Looks after every pull request otto opened until it merges or closes: records CI and reviews, tells a change's \
            author what reviewers ask, and notices when one is merged or closed on GitHub.",
    goal: "Look after every pull request otto opened, until it is merged or closed",
    period: "15m",
    file: "babysit-flow.md",
    instructions: include_str!("../workflows/babysit-flow.md"),
    check: "#!/bin/sh\n\
            # No pull request moved is nothing new; anything else, or otto failing, is a wake.\n\
            if [ \"$('{otto}' changes --watch --json)\" = \"[]\" ]; then\n\
            \x20 echo \"no pull request moved\"; exit 0\n\
            fi\n\
            echo \"a pull request moved\"; exit 1\n",
};

pub const AGENTS: &[&Agent] = &[&LANDER, &BABYSITTER];

fn dir() -> PathBuf {
    crate::paths::otto_home().join("agents")
}

/// What `ensure` did, a line each; nothing when everything was already there.
pub type Made = Vec<String>;

/// Make whatever standard agent is missing, not started, and bring every one's instructions up to
/// date. Cheap when there is nothing to do, so poke calls it on every pass.
pub fn ensure() -> Result<Made, OttoError> {
    let mut made = Vec::new();
    std::fs::create_dir_all(dir())?;
    // Poke, the server and `otto install` can all get here at once; one of them makes the agents.
    let lock = std::fs::OpenOptions::new().create(true).append(true).open(dir().join(".lock"))?;
    fs4::fs_std::FileExt::lock_exclusive(&lock)?;
    for agent in AGENTS {
        let file = dir().join(agent.file);
        if std::fs::read_to_string(&file).ok().as_deref() != Some(agent.instructions) {
            crate::state::write_atomic(&file, agent.instructions)?;
        }
    }
    let runs: Vec<RunState> = crate::state::read_all_runs()?
        .into_iter()
        .filter_map(|e| match e {
            RunEntry::Readable(s) => Some(s),
            RunEntry::Unreadable { .. } => None,
        })
        .collect();
    for agent in AGENTS {
        if runs.iter().any(|s| s.role.as_deref() == Some(agent.role)) {
            continue;
        }
        if let Some(id) = adoptable(agent, &runs)? {
            crate::state::transaction(&id, |_path, state| {
                state.role = Some(agent.role.to_string());
                Ok(())
            })?;
            made.push(format!("{} is the {} (already named in config.json)", crate::paths::short_id(&id), agent.role));
            continue;
        }
        let id = make(agent)?;
        made.push(format!("made the {}, {} — not started", agent.role, crate::paths::short_id(&id)));
    }
    let _ = fs4::fs_std::FileExt::unlock(&lock);
    Ok(made)
}

/// A run already doing this agent's job, named by hand before standard agents existed.
fn adoptable(agent: &Agent, runs: &[RunState]) -> Result<Option<String>, OttoError> {
    let named = match agent.role {
        "lander" => crate::config::lander()?,
        _ => None,
    };
    Ok(named.filter(|id| runs.iter().any(|s| &s.id == id && !s.status.is_terminal())))
}

fn make(agent: &Agent) -> Result<String, OttoError> {
    let instructions = dir().join(agent.file).display().to_string();
    let id = crate::state::commands::init_run(crate::state::commands::InitArgs {
        goal: agent.goal.to_string(),
        instructions: Some(instructions),
        perpetual: true,
        period: Some(agent.period.to_string()),
        name: Some(crate::paths::unique_name(agent.role)),
        ..Default::default()
    })?;
    crate::state::transaction(&id, |path, state| {
        state.role = Some(agent.role.to_string());
        // Asleep and due rather than `running` with no wake, as `otto run` leaves a run for the wake
        // it starts next: started, it is a check script's look first, which is free.
        state.status = crate::state::Status::Sleeping;
        state.next_wake_at = Some(crate::clock::Timestamp::now());
        let reason = Some(NOT_STARTED.to_string());
        state.paused = Some(crate::state::Pause { at: crate::clock::Timestamp::now(), until: None, reason: reason.clone() });
        crate::event::record(path, &crate::event::Event::Paused { until: None, reason })
    })?;
    // The otto making it, by its path: the reviver's PATH needn't have one, and a build that isn't
    // the installed one asks itself.
    let otto = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "otto".to_string());
    crate::core::set_check(&id, &agent.check.replace("{otto}", &otto), 0)?;
    if agent.role == LANDER.role && crate::config::lander()?.is_none() {
        crate::config::save_lander(Some(&id))?;
    }
    Ok(id)
}

/// One standard agent, for the page that lists them.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentView {
    pub role: &'static str,
    pub about: &'static str,
    /// Its run, as the runs list shows it; `None` until otto has made it.
    pub run: Option<crate::core::LsRow>,
}

pub fn view() -> Result<Vec<AgentView>, OttoError> {
    let rows = crate::core::list_runs(true)?.rows;
    Ok(AGENTS
        .iter()
        .map(|a| AgentView {
            role: a.role,
            about: a.about,
            // A retired one and a live one with the same role can't both exist (`ensure`), but the
            // live one leads if they somehow do.
            run: rows.iter().filter(|r| r.role.as_deref() == Some(a.role)).min_by_key(|r| r.terminal).cloned(),
        })
        .collect())
}

/// Whether a run is a standard agent nobody has started yet.
pub fn not_started(state: &RunState) -> bool {
    state.role.is_some() && state.paused.as_ref().is_some_and(|p| p.until.is_none() && p.reason.as_deref() == Some(NOT_STARTED))
}

/// The repositories a standard agent's wake may work in, beyond its own: every one with a change
/// not yet cleaned up after.
pub fn grants(state: &RunState) -> Vec<String> {
    if state.role.is_none() {
        return Vec::new();
    }
    crate::changes::repos_in_play()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_support::TempHome;

    fn agents() -> Vec<RunState> {
        crate::state::read_all_runs()
            .unwrap()
            .into_iter()
            .filter_map(|e| match e {
                RunEntry::Readable(s) if s.role.is_some() => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn makes_each_agent_once_not_started() {
        let _home = TempHome::new();
        let made = ensure().unwrap();
        assert_eq!(made.len(), AGENTS.len(), "{made:?}");
        let all = agents();
        assert_eq!(all.len(), AGENTS.len());
        let lander = all.iter().find(|s| s.role.as_deref() == Some("lander")).unwrap();
        assert!(not_started(lander));
        assert!(lander.check.is_some(), "it sleeps on its check script");
        assert!(lander.policy.perpetual);
        assert_eq!(lander.status, crate::state::Status::Sleeping, "due, and held back only by its pause");
        assert_eq!(crate::config::lander().unwrap().as_deref(), Some(lander.id.as_str()));
        assert!(ensure().unwrap().is_empty(), "a second pass makes nothing");
        assert_eq!(agents().len(), AGENTS.len());
    }

    #[test]
    fn a_stopped_agent_stays_stopped() {
        let _home = TempHome::new();
        ensure().unwrap();
        let id = agents()[0].id.clone();
        crate::state::transaction(&id, |_p, s| {
            s.status = crate::state::Status::Stopped;
            Ok(())
        })
        .unwrap();
        assert!(ensure().unwrap().is_empty());
    }

    #[test]
    fn a_lander_named_by_hand_is_adopted() {
        let _home = TempHome::new();
        let id = crate::state::commands::init_run(crate::state::commands::InitArgs { goal: "land".into(), name: Some("lander".into()), ..Default::default() }).unwrap();
        crate::config::save_lander(Some(&id)).unwrap();
        let made = ensure().unwrap();
        assert!(made[0].contains("already named"), "{made:?}");
        let state = crate::state::read_run(&id).unwrap();
        assert_eq!(state.role.as_deref(), Some("lander"));
        assert!(!not_started(&state), "it was already running; adopting it doesn't pause it");
    }

    #[test]
    fn instructions_are_rewritten_from_the_binary() {
        let _home = TempHome::new();
        ensure().unwrap();
        let file = dir().join(LANDER.file);
        std::fs::write(&file, "stale").unwrap();
        ensure().unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), LANDER.instructions);
    }
}
