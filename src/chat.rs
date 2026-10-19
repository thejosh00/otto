//! `otto chat` — asking a run about its work, in a conversation.
//!
//! A gate is the run asking; a note is the person telling. Neither answers a question the person
//! has — why it picked that eval, what is actually left, what it found in the logs — so before
//! this, the answer was the handoff and the journal read by hand, or a note and the hope that the
//! next handoff mentioned it. A chat is an interactive claude session, started in the run's
//! working directory under the run's own launcher, that answers from the record: everything in the
//! run directory, the world the run works on, and each wake's own transcript, which is the one
//! place that says *why* a wake did something rather than what it wrote down.
//!
//! **A chat is not a wake.** It has no wake number, holds no wake lock, is not validated at exit,
//! counts toward no budget and is no tick for the stall count, and so it can sit beside a wake that
//! is working. "The session holds the conversation, the run directory holds the truth" (DESIGN.md
//! §3) applies to it exactly: it is a session, the person holds it, and when it ends it is gone.
//!
//! What it must not become is a wake nobody is checking — "just fix it" acted on with no repo lock,
//! no handoff, no authorization, and a next wake that cannot see why the world moved. So it is held
//! to reading three ways, each a backstop for the one before: `harness/chat.md` says so; Edit and
//! Write are refused outright and every other tool asks first (`launcher::chat_argv`); and
//! `CHAT_ENV` makes `otto state` refuse its writes (`refuse_state_write`). Shell redirection gets
//! past the second and `env -u` past the third — they stop a model that forgot, not one set on it,
//! and the person approving each command is the rest.
//!
//! What the person decides leaves as a note, through `otto note`, which is the only way a chat
//! reaches the run. The journal says a chat happened (`chat-started`, `chat-ended`, with what it
//! used and the notes left while it was open); what was said in it stays in its transcript.

use crate::clock::Timestamp;
use crate::error::OttoError;
use crate::event::Event;
use crate::liveness::{Liveness, LockLiveness};
use crate::state::RunState;
use crate::wake::{launcher, transcript};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

/// What a chat session is told it is. Embedded, like the wake's harness (`wake::prompt`).
pub const HARNESS: &str = include_str!("../harness/chat.md");

/// Set, to the run's id, in a chat's environment and nowhere else: what makes `otto state` refuse
/// a chat's writes. The env guard clears it for a wake (`launcher::UNINHERITED`), so a wake started
/// from inside a chat — `otto note --now` — writes as any wake does.
pub const CHAT_ENV: &str = "OTTO_CHAT";

/// Tools a chat is refused whatever its permission mode: the ones that change files. The work is
/// the wakes' to do.
pub const REFUSED_TOOLS: &[&str] = &["Edit", "Write", "NotebookEdit"];

/// The otto commands a chat runs without asking: the ones that only read the run. With no
/// terminal nobody can answer a prompt, and `live_chat` found what that meant — even `otto show`
/// refused, and the chat reading `run.json` by hand. With one, asking a person to approve every
/// look is noise beside the one approval that matters, the note's.
pub const READS: &[&str] = &[
    "Bash(otto show *)",
    "Bash(otto logs *)",
    "Bash(otto inbox read *)",
    "Bash(otto note * --list)",
    "Bash(otto state get *)",
    "Bash(otto state tail *)",
];

/// With no terminal, `otto note` runs without asking too: the question is the only one there to ask
/// for a note, and the harness leaves one only when it does. With a terminal, approving it is how
/// the person agrees to its wording.
pub const UNATTENDED_NOTE: &str = "Bash(otto note *)";

/// How many wakes' transcripts a chat's opening message lists, newest first. The rest are named
/// by the journal, and the message says where.
const TRANSCRIPTS_LISTED: usize = 10;

