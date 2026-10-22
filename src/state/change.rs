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
    /// Draft a reply to a reviewer's comment on its pull request, for the person to post
    Reply(ReplyArgs),
    /// Ask the person something about a change — a judgement that is theirs — without stopping the run
    Ask(AskArgs),
    /// The lander: what waits on it, across every run — approved changes to land, then cleaning up
    Due(DueArgs),
    /// The babysitter: every open pull request that has moved since it last looked
    Watch(WatchArgs),
    /// The babysitter: it changed the code on a pull request itself, as the person allowed — the
    /// lander pushes it
    Fix(FixArgs),
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
    /// The lander: it opened the change's pull request (--url), or pushed its next version to it
    PrPushed(PrPushedArgs),
    /// The babysitter: what its pull request says now — CI, review, whether it merges — and when it said it
    PrSeen(PrSeenArgs),
    /// The babysitter (or the lander): hand what a reviewer or CI asked of its pull request to the change's author
    Relay(RelayArgs),
    /// The lander or the babysitter: its pull request was merged. otto looks, and tells its author
    Merged(LandedArgs),
    /// The lander or the babysitter: its pull request was closed without merging
    Closed(BounceArgs),
    /// The lander: it posted a reply the person said to post
    Posted(PostedArgs),
}

#[derive(clap::Args, Debug)]
pub struct ReplyArgs {
    /// The run: its number or name (a unique prefix of the name will do)
    pub id: String,
    /// The change's number
    pub change: u32,
    /// The comment it answers: its id, or its link
    #[arg(long, required = true)]
    pub comment: String,
    /// The reply, as it would be posted
    #[arg(long, required = true)]
    pub text: String,
}

