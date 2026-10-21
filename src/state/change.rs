//! `otto state change`: what a wake says about code it hands back, and what the lander says about
//! landing it (see `crate::changes`).

use crate::changes::{self, ChangeRef};
use crate::error::OttoError;

#[derive(clap::Subcommand, Debug)]
pub enum ChangeCommand {
    /// Hand back code committed on a branch, for the person to review and the lander to land. The
    /// same branch (or --key) again is its next version. It doesn't stop the run
    Submit(SubmitArgs),
    /// Take back a change you no longer want: overtaken, or the wrong approach
    Withdraw(WithdrawArgs),
    /// The lander: what waits on it, across every run — approved changes to land, then cleaning up
    Due(DueArgs),
    /// The lander: take an approved change in hand, and print what lands
    Claim(LanderArgs),
    /// The lander: refuse to push a commit that isn't what the person approved — exit 2 means DO NOT ACT
    Check(CheckArgs),
    /// The lander: it is on its base branch now. otto looks, and tells its author
    Landed(LandedArgs),
    /// The lander: it was rebased onto a base that moved and its patch changed, so it goes back to the person
    Rebased(RebasedArgs),
    /// The lander: it couldn't land — a conflict, tests red after a rebase — so it goes back to its author
    Bounce(BounceArgs),
    /// The lander: its worktree and branch are gone (otto looks), or say what was left and why
    Cleaned(CleanedArgs),
}

impl ChangeCommand {
    pub fn writes(&self) -> bool {
        !matches!(self, ChangeCommand::Due(_) | ChangeCommand::Check(_))
    }
}

