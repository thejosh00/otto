//! tests/migration_check.rs — `scripts/migration-check.sh`, through its own test
//! (`test/migration-check.sh`), so `cargo test` covers it. That test builds a fake otto home in a
//! temp directory and never reads or writes the real one.

use std::path::Path;
use std::process::Command;

#[test]
fn migration_check_records_and_compares_what_must_survive() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("bash")
        .arg(repo.join("test/migration-check.sh"))
        .current_dir(repo)
        .env_remove("OTTO_HOME")
        .output()
        .expect("run test/migration-check.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "test/migration-check.sh failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("all migration-check expectations hold"), "{stdout}");
}
