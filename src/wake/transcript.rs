//! What a wake spent, read from the session transcript claude leaves behind.
//!
//! This module exists because otto stopped passing `-p`. Print mode bills SDK credits rather
//! than the subscription, so it had to go — but `--output-format json` only works with `-p`, and
//! that result document was where every per-wake number came from: `total_cost_usd`, `num_turns`,
//! `permission_denials`. Dropping one flag dropped the entire accounting story.
//!
//! The replacement is the transcript. `--session-id` *does* work without `-p`, so otto picks the
//! session id itself and afterwards reads
//! `~/.claude/projects/<cwd-slug>/<session-id>.jsonl`. Two things follow from the swap, and
//! both are honest limits rather than bugs:
//!
//! - **There is no dollar cost in a transcript**, and on a subscription there is no per-wake
//!   dollar figure to find. So `budget.usd` stops being enforceable and otto stops reporting a
//!   number it cannot measure, rather than multiplying tokens by a price table and calling an
//!   estimate a measurement. Tokens are real and are what gets journaled.
//! - **A named permission denial is gone.** The result document listed the tool a blocked wake
//!   wanted; nothing in the transcript says "this was denied" in a stable form. What is stable is
//!   `is_error` on a `tool_result` block, so otto counts failed tool calls instead and does not
//!   pretend to know which of them were permission problems.
//!
//! Accounting never fails a wake. A missing or unreadable transcript yields `None` and a journal
//! line; the contract is validated the same either way. That case is expected under yolo, where
//! `~/.claude` is not visible inside the sandbox at all (see `detach`).

use std::io::BufRead;
use std::path::{Path, PathBuf};

/// What one wake used. Tokens rather than dollars, for the reason in the module doc.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Usage {
    /// Assistant messages in the transcript. The nearest thing to `num_turns`.
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Tokens served from the prompt cache, and tokens written to it. Recorded because the
    /// harness is a deliberately byte-stable prefix (see `prompt`) and this is the only evidence
    /// the stability buys anything. A run of wakes where `cache_read` stays zero means the prefix
    /// is being invalidated somewhere and every wake pays a cold start.
    pub cache_read: u64,
    pub cache_creation: u64,
    /// Tool calls that came back flagged as errors. Not the same as the old
    /// `permission_denials` — see the module doc.
    pub tool_errors: u64,
}

