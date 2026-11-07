//! tests/live_witness.rs — prove the witness skill finds planted defects and invents none, against
//! a real model.
//!
//! What `cargo test` cannot reach: whether a wake following `skills/witness/SKILL.md` flags all
//! three defects planted in a fixture change (a branch no test covers, a new test that also passes
//! on base, a summary naming a script that doesn't exist); reports no finding on a clean change;
//! lists a criterion it can't run as not checked; and leaves the subject repo and the real otto
//! home as they were.
//!
//! Two runs at once, ~10 minutes. Set `OTTO_WITNESS_EVIDENCE_DIR` to keep the reports and the
//! before/after comparison.
//!
//!   cargo test --test live_witness -- --ignored --nocapture

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use support::*;

fn timeout() -> Duration {
    let secs = std::env::var("OTTO_LIVE_WITNESS_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(1200);
    Duration::from_secs(secs)
}

fn fixture(rel: &str) -> PathBuf {
    repo_path("test/fixtures/witness").join(rel)
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

fn committed(repo: &Path, message: &str) {
    git_ok(repo, &["add", "-A"]);
    git_ok(repo, &["-c", "user.name=fixture", "-c", "user.email=fixture@example.com", "commit", "-qm", message]);
}

/// The repo: `main` holding the base, and a branch for each change holding its overlay.
fn fixture_repo(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    git_ok(repo, &["init", "-q", "-b", "main"]);
    copy(&fixture("base"), repo);
    committed(repo, "fees");
    for branch in ["planted", "clean"] {
        git_ok(repo, &["checkout", "-qb", branch, "main"]);
        copy(&fixture(branch), repo);
        std::fs::remove_file(repo.join("SUMMARY.md")).unwrap();
        committed(repo, "surcharge on heavy parcels");
    }
    git_ok(repo, &["checkout", "-q", "main"]);
}

/// What must not move: the subject repo's status and refs, and the real otto home's runs.
fn world(repo: &Path, real_runs: &Path) -> String {
    let status = stdout(&git(repo, &["status", "--porcelain", "--branch"]));
    let refs = stdout(&git(repo, &["for-each-ref"]));
    let mut runs: Vec<String> =
        std::fs::read_dir(real_runs).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    runs.sort();
    format!("--- git status\n{status}\n--- git for-each-ref\n{refs}\n--- real runs\n{}\n", runs.join("\n"))
}

fn section<'a>(report: &'a str, heading: &str) -> &'a str {
    let Some((_, rest)) = report.split_once(heading) else { return "" };
    rest.split("\n## ").next().unwrap_or("")
}

fn witness(checks: &mut Checks, id: &str, reference: &str, spec: &Path, workdir: &Path) -> Run {
    let goal = format!("Witness {reference}@v1, one version only. Its spec is {}", spec.display());
    start_run(checks, id, &goal, &["--skill", "witness", "--workdir", workdir.to_str().unwrap(), "--until", "the report for that version is written"])
}