#[derive(clap::Args, Debug)]
pub struct ChatArgs {
    /// The run: its number or name (a unique prefix of the name will do)
    pub id: String,
    /// What to ask first. Without one, the chat opens by saying where the run stands. With no
    /// terminal — from a script, or into a file — it is answered once, and the chat ends
    pub question: Option<String>,
    /// Print the command the chat would run, and start nothing
    #[arg(long)]
    pub dry_run: bool,
}

pub fn chat(args: ChatArgs) -> Result<(), OttoError> {
    let id = crate::paths::resolve_run_id(&args.id)?;
    let state = crate::state::read_run(&id)?;
    let dir = crate::paths::run_dir(&id)?;
    // A conversation needs a terminal both ways; anything less — a script, `> answer.md` — is one
    // question, answered once.
    let attended = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if !attended && args.question.is_none() && !args.dry_run {
        return Err(OttoError::usage(
            "there is no terminal to chat in — give it a question, `otto chat <run> \"…\"`, and it is answered once",
        ));
    }
    let launcher_def = launcher::resolve(&state)?;
    let started = Timestamp::now();
    let session = transcript::chat_session_id(&id, &started.to_string());
    let opening = Opening {
        state: &state,
        dir: &dir,
        short: &crate::paths::short_id(&id),
        working: LockLiveness.probe(&id).is_busy(),
        transcripts: wake_transcripts(&dir, &state),
        question: args.question.as_deref(),
        attended,
    };
    let argv = launcher::chat_argv(&state, &launcher_def, &opening.text(), &session, attended);
    if args.dry_run {
        println!("{}", crate::wake::dry_run_line(&argv));
        return Ok(());
    }

    crate::state::log_event(&id, Event::ChatStarted { session: session.clone(), launcher: launcher_def.name.clone() })?;
    if attended {
        eprintln!(
            "chatting about {} — it reads the run and changes nothing; what you decide, it leaves as a note. /exit to end\n",
            crate::paths::label_of(&id)
        );
    }
    let ran = run_chat(&argv, crate::config::workdir_for(&state).as_deref(), attended);
    // The session is over whichever way it ended, so its end is journaled before any error is.
    let used = transcript::usage_for(&session);
    let notes = notes_left_since(&dir, started);
    crate::state::log_event(
        &id,
        Event::ChatEnded {
            session,
            turns: used.as_ref().map(|u| u.turns),
            input_tokens: used.as_ref().map(|u| u.input_tokens),
            output_tokens: used.as_ref().map(|u| u.output_tokens),
            cache_read: used.as_ref().map(|u| u.cache_read),
            cache_creation: used.as_ref().map(|u| u.cache_creation),
            notes: notes.clone(),
        },
    )?;
    ran?;
    if attended {
        eprintln!("\n{}", ended_line(&crate::paths::short_id(&id), &notes, &state));
    }
    Ok(())
}

/// The line after a chat: what of it reached the run.
fn ended_line(short: &str, notes: &[String], state: &RunState) -> String {
    if !notes.is_empty() {
        let which = notes.join(", ");
        return format!("chat ended — note(s) {which} left for the run; `otto note {short} --list` to see what is pending");
    }
    if state.status.is_terminal() {
        return "chat ended — nothing in it reached the run".to_string();
    }
    format!("chat ended — nothing in it reached the run; `otto note {short} \"…\"` to tell it something")
}

/// Run the chat, and wait for it. Attended, it has this terminal. Not, it runs exactly as a wake
/// does — stdin at `/dev/null`, stdout on a pipe, which is what makes claude answer its prompt and
/// exit (`exec`) — and what it says is copied through to this stdout as it comes.
///
/// Ctrl-C is the chat's: the terminal sends SIGINT to its whole foreground group, otto included,
/// and otto dying there would leave a chat with no end in the journal. So otto ignores it until the
/// chat exits — from after the spawn, so the chat itself does not inherit the ignore.
fn run_chat(argv: &[String], workdir: Option<&Path>, attended: bool) -> Result<(), OttoError> {
    let mut command = std::process::Command::new(&argv[0]);
    command.args(&argv[1..]);
    if let Some(dir) = workdir {
        command.current_dir(dir);
    }
    if !attended {
        command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped());
    }
    let mut child = command.spawn().map_err(|e| OttoError::usage(format!("could not start {}: {e}", argv[0])))?;
    let _quiet = IgnoreInterrupt::new();
    if let Some(mut said) = child.stdout.take() {
        std::io::copy(&mut said, &mut std::io::stdout())?;
    }
    let status = child.wait()?;
    match status.code() {
        Some(0) | None => Ok(()),
        Some(code) => Err(OttoError::new(format!("the chat exited with code {code}"), code)),
    }
}

