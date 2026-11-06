//! tests/live_tmux.rs — a run's session is found by its exact name, against a real tmux server.
//!
//! tmux takes a bare target as a prefix too. With no `otto-1`, `-t otto-1` finds `otto-12`: stopping
//! run 1 killed run 12's wake, and a live test doing that killed the wake running it. No model,
//! so no tokens, but `cargo test` runs no tmux, so it lives here.
//!
//!   cargo test --test live_tmux -- --ignored --nocapture

mod support;

use std::process::Command;
use support::*;

fn tmux(args: &[&str]) -> bool {
    Command::new("tmux").args(args).output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
#[ignore = "needs tmux — run with --ignored"]
fn stopping_a_run_leaves_a_session_its_name_is_a_prefix_of() {
    let mut checks = Checks::new();
    checks.step("0 - run 1, with no wake, and another run's live session otto-12");
    require_launcher(&mut checks);
    let out = otto(&["state", "init", "--id", "1", "--perpetual", "--goal", "be stopped"]);
    checks.check(out.status.success(), "run 1 exists", format!("init failed: {}", String::from_utf8_lossy(&out.stderr)));
    checks.check(tmux(&["new-session", "-d", "-s", "otto-12", "sleep 300"]), "otto-12 is alive", "could not start otto-12");

    checks.step("1 - stopping run 1 leaves it alone");
    let out = otto(&["stop", "1", "--reason", "live test"]);
    checks.check(out.status.success(), "run 1 stopped", format!("stop failed: {}", String::from_utf8_lossy(&out.stderr)));
    checks.check(tmux(&["has-session", "-t", "=otto-12"]), "otto-12 is still alive", "stopping run 1 killed otto-12");

    checks.step("2 - run 1's own session can start beside it");
    let out = otto(&["state", "init", "--id", "2", "--perpetual", "--goal", "start a session"]);
    checks.check(out.status.success(), "run 2 exists", "init failed");
    checks.check(tmux(&["new-session", "-d", "-s", "otto-2x", "sleep 300"]), "otto-2x is alive", "could not start otto-2x");
    let out = otto(&["attach", "2"]);
    checks.check(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("no live session"), "run 2 has no session, whatever otto-2x is", "run 2 took otto-2x for its own");

    let _ = tmux(&["kill-server"]);
    checks.finish();
}
