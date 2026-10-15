//! tests/live_todo.rs — does a real wake keep the run's todo, and keep it true across wakes?
//!
//! `cargo test` covers parsing `artifacts/todo.md` and what otto does with it. What it cannot cover
//! is the half that is prose: whether a model handed `harness/wake.md` ("The todo") keeps one when
//! the work comes in pieces, marks it as the work happens, and — when a person drops a piece at a
//! gate — marks that piece skipped rather than deleting it. `test/fixtures/todo-flow.md` never
//! mentions a todo, so a todo here is the harness working, not the instructions.
//!
//! Two wakes and a few minutes, about what `live_gate` costs.
//!
//!   cargo test --test live_todo -- --ignored --nocapture

mod support;

use std::time::Duration;
use support::*;

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_TODO_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    Duration::from_secs(secs)
}

/// The mark of the first todo item mentioning `word`, as the file has it: `x`, ` `, `-`, `~`, `!`.
fn mark_of(todo: &str, word: &str) -> Option<char> {
    todo.lines()
        .filter_map(|line| line.strip_prefix("- [").or_else(|| line.strip_prefix("* [")))
        .find(|rest| rest.to_lowercase().contains(word))
        .and_then(|rest| rest.chars().next())
}

/// `otto show`'s todo line as (accounted for, total) — read through otto's own parser, the one
/// the page and the CLI use, rather than a second one here.
fn shown_progress(run: &Run) -> Option<(usize, usize)> {
    let shown = stdout(&otto(&["show", &run.id]));
    let line = shown.lines().find(|l| l.trim_start().starts_with("todo "))?;
    let (done, total) = line.split_whitespace().nth(1)?.split_once('/')?;
    Some((done.parse().ok()?, total.parse().ok()?))
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_todo() {
    let timeout = timeout();
    let mut checks = Checks::new();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);
    use_this_build_first_on_path();

    checks.step("1 - start todo-flow, which never mentions a todo");
    let run_id = format!("todo-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    let instructions = repo_path("test/fixtures/todo-flow.md");
    let run = start_run(
        &mut checks,
        &run_id,
        "Write the three notes todo-flow asks for",
        &[
            "--instructions",
            instructions.to_str().unwrap(),
            "--until",
            "the run is done, with each note written or skipped at the go-on gate",
        ],
    );

    checks.step(&format!("2 - wait for the go-on gate (up to {}s)", timeout.as_secs()));
    if !await_field(&run, "status", "awaiting_human", timeout) {
        checks.bail(&run, "never reached the go-on gate");
    }
    checks.check(await_idle(Duration::from_secs(120)), "the wake that asked has exited", "a wake is still running at the gate");
    checks.check(
        run.artifact("notes/alpha.md").is_file() && run.artifact("notes/beta.md").is_file(),
        "alpha.md and beta.md written",
        "alpha.md or beta.md is missing",
    );

    checks.step("3 - the todo, kept without being asked");
    let todo_path = run.artifact("todo.md");
    let at_gate = std::fs::read_to_string(&todo_path).unwrap_or_default();
    if at_gate.trim().is_empty() {
        checks.bad("no artifacts/todo.md after the first wake: three notes across a gate didn't read as work in pieces");
    } else {
        checks.ok("the first wake keeps artifacts/todo.md");
        for line in at_gate.lines() {
            checks.note(format!("| {line}"));
        }
        for done in ["alpha", "beta"] {
            let mark = mark_of(&at_gate, done);
            checks.check(mark == Some('x'), format!("{done} is ticked"), format!("{done} is {mark:?} at the gate, not [x]"));
        }
        let gamma = mark_of(&at_gate, "gamma");
        checks.check(
            matches!(gamma, Some(' ') | Some('~')),
            "gamma is still open",
            format!("gamma is {gamma:?} at the gate; it should still be open"),
        );
    }
    let at_gate_progress = shown_progress(&run);
    match at_gate_progress {
        Some((done, total)) => checks.ok(format!("otto show counts it: {done}/{total}")),
        None if !at_gate.trim().is_empty() => checks.bad("otto show has no todo line for a todo that exists"),
        None => {}
    }
    let handoff = std::fs::read_to_string(run.handoff_path()).unwrap_or_default();
    if handoff.contains("todo.md") {
        checks.ok("the handoff points into the todo");
    } else {
        checks.note("the handoff doesn't mention todo.md");
    }

    checks.step("4 - drop a piece at the gate");
    if !answer_gate(&mut checks, &run, "go-on", "Skip gamma — two notes are enough for this test.", timeout) {
        checks.bail(&run, "could not answer the go-on gate");
    }

    checks.step("5 - a second wake finishes");
    if !await_field(&run, "status", "done", timeout) {
        checks.bail(&run, "never finished");
    }
    // The wake may still be tidying after it set `done` — `set-status` is what tells it about
    // anything left open — so read the todo only once it has exited.
    checks.check(await_idle(Duration::from_secs(120)), "the finishing wake has exited", "a wake is still running at done");
    let completed = run.events("wake-complete");
    checks.check(completed >= 2, format!("{completed} wakes passed the contract"), format!("only {completed} wake(s) passed the contract"));
    checks.check(!run.artifact("notes/gamma.md").exists(), "gamma.md not written, as answered", "gamma.md written despite Skip gamma");

    checks.step("6 - the todo at done: the dropped piece accounted for, nothing left open");
    let at_done = std::fs::read_to_string(&todo_path).unwrap_or_default();
    for line in at_done.lines() {
        checks.note(format!("| {line}"));
    }
    match mark_of(&at_done, "gamma") {
        Some('-') => checks.ok("gamma is marked [-], not deleted"),
        Some(mark) => checks.bad(format!("gamma is [{mark}] at done; a piece dropped at the gate should be [-]")),
        None if at_done.trim().is_empty() => checks.bad("no todo at done"),
        None => checks.bad("gamma's item is gone: dropped work was deleted rather than marked [-]"),
    }
    match (at_gate_progress, shown_progress(&run)) {
        (Some((_, before)), Some((done, total))) => {
            checks.check(total >= before, format!("no item deleted ({before} → {total})"), format!("the todo shrank from {before} items to {total}"));
            checks.check(done == total, format!("every item accounted for ({done}/{total})"), format!("{} item(s) left open at done", total - done));
        }
        (_, Some((done, total))) => checks.check(done == total, format!("every item accounted for ({done}/{total})"), format!("{} item(s) left open at done", total - done)),
        (_, None) => {}
    }
    let shown = stdout(&otto(&["show", &run.id]));
    checks.check(!shown.contains("still open in its todo"), "otto show flags nothing left open", "otto show flags items left open at done");
    let journal = std::fs::read_to_string(run.dir.join("journal.jsonl")).unwrap_or_default();
    match journal.lines().find(|l| l.contains(r#""event":"status-changed""#) && l.contains(r#""to":"done""#)) {
        Some(line) => checks.check(!line.contains("leftOpen"), "journaled done with nothing left open", format!("journaled done with items open: {line}")),
        None => checks.bad("no status-changed → done in the journal"),
    }

    finish_run(checks, &run, "done", None, None);
}
