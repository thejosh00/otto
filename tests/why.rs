//! tests/why.rs — `otto why` through the real binary, on a home with its files planted: the journal,
//! `poke.log` and its rotated files, the check history, and a transcript. One real wake with a stub
//! for the model, for running against a wake that's holding its lock.

mod support;

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime};

const OTTO: &str = support::OTTO;
const LONG: &str = "2026-09-13-once-an-hour-check-for-tasks-to-complete";

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("projects")).unwrap();
        // A wake does what's in `next`, if anything, and always leaves a handoff.
        let stub = root.join("stub");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nnext='{root}/next'\nif [ -f \"$next\" ]; then\n  mv \"$next\" \"$next.running\"\n  . \"$next.running\"\nfi\n\
                 printf '# h\\n## Goal\\ng\\n## Where this is\\nHere.\\n## Decided\\nn\\n## This wake did\\nn\\n## Next wake must\\nn\\n## Needs\\nnothing\\n## Don'\"'\"'t re-derive\\nn\\n' | '{OTTO}' state handoff \"$OTTO_RUN_ID\" --stdin >/dev/null\n",
                root = root.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        // Named `claude` too, so nothing here can ever reach the real one.
        std::fs::write(
            root.join("home/config.json"),
            serde_json::json!({"launchers": [{"name": "claude", "command": stub.display().to_string()}]}).to_string(),
        )
        .unwrap();
        Scratch { _dir: dir, root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = support::otto_command();
        command
            .args(args)
            .current_dir(self.root.join("repo"))
            .env("OTTO_HOME", self.root.join("home"))
            .env("OTTO_CLAUDE_PROJECTS_DIR", self.root.join("projects"))
            .env("TMUX_TMPDIR", &self.root)
            .env("OTTO_BIN", OTTO)
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT");
        command
    }

    fn otto(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.otto(args);
        assert!(output.status.success(), "otto {args:?}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    /// A run with no wake yet and the journal given: what's on a real home, without the waiting.
    fn run_with(&self, name: &str, journal: &[Value]) -> String {
        let repo = self.root.join("repo");
        let said = self.ok(&["state", "init", "--goal", "g", "--name", name, "--perpetual", "--detach", "none", "--workdir", repo.to_str().unwrap()]);
        let list: Value = serde_json::from_str(&self.ok(&["state", "list", "--json"])).unwrap();
        let id = list.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("{said}"))["id"].as_str().unwrap().to_string();
        let lines: String = journal.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(self.run_dir(&id).join("journal.jsonl"), lines).unwrap();
        id
    }

    fn run_dir(&self, id: &str) -> PathBuf {
        self.root.join("home/runs").join(id)
    }

    fn poke_log(&self, name: &str, text: &str) {
        std::fs::create_dir_all(self.root.join("home/logs")).unwrap();
        std::fs::write(self.root.join("home/logs").join(name), text).unwrap();
    }

    fn history(&self, id: &str, text: &str) {
        std::fs::write(self.run_dir(id).join("check-history.jsonl"), text).unwrap();
    }
}

fn line(ts: &str, event: &str, more: Value) -> Value {
    let mut value = serde_json::json!({"event": event});
    for (k, v) in more.as_object().unwrap() {
        value[k] = v.clone();
    }
    value["ts"] = ts.into();
    value
}

fn t(hms: &str) -> String {
    format!("2026-11-07T{hms}Z")
}

/// Three wakes: 1, ended; 2, the one asked about; 3, the next.
fn three_wakes() -> Vec<Value> {
    vec![
        line(&t("01:00:00"), "wake-started", serde_json::json!({"wake": 1, "cause": "person"})),
        line(&t("01:05:00"), "cycle", serde_json::json!({"message": "from wake 1"})),
        line(&t("01:10:00"), "wake-complete", serde_json::json!({"status": "sleeping", "progress": true})),
        line(&t("01:20:00"), "note-added", serde_json::json!({"note": "007"})),
        line(&t("02:00:00"), "wake-started", serde_json::json!({"wake": 2, "cause": "check-changed", "session": "abc-session"})),
        line(&t("02:01:00"), "cycle", serde_json::json!({"message": "from wake 2"})),
        line(&t("02:02:00"), "wake-complete", serde_json::json!({"status": "sleeping", "progress": true})),
        line(&t("02:30:00"), "timer-armed", serde_json::json!({"note": "after wake 2"})),
        line(&t("03:00:00"), "wake-started", serde_json::json!({"wake": 3, "cause": "period"})),
        line(&t("03:01:00"), "cycle", serde_json::json!({"message": "from wake 3"})),
        line(&t("03:02:00"), "wake-complete", serde_json::json!({"status": "sleeping", "progress": true})),
    ]
}

fn check(at: &str, code: i32, result: &str, note: &str) -> String {
    format!("{}\n", serde_json::json!({"at": at, "code": code, "timedOut": false, "result": result, "note": note}))
}

fn timeline_order(said: &str, wanted: &[&str]) {
    let mut from = 0;
    for piece in wanted {
        let at = said[from..].find(piece).unwrap_or_else(|| panic!("{piece:?} missing or out of order in:\n{said}"));
        from += at + piece.len();
    }
}

#[test]
fn a_wake_is_told_from_all_three_sources_in_order() {
    let s = Scratch::new();
    let id = s.run_with("all", &three_wakes());
    s.poke_log(
        "poke.log",
        &format!(
            "{a} spawn  {id:<34} from before the window\n\
             {b} defer  {id:<34} a run was busy\n\
             {c} nothing due (3 run(s) checked)\n\
             {d} 2h4m since the last pass, at {b}\n\
             {e} spawn  {id:<34} check script reported a change → wake started\n\
             {f} spawn  {id:<34} after the next wake started\n",
            a = t("01:00:00"),
            b = t("01:30:00"),
            c = t("01:35:00"),
            d = t("01:50:00"),
            e = t("02:00:01"),
            f = t("03:00:02"),
        ),
    );
    let mut history = check(&t("01:00:00"), 0, "no-change", "before the window");
    history.push_str(&check(&t("01:15:00"), 0, "no-change", "first"));
    history.push_str(&check(&t("01:25:00"), 0, "no-change", "second"));
    history.push_str(&check(&t("01:40:00"), 0, "no-change", "third"));
    history.push_str(&check(&t("02:00:00"), 1, "changed", "found one"));
    s.history(&id, &history);
    std::fs::create_dir_all(s.root.join("projects/p")).unwrap();
    std::fs::write(s.root.join("projects/p/abc-session.jsonl"), "").unwrap();
    std::fs::write(s.run_dir(&id).join("wake-2-output.txt"), "").unwrap();

    let said = s.ok(&["why", &id, "--wake", "2"]);
    timeline_order(
        &said,
        &[
            // The answer first.
            "wake 2",
            "started         2026-11-07T02:00:00Z, cause: check-changed",
            "ended           completed 2026-11-07T02:02:00Z, status sleeping",
            "projects/p/abc-session.jsonl",
            "wake-2-output.txt",
            // Then the sequence, from the end of wake 1 to the start of wake 3.
            "journal wake-complete",
            "check   no-change",
            "journal note-added",
            "check   no-change",
            "poke    defer           a run was busy",
            "check   no-change",
            "poke    gap             2h4m since the last pass",
            "check   changed         exit 1: found one",
            "journal wake-started    wake=2",
            "poke    spawn           check script reported a change",
            "journal cycle           message=from wake 2",
            "journal wake-complete",
            "journal timer-armed     note=after wake 2",
            "journal wake-started    wake=3",
        ],
    );
    for outside in ["from wake 1", "from wake 3", "from before the window", "after the next wake started", "before the window", "nothing due"] {
        assert!(!said.contains(outside), "{outside:?} is outside the window:\n{said}");
    }
    assert!(said.contains("poke's times are when its pass finished"), "{said}");

    // Without --wake, the latest.
    let latest = s.ok(&["why", &id]);
    assert!(latest.contains("wake 3 ·") && latest.contains("from wake 3"), "{latest}");
    // Open-ended after the last wake, and what's there comes in.
    assert!(latest.contains("poke    spawn           after the next wake started"), "{latest}");
    // By its name too.
    assert_eq!(s.ok(&["why", "all"]), latest);
}

#[test]
fn no_change_passes_collapse_to_one_line_with_the_count_and_the_span() {
    let s = Scratch::new();
    let id = s.run_with("quiet", &three_wakes());
    s.poke_log("poke.log", &format!("{} nothing due (1 run(s) checked)\n", t("00:00:00")));
    let mut history = String::new();
    for minute in 11..=40 {
        history.push_str(&check(&format!("2026-11-07T01:{minute}:30Z"), 0, "no-change", &format!("look {minute}")));
    }
    s.history(&id, &history);
    // Wake 2's window would have the note in the middle, so these are split around it.
    let said = s.ok(&["why", &id, "--wake", "2"]);
    assert!(said.contains("no-change ×9 to 2026-11-07T01:19:30Z: exit 0: look 19"), "{said}");
    assert!(said.contains("no-change ×21 to 2026-11-07T01:40:30Z: exit 0: look 40"), "{said}");
    assert!(said.lines().count() < 25, "{said}");
    // A result other than "nothing" keeps a line of its own, and a timeout says so.
    let mut history = check(&t("01:15:00"), 0, "no-change", "a");
    history.push_str(&format!("{}\n", serde_json::json!({"at": t("01:16:00"), "code": 124, "timedOut": true, "result": "error"})));
    history.push_str(&check(&t("01:17:00"), 2, "error", "broke"));
    history.push_str(&check(&t("01:18:00"), 0, "no-change", "b"));
    s.history(&id, &history);
    let said = s.ok(&["why", &id, "--wake", "2"]);
    timeline_order(&said, &["no-change", "timed-out", "error", "no-change"]);
    assert!(!said.contains('×'), "{said}");
}

#[test]
fn the_rotated_logs_are_read_in_age_order() {
    let s = Scratch::new();
    let id = s.run_with("rotated", &three_wakes());
    for (name, hms, word) in [
        ("poke.log.3", "01:30:00", "oldest"),
        ("poke.log.2", "01:31:00", "older"),
        ("poke.log.1", "01:32:00", "newer"),
        ("poke.log", "01:33:00", "newest"),
    ] {
        s.poke_log(name, &format!("{} defer  {id:<34} {word}\n", t(hms)));
    }
    // The oldest file reaches back past this window, so nothing is missing...
    let oldest = format!("{} nothing due (1 run(s) checked)\n{} defer  {id:<34} oldest\n", t("00:00:00"), t("01:30:00"));
    s.poke_log("poke.log.3", &oldest);
    let said = s.ok(&["why", &id, "--wake", "2"]);
    timeline_order(&said, &["defer           oldest", "defer           older", "defer           newer", "defer           newest"]);
    assert!(!said.contains("poke.log        not recorded"), "{said}");
    // ...until the oldest file's gone, and the window starts before what's left.
    std::fs::remove_file(s.root.join("home/logs/poke.log.3")).unwrap();
    std::fs::remove_file(s.root.join("home/logs/poke.log.2")).unwrap();
    std::fs::remove_file(s.root.join("home/logs/poke.log.1")).unwrap();
    let said = s.ok(&["why", &id, "--wake", "2"]);
    assert!(said.contains(&format!("poke.log        not recorded before {}", t("01:33:00"))), "{said}");
}

#[test]
fn a_run_gets_only_its_own_lines_from_poke_log() {
    let s = Scratch::new();
    let id = s.run_with("one", &three_wakes());
    assert_eq!(id, "1", "the point is an id that's in others");
    let at = t("01:30:00");
    s.poke_log(
        "poke.log",
        &format!(
            "{at} defer  13                                 thirteen's\n\
             {at} defer  {LONG} the long one's\n\
             {at} notify 21                                 says run 1 is busy\n\
             {at} defer  1                                  run one's\n"
        ),
    );
    let said = s.ok(&["why", "1", "--wake", "2"]);
    assert!(said.contains("defer           run one's"), "{said}");
    for other in ["thirteen's", "the long one's", "says run 1"] {
        assert!(!said.contains(other), "{other:?}:\n{said}");
    }
}

#[test]
fn a_kill_says_what_it_recorded_and_an_old_one_reads_not_recorded() {
    let s = Scratch::new();
    let old = s.run_with(
        "old-kill",
        &[
            serde_json::json!({"event": "wake-started", "wake": 44, "deadlineAt": t("03:00:00"), "launcher": "claude", "ts": t("02:00:00")}),
            line(&t("02:00:30"), "wake-complete", serde_json::json!({"status": "sleeping"})),
            line("2026-11-08T16:04:24Z", "wake-killed", serde_json::json!({"reason": "wake is 1170m past its deadline"})),
            line("2026-11-08T16:04:24Z", "wake-incomplete", serde_json::json!({"reason": "killed by poke: wake is 1170m past its deadline", "consecutive": 1})),
            line("2026-11-08T16:09:25Z", "wake-started", serde_json::json!({"wake": 45})),
        ],
    );
    s.poke_log("poke.log", &format!("2026-11-01T00:00:00Z kill   {old:<34} killed — wake is 1170m past its deadline\n"));
    let said = s.ok(&["why", &old, "--wake", "44"]);
    timeline_order(
        &said,
        &[
            "cause: not recorded",
            "ended           completed 2026-11-07T02:00:30Z, status sleeping",
            "then            killed 2026-11-08T16:04:24Z: wake is 1170m past its deadline",
            "then            incomplete 2026-11-08T16:04:24Z",
            "transcript      session not recorded",
            "journal wake-started",
            "journal wake-complete",
            "journal wake-killed     wake is 1170m past its deadline",
            "not recorded: wake, deadlineAt, outcome, holder, signalLanded, tmuxKilled",
            "journal wake-incomplete killed by poke",
            "journal wake-started    wake=45",
        ],
    );

    let new = s.run_with(
        "new-kill",
        &[
            line(&t("02:00:00"), "wake-started", serde_json::json!({"wake": 7, "cause": "timer", "session": "gone-session"})),
            line(
                &t("03:00:00"),
                "wake-killed",
                serde_json::json!({"reason": "wake is 16m past its deadline", "wake": 7, "pid": 42, "deadlineAt": t("02:45:00"),
                    "outcome": null, "holder": "this-wake", "signalLanded": true, "tmuxKilled": false}),
            ),
        ],
    );
    let said = s.ok(&["why", &new]);
    assert!(said.contains("holder=this-wake") && said.contains("signalLanded=true") && said.contains("tmuxKilled=false"), "{said}");
    assert!(said.contains("outcome=none"), "{said}");
    assert!(!said.contains("not recorded: wake"), "{said}");
    // A session that's on record, with its transcript gone, isn't one that was never recorded.
    assert!(said.contains("transcript      session gone-session: no longer on disk"), "{said}");
}

#[test]
fn garbled_lines_are_counted_and_stop_nothing() {
    let s = Scratch::new();
    let id = s.run_with("garbled", &three_wakes());
    let journal = s.run_dir(&id).join("journal.jsonl");
    let text = std::fs::read_to_string(&journal).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    // Two in wake 2's window, one with no time, and one out of it, which isn't this account's.
    lines.insert(5, "{not json");
    lines.insert(6, "{\"event\":\"cycle\",\"message\":\"no time\"}");
    lines.insert(1, "also not json");
    std::fs::write(&journal, lines.join("\n") + "\n").unwrap();
    s.poke_log(
        "poke.log",
        &format!(
            "{before} nothing due (1 run(s) checked)\notto poke: from before\n{inside} defer  {id:<34} fine\notto poke: an error with no time\n\u{1b}garbage\n{after} nothing due (1 run(s) checked)\n",
            before = t("00:00:00"),
            inside = t("01:30:00"),
            after = t("05:00:00"),
        ),
    );
    s.history(&id, &format!("nope\n{}", check(&t("01:15:00"), 0, "no-change", "fine")));

    let said = s.ok(&["why", &id, "--wake", "2"]);
    assert!(said.contains("skipped         2 unreadable or undated line(s) in journal.jsonl"), "{said}");
    assert!(said.contains("skipped         2 unreadable or undated line(s) in poke.log"), "{said}");
    assert!(said.contains("skipped         1 unreadable or undated line(s) in check-history.jsonl"), "{said}");
    assert!(said.contains("defer           fine") && said.contains("message=from wake 2"), "{said}");
}

#[test]
fn a_source_that_doesnt_go_back_far_enough_reads_not_recorded() {
    let s = Scratch::new();
    let id = s.run_with("short", &three_wakes());

    // No file at all: said, never fatal.
    let said = s.ok(&["why", &id, "--wake", "2"]);
    assert!(said.contains("poke.log        not recorded: there is no poke.log"), "{said}");
    assert!(said.contains("check results   not recorded: no history for this run"), "{said}");

    // A history that begins after the window has nothing to say about it, which isn't "nothing".
    s.history(&id, &check("2026-11-09T00:00:00Z", 0, "no-change", "later"));
    let said = s.ok(&["why", &id, "--wake", "2"]);
    assert!(said.contains("check results   not recorded: the history begins 2026-11-09T00:00:00Z, after this"), "{said}");
    assert!(!said.contains("later"), "{said}");

    // Beginning in the middle: what it has, and where that starts.
    s.history(&id, &check(&t("01:40:00"), 0, "no-change", "from here"));
    let said = s.ok(&["why", &id, "--wake", "2"]);
    assert!(said.contains(&format!("check results   not recorded before {}", t("01:40:00"))), "{said}");
    assert!(said.contains("from here"), "{said}");

    // Covering the window: no words about it.
    s.history(&id, &(check(&t("00:00:00"), 0, "no-change", "old") + &check(&t("01:40:00"), 0, "no-change", "from here")));
    let said = s.ok(&["why", &id, "--wake", "2"]);
    assert!(!said.contains("check results"), "{said}");

    // No journal: said, and no wake to speak of.
    std::fs::remove_file(s.run_dir(&id).join("journal.jsonl")).unwrap();
    let said = s.ok(&["why", &id]);
    assert!(said.contains("no wake on record: it has no journal"), "{said}");
}

#[test]
fn a_wake_with_no_ending_says_that_and_not_a_guess() {
    let s = Scratch::new();
    let id = s.run_with(
        "unended",
        &[
            line(&t("01:00:00"), "wake-started", serde_json::json!({"wake": 1})),
            line(&t("01:01:00"), "cycle", serde_json::json!({"message": "one"})),
            line(&t("02:00:00"), "wake-started", serde_json::json!({"wake": 2})),
        ],
    );
    let said = s.ok(&["why", &id, "--wake", "1"]);
    assert!(said.contains("ended           no ending on record\n"), "{said}");
    let said = s.ok(&["why", &id]);
    assert!(said.contains("ended           no ending on record, and no wake is running now"), "{said}");
    // The wake before it had no ending, so its window goes back to where that wake started.
    timeline_order(&said, &["journal wake-started    wake=1", "message=one", "journal wake-started    wake=2"]);
}

#[test]
fn a_busy_window_fits_a_screen_and_says_what_it_cut() {
    let s = Scratch::new();
    let mut journal = vec![line(&t("01:00:00"), "wake-started", serde_json::json!({"wake": 1, "cause": "period"}))];
    for minute in 0..60 {
        journal.push(line(&format!("2026-11-07T01:{minute:02}:30Z"), "cycle", serde_json::json!({"message": format!("step {minute}")})));
        journal.push(line(&format!("2026-11-07T01:{minute:02}:40Z"), "facts-recorded", serde_json::json!({"facts": {"n": minute}})));
    }
    journal.push(line(&t("02:00:00"), "wake-complete", serde_json::json!({"status": "sleeping"})));
    journal.push(line(&t("03:00:00"), "wake-started", serde_json::json!({"wake": 2})));
    let id = s.run_with("busy", &journal);
    let said = s.ok(&["why", &id, "--wake", "1"]);
    assert!(said.lines().count() <= 45, "{} lines:\n{said}", said.lines().count());
    assert!(said.chars().filter(|c| *c == '\n').count() > 25, "{said}");
    for kept in ["started         2026-11-07T01:00:00Z, cause: period", "journal wake-started    wake=1", "journal wake-complete", "journal wake-started    wake=2"] {
        assert!(said.contains(kept), "{kept:?}:\n{said}");
    }
    let cut = said.lines().find(|l| l.starts_with("… cut to fit a screen")).unwrap_or_else(|| panic!("{said}"));
    assert!(cut.contains("journal") && cut.contains(&format!("otto logs {id} --since 2026-11-07T01:")), "{cut}");
    for l in said.lines() {
        // The whole path of a transcript can be long; nothing else wraps.
        assert!(l.chars().count() <= 110, "{l}");
    }
}

#[test]
fn a_wake_or_run_that_isnt_there_says_so() {
    let s = Scratch::new();
    let id = s.run_with("few", &three_wakes());
    let output = s.otto(&["why", "no-such-run"]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let output = s.otto(&["why", &id, "--wake", "99"]);
    assert_eq!(output.status.code(), Some(1), "a usage error: {output:?}");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("never had a wake 99") && said.contains("from 1 to 3"), "{said}");
    let none = s.run_with("none", &[]);
    assert!(s.ok(&["why", &none]).contains("no wake on record"));
    let output = s.otto(&["why", &none, "--wake", "1"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
}

/// Every path under `dir`, with what's in it and when it last changed. A directory's mtime is here
/// too, so a file made and removed again would show.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    let mut seen = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(path) = todo.pop() {
        let meta = std::fs::symlink_metadata(&path).unwrap();
        let bytes = if meta.is_dir() {
            todo.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            Vec::new()
        } else {
            std::fs::read(&path).unwrap()
        };
        seen.insert(path, (bytes, meta.modified().unwrap()));
    }
    seen
}

#[test]
fn it_changes_nothing_under_the_home() {
    // No `logs/`, no `wake.lock`, nothing `otto run` would have left behind: these are what a
    // careless read would make.
    let s = Scratch::new();
    let id = s.run_with("still", &three_wakes());
    // No ending on record, so this one does ask whether a wake is running.
    let open = s.run_with("open", &[line(&t("01:00:00"), "wake-started", serde_json::json!({"wake": 1}))]);
    let home = s.root.join("home");
    assert!(!home.join("logs").exists() && !s.run_dir(&open).join("wake.lock").exists());
    let before = snapshot(&home);
    std::thread::sleep(Duration::from_millis(20));
    assert!(s.ok(&["why", &open]).contains("no wake is running now"));
    s.ok(&["why", &id]);
    s.ok(&["why", &id, "--wake", "1"]);
    s.ok(&["why", "still", "--wake", "2"]);
    let _ = s.otto(&["why", &id, "--wake", "99"]);
    assert_eq!(before, snapshot(&home), "without the files");

    // And with every source there, and a lock file that it does probe.
    s.poke_log("poke.log", &format!("{} defer  {id:<34} x\n", t("01:30:00")));
    s.poke_log("poke.log.1", &format!("{} defer  {id:<34} y\n", t("00:30:00")));
    s.history(&id, &check(&t("01:30:00"), 0, "no-change", "z"));
    std::fs::write(s.run_dir(&open).join("wake.lock"), "").unwrap();
    let before = snapshot(&home);
    std::thread::sleep(Duration::from_millis(20));
    s.ok(&["why", &open]);
    s.ok(&["why", &id]);
    s.ok(&["why", &id, "--wake", "2"]);
    assert_eq!(before, snapshot(&home), "with them");
}

#[test]
fn it_says_a_wake_is_running_and_the_wake_carries_on() {
    let s = Scratch::new();
    let repo = s.root.join("repo");
    s.ok(&["run", "--goal", "g", "--name", "held", "--perpetual", "--period", "1h", "--detach", "none", "--workdir", repo.to_str().unwrap()]);
    let list: Value = serde_json::from_str(&s.ok(&["state", "list", "--json"])).unwrap();
    let id = list[0]["id"].as_str().unwrap().to_string();

    let (up, go) = (s.root.join("up"), s.root.join("go"));
    std::fs::write(
        s.root.join("next"),
        format!("touch '{}'\nwhile [ ! -f '{}' ]; do sleep 0.05; done\n", up.display(), go.display()),
    )
    .unwrap();
    let mut wake = s.command(&["wake", &id]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while !up.exists() {
        assert!(Instant::now() < until, "the wake never came up");
        std::thread::sleep(Duration::from_millis(50));
    }

    let dir = s.run_dir(&id);
    let holder = std::fs::read_to_string(dir.join("wake-holder.json")).unwrap();
    let pid = serde_json::from_str::<Value>(&holder).unwrap()["pid"].as_u64().unwrap();
    assert_eq!(pid, wake.id() as u64);
    let lock_changed = std::fs::metadata(dir.join("wake.lock")).unwrap().modified().unwrap();
    let journal = std::fs::read(dir.join("journal.jsonl")).unwrap();
    let state = std::fs::read(dir.join("run.json")).unwrap();
    for _ in 0..25 {
        let said = s.ok(&["why", &id]);
        assert!(said.contains(&format!("ended           still running, pid {pid}, deadline ")), "{said}");
    }
    // Poke asks too, and still sees a wake: a probe from `otto why` didn't take the lock from it.
    let said = s.ok(&["poke", "--dry-run", "--verbose"]);
    assert!(!said.contains("spawn"), "{said}");
    assert_eq!(std::fs::read_to_string(dir.join("wake-holder.json")).unwrap(), holder, "the holder's record is its own");
    assert_eq!(std::fs::metadata(dir.join("wake.lock")).unwrap().modified().unwrap(), lock_changed);
    assert_eq!(std::fs::read(dir.join("journal.jsonl")).unwrap(), journal);
    assert_eq!(std::fs::read(dir.join("run.json")).unwrap(), state);
    assert!(wake.try_wait().unwrap().is_none(), "the wake is still going");

    std::fs::write(&go, "").unwrap();
    assert!(wake.wait().unwrap().success());
    let said = s.ok(&["why", &id]);
    assert!(said.contains("ended           completed "), "{said}");
    assert!(!said.contains("still running"), "{said}");
    let journal = std::fs::read_to_string(dir.join("journal.jsonl")).unwrap();
    assert!(journal.lines().any(|l| l.contains("\"wake-complete\"")) && !journal.contains("wake-incomplete"), "{journal}");
}
