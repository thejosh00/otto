//! Desktop notifications for the moments a run needs a person: a gate opened, a gate left
//! unanswered past `policy.gateStaleAfterHours`, a run blocked, a budget past 80%.
//!
//! Sent from poke rather than from the wake that caused them, for two reasons. A wake under
//! a sandboxing launcher may not be able to reach the notification centre at all;
//! poke runs from launchd in the person's own session. And poke already visits every run every
//! few minutes, so it can see a gate go *stale* — something no single wake is around to notice.
//!
//! Each notice has a key naming what it is about (`gate:003`, `gate-stale:003`, `blocked:<at>`,
//! `budget`), recorded in `run.json`'s `notified` when sent, so a five-minute poller says it
//! once. Keys that no longer apply are dropped, which is what lets the same kind of notice fire
//! again for the next gate, the next block.

use crate::clock::Timestamp;
use crate::event::Event;
use crate::exec::Exec;
use crate::state::{RunEntry, RunState, Status};
use std::collections::BTreeMap;
use time::{Duration, OffsetDateTime};

// Enough for `osascript` to hand the notice over; it does not wait for it to be seen.
const SEND_TIMEOUT_SECONDS: u64 = 10;
// A notification banner shows a line or two; the rest is in `otto show`.
const DETAIL_MAX_CHARS: usize = 120;
const BUDGET_WARN_FRACTION: f64 = 0.8;

#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub key: String,
    pub message: String,
}

/// Every notice that applies to `state` right now, sent or not. Pure, so the rules are testable
/// without a run directory; `pass` decides which of these are new.
pub fn due(state: &RunState, now: OffsetDateTime, elapsed_hours: f64, short: &str) -> Vec<Notice> {
    let mut notices = Vec::new();
    if state.status.is_terminal() {
        return notices;
    }
    let gate = state.gate.as_ref().filter(|g| g.answered_at.is_none());

    if state.status == Status::Blocked {
        // A block normally comes with a gate; keying on it means the gate's own notice and this
        // one are the same notice, not two banners for one event.
        let anchor = match (gate, &state.blocked) {
            (Some(g), _) => g.id.clone(),
            (None, Some(b)) => b.at.to_string(),
            (None, None) => "unknown".to_string(),
        };
        let why = match &state.blocked {
            Some(b) => match &b.detail {
                Some(detail) => format!(" ({}): {}", b.cause.label(), crate::exec::truncate(detail, DETAIL_MAX_CHARS)),
                None => format!(" ({})", b.cause.label()),
            },
            None => String::new(),
        };
        notices.push(Notice {
            key: format!("blocked:{anchor}"),
            message: format!("Blocked{why}. otto show {short}"),
        });
    } else if let Some(g) = gate {
        notices.push(Notice {
            key: format!("gate:{}", g.id),
            message: format!("Needs you: {}. otto show {short}", g.slug.replace('-', " ")),
        });
    }

    if let Some(g) = gate {
        let stale_after = state.policy.gate_stale_after_hours;
        let waited = now - g.asked_at.dt();
        if stale_after > 0 && waited >= Duration::hours(stale_after as i64) {
            notices.push(Notice {
                key: format!("gate-stale:{}", g.id),
                message: format!(
                    "Gate {} ({}) has waited {}h for an answer. otto show {short}",
                    g.id,
                    g.slug,
                    waited.whole_hours()
                ),
            });
        }
    }

    // Past 100% the run blocks itself with a gate, and that is the notice worth having.
    if let Some(fraction) = state.budget.worst_fraction(elapsed_hours) {
        if (BUDGET_WARN_FRACTION..1.0).contains(&fraction) {
            notices.push(Notice {
                key: "budget".to_string(),
                message: format!("{:.0}% of its budget spent. otto show {short}", fraction * 100.0),
            });
        }
    }
    notices
}

/// Which of `due` are new, and what `notified` should become: the keys still due, plus the new
/// ones stamped `now`. `None` when nothing changes, so a quiet pass writes nothing.
fn reconcile(
    notified: &BTreeMap<String, Timestamp>,
    due: &[Notice],
    now: Timestamp,
) -> Option<(Vec<Notice>, BTreeMap<String, Timestamp>)> {
    let fresh: Vec<Notice> = due.iter().filter(|n| !notified.contains_key(&n.key)).cloned().collect();
    let mut next: BTreeMap<String, Timestamp> =
        notified.iter().filter(|(k, _)| due.iter().any(|n| &n.key == *k)).map(|(k, v)| (k.clone(), *v)).collect();
    for notice in &fresh {
        next.insert(notice.key.clone(), now);
    }
    (next != *notified).then_some((fresh, next))
}

