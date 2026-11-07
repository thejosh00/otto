//! tests/wake_cause.rs — what `wake-started` says started it, through the real binary: the commands
//! a person types, the process boundary to the wake, and `otto poke`, with a stub for the model.
//!
//! Every run here is `--detach none`, so nothing needs tmux: a terminal's command runs the wake in
//! place, and poke's and the lander's run as detached processes, which `wait_for` waits out.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (home, repo) = (root.join("home"), root.join("repo"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        // Whatever a wake does this time is in `<run>.next`; it always leaves a handoff.
        let stub = root.join("stub");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nnext='{root}/'\"$OTTO_RUN_ID\".next\nif [ -f \"$next\" ]; then\n  mv \"$next\" \"$next.running\"\n  . \"$next.running\"\nfi\n\
                 printf '# h\\n## Goal\\ng\\n## Where this is\\nHere.\\n## Decided\\nn\\n## This wake did\\nn\\n## Next wake must\\nn\\n## Needs\\nnothing\\n## Don'\"'\"'t re-derive\\nn\\n' | '{otto}' state handoff \"$OTTO_RUN_ID\" --stdin >/dev/null\n",
                root = root.display(),
                otto = env!("CARGO_BIN_EXE_otto"),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        // Named `claude` too, so nothing here can ever reach the real one.
        std::fs::write(
            home.join("config.json"),
            serde_json::json!({"launchers": [{"name": "claude", "command": stub.display().to_string()}]}).to_string(),
        )
        .unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        Scratch { _dir: dir, root }
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn otto(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_otto"))
            .args(args)
            .current_dir(self.repo())
            .env("OTTO_HOME", self.root.join("home"))
            .env("TMUX_TMPDIR", &self.root)
            .env("OTTO_BIN", env!("CARGO_BIN_EXE_otto"))
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT")
            .output()
            .unwrap_or_else(|e| panic!("could not run otto {args:?}: {e}"))
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.otto(args);
        assert!(output.status.success(), "otto {args:?}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    /// A run with a first wake that does nothing but leave its handoff, returning its id.
    fn run(&self, name: &str) -> String {
        let repo = self.repo();
        let repo = repo.to_str().unwrap();
        self.ok(&["run", "--goal", "g", "--name", name, "--perpetual", "--period", "1h", "--detach", "none", "--repo", repo, "--workdir", repo]);
        let list: Value = serde_json::from_str(&self.ok(&["state", "list", "--json"])).unwrap();
        list.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap()["id"].as_str().unwrap().to_string()
    }

    /// What this run's next wake does.
    fn next_wake_does(&self, id: &str, script: &str) {
        std::fs::write(self.root.join(format!("{id}.next")), script).unwrap();
    }

    fn journal(&self, id: &str) -> Vec<Value> {
        let path = self.root.join("home/runs").join(id).join("journal.jsonl");
        std::fs::read_to_string(path).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// The causes on this run's `wake-started` lines, in order.
    fn causes(&self, id: &str) -> Vec<String> {
        self.journal(id).iter().filter(|l| l["event"] == "wake-started").map(|l| l["cause"].as_str().unwrap_or("").to_string()).collect()
    }

    /// Wait for a wake started in another process to have started and finished.
    fn wait_for(&self, id: &str, wakes: usize) {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let journal = self.journal(id);
            let finished = journal.iter().filter(|l| l["event"] == "wake-complete" || l["event"] == "wake-incomplete").count();
            if finished >= wakes {
                return;
            }
            assert!(Instant::now() < until, "{id} never got to {wakes} wakes: {journal:#?}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).current_dir(dir).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

const OTTO: &str = env!("CARGO_BIN_EXE_otto");

#[test]
fn what_a_person_does_is_what_the_wake_says_started_it() {
    let s = Scratch::new();
    let id = s.run("person");
    s.ok(&["wake", &id]);
    s.ok(&["note", &id, "use the small one", "--now"]);

    let question = s.root.join("question.md");
    std::fs::write(&question, "Which?\n\n## Options\n- **Left** *(default)*\n- **Right**\n").unwrap();
    s.next_wake_does(&id, &format!("'{OTTO}' state open-gate \"$OTTO_RUN_ID\" --slug which --question-file '{}'\n", question.display()));
    s.ok(&["wake", &id]);
    s.ok(&["answer", &id, "--choice", "left"]);

    s.next_wake_does(&id, &format!("'{OTTO}' state message \"$OTTO_RUN_ID\" --kind update --title News --body 'It moved.'\n"));
    s.ok(&["wake", &id]);
    s.ok(&["inbox", "done", &id, "--reply", "thanks", "--now"]);

    s.ok(&["stop", &id, "--reason", "enough"]);
    let before = s.causes(&id).len();
    s.ok(&["resume", &id]);
    s.wait_for_detached(&id, before + 1);

    assert_eq!(
        s.causes(&id),
        ["person", "person", "note", "person", "gate-answered", "person", "note", "person"],
        "the run, a wake, a note, a wake opening a gate, its answer, a wake sending a message, a reply, a resume"
    );
}

impl Scratch {
    /// How many wakes have started, once the last has finished: a resume's wake is detached.
    fn wait_for_detached(&self, id: &str, wakes: usize) {
        let until = Instant::now() + Duration::from_secs(30);
        while self.causes(id).len() < wakes {
            assert!(Instant::now() < until, "{id} never started wake {wakes}");
            std::thread::sleep(Duration::from_millis(100));
        }
        self.wait_for(id, wakes);
    }
}

/// A change from approval to landing: the lander is started because of the approval, and its author
/// comes back at the next poke because of the landing, which `otto poke` can only know from the
/// journal, and it agrees with what poke says about that run.
#[test]
fn a_change_approved_starts_the_lander_and_a_change_landed_brings_its_author_back() {
    let s = Scratch::new();
    let lander = s.run("lander");
    s.ok(&["config", "lander", &lander]);
    let author = s.run("author");

    let repo = s.repo();
    git(&repo, &["checkout", "-q", "-b", "demo/one"]);
    std::fs::write(repo.join("f"), "x\n").unwrap();
    git(&repo, &["add", "f"]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    git(&repo, &["checkout", "-q", "main"]);
    std::fs::write(s.root.join("summary.md"), "What it does.\n").unwrap();
    s.next_wake_does(
        &author,
        &format!(
            "'{OTTO}' state change submit \"$OTTO_RUN_ID\" --repo '{}' --branch demo/one --title One --summary-file '{}' --tests none --no-evidence 'a stub wake'\n",
            repo.display(),
            s.root.join("summary.md").display()
        ),
    );
    s.ok(&["wake", &author]);

    git(&repo, &["merge", "-q", "--ff-only", "demo/one"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    s.next_wake_does(
        &lander,
        &format!(
            "'{OTTO}' state change claim \"$OTTO_RUN_ID\" {author}/1\n'{OTTO}' state change landed \"$OTTO_RUN_ID\" {author}/1 --sha {sha}\n"
        ),
    );
    let lander_wakes = s.causes(&lander).len();
    s.ok(&["changes", "approve", &format!("{author}/1@v1"), "--push"]);
    s.wait_for_detached(&lander, lander_wakes + 1);
    assert_eq!(s.causes(&lander).last().unwrap(), "change-approved");

    let armed = s.journal(&author);
    assert!(
        armed.iter().any(|l| l["event"] == "timer-armed" && l["cause"] == "change-landed"),
        "the landing says what the sleep is for: {armed:#?}"
    );
    let author_wakes = s.causes(&author).len();
    let said = s.ok(&["poke", "--verbose"]);
    s.wait_for_detached(&author, author_wakes + 1);
    assert_eq!(s.causes(&author).last().unwrap(), "change-landed", "poke said:\n{said}");
    assert!(said.lines().any(|l| l.contains("spawn") && l.contains(&author)), "{said}");
}

/// Poke's decision, through the argv it spawns, to the line the child writes, and what a person reads.
#[test]
fn poke_s_reason_is_on_the_line_the_wake_writes() {
    let s = Scratch::new();
    let id = s.run("poked");
    s.next_wake_does(&id, &format!("'{OTTO}' state arm-timer \"$OTTO_RUN_ID\" --in 1\n"));
    s.ok(&["wake", &id]);
    std::thread::sleep(Duration::from_secs(2));
    let said = s.ok(&["poke"]);
    assert!(said.lines().any(|l| l.contains("spawn") && l.contains("missed its wake")), "{said}");
    s.wait_for_detached(&id, 3);
    assert_eq!(s.causes(&id), ["person", "person", "timer"]);

    let logs = s.ok(&["logs", &id]);
    assert!(logs.contains("cause=timer"), "{logs}");
}

/// A run with lines from before causes were recorded: they read, and say so.
#[test]
fn an_old_journal_line_shows_as_not_recorded() {
    let s = Scratch::new();
    let id = s.run("old");
    let journal = s.root.join("home/runs").join(&id).join("journal.jsonl");
    let lines = std::fs::read_to_string(&journal).unwrap();
    let old = "{\"event\":\"wake-started\",\"wake\":0,\"deadlineAt\":\"2026-09-12T12:00:00Z\",\"launcher\":\"claude\",\"ts\":\"2026-09-12T11:00:00Z\"}\n";
    std::fs::write(&journal, format!("{old}{lines}")).unwrap();

    let logs = s.ok(&["logs", &id]);
    assert!(logs.contains("cause=not recorded"), "{logs}");
    assert!(logs.contains("cause=person"), "{logs}");
}

/// The hidden flag isn't something a person is offered.
#[test]
fn the_flag_stays_out_of_the_help() {
    let s = Scratch::new();
    assert!(!s.ok(&["wake", "--help"]).contains("--cause"));
}
