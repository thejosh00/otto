//! A command on a pseudo-terminal: what the web page's chat panel types into (plans/web-chat.md).
//!
//! A chat has to have a terminal. Print mode would let a page drive claude through pipes, but it
//! bills SDK credits rather than the subscription, and a chat's permission prompts need a person at
//! a prompt (DESIGN.md §7). So `otto serve` gives `otto chat` a terminal of its own: the child gets
//! the pty's far end as stdin, stdout, stderr and controlling terminal, and the server keeps the
//! near end, whose bytes the page's terminal draws and whose input is what the person types.
//!
//! std has no pty API and this is a handful of calls, so they are made through `libc`, as
//! `live_tests` and `landing::install` already make `setsid`, rather than through a crate.
//!
//! **Hanging up** signals two process groups: the child's own, and whichever is in front on the
//! terminal. The child is `otto chat`, which ignores SIGHUP so that it can journal the chat's end;
//! what has to go is claude, and a program that manages the terminal can move itself into a group
//! of its own and take the front — an interactive shell does, which is how the panel's first
//! hand test found this. So [`Pty::kill_foreground`] comes before [`Pty::kill_all`]: the one that
//! lets `otto chat` finish up, then the one that doesn't wait for it.

// Nothing outside the tests starts one until the server's live chats do (plans/web-chat.md, step 3).
#![cfg_attr(not(test), allow(dead_code))]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::process::Stdio;
use tokio::io::unix::AsyncFd;
use tokio::io::Interest;

// <sys/ttycom.h> on macOS; `libc` names them for the other BSDs but not for Apple.
const TIOCSCTTY: libc::c_ulong = 0x2000_7461;
const TIOCSWINSZ: libc::c_ulong = 0x8008_7467;

/// A terminal's size in characters, as the page's terminal measures itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl Size {
    fn winsize(self) -> libc::winsize {
        libc::winsize { ws_row: self.rows.max(1), ws_col: self.cols.max(1), ws_xpixel: 0, ws_ypixel: 0 }
    }
}

/// The server's end of a pseudo-terminal with a command running on the other. Shared between the
/// task reading what the command draws and whoever writes what the person types, so every method
/// takes `&self`.
pub struct Pty {
    master: AsyncFd<OwnedFd>,
    /// The child's process group, which is its pid: it is the leader of a session of its own.
    group: libc::pid_t,
}

/// What a pty's command runs as. `TERM` is set to what the page's terminal (xterm.js) emulates
/// unless `env` names it: `otto serve` started by launchd has none to pass on, and a TUI with no
/// `TERM` draws nothing.
pub struct Command<'a> {
    pub argv: &'a [String],
    pub cwd: Option<&'a Path>,
    pub env: &'a [(String, String)],
    pub size: Size,
}

pub const TERM: &str = "xterm-256color";

impl Pty {
    /// Start `command` on a new pseudo-terminal. The child is waited on through the
    /// `tokio::process::Child` returned beside the pty; dropping that child doesn't stop it.
    pub fn spawn(command: &Command) -> io::Result<(Pty, tokio::process::Child)> {
        let (program, args) = command.argv.split_first().ok_or_else(|| io::Error::other("no command to run"))?;
        let (master, slave) = open(command.size)?;

        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args);
        if let Some(dir) = command.cwd {
            cmd.current_dir(dir);
        }
        if !command.env.iter().any(|(k, _)| k == "TERM") {
            cmd.env("TERM", TERM);
        }
        cmd.envs(command.env.iter().map(|(k, v)| (k, v)));
        cmd.stdin(Stdio::from(slave.try_clone()?)).stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                // A session of its own, so the pty can be its controlling terminal and its
                // process group is its pid…
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                // …and the pty that is now its stdin becomes that controlling terminal, so
                // Ctrl-C typed in the page reaches it as SIGINT and claude can open /dev/tty.
                // macOS makes it so without asking, which is why no test fails without this line;
                // POSIX doesn't promise that, and asking for what is already so succeeds.
                if libc::ioctl(0, TIOCSCTTY, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        // `cmd` holds the parent's copies of the far end; it goes here, so once the child and
        // everything it starts have exited, reads see the end rather than waiting for ever.
        drop(cmd);
        let group = child.id().ok_or_else(|| io::Error::other("the command exited before it could be watched"))? as libc::pid_t;
        Ok((Pty { master: AsyncFd::new(master)?, group }, child))
    }