/// One sweep over every run: record and send whatever is newly due. Returns a line per notice
/// for poke's report. A failed send is reported but still counts as sent — retrying a broken
/// `osascript` every five minutes would only fill the journal.
pub fn pass(runs: &[RunEntry], now: OffsetDateTime, exec: &mut dyn Exec, dry_run: bool) -> Vec<String> {
    let ids: Vec<String> = runs.iter().map(|r| r.id().to_string()).collect();
    let mut lines = Vec::new();
    for entry in runs {
        let RunEntry::Readable(snapshot) = entry else { continue };
        let short = crate::paths::short_id_among(&snapshot.id, &ids);
        let stamp = Timestamp::at(now);

        // Decide from the snapshot first, so the common case — nothing new — takes no lock and
        // rewrites no `run.json`.
        let notices = due(snapshot, now, crate::wake::elapsed_hours(snapshot), &short);
        if reconcile(&snapshot.notified, &notices, stamp).is_none() {
            continue;
        }
        if dry_run {
            for n in notices.iter().filter(|n| !snapshot.notified.contains_key(&n.key)) {
                lines.push(format!("notify {:<34} [dry-run] {}", snapshot.id, n.message));
            }
            continue;
        }

        let title = format!("otto · {short}");
        let sent = crate::state::transaction(&snapshot.id, |path, state| {
            let notices = due(state, now, crate::wake::elapsed_hours(state), &short);
            let Some((fresh, next)) = reconcile(&state.notified, &notices, stamp) else {
                return Ok(Vec::new());
            };
            for n in &fresh {
                crate::event::record(
                    path,
                    &Event::Notified { key: n.key.clone(), title: title.clone(), message: n.message.clone() },
                )?;
            }
            state.notified = next;
            Ok(fresh)
        });
        // Sent after the transaction, so the run's lock is not held while `osascript` runs.
        match sent {
            Ok(fresh) => {
                for n in fresh {
                    let result = send(exec, &title, &n.message);
                    lines.push(match result {
                        Ok(()) => format!("notify {:<34} {}", snapshot.id, n.message),
                        Err(e) => format!("notify {:<34} could not send ({e}): {}", snapshot.id, n.message),
                    });
                }
            }
            Err(e) => lines.push(format!("notify {:<34} could not record: {e}", snapshot.id)),
        }
    }
    lines
}

