//! tests/live_land_install.rs — prove a real lander lets otto's install after a landing run its
//! course, against a real model.
//!
//! What `cargo test` cannot reach: what a model does when the landing step's result says
//! `landed at <sha>, install pending` while the change is still `landing` (otto marks it landed and
//! tells the author only once the install is over). It must not ask again, send the change back,
//! say `landed` itself or open a gate; and the change must end landed and cleaned up, its note
//! saying how the install went. The scratch repository is made to be "the otto repository": the
//! skills link (a scratch `OTTO_CLAUDE_SKILLS_DIR`) points at it, its history holds this binary's
//! commit, and its install.sh is a stand-in that takes a little while and installs nothing.
//!
//! And what it does when it is woken for something else while that install runs (report 55): the
//! landing itself doesn't wake it, `due` names the change as installing, and it neither goes
//! looking through otto's landing records nor touches the change. The test wakes it once, mid-install.
//!
//! Two or three wakes, ~6 minutes. An `OTTO_HOME` of its own under the scratch repo. The real
//! install.sh is never run and `~/.local/bin` is never touched.
//!
//!   cargo test --test live_land_install -- --ignored --nocapture

mod support;

use serde_json::Value;
use std::time::{Duration, Instant};
use support::*;

const TIMEOUT: Duration = Duration::from_secs(900);