    /// The next bytes the command drew, or `Ok(0)` once nothing is left on the far end.
    ///
    /// Something must keep reading for as long as the command runs, even with nobody watching:
    /// what isn't read fills the terminal's buffer and blocks the command's next write, and on
    /// macOS a process can't finish exiting while output it left — an echoed `^C` is enough — is
    /// still unread.
    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut ready = self.master.readable().await?;
            match ready.try_io(|fd| cvt(unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } as i64)) {
                Ok(Ok(n)) => return Ok(n),
                // With every copy of the far end closed, macOS answers a read with EIO, not EOF.
                Ok(Err(e)) if e.raw_os_error() == Some(libc::EIO) => return Ok(0),
                Ok(Err(e)) => return Err(e),
                Err(_would_block) => continue,
            }
        }
    }

    /// Type `bytes`, all of them.
    pub async fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let n = self
                .master
                .async_io(Interest::WRITABLE, |fd| cvt(unsafe { libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) } as i64))
                .await?;
            bytes = &bytes[n..];
        }
        Ok(())
    }

    /// Tell the command the page's terminal is now `size`; the kernel sends it SIGWINCH.
    pub fn resize(&self, size: Size) -> io::Result<()> {
        let ws = size.winsize();
        cvt(unsafe { libc::ioctl(self.master.as_raw_fd(), TIOCSWINSZ, &ws) }).map(drop)
    }

    /// SIGHUP to whatever is in front on the terminal and to the child's own group — see the
    /// module doc for why both.
    pub fn hang_up(&self) -> io::Result<()> {
        if let Some(front) = self.foreground().filter(|&front| front != self.group) {
            signal(front, libc::SIGHUP)?;
        }
        signal(self.group, libc::SIGHUP)
    }

    /// SIGKILL to whatever is in front on the terminal, for a hang-up that wasn't heeded: claude,
    /// in a group of its own or in the child's. The child's group is spared when they differ, so
    /// `otto chat` can still journal the end.
    pub fn kill_foreground(&self) -> io::Result<()> {
        signal(self.foreground().unwrap_or(self.group), libc::SIGKILL)
    }

    /// SIGKILL to both groups: for a chat whose own `otto chat` won't go either.
    pub fn kill_all(&self) -> io::Result<()> {
        if let Some(front) = self.foreground().filter(|&front| front != self.group) {
            signal(front, libc::SIGKILL)?;
        }
        signal(self.group, libc::SIGKILL)
    }

    /// The process group in front on the terminal, while there is one.
    fn foreground(&self) -> Option<libc::pid_t> {
        Some(unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) }).filter(|&group| group > 0)
    }
}

/// `signal` to a process group: nothing left in it is no error, as what the signal was for has
/// already happened.
fn signal(group: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
    match cvt(unsafe { libc::killpg(group, signal) }) {
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        other => other.map(drop),
    }
}

/// A new pty: the near end non-blocking for the reactor, both ends closed on exec so neither leaks
/// into the child as anything but the stdio it is given.
fn open(size: Size) -> io::Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
    let mut ws = size.winsize();
    cvt(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) })?;
    // SAFETY: openpty succeeded, so both are open descriptors nothing else owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        cvt(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;
    }
    let flags = cvt(unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) })?;
    cvt(unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags as libc::c_int | libc::O_NONBLOCK) })?;
    Ok((master, slave))
}

