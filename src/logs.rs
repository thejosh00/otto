//! src/logs.rs — otto's two launchd logs, kept usable as evidence: a stamp on every line of
//! `serve.log`, and `poke.log` rotated, with a note in it when the reviver went quiet for a while.
//!
//! Launchd opens each log once and hands the job the descriptor, and a descriptor follows a rename.
//! So rotating moves the pass's own output into `poke.log.1` unless it is pointed back at a fresh
//! `poke.log`, and a file renamed from under launchd is not made again until its next spawn. Both are
//! handled by rotating only from the process writing to the log, and re-pointing that process.
//!
//! Nothing here may affect a pass: every failure is said on stderr and otherwise forgotten.

use crate::clock::Timestamp;
use std::io::{BufRead, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::time::Duration;

/// `poke.log` rolls over at this size. At a line per five-minute pass, that is months of history.
const ROTATE_AT: u64 = 1024 * 1024;
/// Old files kept beside it: `poke.log.1`, the newest, to `poke.log.3`.
const KEPT: u32 = 3;
/// The reviver runs every 5 minutes; a gap this long since its last pass is worth a line.
const GAP_MINUTES: i64 = 30;
/// How long a stamper gets to drain its pipe when the server is on its way out. A child holding the
/// pipe open means it never sees the end, and that must not hold the exit up.
const DRAIN: Duration = Duration::from_millis(500);

extern "C" {
    fn dup2(from: i32, to: i32) -> i32;
}

fn point(from: &impl AsRawFd, to: i32) -> std::io::Result<()> {
    // SAFETY: both are open descriptors, and `to` is one of the standard streams, which std reads
    // by number every time and so never holds a stale view of.
    match unsafe { dup2(from.as_raw_fd(), to) } {
        -1 => Err(std::io::Error::last_os_error()),
        _ => Ok(()),
    }
}

fn same_file(fd: BorrowedFd, path: &Path) -> bool {
    let Ok(open) = fd.try_clone_to_owned().map(std::fs::File::from).and_then(|f| f.metadata()) else {
        return false;
    };
    std::fs::metadata(path).is_ok_and(|named| (open.dev(), open.ino()) == (named.dev(), named.ino()))
}

/// Whether this process's stdout is `poke.log`: launchd's reviver is, a poke by hand or the web
/// page's "poke now" isn't. Only the reviver looks after the log, and only its passes are in it.
pub fn poke_writes_its_log() -> bool {
    crate::paths::poke_log_path().is_ok_and(|path| same_file(std::io::stdout().as_fd(), &path))
}

fn rotated(log: &Path, n: u32) -> PathBuf {
    let mut name = log.as_os_str().to_owned();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

/// The stamp on the last line that has one. Not every line does: `otto poke: ...` lines and errors
/// don't, and a log can hold anything. No file is simply no stamp.
fn last_stamp_in(path: &Path) -> std::io::Result<Option<Timestamp>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.lines().rev().find_map(|line| Timestamp::parse(line.split_whitespace().next()?).ok()))
}

/// When the reviver last wrote a stamped line. A `poke.log` with none in it yet (a pass that rotated
/// it and died before saying anything) leaves the answer in `poke.log.1`.
pub fn last_pass() -> std::io::Result<Option<Timestamp>> {
    let log = crate::paths::poke_log_path().map_err(|e| std::io::Error::other(e.message))?;
    match last_stamp_in(&log)? {
        Some(at) => Ok(Some(at)),
        None => last_stamp_in(&rotated(&log, 1)),
    }
}

fn gap_line(last: Timestamp, now: Timestamp) -> Option<String> {
    let minutes = (now.dt() - last.dt()).whole_minutes();
    if minutes < GAP_MINUTES {
        return None;
    }
    let gap = if minutes < 60 { format!("{minutes}m") } else { format!("{}h{}m", minutes / 60, minutes % 60) };
    Some(format!("{now} {gap} since the last pass, at {last}"))
}

/// The stamped line for a pass that came long after the last one. Read before this pass says
/// anything. No earlier pass to compare with is not a gap, and a log nobody can read is not a reason
/// to do anything but say so.
pub fn gap_note(now: Timestamp) -> Option<String> {
    let last = match last_pass() {
        Ok(last) => last,
        Err(e) => {
            eprintln!("otto poke: couldn't read when the last pass was: {e}");
            return None;
        }
    };
    if last.is_none() && crate::paths::poke_log_path().is_ok_and(|log| std::fs::metadata(log).is_ok_and(|m| m.len() > 0)) {
        eprintln!("otto poke: couldn't tell when the last pass was: no stamped line in the log");
    }
    gap_line(last?, now)
}