/// SIGINT ignored for as long as this lives; whatever it was before, restored after. std has no
/// signal API, and this is the one call otto needs from one, so it is declared rather than a crate.
struct IgnoreInterrupt(usize);

extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

const SIGINT: i32 = 2;
const SIG_IGN: usize = 1;

impl IgnoreInterrupt {
    fn new() -> Self {
        // SAFETY: `signal` with SIG_IGN installs no handler code; it only sets a disposition.
        IgnoreInterrupt(unsafe { signal(SIGINT, SIG_IGN) })
    }
}

impl Drop for IgnoreInterrupt {
    fn drop(&mut self) {
        // SAFETY: puts back exactly the disposition `new` replaced.
        unsafe {
            signal(SIGINT, self.0);
        }
    }
}

/// Refuse an `otto state` write made from a chat — `chat` is `CHAT_ENV`'s value, the run the chat
/// is about. Reads pass: a chat orients from `otto state get` and `tail` like a wake does.
pub fn refuse_state_write(command: &crate::cli::StateCommand, chat: Option<&str>) -> Result<(), OttoError> {
    match chat.filter(|run| !run.is_empty()) {
        Some(run) if command.writes() => Err(OttoError::conflict(format!(
            "`otto state` writes are refused in a chat: this is a chat about run {run}, not a wake of it, \
             and nothing in a chat changes the run. To change how it works, leave it a note once the person \
             has said so — `otto note {run} \"…\"`"
        ))),
        _ => Ok(()),
    }
}

/// One wake's transcript, as a chat's opening message lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WakeTranscript {
    wake: u64,
    started: String,
    path: PathBuf,
}

/// What the chat is told about the transcripts its wakes left, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Transcripts {
    listed: Vec<WakeTranscript>,
    /// Wakes with a session on record, past those listed.
    more: usize,
}

/// The transcripts of the run's wakes that can still be found, newest first: each `wake-started`
/// names its session, and `run.json` names the last wake's — the one place a wake from before
/// the journal named them is still on record.
fn wake_transcripts(dir: &Path, state: &RunState) -> Transcripts {
    let mut sessions: Vec<(u64, String, String)> = Vec::new();
    let journal = std::fs::read_to_string(dir.join("journal.jsonl")).unwrap_or_default();
    for line in journal.lines().filter(|l| l.contains("\"wake-started\"")) {
        let Some(line) = crate::core::parse_line(line) else { continue };
        let v = &line.value;
        if let (Some(n), Some(session)) = (v["wake"].as_u64(), v["session"].as_str()) {
            sessions.push((n, v["ts"].as_str().unwrap_or_default().to_string(), session.to_string()));
        }
    }
    if let Some(wake) = &state.wake {
        if let Some(session) = wake.session.as_ref().filter(|_| !sessions.iter().any(|(n, ..)| *n == wake.n as u64)) {
            sessions.push((wake.n as u64, wake.started_at.to_string(), session.clone()));
        }
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.0));
    let mut found = Transcripts::default();
    for (i, (wake, started, session)) in sessions.iter().enumerate() {
        if found.listed.len() == TRANSCRIPTS_LISTED {
            found.more = sessions.len() - i;
            break;
        }
        if let Some(path) = transcript::find_transcript(session) {
            found.listed.push(WakeTranscript { wake: *wake, started: started.clone(), path });
        }
    }
    found
}

