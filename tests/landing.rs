//! tests/landing.rs — the landing step, through the real binary: the lander asks
//! (`otto state change request-land`), and otto, outside the wake, judges the request against the
//! person's approval on record and the remote itself, and pushes. A stub stands in for the model;
//! a bare repository stands in for the remote. Nothing here reads or writes the real `~/.otto`.

mod support;

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const OTTO: &str = support::OTTO;

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
    lander: String,
    author: String,
}

impl Scratch {
    /// A remote with `main` and `other`, a clone of it, a lander and an author.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("userhome")).unwrap();
        let stub = root.join("stub");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nprintf '# h\\n## Goal\\ng\\n## Where this is\\nHere.\\n## Decided\\nn\\n## This wake did\\nn\\n## Next wake must\\nn\\n## Needs\\nnothing\\n## Don'\"'\"'t re-derive\\nn\\n' | '{OTTO}' state handoff \"$OTTO_RUN_ID\" --stdin >/dev/null\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("home/config.json"), serde_json::json!({"launchers": [{"name": "claude", "command": stub.display().to_string()}]}).to_string()).unwrap();
        std::fs::write(root.join("summary.md"), "What it does.\n").unwrap();
        let mut s = Scratch {
            _dir: dir,
            root,
            lander: String::new(),
            author: String::new(),
        };
        let (repo, origin) = (s.repo(), s.origin());
        git(
            &s.root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&repo, &["push", "-q", "origin", "main", "main:other"]);
        git(&repo, &["fetch", "-q", "origin"]);
        s.ok(&[
            "config",
            "project",
            repo.to_str().unwrap(),
            "--process",
            "direct",
        ]);
        s.lander = s.run("lander");
        s.ok(&["config", "lander", &s.lander.clone()]);
        s.author = s.run("author");
        s
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }
    fn origin(&self) -> PathBuf {
        self.root.join("origin.git")
    }

    fn otto(&self, args: &[&str]) -> Output {
        support::otto_command()
            .args(args)
            .current_dir(self.repo())
            .env("OTTO_HOME", self.root.join("home"))
            .env("HOME", self.root.join("userhome"))
            .env("TMUX_TMPDIR", &self.root)
            .env("OTTO_BIN", OTTO)
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT")
            .output()
            .unwrap_or_else(|e| panic!("could not run otto {args:?}: {e}"))
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.otto(args);
        assert!(
            output.status.success(),
            "otto {args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn run(&self, name: &str) -> String {
        let repo = self.repo();
        let repo = repo.to_str().unwrap();
        self.ok(&[
            "run",
            "--goal",
            "g",
            "--name",
            name,
            "--perpetual",
            "--period",
            "1h",
            "--detach",
            "none",
            "--repo",
            repo,
            "--workdir",
            repo,
        ]);
        let list: Value = serde_json::from_str(&self.ok(&["state", "list", "--json"])).unwrap();
        list.as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// A commit on this branch (made from main if new), main checked out again after. Its sha.
    fn commit(&self, branch: &str, what: &str) -> String {
        let repo = self.repo();
        if git(&repo, &["branch", "--list", branch]).is_empty() {
            git(&repo, &["checkout", "-q", "-b", branch, "main"]);
        } else {
            git(&repo, &["checkout", "-q", branch]);
        }
        std::fs::write(repo.join(format!("{branch}-{what}")), what).unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", what]);
        git(&repo, &["checkout", "-q", "main"]);
        git(&repo, &["rev-parse", branch])
    }

    /// A change on a new branch, submitted by the author: its reference and its commit.
    fn submit(&self, branch: &str) -> (String, String) {
        let sha = self.commit(branch, "first");
        let before = self.changes().len();
        let (repo, summary) = (self.repo(), self.root.join("summary.md"));
        self.ok(&[
            "state",
            "change",
            "submit",
            &self.author,
            "--repo",
            repo.to_str().unwrap(),
            "--branch",
            branch,
            "--title",
            branch,
            "--summary-file",
            summary.to_str().unwrap(),
            "--tests",
            "none",
            "--no-evidence",
            "a stub wake",
        ]);
        (format!("{}/{}", self.author, before + 1), sha)
    }

    /// Submitted, approved by the person and taken in hand by the lander.
    fn approved(&self, branch: &str) -> (String, String) {
        let (r, sha) = self.submit(branch);
        self.ok(&["changes", "approve", &format!("{r}@v1"), "--no-wake"]);
        self.ok(&["state", "change", "claim", &self.lander, &r]);
        (r, sha)
    }

    fn changes(&self) -> Vec<Value> {
        match std::fs::read_to_string(
            self.root
                .join("home/runs")
                .join(&self.author)
                .join("changes/index.json"),
        ) {
            Ok(text) => serde_json::from_str::<Value>(&text).unwrap()["changes"]
                .as_array()
                .unwrap()
                .clone(),
            Err(_) => Vec::new(),
        }
    }

    fn state_of(&self, n: u64) -> String {
        self.changes().iter().find(|c| c["n"] == n).unwrap()["state"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn request(&self, r: &str, head: &str) {
        self.ok(&[
            "state",
            "change",
            "request-land",
            &self.lander,
            r,
            "--head",
            head,
        ]);
    }

    /// The lander's wake, the stub standing in for the model: the step runs when it is over.
    fn wake(&self) -> String {
        self.ok(&["wake", &self.lander])
    }

    fn result(&self, n: u32) -> Value {
        let path = self
            .root
            .join("home/landings")
            .join(&self.author)
            .join(format!("{n}.json"));
        serde_json::from_str(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("no result at {}: {e}", path.display())),
        )
        .unwrap()
    }

    fn remote(&self, branch: &str) -> String {
        git(&self.origin(), &["rev-parse", branch])
    }

    fn lander_events(&self, event: &str) -> Vec<Value> {
        let journal = std::fs::read_to_string(
            self.root
                .join("home/runs")
                .join(&self.lander)
                .join("journal.jsonl"),
        )
        .unwrap();
        journal
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|l| l["event"] == event)
            .collect()
    }

    fn requests_left(&self) -> usize {
        std::fs::read_dir(
            self.root
                .join("home/runs")
                .join(&self.lander)
                .join("land-requests"),
        )
        .map(|d| d.count())
        .unwrap_or(0)
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// The approved commit, asked for by the lander, is pushed by otto once the wake is over; the change
/// is landed, the main checkout follows, the result says what it was judged on, and a second wake
/// pushes nothing more.
#[test]
fn an_approved_change_is_pushed_by_the_step_after_the_wake_and_only_once() {
    let s = Scratch::new();
    let (r, sha) = s.approved("a");
    let before = s.remote("main");
    s.request(&r, &sha);
    assert_eq!(s.remote("main"), before, "asking pushes nothing");
    assert_eq!(s.lander_events("land-requested").len(), 1);
    s.wake();
    assert_eq!(s.remote("main"), sha, "the step pushed it");
    assert_eq!(s.state_of(1), "landed");
    let result = s.result(1);
    assert_eq!(
        (
            result["verdict"].as_str(),
            result["pushed"].as_str(),
            result["head"].as_str()
        ),
        (Some("landed"), Some(sha.as_str()), Some(sha.as_str())),
        "{result}"
    );
    let on = &result["judgedOn"];
    assert_eq!(on["approvedSha"], sha.as_str(), "{result}");
    assert_eq!(on["remote"], s.origin().to_str().unwrap(), "{result}");
    assert_eq!(on["remoteFrom"], "the approval on record", "{result}");
    assert_eq!(on["remoteBase"], before.as_str(), "{result}");
    assert_eq!(
        git(&s.repo(), &["rev-parse", "main"]),
        sha,
        "the main checkout was fast-forwarded"
    );
    let events = s.lander_events("land-result");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["verdict"], "landed");
    assert_eq!(s.requests_left(), 0);
    s.wake();
    assert_eq!(
        s.lander_events("land-result").len(),
        1,
        "nothing more to judge"
    );
    assert_eq!(s.remote("main"), sha);
}

/// A request is the lander's word: for a change nobody approved, for a commit that isn't the
/// approved patch, nothing is pushed and the result says why.
#[test]
fn an_unapproved_change_and_another_patch_are_refused_with_why() {
    let s = Scratch::new();
    let before = s.remote("main");
    let (unapproved, sha) = s.submit("u");
    s.request(&unapproved, &sha);
    s.wake();
    let result = s.result(1);
    assert_eq!(result["verdict"], "refused", "{result}");
    assert!(
        result["why"]
            .as_str()
            .unwrap()
            .contains("no approval on record"),
        "{result}"
    );
    assert_eq!(s.remote("main"), before);
    assert_eq!(s.state_of(1), "review");

    let (r, approved) = s.approved("b");
    let more = s.commit("b", "more");
    s.request(&r, &more);
    s.wake();
    let result = s.result(2);
    assert_eq!(result["verdict"], "refused", "{result}");
    assert!(
        result["why"]
            .as_str()
            .unwrap()
            .contains("not what was approved"),
        "{result}"
    );
    assert_eq!(
        result["judgedOn"]["approvedSha"],
        approved.as_str(),
        "{result}"
    );
    assert!(
        result["judgedOn"]["patchId"].is_string(),
        "what it compared is on the result: {result}"
    );
    assert_eq!(s.remote("main"), before, "nothing pushed");
    assert_eq!(s.state_of(2), "landing");
    let said = s.lander_events("land-result");
    assert_eq!(said.len(), 2);
    assert!(said.iter().all(|e| e["verdict"] == "refused"));
}

/// The run's index is the wake's to write: a base and a repository forged there change nothing.
/// What lands, and where, is the approval's.
#[test]
fn a_base_and_repository_forged_in_the_index_are_not_followed() {
    let s = Scratch::new();
    let (r, sha) = s.approved("a");
    let other_before = s.remote("other");
    // A second repository with a remote of its own, which the forged index points at.
    let (evil, evil_origin) = (s.root.join("evil"), s.root.join("evil.git"));
    git(
        &s.root,
        &[
            "clone",
            "-q",
            s.origin().to_str().unwrap(),
            evil.to_str().unwrap(),
        ],
    );
    git(
        &s.root,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            evil_origin.to_str().unwrap(),
        ],
    );
    git(
        &evil,
        &["remote", "set-url", "origin", evil_origin.to_str().unwrap()],
    );
    git(&evil, &["fetch", "-q", s.repo().to_str().unwrap(), "a"]);
    s.request(&r, &sha);
    let index_path = s
        .root
        .join("home/runs")
        .join(&s.author)
        .join("changes/index.json");
    let mut index: Value =
        serde_json::from_str(&std::fs::read_to_string(&index_path).unwrap()).unwrap();
    index["changes"][0]["base"] = "other".into();
    index["changes"][0]["repo"] = evil.to_str().unwrap().into();
    std::fs::write(&index_path, index.to_string()).unwrap();
    s.wake();
    let result = s.result(1);
    assert_eq!(result["verdict"], "landed", "{result}");
    assert_eq!(result["judgedOn"]["base"], "main", "{result}");
    assert_eq!(
        result["judgedOn"]["repo"],
        s.repo().to_str().unwrap(),
        "{result}"
    );
    assert_eq!(s.remote("main"), sha);
    assert_eq!(
        s.remote("other"),
        other_before,
        "the forged base got nothing"
    );
    assert!(
        git(&evil_origin, &["for-each-ref"]).is_empty(),
        "the forged repository's remote got nothing"
    );
}

/// The remote's base moved since the lander looked: nothing is forced, the result says so, and the
/// same patch rebased onto the new base lands.
#[test]
fn a_moved_base_is_not_forced_and_the_clean_rebase_lands() {
    let s = Scratch::new();
    let (r, sha) = s.approved("a");
    // Someone else pushes to the remote; this checkout's origin/main doesn't hear of it.
    let elsewhere = s.root.join("elsewhere");
    git(
        &s.root,
        &[
            "clone",
            "-q",
            s.origin().to_str().unwrap(),
            elsewhere.to_str().unwrap(),
        ],
    );
    std::fs::write(elsewhere.join("theirs"), "theirs").unwrap();
    git(&elsewhere, &["add", "."]);
    git(&elsewhere, &["commit", "-q", "-m", "theirs"]);
    git(&elsewhere, &["push", "-q", "origin", "main"]);
    let moved = s.remote("main");
    s.request(&r, &sha);
    s.wake();
    let result = s.result(1);
    assert_eq!(result["verdict"], "base moved", "{result}");
    assert_eq!(result["judgedOn"]["remoteBase"], moved.as_str(), "{result}");
    assert_eq!(s.remote("main"), moved, "nothing forced");
    assert_eq!(s.state_of(1), "landing");
    let due: Value =
        serde_json::from_str(&s.ok(&["state", "change", "due", &s.lander, "--json"])).unwrap();
    assert_eq!(due[0]["todo"], "land again", "{due}");
    assert!(
        due[0]["landing"].as_str().unwrap().contains("base moved"),
        "{due}"
    );

    git(&s.repo(), &["fetch", "-q", "origin"]);
    git(&s.repo(), &["rebase", "-q", "origin/main", "a"]);
    git(&s.repo(), &["checkout", "-q", "main"]);
    let rebased = git(&s.repo(), &["rev-parse", "a"]);
    assert_ne!(rebased, sha);
    s.request(&r, &rebased);
    s.wake();
    let result = s.result(1);
    assert_eq!(result["verdict"], "landed", "{result}");
    assert_eq!(s.remote("main"), rebased);
    assert_eq!(s.state_of(1), "landed");
}

/// Two approved changes on one base in one wake: the second rebased onto the first one's head.
#[test]
fn two_changes_on_one_base_land_in_the_order_asked() {
    let s = Scratch::new();
    let (ra, a) = s.approved("a");
    let (rb, _) = s.approved("b");
    git(&s.repo(), &["rebase", "-q", "a", "b"]);
    git(&s.repo(), &["checkout", "-q", "main"]);
    let b = git(&s.repo(), &["rev-parse", "b"]);
    s.request(&ra, &a);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    s.request(&rb, &b);
    s.wake();
    assert_eq!(
        (
            s.result(1)["verdict"].as_str(),
            s.result(2)["verdict"].as_str()
        ),
        (Some("landed"), Some("landed")),
        "{} {}",
        s.result(1),
        s.result(2)
    );
    assert_eq!(s.remote("main"), b);
    assert_eq!(git(&s.origin(), &["rev-parse", "main~1"]), a);
    assert_eq!(
        (s.state_of(1), s.state_of(2)),
        ("landed".to_string(), "landed".to_string())
    );
}

/// The wake's parent was killed between the request and the step: one poke pass lands it, once.
#[test]
fn a_request_the_parent_never_got_to_is_landed_once_by_poke() {
    let s = Scratch::new();
    let (r, sha) = s.approved("a");
    s.request(&r, &sha);
    s.ok(&["poke"]);
    assert_eq!(s.remote("main"), sha);
    assert_eq!(s.state_of(1), "landed");
    s.ok(&["poke"]);
    assert_eq!(
        s.lander_events("land-result").len(),
        1,
        "judged and pushed once"
    );
    assert_eq!(s.requests_left(), 0);
    assert_eq!(s.remote("main"), sha);
}

/// The step died after its push and before it recorded it: the next pass finds the remote has the
/// commit it was pushing, records the landing and pushes nothing.
#[test]
fn a_push_that_was_never_recorded_is_recorded_and_not_pushed_again() {
    let s = Scratch::new();
    let (r, sha) = s.approved("a");
    s.request(&r, &sha);
    git(
        &s.repo(),
        &["push", "-q", "origin", &format!("{sha}:refs/heads/main")],
    );
    s.ok(&["poke"]);
    let result = s.result(1);
    assert_eq!(result["verdict"], "landed", "{result}");
    assert!(
        result["why"].as_str().unwrap().contains("nothing pushed"),
        "{result}"
    );
    assert_eq!(s.state_of(1), "landed");
}

/// Everything a wake could leave in the repository to run code, or to send the push somewhere
/// else, when otto pushes: none of it is read. Each would leave a marker file or change where the
/// commit went. With the remote on the approval, and with a record from before it was kept.
#[test]
fn nothing_a_wake_planted_in_the_repository_runs_or_redirects_the_push() {
    for record_has_remote in [true, false] {
        let s = Scratch::new();
        let (r, sha) = s.approved("a");
        let repo = s.repo();
        let marker = |name: &str| s.root.join(format!("marker-{name}"));
        let touch = |name: &str| format!("touch '{}'", marker(name).display());
        let hook = |dir: &Path, hook: &str, name: &str| {
            std::fs::create_dir_all(dir).unwrap();
            let path = dir.join(hook);
            std::fs::write(&path, format!("#!/bin/sh\n{}\n", touch(name))).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let evil = s.root.join("evil.git");
        git(
            &s.root,
            &["init", "-q", "--bare", "-b", "main", evil.to_str().unwrap()],
        );
        if !record_has_remote {
            let path = s.root.join("home/approvals").join(&s.author).join("1.json");
            let mut record: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(
                record["remote"],
                s.origin().to_str().unwrap(),
                "approving keeps the remote"
            );
            record.as_object_mut().unwrap().remove("remote");
            std::fs::write(&path, record.to_string()).unwrap();
        }
        s.request(&r, &sha);
        // Planted after the person approved, as a wake would.
        hook(&repo.join(".git/hooks"), "pre-push", "pre-push");
        hook(&repo.join(".git/hooks"), "post-merge", "post-merge");
        hook(
            &repo.join(".git/hooks"),
            "reference-transaction",
            "reference-transaction",
        );
        hook(&s.root.join("hooks"), "pre-push", "hookspath");
        git(
            &repo,
            &[
                "config",
                "core.hooksPath",
                s.root.join("hooks").to_str().unwrap(),
            ],
        );
        git(
            &repo,
            &[
                "config",
                "credential.helper",
                &format!("!{}; true", touch("credential")),
            ],
        );
        git(
            &repo,
            &["config", "remote.origin.pushurl", evil.to_str().unwrap()],
        );
        git(
            &repo,
            &[
                "config",
                &format!("url.{}.insteadOf", evil.display()),
                s.origin().to_str().unwrap(),
            ],
        );
        git(&repo, &["config", "core.sshCommand", &touch("ssh")]);
        git(
            &repo,
            &[
                "config",
                "core.alternateRefsCommand",
                &touch("alternaterefs"),
            ],
        );
        git(&repo, &["config", "core.fsmonitor", &touch("fsmonitor")]);
        git(&repo, &["config", "extensions.worktreeConfig", "true"]);
        hook(&s.root.join("wthooks"), "pre-push", "worktreeconfig");
        std::fs::write(
            repo.join(".git/config.worktree"),
            format!(
                "[core]\n\thooksPath = {}\n",
                s.root.join("wthooks").display()
            ),
        )
        .unwrap();
        // The checkout's idea of the remote's base, forged to say it is already there.
        git(&repo, &["update-ref", "refs/remotes/origin/main", &sha]);

        s.wake();
        let result = s.result(1);
        assert_eq!(
            result["verdict"], "landed",
            "remote on record: {record_has_remote}: {result}"
        );
        assert_eq!(
            s.remote("main"),
            sha,
            "it went to the remote the person approved (remote on record: {record_has_remote})"
        );
        assert!(git(&evil, &["for-each-ref"]).is_empty(), "and nowhere else");
        let expected = if record_has_remote {
            "the approval on record"
        } else {
            "the repository's config"
        };
        assert_eq!(result["judgedOn"]["remoteFrom"], expected, "{result}");
        let left: Vec<String> = std::fs::read_dir(&s.root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("marker-"))
            .collect();
        assert!(
            left.is_empty(),
            "something a wake planted ran (remote on record: {record_has_remote}): {left:?}"
        );
    }
}
