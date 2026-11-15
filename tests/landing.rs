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

    /// The commits pinned for the step: `refs/otto/land/*` in the repository.
    fn pins(&self) -> String {
        git(
            &self.repo(),
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/otto/land",
            ],
        )
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

    // Asked again for the same commit, it is judged again, not answered from the last result.
    s.request(&r, &sha);
    s.wake();
    assert_eq!(s.lander_events("land-result").len(), 2);

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
    // The lander's own check: the second, rebased onto the first, isn't the approved patch against
    // a base that doesn't have the first yet; once the first is asked for, it is.
    let check = |r: &str, head: &str| s.otto(&["state", "change", "check", &s.lander, r, "--head", head]);
    assert!(!check(&rb, &b).status.success(), "before the first is asked for");
    s.request(&ra, &a);
    let out = check(&rb, &b);
    assert!(
        out.status.success(),
        "on the first one's requested head: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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

/// A request is there and no wake's parent ran the step for it: one poke pass lands it, once.
/// Nothing is killed here; `a_parent_killed_after_the_request_leaves_it_for_poke_to_land_once` does that.
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

/// A commit already on the remote's base is a landing only when it is the approved commit or one
/// this step was pushing when it died: a rebased commit somebody else put there is not marked
/// landed on the lander's word, and the one the step was pushing is recorded without a second push.
#[test]
fn a_rebased_commit_already_on_the_base_counts_only_if_the_step_was_pushing_it() {
    for step_was_pushing in [false, true] {
        let s = Scratch::new();
        let (r, sha) = s.approved("a");
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
        git(&s.repo(), &["fetch", "-q", "origin"]);
        git(&s.repo(), &["rebase", "-q", "origin/main", "a"]);
        git(&s.repo(), &["checkout", "-q", "main"]);
        let rebased = git(&s.repo(), &["rev-parse", "a"]);
        git(
            &s.repo(),
            &[
                "push",
                "-q",
                "origin",
                &format!("{rebased}:refs/heads/main"),
            ],
        );
        if step_was_pushing {
            let dir = s.root.join("home/landings").join(&s.author);
            std::fs::create_dir_all(&dir).unwrap();
            let pushing = serde_json::json!({"run": s.author, "n": 1, "head": rebased, "verdict": "pushing", "judgedOn": {"approvedSha": sha}, "at": "2026-01-01T00:00:00Z"});
            std::fs::write(dir.join("1.json"), pushing.to_string()).unwrap();
        }
        s.request(&r, &rebased);
        s.wake();
        let result = s.result(1);
        if step_was_pushing {
            assert_eq!(result["verdict"], "landed", "{result}");
            assert!(
                result["why"].as_str().unwrap().contains("nothing pushed"),
                "{result}"
            );
            assert_eq!(s.state_of(1), "landed");
        } else {
            assert_eq!(result["verdict"], "refused", "{result}");
            assert!(
                result["why"].as_str().unwrap().contains("already contains"),
                "{result}"
            );
            assert_eq!(s.state_of(1), "landing");
        }
        assert_eq!(s.remote("main"), rebased);
    }
}

/// The step failing is its own failure: the wake that asked still completed, and nothing was pushed.
#[test]
fn a_step_that_cannot_run_does_not_fail_the_wake() {
    let s = Scratch::new();
    let (r, sha) = s.approved("a");
    let before = s.remote("main");
    // Where the step keeps its results is a file: it can make no scratch repository and record nothing.
    std::fs::write(s.root.join("home/landings"), "in the way").unwrap();
    s.request(&r, &sha);
    let completed = s.lander_events("wake-complete").len();
    s.wake();
    assert_eq!(
        s.lander_events("wake-complete").len(),
        completed + 1,
        "the wake completed"
    );
    assert_eq!(
        s.remote("main"),
        before,
        "nothing pushed without a record of it"
    );
    assert_eq!(s.state_of(1), "landing");
    assert_eq!(
        s.requests_left(),
        1,
        "the request waits for a step that can run"
    );
}

/// The commit is pinned by a ref from the asking until the step's answer is final: gone once it
/// landed, gone once it was refused for good, and still there while the request is kept or the
/// lander is to ask again.
#[test]
fn the_pin_is_removed_once_the_result_is_final() {
    let s = Scratch::new();
    let (r, sha) = s.approved("a");
    s.request(&r, &sha);
    assert_eq!(s.pins(), format!("refs/otto/land/{}-1 {sha}", s.author));
    s.wake();
    assert_eq!(s.result(1)["verdict"], "landed");
    assert_eq!(s.pins(), "", "landed: the pin is gone");

    // Refused for good, with no approval to say which repository: gone too.
    let (unapproved, usha) = s.submit("u");
    s.request(&unapproved, &usha);
    assert_eq!(s.pins(), format!("refs/otto/land/{}-2 {usha}", s.author));
    s.wake();
    assert_eq!(s.result(2)["verdict"], "refused");
    assert_eq!(s.pins(), "", "refused: the pin is gone");

    // Refused for good with an approval: another patch.
    let (other, _) = s.approved("o");
    let wrong = s.commit("o", "second");
    s.request(&other, &wrong);
    s.wake();
    assert_eq!(s.result(3)["verdict"], "refused");
    assert_eq!(s.pins(), "", "refused: the pin is gone");

    // The base moved: the lander is to rebase and ask again, and the pin stays until then.
    let (m, msha) = s.approved("m");
    let moved = s.commit("elsewhere", "first");
    git(
        &s.repo(),
        &["push", "-q", "origin", &format!("{moved}:refs/heads/main")],
    );
    s.request(&m, &msha);
    s.wake();
    assert_eq!(s.result(4)["verdict"], "base moved");
    assert_eq!(s.pins(), format!("refs/otto/land/{}-4 {msha}", s.author));
}

/// A result that couldn't be recorded isn't final: the request is kept, and so is the pin.
#[test]
fn the_pin_is_kept_while_the_request_is() {
    let s = Scratch::new();
    let (r, sha) = s.approved("a");
    s.request(&r, &sha);
    std::fs::write(s.root.join("home/landings"), "in the way\n").unwrap();
    s.wake();
    assert_eq!(s.requests_left(), 1);
    assert_eq!(s.pins(), format!("refs/otto/land/{}-1 {sha}", s.author));
}

/// The wake's parent really is killed, by the lander's own stub, after the request is written and
/// before any step can run: nothing is judged or pushed by the dead parent, one poke pass lands
/// it, and a second does nothing.
#[test]
fn a_parent_killed_after_the_request_leaves_it_for_poke_to_land_once() {
    use std::os::unix::process::ExitStatusExt;
    let s = Scratch::new();
    let (r, sha) = s.approved("a");
    let before = s.remote("main");
    let stub = s.root.join("stub");
    let (once, killed) = (s.root.join("asked"), s.root.join("killed"));
    let handoff = std::fs::read_to_string(&stub).unwrap();
    // The first time: ask, then find the `otto` this wake belongs to and kill it outright.
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\nif [ ! -e '{once}' ]; then\n  : > '{once}'\n  '{OTTO}' state change request-land \"$OTTO_RUN_ID\" {r} --head {sha} >/dev/null || exit 1\n  p=$PPID\n  while [ \"$p\" -gt 1 ]; do\n    c=$(ps -o comm= -p \"$p\")\n    case \"$c\" in */otto) kill -9 \"$p\" && echo \"$c\" > '{killed}'; break;; esac\n    p=$(ps -o ppid= -p \"$p\" | tr -d ' ')\n  done\n  exit 0\nfi\n{}",
            handoff.trim_start_matches("#!/bin/sh\n"),
            once = once.display(),
            killed = killed.display(),
        ),
    )
    .unwrap();
    let out = s.otto(&["wake", &s.lander]);
    assert_eq!(
        out.status.signal(),
        Some(9),
        "the wake's parent was killed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(killed.exists(), "the stub found its otto and killed it");
    assert_eq!(s.requests_left(), 1, "the request outlived the parent");
    assert_eq!(s.lander_events("land-requested").len(), 1);
    assert_eq!(
        s.lander_events("land-result").len(),
        0,
        "the parent judged nothing"
    );
    assert!(!s.root.join("home/landings").join(&s.author).exists());
    assert_eq!(s.remote("main"), before, "and pushed nothing");

    s.ok(&["poke"]);
    assert_eq!(s.remote("main"), sha, "one poke pass landed it");
    assert_eq!(s.state_of(1), "landed");
    assert_eq!(s.result(1)["verdict"], "landed");
    assert_eq!(s.lander_events("land-result").len(), 1);
    assert_eq!(s.requests_left(), 0);
    s.ok(&["poke"]);
    assert_eq!(
        s.lander_events("land-result").len(),
        1,
        "a second pass does nothing"
    );
    assert_eq!(s.lander_events("land-requested").len(), 1);
    assert_eq!(s.remote("main"), sha);
}