#[derive(clap::Args, Debug)]
pub struct AskArgs {
    /// The run that wrote it: its number or name (a unique prefix of the name will do)
    pub id: String,
    /// The change's number
    pub change: u32,
    /// The question, written to be answered cold: where it stands, the decision, options under `## Options`
    #[arg(long = "question-file", value_name = "PATH", required = true)]
    pub question_file: String,
    /// The babysitter asking, rather than its author: the answer comes to it
    #[arg(long, value_name = "BABYSITTER")]
    pub by: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct FixArgs {
    /// The babysitter run: its number or name
    pub babysitter: String,
    /// The change: <run>/<number>
    pub change: String,
    /// The commit with the fix, on top of what its pull request has
    #[arg(long, required = true)]
    pub head: String,
    /// What it changed and why, for the person reading the change
    #[arg(long = "summary-file", value_name = "PATH", required = true)]
    pub summary_file: String,
    /// What the tests said, in a line
    #[arg(long)]
    pub tests: Option<String>,
    /// Why this is within what the person allowed: the words you went by, or their answer
    #[arg(long, required = true)]
    pub why: String,
}

#[derive(clap::Args, Debug)]
pub struct PrPushedArgs {
    /// The lander run: its number or name
    pub lander: String,
    /// The change: <run>/<number>
    pub change: String,
    /// The commit its pull request's branch has now
    #[arg(long, required = true)]
    pub head: String,
    /// The pull request's address, when you just opened it
    #[arg(long)]
    pub url: Option<String>,
    /// You turned on GitHub's auto-merge for it (`gh pr merge --auto`)
    #[arg(long)]
    pub auto_merge: bool,
}

#[derive(clap::Args, Debug)]
pub struct PrSeenArgs {
    /// The babysitter run: its number or name
    pub babysitter: String,
    /// The change: <run>/<number>
    pub change: String,
    /// The pull request's `updatedAt`, as `gh pr view --json updatedAt` gave it
    #[arg(long, required = true)]
    pub updated: String,
    /// pending, passing or failing
    #[arg(long)]
    pub ci: Option<String>,
    /// approved, changes-requested, waiting, or none (nobody has to review it)
    #[arg(long)]
    pub review: Option<String>,
    /// clean, behind or conflicting
    #[arg(long)]
    pub mergeable: Option<String>,
    /// What happened on it, in a line, for the change's history: "CI passing; Sam approved"
    #[arg(long)]
    pub what: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct RelayArgs {
    /// The babysitter run (or the lander's): its number or name
    pub babysitter: String,
    /// The change: <run>/<number>
    pub change: String,
    /// Who: "a reviewer (sam)", "CI"
    #[arg(long, required = true)]
    pub who: String,
    /// What they said, quoted — the comment's words, or the failing job and its first error lines
    #[arg(long, required = true)]
    pub text: String,
}

#[derive(clap::Args, Debug)]
pub struct PostedArgs {
    /// The lander run: its number or name
    pub lander: String,
    /// The change: <run>/<number>
    pub change: String,
    /// The comment the reply answered
    #[arg(long, required = true)]
    pub comment: String,
}

impl ChangeCommand {
    pub fn writes(&self) -> bool {
        !matches!(self, ChangeCommand::Due(_) | ChangeCommand::Watch(_) | ChangeCommand::Check(_))
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
pub struct WatchArgs {
    /// The babysitter run: its number or name
    pub babysitter: String,
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
        ChangeCommand::Watch(a) => watch(a),
        ChangeCommand::Fix(a) => {
            let r = ChangeRef::parse(&a.change)?;
            let given = std::path::Path::new(&a.summary_file);
            let found = if given.is_file() { given.to_path_buf() } else { crate::paths::run_dir(&crate::paths::resolve_run_id(&a.babysitter)?)?.join(given) };
            let summary = std::fs::read_to_string(&found).map_err(|e| OttoError::usage(format!("cannot read {}: {e}", a.summary_file)))?;
            let c = changes::fix(&a.babysitter, &r, &changes::Fix { head: &a.head, summary: &summary, tests: a.tests.as_deref(), why: &a.why })?;
            println!("{} v{} goes to its pull request: the lander pushes it, and its author is told", r.display(), c.latest().k);
            Ok(())
        }
        ChangeCommand::Reply(a) => {
            let id = crate::paths::resolve_run_id(&a.id)?;
            changes::reply_draft(&ChangeRef { run: id, n: a.change }, &a.comment, &a.text)?;
            println!("drafted; the person reads it on the change and says whether to post it");
            Ok(())
        }
        ChangeCommand::Ask(a) => {
            let id = crate::paths::resolve_run_id(&a.id)?;
            let given = std::path::Path::new(&a.question_file);
            // Relative to whoever asks: the babysitter keeps its question with its own artifacts.
            let asker = match &a.by {
                Some(by) => crate::paths::resolve_run_id(by)?,
                None => id.clone(),
            };
            let found = if given.is_file() { given.to_path_buf() } else { crate::paths::run_dir(&asker)?.join(given) };
            let question = std::fs::read_to_string(&found).map_err(|e| OttoError::usage(format!("cannot read {}: {e}", a.question_file)))?;
            let c = changes::ask_as(a.by.as_deref(), &ChangeRef { run: id, n: a.change }, &question)?;
            println!(
                "asked about change {}{} — carry on with other work; the answer reaches you as a note",
                c.n,
                c.ask.as_ref().filter(|q| !q.options.is_empty()).map(|q| format!(" ({})", q.options.join(" / "))).unwrap_or_default()
            );
            Ok(())
        }
        ChangeCommand::PrPushed(a) => {
            let r = ChangeRef::parse(&a.change)?;
            let c = changes::pr_pushed(&a.lander, &r, &a.head, a.url.as_deref(), a.auto_merge)?;
            println!("{}: {}", r.display(), c.timeline.last().map(|m| m.what.as_str()).unwrap_or(""));
            Ok(())
        }
        ChangeCommand::PrSeen(a) => {
            let r = ChangeRef::parse(&a.change)?;
            let c = changes::pr_seen(
                &a.babysitter,
                &r,
                &changes::Seen { updated: &a.updated, ci: a.ci.as_deref(), review: a.review.as_deref(), mergeable: a.mergeable.as_deref(), what: a.what.as_deref() },
            )?;
            println!("{} is {}", r.display(), c.state.as_str());
            Ok(())
        }
        ChangeCommand::Relay(a) => {
            let r = ChangeRef::parse(&a.change)?;
            changes::relay(&a.babysitter, &r, &a.who, &a.text)?;
            println!("{} is with its author, with what {} said", r.display(), a.who);
            Ok(())
        }
        ChangeCommand::Merged(a) => {
            let r = ChangeRef::parse(&a.change)?;
            changes::merged(&a.lander, &r, &a.sha)?;
            println!("{} merged; its author is told. Clean up next: its worktree and branch, then `otto state change cleaned`", r.display());
            Ok(())
        }
        ChangeCommand::Closed(a) => {
            let r = ChangeRef::parse(&a.change)?;
            changes::closed(&a.lander, &r, &a.reason)?;
            println!("{} closed. Clean up next: its worktree and branch, then `otto state change cleaned`", r.display());
            Ok(())
        }
        ChangeCommand::Posted(a) => {
            let r = ChangeRef::parse(&a.change)?;
            changes::posted(&a.lander, &r, &a.comment)?;
            println!("{}: posted", r.display());
            Ok(())
        }
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

fn watch(a: WatchArgs) -> Result<(), OttoError> {
    changes::require_babysitter(&a.babysitter)?;
    let watched = changes::watch()?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&watched)?);
        return Ok(());
    }
    if watched.is_empty() {
        println!("no pull request has moved since you last looked");
        return Ok(());
    }
    for d in &watched {
        println!(
            "{:<10} {}  ({}; {}{})",
            d.reference,
            d.title,
            d.pr.as_ref().map(|p| p.url.as_str()).unwrap_or(""),
            d.news.as_deref().unwrap_or(""),
            if d.author_alive { "" } else { "; its author has finished" }
        );
    }
    Ok(())
}
