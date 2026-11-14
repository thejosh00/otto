//! tests/closed_pipe.rs — a listing piped into a reader that leaves early (`otto feedback --all |
//! head -1`) ends quietly: nothing on stderr, and the status a shell gives a command SIGPIPE ended.
//! Through the real binary and a real pipe, with more to say than a pipe holds, so otto is still
//! writing when the reader goes.

mod support;

use std::io::{BufRead, BufReader, Read};
use std::process::Stdio;

/// More than any pipe's buffer (64KB here, as on Linux), several times over.
const LISTING_AT_LEAST: usize = 256 * 1024;
const REPORTS: usize = 30;

/// `READER_LEFT` in `src/main.rs`: 128 + SIGPIPE.
const READER_LEFT: i32 = 141;

fn file_reports() {
    for n in 0..REPORTS {
        let text = format!(
            "report {n} of a long listing: {}",
            "words to fill a pipe with ".repeat(450)
        );
        let filed = support::otto_command()
            .env_remove("OTTO_RUN_ID")
            .args(["feedback", "add", &text])
            .output()
            .unwrap();
        assert!(
            filed.status.success(),
            "could not file report {n}: {}",
            String::from_utf8_lossy(&filed.stderr)
        );
    }
}

#[test]
fn a_reader_that_leaves_early_ends_the_listing_quietly() {
    file_reports();

    // A reader that stays gets all of it.
    let whole = support::otto(&["feedback", "--all"]);
    assert_eq!(
        whole.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&whole.stderr)
    );
    assert!(
        whole.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&whole.stderr)
    );
    assert!(
        whole.stdout.len() >= LISTING_AT_LEAST,
        "the listing is only {} bytes, too short to fill a pipe",
        whole.stdout.len()
    );
    let listing = String::from_utf8_lossy(&whole.stdout);
    for n in 0..REPORTS {
        assert!(
            listing.contains(&format!("report {n} of a long listing")),
            "report {n} is missing from the whole listing"
        );
    }

    // One that reads a line and closes its end, as `head -1` does.
    let mut child = support::otto_command()
        .args(["feedback", "--all"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    assert!(!first.trim().is_empty(), "no first line");
    drop(reader);
    let mut said = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut said)
        .unwrap();
    let status = child.wait().unwrap();
    assert_eq!(
        said, "",
        "otto said something on stderr when its reader left"
    );
    assert_eq!(status.code(), Some(READER_LEFT), "{status:?}");
}