impl Scratch {
    /// As `approved`, with the repository's origin — and so the remote on the approval — at `url`.
    fn approved_at(&self, branch: &str, url: &str) -> (String, String) {
        let (r, sha) = self.submit(branch);
        git(&self.repo(), &["remote", "set-url", "origin", url]);
        self.ok(&["changes", "approve", &format!("{r}@v1"), "--no-wake"]);
        self.ok(&["state", "change", "claim", &self.lander, &r]);
        (r, sha)
    }

    /// A script that leaves `marker-<name>` when anything runs it, and fails.
    fn marker_script(&self, name: &str) -> (PathBuf, PathBuf) {
        let (script, marker) = (
            self.root.join(format!("run-{name}")),
            self.root.join(format!("marker-{name}")),
        );
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, marker)
    }
}

/// The remote on the approval is reached over ssh, where `core.sshCommand` in the repository's
/// config and `GIT_SSH_COMMAND` in the environment would each be run as the ssh: the step runs
/// neither. Nothing listens at the address, so it fails to reach the remote and says so.
#[test]
fn the_repositorys_ssh_command_is_not_run_for_an_ssh_remote() {
    let s = Scratch::new();
    let (r, sha) = s.approved_at("a", "ssh://git@127.0.0.1:1/nowhere.git");
    let (from_config, config_marker) = s.marker_script("ssh-config");
    let (from_env, env_marker) = s.marker_script("ssh-env");
    git(
        &s.repo(),
        &["config", "core.sshCommand", from_config.to_str().unwrap()],
    );
    s.request(&r, &sha);
    let out = support::otto_command()
        .args(["poke"])
        .current_dir(s.repo())
        .env("OTTO_HOME", s.root.join("home"))
        .env("HOME", s.root.join("userhome"))
        .env("TMUX_TMPDIR", &s.root)
        .env("OTTO_BIN", OTTO)
        .env("GIT_SSH_COMMAND", &from_env)
        .env("GIT_SSH", &from_env)
        .env_remove("TMUX")
        .env_remove("OTTO_RUN_ID")
        .env_remove("OTTO_RUN_DIR")
        .env_remove("OTTO_CHAT")
        .output()
        .unwrap();
    assert!(out.status.success());
    let result = s.result(1);
    assert_eq!(result["verdict"], "remote refused", "{result}");
    assert_eq!(
        result["judgedOn"]["remote"], "ssh://git@127.0.0.1:1/nowhere.git",
        "{result}"
    );
    assert!(
        result["why"].as_str().unwrap().contains("couldn't fetch"),
        "{result}"
    );
    assert!(
        !config_marker.exists(),
        "core.sshCommand from the repository's config was run"
    );
    assert!(
        !env_marker.exists(),
        "GIT_SSH_COMMAND from the environment was run"
    );
}