/// Post one notification through the macOS notification centre. Title and message go in as
/// `argv`, never spliced into the script, so nothing in a gate slug or a block detail needs
/// escaping.
fn send(exec: &mut dyn Exec, title: &str, message: &str) -> Result<(), String> {
    let output = exec.exec(
        &[
            "osascript",
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            title,
            message,
        ],
        None,
        std::time::Duration::from_secs(SEND_TIMEOUT_SECONDS),
    );
    if output.ok() {
        Ok(())
    } else {
        Err(crate::exec::truncate(output.merged().trim(), DETAIL_MAX_CHARS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::fake::FakeExec;
    use crate::paths::test_support::TempHome;
    use crate::state::{test_run_state, Blocked, BlockedCause, Gate};
    use time::macros::datetime;

    fn now() -> OffsetDateTime {
        datetime!(2026-09-12 12:00:00 UTC)
    }

    fn gate(id: &str, asked_hours_ago: i64) -> Gate {
        Gate {
            id: id.to_string(),
            slug: "plan-review".to_string(),
            file: format!("gates/{id}-plan-review.md"),
            asked_at: Timestamp::at(now() - Duration::hours(asked_hours_ago)),
            answered_at: None,
            expires_at: None,
        }
    }

    fn keys(state: &RunState) -> Vec<String> {
        due(state, now(), 0.0, "r").into_iter().map(|n| n.key).collect()
    }

    #[test]
    fn an_open_gate_is_one_notice_that_says_how_to_look() {
        let mut state = test_run_state("r");
        state.status = Status::AwaitingHuman;
        state.gate = Some(gate("003", 1));
        let notices = due(&state, now(), 0.0, "r");
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].key, "gate:003");
        assert!(notices[0].message.contains("plan review") && notices[0].message.contains("otto show r"));
    }

    #[test]
    fn a_gate_past_its_stale_limit_adds_a_second_notice_and_zero_disables_it() {
        let mut state = test_run_state("r");
        state.status = Status::AwaitingHuman;
        state.gate = Some(gate("003", 49));
        assert_eq!(keys(&state), vec!["gate:003", "gate-stale:003"]);
        state.policy.gate_stale_after_hours = 0;
        assert_eq!(keys(&state), vec!["gate:003"]);
    }

    #[test]
    fn a_block_with_a_gate_is_one_notice_carrying_the_cause() {
        let mut state = test_run_state("r");
        state.status = Status::Blocked;
        state.gate = Some(gate("004", 0));
        state.blocked = Some(Blocked { cause: BlockedCause::Stall, detail: None, at: Timestamp::at(now()) });
        let notices = due(&state, now(), 0.0, "r");
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].key, "blocked:004");
        assert!(notices[0].message.starts_with("Blocked (stall)"));
    }

    #[test]
    fn budget_warns_between_eighty_and_a_hundred_percent_only() {
        let mut state = test_run_state("r");
        state.status = Status::Sleeping;
        state.budget.wakes = 10;
        state.budget.spent_wakes = 7;
        assert!(keys(&state).is_empty());
        state.budget.spent_wakes = 8;
        assert_eq!(keys(&state), vec!["budget"]);
        state.budget.spent_wakes = 10;
        assert!(keys(&state).is_empty(), "at 100% the block's own notice takes over");
    }

    #[test]
    fn a_finished_run_never_notifies() {
        let mut state = test_run_state("r");
        state.status = Status::Done;
        state.gate = Some(gate("003", 100));
        assert!(keys(&state).is_empty());
    }

    #[test]
    fn a_pass_sends_each_notice_once_and_forgets_it_when_it_no_longer_applies() {
        let _home = TempHome::new();
        crate::state::commands::test_init("n-once", "a goal").unwrap();
        crate::state::transaction("n-once", |_p, s| {
            s.status = Status::AwaitingHuman;
            s.gate = Some(gate("001", 1));
            Ok(())
        })
        .unwrap();
        let runs = || crate::state::read_all_runs().unwrap();

        let mut exec = FakeExec::new();
        let lines = pass(&runs(), now(), &mut exec, false);
        assert_eq!(lines.len(), 1);
        assert_eq!(exec.calls.borrow().len(), 1);
        assert_eq!(exec.last_call()[0], "osascript");
        assert!(crate::state::read_run("n-once").unwrap().notified.contains_key("gate:001"));

        // A second pass has nothing new to say, and writes nothing.
        let before = crate::state::read_run("n-once").unwrap().updated_at;
        assert!(pass(&runs(), now(), &mut exec, false).is_empty());
        assert_eq!(exec.calls.borrow().len(), 1);
        assert_eq!(crate::state::read_run("n-once").unwrap().updated_at, before);

        // Answered: the key goes, so the next gate is news again.
        crate::state::transaction("n-once", |_p, s| {
            s.status = Status::Sleeping;
            s.gate = None;
            Ok(())
        })
        .unwrap();
        pass(&runs(), now(), &mut exec, false);
        assert!(crate::state::read_run("n-once").unwrap().notified.is_empty());
        assert_eq!(exec.calls.borrow().len(), 1);

        let journal = std::fs::read_to_string(crate::paths::run_dir("n-once").unwrap().join("journal.jsonl")).unwrap();
        assert_eq!(journal.matches("\"event\":\"notified\"").count(), 1);
    }

    #[test]
    fn a_dry_run_says_what_it_would_send_and_changes_nothing() {
        let _home = TempHome::new();
        crate::state::commands::test_init("n-dry", "a goal").unwrap();
        crate::state::transaction("n-dry", |_p, s| {
            s.status = Status::AwaitingHuman;
            s.gate = Some(gate("001", 1));
            Ok(())
        })
        .unwrap();
        let mut exec = FakeExec::new();
        let lines = pass(&crate::state::read_all_runs().unwrap(), now(), &mut exec, true);
        assert!(lines[0].contains("[dry-run]"));
        assert!(exec.calls.borrow().is_empty());
        assert!(crate::state::read_run("n-dry").unwrap().notified.is_empty());
    }
}