#[test]
#[ignore = "spawns real claude sessions and spends real money — run with --ignored"]
fn live_witness() {
    std::env::remove_var("OTTO_RUN_ID");
    std::env::remove_var("OTTO_RUN_DIR");
    let real_runs = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".otto").join("runs");
    let timeout = timeout();
    let mut checks = Checks::new();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

    checks.step("0 - preconditions");
    require_launcher(&mut checks);
    let python = Command::new("python3").arg("--version").output();
    checks.check(python.is_ok_and(|o| o.status.success()), "python3 is here", "the fixture needs python3");

    checks.step("1 - a fixture repo, and an author's run with both changes handed back");
    let root = std::env::temp_dir().join(format!("otto-witness-{stamp}"));
    let repo = root.join("fees");
    fixture_repo(&repo);
    // A project skill: claude finds it by name in the working directory, so this needs no install
    // in the person's own skills.
    let workdir = root.join("work");
    std::fs::create_dir_all(workdir.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink(repo_path("skills/witness"), workdir.join(".claude/skills/witness")).unwrap();

    let author = format!("witness-author-{stamp}");
    let made = otto(&["state", "init", "--id", &author, "--goal", "fees", "--until", "handed back"]);
    checks.check(made.status.success(), "the author's run exists", String::from_utf8_lossy(&made.stderr));
    for branch in ["planted", "clean"] {
        let summary = fixture(branch).join("SUMMARY.md");
        let given = otto(&["state", "change", "submit", &author, "--repo", repo.to_str().unwrap(), "--branch", branch, "--base", "main", "--title", "Surcharge on heavy parcels", "--summary-file", summary.to_str().unwrap()]);
        checks.check(given.status.success(), format!("{branch} handed back"), String::from_utf8_lossy(&given.stderr));
    }
    let before = world(&repo, &real_runs);

    checks.step(&format!("2 - a witness for each, at once (up to {}s)", timeout.as_secs()));
    let spec = fixture("SPEC.md");
    let planted = witness(&mut checks, &format!("witness-planted-{stamp}"), &format!("{author}/1"), &spec, &workdir);
    let clean = witness(&mut checks, &format!("witness-clean-{stamp}"), &format!("{author}/2"), &spec, &workdir);
    for run in [&planted, &clean] {
        if !await_field(run, "status", "done", timeout) {
            checks.bail(run, "the witness never finished");
        }
    }

    checks.step("3 - the planted change: all three defects, and what it couldn't run");
    let read = |run: &Run, n: u32| std::fs::read_to_string(run.artifact(&format!("reports/{author}-{n}-v1/report.md"))).unwrap_or_default();
    let report = read(&planted, 1);
    println!("{report}");
    let findings = section(&report, "## Findings");
    let flagged = |kind: &str, mark: &str| findings.lines().any(|l| l.contains(kind) && l.contains(mark));
    checks.check(flagged("[surviving fault]", "fees.py"), "the branch no test covers", "no surviving fault in fees.py");
    checks.check(flagged("[passes on base]", "test_zero_weight_is_free"), "the new test that passes on base", "the test passing on base wasn't flagged");
    checks.check(flagged("[claim contradicted]", "check_fees.sh"), "the script that doesn't exist", "the missing script wasn't flagged");
    checks.check(section(&report, "## Not checked").contains("printer"), "the printer is not checked", "the printer wasn't listed as not checked");
    checks.check(findings.lines().all(|l| !l.contains("Repro") || l.contains('`')), "each finding has a reproduction", "a finding has no reproduction");

    checks.step("4 - the clean change: no finding, and still what it couldn't run");
    let quiet = read(&clean, 2);
    println!("{quiet}");
    checks.check(quiet.contains("Findings: none"), "no finding on the clean change", "the clean change has findings");
    checks.check(section(&quiet, "## Not checked").contains("printer"), "the printer is not checked here too", "the printer wasn't listed as not checked");

    checks.step("5 - it left everything alone, and said nothing");
    let after = world(&repo, &real_runs);
    checks.check(before == after, "the subject repo and the real runs are as they were", format!("before:\n{before}\nafter:\n{after}"));
    for run in [&planted, &clean] {
        checks.check(run.events("gate-opened") == 0, format!("{} asked nothing", run.id), "it opened a gate");
        checks.check(!run.dir.join("outputs/index.json").exists(), format!("{} published nothing", run.id), "it published");
    }

    if let Some(dir) = std::env::var_os("OTTO_WITNESS_EVIDENCE_DIR").map(PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("planted-report.md"), &report).unwrap();
        std::fs::write(dir.join("clean-report.md"), &quiet).unwrap();
        std::fs::write(dir.join("before-after.txt"), format!("BEFORE\n{before}\nAFTER\n{after}")).unwrap();
    }
    checks.finish();
}