/// The remote on the approval is reached over http and asks who is there, which is when git runs
/// a `credential.helper`: the one in the repository's config is not run.
#[test]
fn the_repositorys_credential_helper_is_not_run_for_an_http_remote() {
    use std::io::{Read as _, Write as _};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut seen = Vec::new();
            let mut buf = [0u8; 1024];
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(n) if n > 0 => seen.extend_from_slice(&buf[..n]),
                    _ => break,
                }
            }
            count.fetch_add(1, Ordering::SeqCst);
            let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"scratch\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    let s = Scratch::new();
    let url = format!("http://127.0.0.1:{port}/nowhere.git");
    let (r, sha) = s.approved_at("a", &url);
    let (helper, marker) = s.marker_script("credential");
    git(
        &s.repo(),
        &["config", "credential.helper", helper.to_str().unwrap()],
    );
    s.request(&r, &sha);
    s.ok(&["poke"]);
    let result = s.result(1);
    assert_eq!(result["verdict"], "remote refused", "{result}");
    assert_eq!(result["judgedOn"]["remote"], url.as_str(), "{result}");
    assert!(
        asked.load(Ordering::SeqCst) > 0,
        "the remote was asked, and wanted to know who: {result}"
    );
    assert!(
        !marker.exists(),
        "credential.helper from the repository's config was run"
    );
}