/// The ids of the notes left since `since`, read from the journal — so a note a wake delivered
/// while the chat was still open counts as much as one still waiting.
fn notes_left_since(dir: &Path, since: Timestamp) -> Vec<String> {
    let journal = std::fs::read_to_string(dir.join("journal.jsonl")).unwrap_or_default();
    journal
        .lines()
        .filter(|l| l.contains("\"note-added\""))
        .filter_map(crate::core::parse_line)
        .filter(|line| line.at.is_some_and(|at| at >= since.dt()))
        .filter_map(|line| line.value["note"].as_str().map(str::to_string))
        .collect()
}

/// A chat's first message: which run, where it is, whether a wake is working, the transcripts —
/// and the person's question, or a request to say where things stand. Paths, not contents, as for
/// a wake (`wake::prompt`): the chat reads what a question needs.
struct Opening<'a> {
    state: &'a RunState,
    dir: &'a Path,
    short: &'a str,
    working: bool,
    transcripts: Transcripts,
    question: Option<&'a str>,
    attended: bool,
}

impl Opening<'_> {
    fn text(&self) -> String {
        let s = self.state;
        let short = self.short;
        let mut out = format!(
            "A chat about run `{label}` — you are not a wake of it; follow your instructions for a chat.\n\n\
             Its directory is `{dir}`: `run.json`, `handoff.md`, `journal.jsonl` (`otto logs {short}`), `gates/`, \
             `notes/`, `messages/`, `outputs/` and `artifacts/`. Use `{short}` for the run in any otto command.\n",
            label = crate::paths::label(&s.id, &s.display_name()),
            dir = self.dir.display(),
        );
        if self.working {
            if let Some(wake) = &s.wake {
                out.push_str(&format!(
                    "\nWake {} is working on it now, since {}: what is on disk may change while you talk. \
                     `otto attach {short}` follows what it is doing.\n",
                    wake.n, wake.started_at
                ));
            }
        } else if s.status.is_terminal() {
            out.push_str(&format!("\nIt is over — {:?}. No wake will read a note left now.\n", s.status));
        }
        let t = &self.transcripts;
        if t.listed.is_empty() {
            out.push_str("\nNo wake's transcript can be found — none has run, or a sandbox hides `~/.claude`.\n");
        } else {
            out.push_str("\nWhat its wakes did, in their own words — each one's session transcript, newest first:\n\n");
            for w in &t.listed {
                out.push_str(&format!("- wake {}, started {} — `{}`\n", w.wake, w.started, w.path.display()));
            }
            if t.more > 0 {
                out.push_str(&format!(
                    "\n{} earlier wake(s) are not listed: the `session` on each `wake-started` line in the journal is the \
                     name of its transcript (`<session>.jsonl`), next to these.\n",
                    t.more
                ));
            }
        }
        out.push('\n');
        match (self.question, self.attended) {
            (Some(q), true) => out.push_str(&format!("The person asks:\n\n{}\n", q.trim())),
            (Some(q), false) => out.push_str(&format!(
                "There is no terminal: this is the one question, and nobody can reply. The person asks:\n\n{}\n",
                q.trim()
            )),
            (None, _) => out.push_str(&format!(
                "Orient — `otto show {short}` first — then tell the person in a few lines where it stands, and wait for their questions.\n"
            )),
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    fn state_command(words: &[&str]) -> crate::cli::StateCommand {
        let mut argv = vec!["otto", "state"];
        argv.extend(words);
        match Cli::try_parse_from(argv).expect("parses").command {
            crate::cli::Command::State { command } => command,
            other => panic!("not a state command: {other:?}"),
        }
    }

    /// A chat can read the run the way a wake orients, and cannot write it, by any `otto state`.
    #[test]
    fn a_chat_reads_through_otto_state_and_never_writes() {
        for read in [&["get", "7"][..], &["tail", "7"], &["list"], &["due"], &["locks"]] {
            assert!(refuse_state_write(&state_command(read), Some("7")).is_ok(), "{read:?} is a read");
        }
        for write in [
            &["set-phase", "7", "--phase", "x"][..],
            &["handoff", "7", "--stdin"],
            &["log", "7", "--event", "cycle", "--message", "x"],
            &["message", "7", "--kind", "update", "--title", "x", "--body", "x"],
        ] {
            let err = refuse_state_write(&state_command(write), Some("7")).expect_err(&format!("{write:?} is a write"));
            assert_eq!(err.code, 2);
            assert!(err.to_string().contains("otto note 7"), "the refusal says what to do instead: {err}");
        }
    }

    /// Outside a chat — every wake — nothing is refused; an empty value is no chat either.
    #[test]
    fn outside_a_chat_every_write_goes_through() {
        let write = state_command(&["set-phase", "7", "--phase", "x"]);
        assert!(refuse_state_write(&write, None).is_ok());
        assert!(refuse_state_write(&write, Some("")).is_ok());
    }

    fn journal(dir: &Path, lines: &[serde_json::Value]) {
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        std::fs::write(dir.join("journal.jsonl"), text.join("\n") + "\n").unwrap();
    }

    /// Writes an empty transcript for `session` where claude would leave one.
    fn transcript_for(session: &str) -> PathBuf {
        let dir = crate::paths::claude_projects_dir().join("-w-work");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{session}.jsonl"));
        std::fs::write(&path, "{}\n").unwrap();
        path
    }

    /// Every wake the journal names a session for is listed, newest first, as long as its
    /// transcript is still there; `run.json`'s last wake counts once, journal or not.
    #[test]
    fn transcripts_are_found_from_the_journal_and_the_last_wake() {
        let _home = crate::paths::test_support::TempHome::new();
        let dir = tempfile::tempdir().unwrap();
        let one = transcript_for("11111111-1111-4111-8111-111111111111");
        let three = transcript_for("33333333-3333-4333-8333-333333333333");
        journal(
            dir.path(),
            &[
                serde_json::json!({"ts": "2026-10-01T09:00:00Z", "event": "wake-started", "wake": 1, "session": "11111111-1111-4111-8111-111111111111"}),
                serde_json::json!({"ts": "2026-10-01T10:00:00Z", "event": "wake-started", "wake": 2, "session": "22222222-2222-4222-8222-222222222222"}),
            ],
        );
        let mut state = crate::state::test_run_state("r1");
        state.wake = Some(crate::state::Wake {
            n: 3,
            started_at: Timestamp::now(),
            deadline_at: Timestamp::now(),
            launcher: "claude".into(),
            pid: None,
            session: Some("33333333-3333-4333-8333-333333333333".into()),
            outcome: None,
        });
        let found = wake_transcripts(dir.path(), &state);
        let listed: Vec<(u64, &Path)> = found.listed.iter().map(|w| (w.wake, w.path.as_path())).collect();
        assert_eq!(listed, vec![(3, three.as_path()), (1, one.as_path())], "wake 2's transcript is gone, so it is not listed");
        assert_eq!(found.more, 0);
    }

    /// A long run lists its newest wakes and says how many more there are, rather than all of them.
    #[test]
    fn a_long_run_lists_its_newest_wakes() {
        let _home = crate::paths::test_support::TempHome::new();
        let dir = tempfile::tempdir().unwrap();
        let lines: Vec<serde_json::Value> = (1..=14u32)
            .map(|n| {
                let session = transcript::session_id("r1", n, "2026-10-01T09:00:00Z");
                transcript_for(&session);
                serde_json::json!({"ts": "2026-10-01T09:00:00Z", "event": "wake-started", "wake": n, "session": session})
            })
            .collect();
        journal(dir.path(), &lines);
        let found = wake_transcripts(dir.path(), &crate::state::test_run_state("r1"));
        assert_eq!(found.listed.iter().map(|w| w.wake).collect::<Vec<_>>(), (5..=14).rev().collect::<Vec<_>>());
        assert_eq!(found.more, 4);
    }

    #[test]
    fn the_notes_a_chat_left_are_the_ones_added_while_it_was_open() {
        let dir = tempfile::tempdir().unwrap();
        journal(
            dir.path(),
            &[
                serde_json::json!({"ts": "2026-10-01T09:00:00Z", "event": "note-added", "note": "1", "file": "notes/001.md", "standing": false}),
                serde_json::json!({"ts": "2026-10-01T10:00:00Z", "event": "chat-started", "session": "s", "launcher": "claude"}),
                serde_json::json!({"ts": "2026-10-01T10:05:00Z", "event": "note-added", "note": "2", "file": "notes/002.md", "standing": true}),
            ],
        );
        let since = Timestamp::at(crate::clock::parse_iso("2026-10-01T10:00:00Z").unwrap());
        assert_eq!(notes_left_since(dir.path(), since), vec!["2".to_string()]);
    }

    fn opening<'a>(state: &'a RunState, dir: &'a Path) -> Opening<'a> {
        Opening {
            state,
            dir,
            short: "7",
            working: false,
            transcripts: Transcripts::default(),
            question: None,
            attended: true,
        }
    }

    /// The opening names the run and its directory, and with no question asks for where it stands.
    #[test]
    fn the_opening_says_which_run_and_where_it_is() {
        let state = crate::state::test_run_state("7");
        let text = opening(&state, Path::new("/home/.otto/runs/7")).text();
        assert!(text.contains("you are not a wake"), "{text}");
        assert!(text.contains("`/home/.otto/runs/7`"), "{text}");
        assert!(text.contains("otto show 7"), "with nothing asked, it orients and says where it stands: {text}");
        assert!(text.contains("No wake's transcript can be found"), "{text}");
    }

    /// A question asked with no terminal is the only one, and the chat is told nobody can reply.
    #[test]
    fn a_question_with_no_terminal_is_answered_once() {
        let state = crate::state::test_run_state("7");
        let dir = Path::new("/r");
        let asked = Opening { question: Some("Why did it skip e2e?"), ..opening(&state, dir) };
        let text = asked.text();
        assert!(text.ends_with("The person asks:\n\nWhy did it skip e2e?\n"), "{text}");
        assert!(!text.contains("no terminal"), "{text}");
        let scripted = Opening { question: Some("Why did it skip e2e?"), attended: false, ..opening(&state, dir) };
        assert!(scripted.text().contains("There is no terminal: this is the one question"), "{}", scripted.text());
    }

    #[test]
    fn the_opening_lists_transcripts_and_says_how_many_more() {
        let state = crate::state::test_run_state("7");
        let listed = vec![WakeTranscript { wake: 12, started: "2026-10-18T09:00:00Z".into(), path: "/t/12.jsonl".into() }];
        let with = Opening { transcripts: Transcripts { listed, more: 11 }, ..opening(&state, Path::new("/r")) };
        let text = with.text();
        assert!(text.contains("- wake 12, started 2026-10-18T09:00:00Z — `/t/12.jsonl`"), "{text}");
        assert!(text.contains("11 earlier wake(s) are not listed"), "{text}");
    }

    /// A finished run is still worth asking about, but a note left for it is read by nobody.
    #[test]
    fn a_finished_run_says_no_note_will_be_read() {
        let mut state = crate::state::test_run_state("7");
        state.status = crate::state::Status::Done;
        let text = opening(&state, Path::new("/r")).text();
        assert!(text.contains("It is over — Done. No wake will read a note left now."), "{text}");
        assert_eq!(ended_line("7", &[], &state), "chat ended — nothing in it reached the run");
    }

    #[test]
    fn the_end_of_a_chat_says_what_reached_the_run() {
        let state = crate::state::test_run_state("7");
        assert!(ended_line("7", &["3".into(), "4".into()], &state).contains("note(s) 3, 4 left for the run"));
        assert!(ended_line("7", &[], &state).contains("`otto note 7 \"…\"`"));
    }
}
