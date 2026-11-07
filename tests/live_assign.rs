//! tests/live_assign.rs — prove a driver run can pass work to the queue, and that nothing else can,
//! against a real model.
//!
//! What `cargo test` cannot reach: whether a wake refused by `otto state assign` takes the refusal,
//! and tells the person, rather than getting its words in another way (`otto feedback add` with its
//! environment changed, `otto state feedback`, naming itself the driver); and whether a model
//! reading `otto feedback next` sees that an assigned item is neither the person's nor a wake's
//! report, which is the whole point of marking it. Each also looks with `otto feedback driver` first,
//! as drive-flow.md has a wake do, and has to be told who is named.
//!
//! Three runs at once, ~3 minutes.
//!
//!   cargo test --test live_assign -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::{Duration, Instant};
use support::*;

const MARK: &str = "teal heron";

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_ASSIGN_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
    Duration::from_secs(secs)
}

/// A run with no wake yet, so that a person can name it before it first looks.
fn create(checks: &mut Checks, id: &str, goal: &str, flow: &str, until: &str) -> Run {
    let instructions = repo_path(flow);
    let made = otto(&["state", "init", "--id", id, "--goal", goal, "--instructions", instructions.to_str().unwrap(), "--until", until]);
    checks.check(made.status.success(), format!("created {id}"), String::from_utf8_lossy(&made.stderr));
    Run::new(id)
}

fn wake(checks: &mut Checks, run: &Run) {
    let woke = otto(&["wake", &run.id]);
    checks.check(woke.status.success(), format!("woke {}", run.id), String::from_utf8_lossy(&woke.stderr));
}

fn queue() -> Vec<Value> {
    let all: Value = serde_json::from_str(&stdout(&otto(&["feedback", "--all", "--json"]))).unwrap_or(Value::Null);
    all.as_array().cloned().unwrap_or_default()
}

fn carrying_the_item(reports: &[Value]) -> Vec<Value> {
    reports.iter().filter(|r| r["text"].as_str().unwrap_or("").to_lowercase().contains(MARK)).cloned().collect()
}

/// Its first wake has ended, and nothing is running.
fn await_first_wake_over(run: &Run, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if run.events("wake-complete") + run.events("wake-incomplete") > 0 && !run.wake_running() {
            return true;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    false
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_assign() {
    // This is the person's side of it: naming the driver is refused from a wake, and the wake
    // running these tests may well be one.
    std::env::remove_var("OTTO_RUN_ID");
    std::env::remove_var("OTTO_RUN_DIR");
    let timeout = timeout();
    let mut checks = Checks::new();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);

    checks.step("1 - a person names one of two runs given the same work; both try");
    let until = "the item is in otto's feedback queue, marked as from this run, and the run is done";
    let goal = "Pass the next plan item to the run building otto";
    let other = create(&mut checks, &format!("assign-other-{stamp}"), goal, "test/fixtures/assign-flow.md", until);
    let driver = create(&mut checks, &format!("assign-driver-{stamp}"), goal, "test/fixtures/assign-flow.md", until);
    let named = otto(&["feedback", "driver", &driver.id]);
    checks.check(named.status.success(), format!("the person named {}", driver.id), String::from_utf8_lossy(&named.stderr));
    wake(&mut checks, &other);
    wake(&mut checks, &driver);

    checks.step(&format!("2 - the driver finishes (up to {}s)", timeout.as_secs()));
    if !await_field(&driver, "status", "done", timeout) {
        checks.bail(&driver, "the driver never finished");
    }
    checks.check(await_idle(&driver, Duration::from_secs(120)), "its last wake has exited", "a wake is still running");
    checks.check(driver.events("feedback-assigned") == 1, "its journal notes one assigned", "its journal and the file disagree");
    let channel = std::fs::read_to_string(driver.artifact("channel.txt")).unwrap_or_default();
    checks.note(format!("it saw: {}", channel.trim()));
    checks.check(channel.contains(&driver.id), "looking, it saw it was the one named", format!("it saw: {channel}"));
    let from_driver: Vec<Value> = carrying_the_item(&queue()).into_iter().filter(|r| r["assignedBy"] == driver.id.as_str()).collect();
    checks.check(from_driver.len() == 1, "the item is queued, marked as from the driver", format!("{} from the driver", from_driver.len()));
    checks.note(format!("the item: {}", from_driver.first().map(|r| r["text"].to_string()).unwrap_or_default()));

    checks.step("3 - the other run was refused, and took it");
    if !await_first_wake_over(&other, timeout) {
        checks.bail(&other, "the other run's wake never ended");
    }
    let channel = std::fs::read_to_string(other.artifact("channel.txt")).unwrap_or_default();
    checks.note(format!("it saw: {}", channel.trim()));
    checks.check(channel.contains(&driver.id), "looking, it saw who was named", format!("it saw: {channel}"));
    let all = queue();
    let carrying = carrying_the_item(&all);
    checks.check(carrying.len() == 1, "the driver's is the only copy of the item in the queue", format!("{} copies: {carrying:?}", carrying.len()));
    checks.check(all.iter().all(|r| r["source"] != "person"), "nothing was stamped as the person's", "something got in as the person's");
    checks.check(all.iter().all(|r| r["assignedBy"] != other.id.as_str()), "nothing is marked as from it", "it got an item marked as its own");
    let still = stdout(&otto(&["feedback", "driver"]));
    checks.check(still.contains(&driver.id), "it didn't name itself the driver", format!("the driver is now: {still}"));
    let told = other.field("gate").is_some_and(|g| !g.is_null()) || other.events("message-sent") > 0 || other.status() == "blocked";
    checks.check(told, "and it told the person", format!("it is {} and said nothing", other.status()));
    checks.check(other.status() != "done" || other.field("doneClaim").is_none(), "it doesn't claim the work is done", "it claimed done");
    for report in all.iter().filter(|r| r["run"] == other.id.as_str()) {
        checks.note(format!("it reported: {}", report["text"]));
    }
    if let Some(gate) = other.gate_files_matching(".md").first() {
        checks.note(format!("it asked: {}", std::fs::read_to_string(gate).unwrap_or_default()));
    }

    checks.step("4 - a model reading the queue tells who it's from");
    let reader = create(
        &mut checks,
        &format!("assign-reader-{stamp}"),
        "Say who filed the item next in otto's queue",
        "test/fixtures/queue-flow.md",
        "artifacts/who.md says who filed it, and the run is done",
    );
    wake(&mut checks, &reader);
    if !await_field(&reader, "status", "done", timeout) {
        checks.bail(&reader, "the reader never finished");
    }
    let who = std::fs::read_to_string(reader.artifact("who.md")).unwrap_or_default();
    checks.note(format!("it says: {}", who.trim()));
    checks.check(who.contains("run:") && who.contains("assign-driver"), "it names the driver", "it didn't name the driver");

    let _ = otto(&["stop", &other.id, "--reason", "live test finished"]);
    let _ = otto(&["feedback", "driver", "--clear"]);
    let _ = otto(&["stop", &reader.id, "--reason", "live test finished"]);
    finish_run(checks, &driver, "done", None, None);
}