/// The lander can't say a change landed: `landed` is refused for one that lands by a push, and the
/// refusal names what to do instead. Nothing is marked, even with the commit already on the base.
#[test]
fn the_lander_saying_it_landed_is_refused_and_told_to_ask() {
    let s = Scratch::new();
    let (r, sha) = s.approved("a");
    git(&s.repo(), &["push", "-q", "origin", &format!("{sha}:main")]);
    git(&s.repo(), &["fetch", "-q", "origin"]);
    let out = s.otto(&["state", "change", "landed", &s.lander, &r, "--sha", &sha]);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "refused");
    assert!(err.contains("request-land"), "{err}");
    assert_eq!(s.state_of(1), "landing");
    // What claim prints carries the remote to fetch from.
    let due = s.ok(&["state", "change", "due", &s.lander, "--json"]);
    assert!(due.contains("\"remote\""), "{due}");
}

// ---- Installing after a landing on the otto repository (src/landing/install.rs) ----

/// A stand-in for install.sh: it says what it was given, and on request does what `otto agent
/// start` does to the job that started it (kills its process group), fails, or dies.
const STAND_IN: &str = r#"#!/bin/sh
root="$OTTO_HOME/.."
{
	echo "args: $*"
	echo "target: ${CARGO_TARGET_DIR-unset}"
	echo "cwd: $(pwd)"
	echo "head: $(git rev-parse HEAD)"
	echo "hooks: $(ls .git/hooks 2>/dev/null | grep -v sample | tr '\n' ' ')"
} >"$root/install-ran"
if [ -f "$root/want-kill" ]; then
	i=0
	while [ ! -s "$root/kill-pgid" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
	kill -9 -- -"$(cat "$root/kill-pgid")" 2>/dev/null
	sleep 0.3
fi
if [ -f "$root/want-die" ]; then
	touch "$root/install-done"
	kill -9 $PPID $$
fi
[ -f "$root/want-fail" ] && { echo "the build broke"; touch "$root/install-done"; exit 3; }
touch "$root/install-done"
"#;

impl Scratch {
    /// Make the scratch repository look like the otto checkout: the marker files and a stand-in
    /// install.sh on main. `linked`: the skills link points at it. `built`: its history holds the
    /// commit the otto under test was built from.
    fn otto_repo(linked: bool, built: bool) -> Scratch {
        let s = Scratch::new();
        let repo = s.repo();
        std::fs::create_dir_all(repo.join("harness")).unwrap();
        std::fs::write(repo.join("harness/wake.md"), "harness").unwrap();
        std::fs::create_dir_all(repo.join("skills/manage-pr")).unwrap();
        std::fs::write(repo.join("skills/manage-pr/SKILL.md"), "skill").unwrap();
        std::fs::write(repo.join("install.sh"), STAND_IN).unwrap();
        std::fs::set_permissions(repo.join("install.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "otto"]);
        git(&repo, &["push", "-q", "origin", "main"]);
        if built {
            let version = s.ok(&["--version"]);
            let commit = version.split('(').nth(1).and_then(|v| v.split(')').next()).unwrap().trim_end_matches("-dirty").to_string();
            git(&repo, &["fetch", "-q", env!("CARGO_MANIFEST_DIR"), "HEAD"]);
            git(&repo, &["branch", "built", &commit]);
        }
        let skills = s.root.join("userhome/.claude/skills");
        std::fs::create_dir_all(&skills).unwrap();
        let target = if linked { repo.join("skills/manage-pr") } else { s.root.join("elsewhere/skills/manage-pr") };
        std::os::unix::fs::symlink(target, skills.join("manage-pr")).unwrap();
        s
    }

    /// otto in a process group of its own, as a launchd job is, with CARGO_TARGET_DIR set; the
    /// group is written where the stand-in install looks for one to kill.
    fn otto_as_a_job(&self, args: &[&str]) {
        use std::os::unix::process::CommandExt;
        let mut child = support::otto_command()
            .args(args)
            .current_dir(self.repo())
            .env("OTTO_HOME", self.root.join("home"))
            .env("HOME", self.root.join("userhome"))
            .env("TMUX_TMPDIR", &self.root)
            .env("OTTO_BIN", OTTO)
            .env("CARGO_TARGET_DIR", self.root.join("shared-target"))
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT")
            .process_group(0)
            .spawn()
            .unwrap();
        std::fs::write(self.root.join("kill-pgid"), child.id().to_string()).unwrap();
        let _ = child.wait();
    }

    fn flag(&self, name: &str) {
        std::fs::write(self.root.join(name), "").unwrap();
    }

    fn wait_for(&self, name: &str) {
        for _ in 0..300 {
            if self.root.join(name).exists() {
                // The wrapper writes the status just after the script ends.
                std::thread::sleep(std::time::Duration::from_millis(300));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("{name} never appeared");
    }

    fn landed_note(&self, n: u64) -> String {
        self.changes().iter().find(|c| c["n"] == n).unwrap()["landed"]["note"].as_str().unwrap_or("").to_string()
    }

    /// Everything under the lander's run, as one text: where a message to the person would be.
    fn lander_files(&self) -> String {
        fn walk(dir: &Path, out: &mut String) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push_str(&text);
                }
            }
        }
        let mut out = String::new();
        walk(&self.root.join("home/runs").join(&self.lander), &mut out);
        out
    }
}

/// From the wake's parent: the landing is installed from a clean clone of the landed commit, the
/// install outlives the death of the job that started it, the result says pending meanwhile, and
/// the author is told only once poke has seen it finish.
#[test]
fn a_landing_on_the_otto_repository_is_installed_and_survives_the_restart_of_the_wakes_parent() {
    let s = Scratch::otto_repo(true, true);
    let (r, sha) = s.approved("a");
    s.request(&r, &sha);
    s.flag("want-kill");
    s.otto_as_a_job(&["wake", &s.lander]);
    assert_eq!(s.remote("main"), sha);
    let result = s.result(1);
    assert_eq!(result["verdict"], "landed", "{result}");
    assert_eq!(result["install"]["state"], "pending", "{result}");
    assert_eq!(s.state_of(1), "landing", "the author isn't told before the install is over");
    s.wait_for("install-done");
    let out = s.ok(&["poke"]);
    assert!(out.contains("installed"), "{out}");
    let result = s.result(1);
    assert_eq!(result["install"]["state"], "installed", "{result}");
    assert_eq!(s.state_of(1), "landed");
    assert!(s.landed_note(1).contains("otto installed this commit"), "{}", s.landed_note(1));
    let ran = std::fs::read_to_string(s.root.join("install-ran")).unwrap();
    assert!(ran.contains(&format!("args: --skills-repo {}", s.repo().display())), "{ran}");
    assert!(ran.contains("target: unset"), "{ran}");
    assert!(ran.contains(&format!("head: {sha}")), "{ran}");
    assert!(ran.contains("hooks: \n") || ran.trim_end().ends_with("hooks:"), "{ran}");
    let cwd = ran.lines().find_map(|l| l.strip_prefix("cwd: ")).unwrap();
    assert_ne!(Path::new(cwd), s.repo(), "installed from a clone, not the main checkout");
    assert!(!Path::new(cwd).exists(), "the clone is gone afterwards");
    s.ok(&["poke"]);
    assert_eq!(s.lander_events("land-install").len(), 1, "finished once");
}

/// From poke: the same, when the pass that lands it is the reviver's and the install restarts it.
#[test]
fn a_landing_poke_makes_is_installed_and_survives_the_restart_of_the_reviver() {
    let s = Scratch::otto_repo(true, true);
    let (r, sha) = s.approved("a");
    s.request(&r, &sha);
    s.flag("want-kill");
    s.otto_as_a_job(&["poke"]);
    assert_eq!(s.remote("main"), sha);
    assert_eq!(s.result(1)["install"]["state"], "pending");
    s.wait_for("install-done");
    s.ok(&["poke"]);
    assert_eq!(s.result(1)["install"]["state"], "installed");
    assert_eq!(s.state_of(1), "landed");
}

/// A repository that only looks like otto's — the name's files, an install.sh — installs nothing:
/// neither one the skills aren't linked to, nor one whose history lacks the installed build's commit.
#[test]
fn a_repository_that_only_looks_like_ottos_installs_nothing() {
    for (linked, built) in [(false, true), (true, false)] {
        let s = Scratch::otto_repo(linked, built);
        let (r, sha) = s.approved("a");
        s.request(&r, &sha);
        s.wake();
        assert_eq!(s.state_of(1), "landed", "linked {linked} built {built}");
        let result = s.result(1);
        assert!(result["install"].is_null(), "{result}");
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!s.root.join("install-ran").exists(), "linked {linked} built {built}");
    }
}

/// The install fails: the landing stands, the person is told, and the author's note says so.
#[test]
fn a_failed_install_leaves_the_landing_and_tells_the_person_and_the_author() {
    let s = Scratch::otto_repo(true, true);
    let (r, sha) = s.approved("a");
    s.request(&r, &sha);
    s.flag("want-fail");
    s.wake();
    s.wait_for("install-done");
    let out = s.ok(&["poke"]);
    assert!(out.contains("failed"), "{out}");
    assert_eq!(s.remote("main"), sha, "the landing stands");
    assert_eq!(s.state_of(1), "landed");
    let result = s.result(1);
    assert_eq!(result["install"]["state"], "failed", "{result}");
    assert!(result["install"]["why"].as_str().unwrap().contains("exited 3: the build broke"), "{result}");
    assert!(s.landed_note(1).contains("installing this commit failed"), "{}", s.landed_note(1));
    assert!(s.lander_files().contains("Installing otto after a landing failed"), "the person has a message");
}

/// The install died without saying how it went: poke reports it as failed rather than waiting for ever.
#[test]
fn an_install_that_died_is_reported_by_poke() {
    let s = Scratch::otto_repo(true, true);
    let (r, sha) = s.approved("a");
    s.request(&r, &sha);
    s.flag("want-die");
    s.wake();
    s.wait_for("install-done");
    let out = s.ok(&["poke"]);
    assert!(out.contains("died"), "{out}");
    assert_eq!(s.state_of(1), "landed");
    assert!(s.landed_note(1).contains("died before it finished"), "{}", s.landed_note(1));
    assert!(s.lander_files().contains("Installing otto after a landing failed"));
}