/// Each file shifted along, the oldest kept one overwritten, and the log itself last: dying at any
/// step leaves every line in some file, and the next rotation carries on past the hole.
fn shift(log: &Path) -> std::io::Result<()> {
    for n in (1..KEPT).rev() {
        match std::fs::rename(rotated(log, n), rotated(log, n + 1)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    std::fs::rename(log, rotated(log, 1))
}

/// Roll `poke.log` once it is over the bound, and point this process at the new one, so what the
/// pass is about to say is where `otto agent status` reads. Under the poke lock, so two passes can't
/// both shift the files.
pub fn rotate_poke_log() {
    if let Err(e) = try_rotate() {
        eprintln!("otto poke: couldn't rotate the log: {e}");
    }
}

fn try_rotate() -> std::io::Result<()> {
    let log = crate::paths::poke_log_path().map_err(|e| std::io::Error::other(e.message))?;
    if std::fs::metadata(&log)?.len() < ROTATE_AT {
        return Ok(());
    }
    let stderr_too = same_file(std::io::stderr().as_fd(), &log);
    shift(&log)?;
    let fresh = std::fs::OpenOptions::new().create(true).append(true).open(&log)?;
    std::io::stdout().flush()?;
    point(&fresh, 1)?;
    if stderr_too {
        point(&fresh, 2)?;
    }
    Ok(())
}

/// The standard streams as they were before the stamping, and word of each stamper finishing.
struct Stamping {
    originals: Vec<(i32, OwnedFd)>,
    finished: Vec<Receiver<()>>,
}

static STAMPING: Mutex<Option<Stamping>> = Mutex::new(None);

/// Put a timestamp on every line this process writes from here on, for as long as it lives. The
/// standard streams become pipes and a thread each stamps what comes through, so it covers every
/// print and eprint anywhere in the code the server calls, a panic's message, and whatever a child
/// process inherits, without any of them having to know. A stamper that can't write (a full disk)
/// goes on reading, so the server never blocks on its log or finds it closed.
pub fn stamp_output() {
    if let Err(e) = try_stamp() {
        eprintln!("otto: its output won't have timestamps: {e}");
    }
}

fn try_stamp() -> std::io::Result<()> {
    let mut stamping = Stamping { originals: Vec::new(), finished: Vec::new() };
    let streams = [(1, std::io::stdout().as_fd().try_clone_to_owned()?), (2, std::io::stderr().as_fd().try_clone_to_owned()?)];
    for (fd, original) in streams {
        let (reader, writer) = std::io::pipe()?;
        let out = std::fs::File::from(original.try_clone()?);
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            stamp_lines(reader, out);
            let _ = done.send(());
        });
        point(&writer, fd)?;
        stamping.originals.push((fd, original));
        stamping.finished.push(finished);
    }
    *STAMPING.lock().unwrap_or_else(|e| e.into_inner()) = Some(stamping);
    // A panic on any other thread leaves the server running, and its stamper with it. One on the
    // main thread ends the process, so its message has to be through before that.
    let shown = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        shown(info);
        if std::thread::current().name() == Some("main") {
            finish_stamping();
        }
    }));
    Ok(())
}

fn stamp_lines(from: impl std::io::Read, mut to: impl Write) {
    let mut from = std::io::BufReader::new(from);
    let mut line = Vec::new();
    loop {
        line.clear();
        match from.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if line.last() != Some(&b'\n') {
            line.push(b'\n');
        }
        let mut stamped = format!("{} ", crate::clock::now_iso()).into_bytes();
        stamped.extend_from_slice(&line);
        let _ = to.write_all(&stamped);
    }
}

/// Before the process exits: let what is already written come through the stampers. A process
/// exiting doesn't wait for a thread, and the last thing said is usually why.
pub fn finish_stamping() {
    let Some(stamping) = STAMPING.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    for (fd, original) in &stamping.originals {
        let _ = point(original, *fd);
    }
    for finished in &stamping.finished {
        let _ = finished.recv_timeout(DRAIN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).unwrap()
    }

    #[test]
    fn a_gap_is_said_from_half_an_hour() {
        let now = at("2026-11-07T12:00:00Z");
        assert_eq!(gap_line(at("2026-11-07T11:55:00Z"), now), None);
        assert_eq!(gap_line(at("2026-11-07T11:30:01Z"), now), None);
        assert_eq!(gap_line(at("2026-11-07T11:30:00Z"), now).unwrap(), "2026-11-07T12:00:00Z 30m since the last pass, at 2026-11-07T11:30:00Z");
        assert_eq!(gap_line(at("2026-11-07T08:48:00Z"), now).unwrap(), "2026-11-07T12:00:00Z 3h12m since the last pass, at 2026-11-07T08:48:00Z");
        assert_eq!(gap_line(at("2026-11-07T13:00:00Z"), now), None, "a clock that went back is not a gap");
    }

    #[test]
    fn the_last_stamp_is_found_past_what_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("poke.log");
        assert_eq!(last_stamp_in(&log).unwrap(), None);
        std::fs::write(&log, b"2026-09-28T03:00:00Z nothing due\n2026-09-28T03:05:00Z nothing due\notto poke: something\n\xff\xfe garbled\n\n").unwrap();
        assert_eq!(last_stamp_in(&log).unwrap(), Some(at("2026-09-28T03:05:00Z")));
        std::fs::write(&log, "otto poke: something\n").unwrap();
        assert_eq!(last_stamp_in(&log).unwrap(), None);
    }

    #[test]
    fn a_line_is_stamped_whole_and_an_unfinished_one_is_finished() {
        let mut out = Vec::new();
        stamp_lines(&b"one\n\ntwo"[..], &mut out);
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        for (line, says) in lines.iter().zip(["one", "", "two"]) {
            let (stamp, rest) = line.split_once(' ').unwrap();
            assert!(Timestamp::parse(stamp).is_ok(), "{line}");
            assert_eq!(rest, says);
        }
    }
}
