//! tests/unknown_keys.rs — a key in `run.json` this otto doesn't name survives every rewrite (plan
//! v3, item 2.1). Without it, each field a newer otto adds is a one-way door: roll back, and the
//! older binary's first rewrite erases it. Through the real binary, in this test's own home.

mod support;

use serde_json::{json, Value};
use std::path::PathBuf;

fn ok(args: &[&str]) -> String {
    let out = support::otto(args);
    assert!(out.status.success(), "otto {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    support::stdout(&out)
}

/// A run with one standing note, returned with the path of its `run.json`.
fn run_with_a_note(name: &str) -> (String, PathBuf) {
    let id = ok(&["state", "init", "--goal", "keep keys", "--name", name, "--perpetual"]).trim().to_string();
    ok(&["note", &id, "--standing", "a standing note"]);
    let path = support::own_home().join("runs").join(&id).join("run.json");
    (id, path)
}

fn read(path: &PathBuf) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Every mutating command a wake runs, in order: each rewrites `run.json`.
fn mutating_commands(id: &str, question: &str) -> Vec<Vec<String>> {
    [
        vec!["state", "tick", id, "--note", "looked"],
        vec!["state", "log", id, "--event", "cycle", "--message", "did"],
        vec!["state", "record-fact", id, "k=1"],
        vec!["state", "open-gate", id, "--slug", "q", "--question-file", question],
        vec!["state", "close-gate", id, "--answer", "A"],
    ]
    .into_iter()
    .map(|c| c.into_iter().map(String::from).collect())
    .collect()
}

fn question_file(name: &str) -> String {
    let path = support::own_home().join(format!("{name}-q.md"));
    std::fs::write(&path, "Which?\n## Options\n- **A** *(default)*\n- B\n").unwrap();
    path.display().to_string()
}

#[test]
fn keys_this_otto_does_not_name_survive_every_mutating_command() {
    let (id, path) = run_with_a_note("unknown-keys");
    // As a newer otto would have written them: at the top, in a note (plan 2.2's `from`), and in
    // the long-lived parts a rewrite edits in place.
    let mut state = read(&path);
    state["future"] = json!({"a": 1});
    state["notes"][0]["from"] = json!("person");
    state["wraps"]["w"] = json!(1);
    state["launcher"]["l"] = json!([1]);
    state["permission"]["p"] = json!("x");
    state["budget"]["b"] = json!(2.5);
    std::fs::write(&path, serde_json::to_string_pretty(&state).unwrap() + "\n").unwrap();

    let question = question_file("unknown-keys");
    for command in mutating_commands(&id, &question) {
        let args: Vec<&str> = command.iter().map(String::as_str).collect();
        ok(&args);
        let after = read(&path);
        let lost: Vec<&str> = [
            ("future", &after["future"], json!({"a": 1})),
            ("notes[0].from", &after["notes"][0]["from"], json!("person")),
            ("wraps.w", &after["wraps"]["w"], json!(1)),
            ("launcher.l", &after["launcher"]["l"], json!([1])),
            ("permission.p", &after["permission"]["p"], json!("x")),
            ("budget.b", &after["budget"]["b"], json!(2.5)),
        ]
        .into_iter()
        .filter(|(_, got, want)| *got != want)
        .map(|(key, _, _)| key)
        .collect();
        assert!(lost.is_empty(), "after otto {args:?}, lost or changed: {lost:?}\n{after:#}");
    }
    // Written back once each, never twice: a key the struct names is not also in the catch-all.
    let text = std::fs::read_to_string(&path).unwrap();
    for key in ["\"future\"", "\"goal\"", "\"notes\"", "\"from\"", "\"spentWakes\"", "\"kind\""] {
        let want = if key == "\"kind\"" { 2 } else { 1 }; // wraps.kind and launcher.kind
        assert_eq!(text.matches(key).count(), want, "{key} in\n{text}");
    }
}

#[test]
fn a_run_with_no_unknown_keys_writes_nothing_new() {
    let (id, path) = run_with_a_note("no-unknown-keys");
    let before = read(&path);
    let question = question_file("no-unknown-keys");
    for command in mutating_commands(&id, &question) {
        let args: Vec<&str> = command.iter().map(String::as_str).collect();
        ok(&args);
    }
    let after = read(&path);
    // The same keys at every level the catch-all sits on: an empty one writes nothing.
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    for part in ["wraps", "launcher", "permission", "budget"] {
        assert_eq!(keys(&before[part]), keys(&after[part]), "{part}");
    }
    assert_eq!(keys(&before["notes"][0]), keys(&after["notes"][0]));
    assert!(after.get("extra").is_none() && after["notes"][0].get("extra").is_none());
}