/// FNV-1a, for turning a run and wake number into a stable session id. A hash, not a random
/// number: nothing here needs unpredictability, and a derived id can be recomputed from the run
/// directory if `wake.session` is ever lost.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The session id for one wake, as a well-formed v4 UUID — `claude --session-id` insists on
/// valid UUID syntax.
///
/// Derived rather than random so that no dependency is needed for 32 hex nibbles, and stable per
/// `(run, wake, start)`. Reuse would be the thing to avoid, since claude would find an existing
/// session file under that id: `wake_number` only ever increments within a run, and the start
/// timestamp separates two runs that happen to share an id.
pub fn session_id(run_id: &str, wake_n: u32, started_iso: &str) -> String {
    let seed = format!("{run_id}:{wake_n}:{started_iso}");
    let high = fnv1a(seed.as_bytes());
    let low = fnv1a(format!("{seed}:low").as_bytes());
    let mut hex = format!("{high:016x}{low:016x}");
    // Version 4 in nibble 12, and variant 10xx in nibble 16, or claude rejects the argument.
    hex.replace_range(12..13, "4");
    let nibble = u8::from_str_radix(&hex[16..17], 16).unwrap_or(0);
    hex.replace_range(16..17, &format!("{:x}", 0x8 | (nibble & 0x3)));
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Find the transcript for a session id.
///
/// Searched rather than derived. The file sits under a directory named for the wake's working
/// directory with the separators mangled (`/Users/me/w/otto` becomes `-Users-me-w-otto`), and the
/// exact escaping of anything less ordinary is claude's business, not otto's. One readdir over a
/// couple of dozen project directories costs nothing and cannot be wrong about the rule.
pub fn find_transcript(session_id: &str) -> Option<PathBuf> {
    let file = format!("{session_id}.jsonl");
    let projects = std::fs::read_dir(crate::paths::claude_projects_dir()).ok()?;
    for entry in projects.flatten() {
        let candidate = entry.path().join(&file);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Sum what the transcript says this session used.
///
/// Line by line: a transcript is a few hundred KB even for a trivial session, most of it file
/// snapshots otto has no interest in, so nothing is held in memory beyond the current line.
pub fn read_usage(path: &Path) -> std::io::Result<Usage> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut usage = Usage::default();
    for line in reader.lines() {
        let line = line?;
        // A truncated or half-written line is skipped rather than failing the read: the wake has
        // already happened and a partial tail must not cost otto the whole measurement.
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("assistant") => {
                usage.turns += 1;
                let Some(fields) = value.get("message").and_then(|m| m.get("usage")) else {
                    continue;
                };
                let field = |name: &str| fields.get(name).and_then(serde_json::Value::as_u64).unwrap_or(0);
                usage.input_tokens += field("input_tokens");
                usage.output_tokens += field("output_tokens");
                usage.cache_read += field("cache_read_input_tokens");
                usage.cache_creation += field("cache_creation_input_tokens");
            }
            Some("user") => {
                // Tool results come back as user turns; an error is a content block with
                // `is_error` set. This is the API's own shape, so it is the stable thing to count.
                let blocks = value.get("message").and_then(|m| m.get("content")).and_then(serde_json::Value::as_array);
                for block in blocks.into_iter().flatten() {
                    if block.get("is_error").and_then(serde_json::Value::as_bool) == Some(true) {
                        usage.tool_errors += 1;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(usage)
}

/// Usage for a session, or `None` when there is no readable transcript. The caller journals the
/// absence and carries on — see the module doc on why this is never fatal.
pub fn usage_for(session_id: &str) -> Option<Usage> {
    let path = find_transcript(session_id)?;
    read_usage(&path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a transcript for `session_id` under a project directory, the way claude would.
    fn write_transcript(session_id: &str, project: &str, lines: &[&str]) -> PathBuf {
        let dir = crate::paths::claude_projects_dir().join(project);
        std::fs::create_dir_all(&dir).expect("project dir");
        let path = dir.join(format!("{session_id}.jsonl"));
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).expect("write transcript");
        path
    }

    fn assistant(input: u64, output: u64, cache_read: u64, cache_creation: u64) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {"usage": {
                "input_tokens": input, "output_tokens": output,
                "cache_read_input_tokens": cache_read, "cache_creation_input_tokens": cache_creation,
            }},
        })
        .to_string()
    }

    #[test]
    fn a_session_id_is_a_well_formed_v4_uuid() {
        let id = session_id("2026-09-13-thing", 1, "2026-09-13T00:00:00Z");
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12]);
        assert!(id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()), "hex and dashes only: {id}");
        // claude validates the version and variant nibbles, so they are not incidental.
        assert!(parts[2].starts_with('4'), "version nibble must be 4: {id}");
        assert!(
            ['8', '9', 'a', 'b'].contains(&parts[3].chars().next().unwrap()),
            "variant nibble must be 8..b: {id}"
        );
    }

    #[test]
    fn a_session_id_is_stable_per_wake_and_distinct_across_them() {
        let a = session_id("r1", 1, "2026-09-13T00:00:00Z");
        assert_eq!(a, session_id("r1", 1, "2026-09-13T00:00:00Z"), "same wake, same id");
        assert_ne!(a, session_id("r1", 2, "2026-09-13T00:00:00Z"), "next wake needs its own session");
        assert_ne!(a, session_id("r2", 1, "2026-09-13T00:00:00Z"), "another run must not collide");
        assert_ne!(a, session_id("r1", 1, "2026-09-13T01:00:00Z"), "a later start is a later session");
    }

    #[test]
    fn usage_is_summed_across_assistant_messages() {
        let _home = crate::paths::test_support::TempHome::new();
        let id = "11111111-1111-4111-8111-111111111111";
        write_transcript(
            id,
            "-Users-me-w-otto",
            &[&assistant(100, 10, 900, 80), &assistant(200, 20, 1000, 0)],
        );
        let usage = usage_for(id).expect("transcript is there");
        assert_eq!(usage.turns, 2);
        assert_eq!(usage.input_tokens, 300);
        assert_eq!(usage.output_tokens, 30);
        assert_eq!(usage.cache_read, 1900);
        assert_eq!(usage.cache_creation, 80);
    }

    /// The whole point of counting these separately: a wake can look busy and be failing every
    /// tool call it makes.
    #[test]
    fn error_tool_results_are_counted_and_clean_ones_are_not() {
        let _home = crate::paths::test_support::TempHome::new();
        let id = "22222222-2222-4222-8222-222222222222";
        let failed = serde_json::json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": "nope"}]},
        })
        .to_string();
        let fine = serde_json::json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "t2", "content": "ok"}]},
        })
        .to_string();
        write_transcript(id, "-w", &[&assistant(1, 1, 0, 0), &failed, &fine]);
        let usage = usage_for(id).expect("transcript is there");
        assert_eq!(usage.tool_errors, 1);
        assert_eq!(usage.turns, 1);
    }

    /// Non-usage records are the bulk of a real transcript, and none of them may be counted.
    #[test]
    fn records_that_are_not_usage_are_ignored() {
        let _home = crate::paths::test_support::TempHome::new();
        let id = "33333333-3333-4333-8333-333333333333";
        write_transcript(
            id,
            "-w",
            &[
                r#"{"type":"file-history-snapshot","snapshot":{"big":"blob"}}"#,
                r#"{"type":"attachment","attachment":{}}"#,
                r#"{"type":"last-prompt","lastPrompt":"x"}"#,
                &assistant(5, 5, 0, 0),
            ],
        );
        let usage = usage_for(id).expect("transcript is there");
        assert_eq!(usage.turns, 1);
        assert_eq!(usage.input_tokens, 5);
    }

    /// A half-written trailing line must not cost the whole measurement.
    #[test]
    fn a_truncated_line_is_skipped_not_fatal() {
        let _home = crate::paths::test_support::TempHome::new();
        let id = "44444444-4444-4444-8444-444444444444";
        write_transcript(id, "-w", &[&assistant(7, 3, 0, 0), r#"{"type":"assist"#]);
        let usage = usage_for(id).expect("still readable");
        assert_eq!(usage.turns, 1);
        assert_eq!(usage.input_tokens, 7);
    }

    /// Expected under yolo, where the sandbox cannot see `~/.claude` — and never fatal.
    #[test]
    fn a_missing_transcript_is_none_rather_than_an_error() {
        let _home = crate::paths::test_support::TempHome::new();
        assert!(usage_for("55555555-5555-4555-8555-555555555555").is_none());
    }

    #[test]
    fn the_transcript_is_found_whatever_the_project_directory_is_called() {
        let _home = crate::paths::test_support::TempHome::new();
        let id = "66666666-6666-4666-8666-666666666666";
        write_transcript("unrelated-session", "-Users-me-w-other", &[&assistant(1, 1, 0, 0)]);
        let expected = write_transcript(id, "-Users-me-w-deeply-nested-thing", &[&assistant(1, 1, 0, 0)]);
        assert_eq!(find_transcript(id), Some(expected));
    }
}