/// A libc return value as a `Result`: -1 is the error in errno.
fn cvt(ret: impl Into<i64>) -> io::Result<usize> {
    let n: i64 = ret.into();
    if n == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const SIZE: Size = Size { cols: 80, rows: 24 };

    fn sh(script: &str) -> (Pty, tokio::process::Child) {
        let argv = ["/bin/sh".to_string(), "-c".to_string(), script.to_string()];
        Pty::spawn(&Command { argv: &argv, cwd: None, env: &[], size: SIZE }).expect("spawn on a pty")
    }

    /// Everything the command draws until the far end closes, bounded so a hang fails the test.
    async fn drain(pty: &Pty) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        let read_all = async {
            loop {
                match pty.read(&mut buf).await.expect("read") {
                    0 => break,
                    n => out.extend_from_slice(&buf[..n]),
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), read_all).await.expect("the far end never closed");
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Read until `want` has been drawn: the command is waiting for input after it.
    async fn until(pty: &Pty, want: &str) -> String {
        let mut out = String::new();
        let mut buf = [0u8; 4096];
        let wait = async {
            while !out.contains(want) {
                let n = pty.read(&mut buf).await.expect("read");
                assert!(n > 0, "ended before {want:?}: {out:?}");
                out.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        };
        tokio::time::timeout(Duration::from_secs(10), wait).await.unwrap_or_else(|_| panic!("never drew {want:?}: {out:?}"));
        out
    }

    #[tokio::test]
    async fn what_the_command_draws_is_read_and_the_end_is_seen() {
        let (pty, mut child) = sh("printf hello");
        assert!(drain(&pty).await.contains("hello"));
        assert!(child.wait().await.unwrap().success());
    }

    /// The far end is a terminal and the child's controlling one: /dev/tty only opens with one, and
    /// a TUI checks both.
    #[tokio::test]
    async fn the_command_has_a_terminal_and_it_controls_it() {
        let (pty, _child) = sh("[ -t 0 ] && [ -t 1 ] && echo is-a-tty; exec 3</dev/tty && echo controls-it; echo \"TERM=$TERM\"");
        let out = drain(&pty).await;
        assert!(out.contains("is-a-tty"), "{out:?}");
        assert!(out.contains("controls-it"), "{out:?}");
        assert!(out.contains(&format!("TERM={TERM}")), "{out:?}");
    }

    /// Ctrl-C typed in the page is the terminal's interrupt, not a byte: it stops what is running.
    #[tokio::test]
    async fn ctrl_c_interrupts_the_command() {
        let (pty, mut child) = sh("echo ready; sleep 60");
        until(&pty, "ready").await;
        pty.write_all(b"\x03").await.unwrap();
        // Read to the end: the interrupted shell can't finish exiting until its echoed ^C has been
        // read (see `Pty::read`).
        assert!(drain(&pty).await.contains("^C"));
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.expect("interrupted").unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(libc::SIGINT), "{status:?}");
    }

    #[tokio::test]
    async fn what_is_typed_reaches_the_command() {
        let (pty, _child) = sh("echo ready; read line; echo \"got:$line\"");
        until(&pty, "ready").await;
        pty.write_all(b"abc\n").await.unwrap();
        assert!(drain(&pty).await.contains("got:abc"));
    }

    #[tokio::test]
    async fn the_command_sees_the_size_it_starts_with_and_each_resize() {
        let (pty, _child) = sh("stty size; echo ready; read x; stty size");
        let before = until(&pty, "ready").await;
        assert!(before.contains("24 80"), "the starting size: {before:?}");
        pty.resize(Size { cols: 132, rows: 40 }).unwrap();
        pty.write_all(b"\n").await.unwrap();
        assert!(drain(&pty).await.contains("40 132"));
    }

    /// The shape of a chat: a leader that ignores SIGHUP, as `otto chat` does, with a child started
    /// before it did. A hang-up ends the child, and the leader outlives it to finish up.
    #[tokio::test]
    async fn a_hang_up_ends_the_group_and_a_leader_ignoring_it_finishes_up() {
        let (pty, mut child) = sh("sleep 60 & trap '' HUP; echo ready; wait; echo finished-up");
        until(&pty, "ready").await;
        pty.hang_up().unwrap();
        assert!(drain(&pty).await.contains("finished-up"));
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.expect("the leader exits").unwrap();
        assert!(status.success(), "{status:?}");
    }

    /// What the panel's first hand test found: an interactive shell — as claude might — takes a
    /// process group of its own and the front of the terminal. The hang-up reaches it there, and the
    /// leader still finishes up.
    #[tokio::test]
    async fn a_hang_up_reaches_a_program_that_took_the_front_of_the_terminal() {
        let (pty, mut child) = sh("PS1='ready$ ' /bin/bash --noprofile --norc -i </dev/tty & trap '' HUP; wait; echo finished-up");
        until(&pty, "ready$ ").await;
        let front = pty.foreground().expect("something is in front");
        assert_ne!(front, pty.group, "bash took a group of its own");
        pty.hang_up().unwrap();
        assert!(drain(&pty).await.contains("finished-up"));
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.expect("the leader exits").unwrap();
        assert!(status.success(), "{status:?}");
    }

    /// Killing what is in front spares a leader in a group of its own, so it can finish up.
    #[tokio::test]
    async fn killing_the_front_spares_the_leader_behind_it() {
        let (pty, mut child) = sh("trap '' HUP; /bin/bash --noprofile --norc -i -c 'trap \"\" HUP; set -m; echo ready; sleep 60' </dev/tty & wait; echo finished-up");
        until(&pty, "ready").await;
        pty.hang_up().unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(500), child.wait()).await.is_err(), "nothing heeds SIGHUP");
        pty.kill_foreground().unwrap();
        assert!(drain(&pty).await.contains("finished-up"));
        assert!(tokio::time::timeout(Duration::from_secs(10), child.wait()).await.expect("the leader exits").unwrap().success());
    }

    #[tokio::test]
    async fn a_kill_ends_what_a_hang_up_did_not() {
        let (pty, mut child) = sh("trap '' HUP; echo ready; sleep 60");
        until(&pty, "ready").await;
        pty.hang_up().unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(500), child.wait()).await.is_err(), "it ignores SIGHUP");
        pty.kill_all().unwrap();
        tokio::time::timeout(Duration::from_secs(10), child.wait()).await.expect("killed").unwrap();
        pty.hang_up().expect("signalling an empty group is not an error");
    }

    #[test]
    fn an_empty_command_is_refused() {
        assert!(Pty::spawn(&Command { argv: &[], cwd: None, env: &[], size: SIZE }).is_err());
    }
}