#[derive(clap::Args, Debug)]
pub struct SubmitArgs {
    /// The run: its number or name (a unique prefix of the name will do)
    pub id: String,
    /// The repository: its main checkout, or any worktree of it
    #[arg(long, required = true)]
    pub repo: String,
    /// The branch the work is committed on
    #[arg(long, required = true)]
    pub branch: String,
    /// What it does, in a few words, for a list: "Answering a stopped run's question points to otto resume"
    #[arg(long, required = true)]
    pub title: String,
    /// What you'd tell the person reviewing it: what it does and why first, then what could go wrong
    #[arg(long = "summary-file", value_name = "PATH", required = true)]
    pub summary_file: String,
    /// What the tests said, in a line: "cargo test: 429 passed"
    #[arg(long)]
    pub tests: Option<String>,
    /// The branch it lands on. Omitted, the remote's default branch
    #[arg(long)]
    pub base: Option<String>,
    /// What this run calls it, when not its branch: the same key again is its next version
    #[arg(long)]
    pub key: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct WithdrawArgs {
    /// The run: its number or name (a unique prefix of the name will do)
    pub id: String,
    /// The change's number
    pub change: u32,
    /// Why, in a sentence: the person reads it on the change
    #[arg(long, required = true)]
    pub reason: String,
}

#[derive(clap::Args, Debug)]
pub struct DueArgs {
    /// The lander run: its number or name
    pub lander: String,
    /// Print it as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct LanderArgs {
    /// The lander run: its number or name
    pub lander: String,
    /// The change: <run>/<number>, as `otto state change due` lists it
    pub change: String,
}

#[derive(clap::Args, Debug)]
pub struct CheckArgs {
    /// The lander run: its number or name
    pub lander: String,
    /// The change: <run>/<number>
    pub change: String,
    /// The commit you are about to put on the base branch
    #[arg(long, required = true)]
    pub head: String,
}

#[derive(clap::Args, Debug)]
pub struct LandedArgs {
    /// The lander run: its number or name
    pub lander: String,
    /// The change: <run>/<number>
    pub change: String,
    /// The commit now on the base branch
    #[arg(long, required = true)]
    pub sha: String,
    /// Anything its author should know, in a sentence: "the main checkout has uncommitted changes, so it wasn't updated"
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct RebasedArgs {
    /// The lander run: its number or name
    pub lander: String,
    /// The change: <run>/<number>
    pub change: String,
    /// The rebased commit
    #[arg(long, required = true)]
    pub head: String,
}

#[derive(clap::Args, Debug)]
pub struct BounceArgs {
    /// The lander run: its number or name
    pub lander: String,
    /// The change: <run>/<number>
    pub change: String,
    /// What stopped it, specifically enough to fix: "a.txt conflicts with 3f2a1c0 on main"
    #[arg(long, required = true)]
    pub reason: String,
}

#[derive(clap::Args, Debug)]
pub struct CleanedArgs {
    /// The lander run: its number or name
    pub lander: String,
    /// The change: <run>/<number>
    pub change: String,
    /// What you left on purpose, and why: "the worktree, which has uncommitted work in it"
    #[arg(long)]
    pub left: Option<String>,
}

pub fn run(command: ChangeCommand) -> Result<(), OttoError> {
    match command {
        ChangeCommand::Submit(a) => submit(a),
        ChangeCommand::Withdraw(a) => {
            let id = crate::paths::resolve_run_id(&a.id)?;
            let c = changes::withdraw(&ChangeRef { run: id, n: a.change }, &a.reason)?;
            println!("change {} withdrawn{}", c.n, if c.cleaned.is_none() { "; the lander removes its worktree and branch" } else { "" });
            Ok(())
        }
        ChangeCommand::Due(a) => due(a),
        ChangeCommand::Claim(a) => {
            let due = changes::claim(&a.lander, &ChangeRef::parse(&a.change)?)?;
            println!("{}", serde_json::to_string_pretty(&due)?);
            Ok(())
        }
        ChangeCommand::Check(a) => {
            println!("{}", changes::check(&a.lander, &ChangeRef::parse(&a.change)?, &a.head)?);
            Ok(())
        }
        ChangeCommand::Landed(a) => {
            let r = ChangeRef::parse(&a.change)?;
            let c = changes::landed(&a.lander, &r, &a.sha, a.note.as_deref())?;
            let local = c.landed.as_ref().is_some_and(|l| l.local_base);
            println!(
                "{} landed on {}; its author is told.{} Clean up next: its worktree and branch, then `otto state change cleaned`",
                r.display(),
                c.base,
                if local { "" } else { " The main checkout's own base branch is behind it." }
            );
            Ok(())
        }
        ChangeCommand::Rebased(a) => {
            let r = ChangeRef::parse(&a.change)?;
            let c = changes::rebased(&a.lander, &r, &a.head)?;
            println!("{} is back with the person as v{}; nothing lands until they approve it again", r.display(), c.latest().k);
            Ok(())
        }
        ChangeCommand::Bounce(a) => {
            let r = ChangeRef::parse(&a.change)?;
            changes::bounce(&a.lander, &r, &a.reason)?;
            println!("{} is back with its author, with why", r.display());
            Ok(())
        }
        ChangeCommand::Cleaned(a) => {
            let r = ChangeRef::parse(&a.change)?;
            changes::cleaned(&a.lander, &r, a.left.as_deref())?;
            println!("{} cleaned up", r.display());
            Ok(())
        }
    }
}

fn submit(a: SubmitArgs) -> Result<(), OttoError> {
    let id = crate::paths::resolve_run_id(&a.id)?;
    // A wake runs in its working directory, and its artifacts are in the run directory: either is
    // where a relative path can mean.
    let given = std::path::Path::new(&a.summary_file);
    let found = if given.is_file() { given.to_path_buf() } else { crate::paths::run_dir(&id)?.join(given) };
    let summary = std::fs::read_to_string(&found).map_err(|e| OttoError::usage(format!("cannot read {}: {e}", a.summary_file)))?;
    let out = changes::submit(
        &id,
        &changes::Submit {
            repo: &a.repo,
            branch: &a.branch,
            base: a.base.as_deref(),
            key: a.key.as_deref(),
            title: &a.title,
            summary: &summary,
            tests: a.tests.as_deref(),
        },
    )?;
    let r = format!("{}/{}", crate::paths::short_id(&id), out.n);
    if out.unchanged {
        println!("change {r} v{} unchanged — the same commit as its latest version, so nothing new for the person", out.version);
        return Ok(());
    }
    println!(
        "change {r} v{}: {} file{}, {} line{} changed — waiting for the person. Carry on with the rest of the work; what they decide \
         reaches you as a note",
        out.version,
        out.files,
        if out.files == 1 { "" } else { "s" },
        out.lines,
        if out.lines == 1 { "" } else { "s" }
    );
    if out.large {
        println!(
            "It is over {} changed lines: a person reviews one concern at a time. If it is more than one, split it into changes of its own",
            changes::LARGE_LINES
        );
    }
    if out.too_big {
        println!("Its patch is over {} bytes, so only its stat is kept; the person reads it in the repo", changes::PATCH_MAX_BYTES);
    }
    Ok(())
}

fn due(a: DueArgs) -> Result<(), OttoError> {
    changes::require_lander(&a.lander)?;
    let due = changes::due()?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&due)?);
        return Ok(());
    }
    if due.is_empty() {
        println!("nothing to land, and nothing to clean up");
        return Ok(());
    }
    for d in &due {
        println!(
            "{:<10} {:<15} {}  ({} → {} in {})",
            d.reference,
            d.todo,
            d.title,
            d.branch,
            d.base,
            changes::home_relative(&d.repo)
        );
    }
    Ok(())
}
