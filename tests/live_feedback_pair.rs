//! tests/live_feedback_pair.rs — prove a wake can tie two items in otto's queue to the one change
//! that settles both, against a real model.
//!
//! What `cargo test` cannot reach: what a wake does when one change settles two items. Marking the
//! second building against the change the first was being built as used to be refused with "one at
//! a time" and nothing else, so the wake that met it (report 57) left the second item open and
//! planned to close it as a duplicate, which it wasn't. So: given a change that is out and two
//! items it settles, does the wake mark both building against it, close neither, and finish
//! without otto getting in its way?
//!
//! Two wakes, the work and the check of its done, ~3 minutes.
//!
//!   cargo test --test live_feedback_pair -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::Duration;
use support::*;

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_FEEDBACK_PAIR_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
    Duration::from_secs(secs)
}

fn queue() -> Vec<Value> {
    let all: Value = serde_json::from_str(&stdout(&otto(&["feedback", "--all", "--json"]))).unwrap_or(Value::Null);
    all.as_array().cloned().unwrap_or_default()
}

#[test]
#[ignore = "spawns a real claude session and spends real money — run with --ignored"]
fn live_feedback_pair() {
    // The items are the person's, and the wake running these tests may well be a wake itself.
    std::env::remove_var("OTTO_RUN_ID");
    std::env::remove_var("OTTO_RUN_DIR");
    let timeout = timeout();
    let mut checks = Checks::new();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - a run whose change is out, and two items in the queue that it settles");
    let mut scratch = ScratchRepo::new(&workspace_dir(), "pair");
    let repo = scratch.repo_path("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "--quiet", "-b", "main"]);
    git_ok(&repo, &["config", "user.name", "otto live test"]);
    git_ok(&repo, &["config", "user.email", "otto@example.invalid"]);
    write_file(&repo.join("greeting.txt"), "\n");
    git_ok(&repo, &["add", "."]);
    git_ok(&repo, &["commit", "--quiet", "-m", "an empty greeting"]);
    git_ok(&repo, &["checkout", "--quiet", "-b", "greet"]);
    write_file(&repo.join("greeting.txt"), "hello\ngoodbye\n");
    git_ok(&repo, &["commit", "--quiet", "-am", "the greeting says hello and goodbye"]);
    git_ok(&repo, &["checkout", "--quiet", "main"]);

    let run_id = format!("pair-{stamp}");
    let instructions = repo_path("test/fixtures/pair-flow.md");
    let made = otto(&[
        "state",
        "init",
        "--id",
        &run_id,
        "--goal",
        "Mark what this run's change settles as being built",
        "--instructions",
        instructions.to_str().unwrap(),
        "--until",
        "every item the change settles is marked building against it, and the run is done",
        "--repo",
        repo.to_str().unwrap(),
    ]);
    checks.check(made.status.success(), format!("created {run_id}"), String::from_utf8_lossy(&made.stderr));
    let run = Run::new(&run_id);

    let summary = scratch.path().join("summary.md");
    write_file(&summary, "The greeting says hello, and it says goodbye.\n");
    let submitted = otto_command()
        .current_dir(&repo)
        .args(["state", "change", "submit", &run_id, "--repo", repo.to_str().unwrap(), "--branch", "greet"])
        .args(["--title", "The greeting says hello and goodbye", "--summary-file", summary.to_str().unwrap()])
        .args(["--tests", "none", "--no-evidence", "a live test's stand-in change"])
        .output()
        .expect("run otto");
    checks.check(submitted.status.success(), "its change is out", String::from_utf8_lossy(&submitted.stderr));
    let change = format!("{run_id}/1");

    for text in ["Make the greeting say hello.", "Make the greeting say goodbye."] {
        let added = otto(&["feedback", "add", text]);
        checks.check(added.status.success(), format!("the person filed: {text}"), String::from_utf8_lossy(&added.stderr));
    }
    checks.check(queue().len() == 2, "two items in the queue", format!("{} items", queue().len()));

    checks.step(&format!("2 - wake it, and wait for it to finish (up to {}s)", timeout.as_secs()));
    let woke = otto(&["wake", &run.id]);
    checks.check(woke.status.success(), format!("woke {}", run.id), String::from_utf8_lossy(&woke.stderr));
    if !await_field(&run, "status", "done", timeout) {
        for report in queue() {
            checks.note(format!("report {}: {} {}", report["id"], report["status"], report["change"]));
        }
        scratch.keep();
        checks.bail(&run, "never finished");
    }
    checks.check(await_idle(&run, Duration::from_secs(120)), "the last wake has exited", "a wake is still running");

    checks.step("3 - both items are being built as the one change");
    let all = queue();
    let items: Vec<&Value> = all.iter().filter(|r| r["source"] == "person").collect();
    checks.check(items.len() == 2, "the person's two items are still two", format!("{} of the person's items", items.len()));
    for item in &items {
        checks.note(format!("report {}: {} · change {} · {}", item["id"], item["status"], item["change"], item["text"]));
        checks.check(
            item["status"] == "building" && item["change"] == change.as_str(),
            format!("report {} is building, as change {change}", item["id"]),
            format!("report {} is {} (change {})", item["id"], item["status"], item["change"]),
        );
        checks.check(item["duplicateOf"].is_null(), "and wasn't closed as a duplicate", format!("closed as a duplicate of {}", item["duplicateOf"]));
    }
    let marked = std::fs::read_to_string(run.artifact("marked.md")).unwrap_or_default();
    checks.note(format!("otto printed, as the wake wrote it down:\n{}", marked.trim()));
    checks.check(!marked.contains("one at a time"), "otto refused nothing", "otto refused one: \"one at a time\"");
    checks.check(marked.matches("is building").count() >= 2, "it said each is building", format!("it said so {} time(s)", marked.matches("is building").count()));

    checks.step("4 - and otto didn't get in its way");
    let reports: Vec<&Value> = all.iter().filter(|r| r["run"] == run_id.as_str()).collect();
    for report in &reports {
        checks.note(format!("the wake reported: {}", report["text"]));
    }
    checks.check(reports.is_empty(), "the wake had nothing to report about otto", format!("{} report(s) about otto", reports.len()));
    checks.check(run.field("gate").is_none_or(|g| g.is_null()), "and nothing to ask", "it opened a gate");

    finish_run(checks, &run, "done", None, None);
}