fn change(run: &Run, n: u64) -> Value {
    let index: Value = std::fs::read_to_string(run.dir.join("changes/index.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    index["changes"]
        .as_array()
        .and_then(|all| all.iter().find(|c| c["n"] == n).cloned())
        .unwrap_or(Value::Null)
}

/// Every command the lander's sessions ran, and how many of those sessions' transcripts could be
/// read. Every wake's session, from the journal's `wake-started` — `wake.session` is only the latest,
/// and landing and cleaning up may be two wakes. Only what the model ran, not what it was given to
/// read: the harness names `check-authorized` itself.
fn commands_run(run: &Run) -> (usize, Vec<String>) {
    let journal = std::fs::read_to_string(run.dir.join("journal.jsonl")).unwrap_or_default();
    let mut sessions: Vec<String> = journal
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| v["event"] == "wake-started")
        .filter_map(|v| v["session"].as_str().map(str::to_string))
        .collect();
    let latest = run.field_str("wake.session");
    if !latest.is_empty() && !sessions.contains(&latest) {
        sessions.push(latest);
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let projects: Vec<_> = std::fs::read_dir(home.join(".claude/projects"))
        .map(|d| d.flatten().map(|p| p.path()).collect())
        .unwrap_or_default();
    let (mut read, mut commands) = (0, Vec::new());
    for session in sessions {
        let Some(text) = projects
            .iter()
            .find_map(|p| std::fs::read_to_string(p.join(format!("{session}.jsonl"))).ok())
        else {
            continue;
        };
        read += 1;
        for entry in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        {
            if entry["type"] != "assistant" {
                continue;
            }
            for block in entry["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use" {
                    if let Some(command) = block["input"]["command"].as_str() {
                        commands.push(command.to_string());
                    }
                }
            }
        }
    }
    (read, commands)
}

/// The lander's journal, as (event, ts, cause).
fn journal(run: &Run) -> Vec<(String, String, String)> {
    std::fs::read_to_string(run.dir.join("journal.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|v| {
            let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
            (s("event"), s("ts"), s("cause"))
        })
        .collect()
}

/// Whether the landing step's result for the author's change 1 says its install is still running.
fn install_pending(home: &std::path::Path, author: &Run) -> bool {
    let Some(id) = author.dir.file_name() else {
        return false;
    };
    std::fs::read_to_string(home.join("landings").join(id).join("1.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some_and(|r| r["install"]["state"] == "pending")
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_land_install() {
    let mut checks = Checks::new();

    checks.step("0 - preconditions, and a home of its own");
    require_launcher(&mut checks);
    let mut scratch = ScratchRepo::new(&workspace_dir(), "land-install");
    let home = scratch.path().join("otto-home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("OTTO_HOME", &home);
    checks.ok(format!("OTTO_HOME={}", home.display()));

    checks.step(
        "1 - a repo on the direct process, where approval pushes, with a real (local) remote",
    );
    let origin = scratch.origin_path();
    let repo = scratch.repo_path("greeter");
    git_ok(
        scratch.path(),
        &[
            "init",
            "--quiet",
            "--bare",
            "--initial-branch=main",
            origin.to_str().unwrap(),
        ],
    );
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "--quiet", "--initial-branch=main"]);
    write_file(
        &repo.join("README.md"),
        "# greeter\n\nSays hello.\n\n## Testing\n\n```\npython3 -m unittest\n```\n",
    );
    write_file(
        &repo.join("greet.py"),
        "def greet(name: str) -> str:\n    return f\"Hello, {name}!\"\n",
    );
    // What makes it the otto repository: the marker files, a stand-in install.sh that only says it
    // ran, the skills link pointing here, and a branch at the commit this otto was built from.
    write_file(&repo.join("harness/wake.md"), "harness\n");
    write_file(&repo.join("skills/manage-pr/SKILL.md"), "skill\n");
    let ran = scratch.path().join("install-ran");
    write_file(
        &repo.join("install.sh"),
        &format!("#!/bin/sh\nsleep 120\necho \"$*\" > '{}'\n", ran.display()),
    );
    std::fs::set_permissions(
        repo.join("install.sh"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let skills = scratch.path().join("claude-skills");
    std::fs::create_dir_all(&skills).unwrap();
    std::os::unix::fs::symlink(repo.join("skills/manage-pr"), skills.join("manage-pr")).unwrap();
    std::env::set_var("OTTO_CLAUDE_SKILLS_DIR", &skills);
    let version = stdout(&otto(&["--version"]));
    let built = version
        .split('(')
        .nth(1)
        .and_then(|v| v.split(')').next())
        .unwrap_or("")
        .trim_end_matches("-dirty")
        .to_string();
    git_ok(
        &repo,
        &["fetch", "--quiet", env!("CARGO_MANIFEST_DIR"), "HEAD"],
    );
    git_ok(&repo, &["branch", "built", &built]);
    git_ok(&repo, &["add", "-A"]);
    git_ok(
        &repo,
        &[
            "-c",
            "user.email=otto@local",
            "-c",
            "user.name=otto",
            "commit",
            "--quiet",
            "-m",
            "greeter: hello",
        ],
    );
    git_ok(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_ok(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git_ok(&repo, &["remote", "set-head", "origin", "main"]);
    let out = otto(&[
        "config",
        "project",
        repo.to_str().unwrap(),
        "--process",
        "direct",
    ]);
    checks.check(
        out.status.success(),
        "its process is direct",
        format!(
            "config project failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
    );
    let workdir = scratch.path().to_str().unwrap().to_string();

    checks.step("2 - a lander, wrapping land-flow, and a stand-in author with one change");
    let out = otto(&[
        "state",
        "init",
        "--name",
        "author",
        "--perpetual",
        "--repo",
        repo.to_str().unwrap(),
        "--workdir",
        &workdir,
        "--goal",
        "Stand in for a run that writes code",
    ]);
    checks.check(
        out.status.success(),
        "the author exists",
        format!(
            "state init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
    );
    let author = Run::new(stdout(&out).lines().last().unwrap_or("2").to_string());
    let land_flow = repo_path("workflows/land-flow.md");
    let out = otto(&[
        "state",
        "init",
        "--name",
        "lander",
        "--perpetual",
        "--period",
        "10m",
        "--instructions",
        land_flow.to_str().unwrap(),
        "--repo",
        &workdir,
        "--workdir",
        &workdir,
        "--goal",
        "Land the changes I approve, and clean up after them",
    ]);
    checks.check(
        out.status.success(),
        "the lander exists",
        format!(
            "state init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
    );
    let lander = Run::new(stdout(&out).lines().last().unwrap_or("1").to_string());
    let out = otto(&["config", "lander", "lander"]);
    checks.check(
        out.status.success(),
        "it is the lander",
        "otto config lander failed",
    );

    let worktree = scratch.path().join(".work-trees/farewell");
    git_ok(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "author/farewell",
            worktree.to_str().unwrap(),
            "main",
        ],
    );
    write_file(
        &worktree.join("farewell.py"),
        "def farewell(name: str) -> str:\n    return f\"Goodbye, {name}!\"\n",
    );
    git_ok(&worktree, &["add", "-A"]);
    git_ok(
        &worktree,
        &[
            "-c",
            "user.email=otto@local",
            "-c",
            "user.name=otto",
            "commit",
            "--quiet",
            "-m",
            "a farewell",
        ],
    );
    let approved = stdout(&git(&worktree, &["rev-parse", "HEAD"]));
    let summary = scratch.path().join("summary.md");
    write_file(&summary, "A farewell, beside the greeting.\n");
    let out = otto(&[
        "state",
        "change",
        "submit",
        &author.id,
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "author/farewell",
        "--title",
        "A farewell",
        "--summary-file",
        summary.to_str().unwrap(),
        "--tests",
        "no tests touch it",
        "--no-evidence",
        "a stand-in: the test's own commit",
    ]);
    checks.check(
        out.status.success(),
        "handed back",
        format!("submit failed: {}", String::from_utf8_lossy(&out.stderr)),
    );
    let _ = otto(&[
        "stop",
        &author.id,
        "--reason",
        "a stand-in: it writes nothing",
    ]);

    checks.step(&format!(
        "3 - approve it, plainly (up to {}min)",
        TIMEOUT.as_secs() / 60
    ));
    let out = otto(&["changes", "approve", "author/1@v1"]);
    checks.check(
        out.status.success(),
        "approved",
        format!("approve failed: {}", String::from_utf8_lossy(&out.stderr)),
    );
    let d = &change(&author, 1)["decision"];
    checks.check(
        d["mode"] == "push",
        "to push, as its process says",
        format!("the decision: {d}"),
    );

    let start = Instant::now();
    let mut nudged = false;
    let state = loop {
        let c = change(&author, 1);
        let state = c["state"].as_str().unwrap_or("").to_string();
        if (state == "landed" && !c["cleaned"].is_null())
            || state == "revising"
            || state == "needs-you"
            || start.elapsed() > TIMEOUT
        {
            break state;
        }
        // Once, while otto installs it: the lander is due for some other reason (here, its timer).
        if !nudged && install_pending(&home, &author) && !lander.wake_running() {
            nudged = otto(&["state", "arm-timer", &lander.id, "--in", "1"])
                .status
                .success();
            std::thread::sleep(Duration::from_secs(2));
        }
        let _ = otto(&["poke"]);
        std::thread::sleep(Duration::from_secs(15));
    };

    checks.step("4 - it landed, as the commit approved");
    checks.check(
        state == "landed",
        "it landed",
        format!("the change is {state}"),
    );
    // The lander asks; otto pushes, outside the lander's wake, and records the result itself.
    checks.check(
        lander.events("land-requested") > 0 && lander.events("land-result") > 0,
        "the lander asked otto to land it, and otto recorded the result",
        format!(
            "land-requested {} time(s), land-result {} time(s) in the lander's journal",
            lander.events("land-requested"),
            lander.events("land-result")
        ),
    );
    let main_now = stdout(&git(&origin, &["rev-parse", "main"]));
    checks.check(
        main_now == approved,
        "origin/main is the very commit approved",
        format!("origin/main is {main_now}, approved {approved}"),
    );

    checks.step("4b - otto installed it, once, and the note says so");
    let c = change(&author, 1);
    let note = c["landed"]["note"].as_str().unwrap_or("").to_string();
    checks.check(
        note.contains("otto installed this commit"),
        "the landing's note says it was installed",
        format!("the note: {note:?}"),
    );
    checks.check(ran.exists(), "the stand-in install.sh ran", "it never ran");
    checks.check(
        lander.events("land-install") == 1,
        "one install",
        format!("land-install {} time(s)", lander.events("land-install")),
    );
    checks.check(
        lander.events("land-requested") == 1,
        "the lander asked once, and waited out the install",
        format!("land-requested {} time(s)", lander.events("land-requested")),
    );

    checks.step("4c - the install didn't cost the lander a wake with nothing in it (report 55)");
    let events = journal(&lander);
    let at = |name: &str| {
        events
            .iter()
            .find(|(event, _, _)| event == name)
            .map(|(_, ts, _)| ts.clone())
            .unwrap_or_default()
    };
    let (landed_at, installed_at) = (at("land-result"), at("land-install"));
    let during: Vec<&(String, String, String)> = events
        .iter()
        .filter(|(event, ts, _)| {
            event == "wake-started"
                && !landed_at.is_empty()
                && !installed_at.is_empty()
                && *ts >= landed_at
                && *ts <= installed_at
        })
        .collect();
    let for_landing = during
        .iter()
        .filter(|(_, _, cause)| cause == "change-landed")
        .count();
    checks.check(
        for_landing == 0,
        "the landing didn't wake the lander while otto was installing it",
        format!("{for_landing} wake(s) for change-landed between the landing and the end of its install"),
    );
    checks.check(
        nudged && !during.is_empty(),
        format!(
            "it was woken for something else mid-install ({} wake(s)), and read `due` then",
            during.len()
        ),
        format!(
            "no wake fell inside the install (nudged: {nudged}, landed {landed_at}, installed {installed_at}): what it does then wasn't seen"
        ),
    );
    let woken_after = events.iter().any(|(event, ts, cause)| {
        event == "timer-armed" && cause == "change-landed" && !installed_at.is_empty() && *ts >= installed_at
    });
    checks.check(
        woken_after,
        "and it was woken to clean up once the install was over",
        "no change-landed wake was armed after the install finished",
    );

    checks.step("5 - the lander asked nothing");
    checks.check(
        lander.events("gate-opened") == 0,
        "it opened no gate",
        format!("it opened {} gate(s)", lander.events("gate-opened")),
    );
    checks.check(
        lander.field_str("gate.slug").is_empty(),
        "and has none open",
        format!("its gate is {}", lander.field_str("gate.slug")),
    );

    checks.step("6 - and confirmed the approval one way: the change's own check");
    let (sessions, commands) = commands_run(&lander);
    if sessions == 0 {
        checks
            .skip("none of the lander's transcripts could be read, which says nothing either way");
    } else {
        checks.note(format!(
            "{sessions} session(s), {} command(s)",
            commands.len()
        ));
        let checked: Vec<&String> = commands
            .iter()
            .filter(|c| c.contains("state change check"))
            .collect();
        checks.check(
            !checked.is_empty(),
            "it ran `change check`",
            "no command it ran was `otto state change check`",
        );
        let authorized: Vec<&String> = commands
            .iter()
            .filter(|c| c.contains("check-authorized"))
            .collect();
        checks.check(
            authorized.is_empty(),
            "it never ran `check-authorized`",
            format!("it ran: {authorized:?}"),
        );
        let pushed: Vec<&String> = commands
            .iter()
            .filter(|c| c.contains("git") && c.contains(" push "))
            .collect();
        checks.check(
            pushed.is_empty(),
            "it never ran `git push`",
            format!("it ran: {pushed:?}"),
        );
        let said: Vec<&String> = commands
            .iter()
            .filter(|c| c.contains("state change landed"))
            .collect();
        checks.check(
            said.is_empty(),
            "it never said `landed` itself",
            format!("it ran: {said:?}"),
        );
        let looked: Vec<&String> = commands
            .iter()
            .filter(|c| c.contains("landings"))
            .collect();
        checks.check(
            looked.is_empty(),
            "it never went looking through otto's landing records",
            format!("it ran: {looked:?}"),
        );
        let asked: Vec<&String> = commands
            .iter()
            .filter(|c| c.contains("state change request-land"))
            .collect();
        checks.check(
            !asked.is_empty(),
            "it ran `change request-land`",
            "no command it ran was `otto state change request-land`",
        );
    }

    // Whether it had to go looking is the model's account; shown for whoever reads the transcript.
    let reports = stdout(&otto(&["feedback", "--all", "--json"]));
    checks.note(format!("what the lander reported about otto: {reports}"));
    println!("\nthe lander's output:\n{}", lander.wake_log(80));
    lander.journal_dump();
    if checks.fail_count() > 0 {
        scratch.keep();
    }
    let _ = otto(&["stop", &lander.id, "--reason", "live test finished"]);
    checks.finish();
}
