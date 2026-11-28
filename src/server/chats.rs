//! Live chats: `otto chat` on a pseudo-terminal, for the web page's chat panel (plans/web-chat.md).
//!
//! The page can't hold a chat itself (DESIGN.md §7), so the server holds one per run and the page
//! attaches to it over a websocket. Closing the panel or reloading the page detaches; it doesn't
//! end the chat, and attaching again replays what it last drew. A chat ends when it exits — `/exit`
//! typed in it — when the page's **End chat** asks (`end`), after [`Timing::idle`] with nobody
//! attached, or when the server stops (`hang_up_all`).
//!
//! **This is the one thing the server holds.** Every other handler is a view over the run
//! directory. A live chat is a process, so it lives as long as the server does; but it journals its
//! own start and end, and it changes nothing about any run, so losing one loses a conversation and
//! nothing else.
//!
//! The socket: binary frames are the terminal's bytes, both ways. Text frames are JSON control:
//! `{"resize":{"cols":..,"rows":..}}` from the page; `{"chat":{..}}` first, `{"reset":true}` before
//! a fresh copy of the screen for a page that fell behind, and `{"ended":{"code":..}}` or
//! `{"error":".."}` last, to it.

use super::pty::{self, Pty, Size};
use super::{ApiError, AppState};
use crate::error::OttoError;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::body::Bytes;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};

/// How much of what a chat drew is kept for a page that attaches later.
pub const SCROLLBACK: usize = 256 * 1024;

/// What a chat runs, given the run's id and what the page was showing: `otto chat` itself, but for
/// the tests, which give a script in its place.
pub type Launch = Arc<dyn Fn(&str, Option<&str>) -> Result<Vec<String>, OttoError> + Send + Sync>;

/// `otto chat <run> [--about …]`, by this very binary.
pub fn otto_chat() -> Launch {
    Arc::new(|run: &str, about: Option<&str>| {
        let exe = crate::paths::current_exe()?;
        let mut argv = vec![exe.display().to_string(), "chat".to_string(), run.to_string()];
        if let Some(about) = about {
            argv.extend(["--about".to_string(), about.to_string()]);
        }
        Ok(argv)
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// How long a chat with nobody attached is kept.
    pub idle: Duration,
    /// How long a hang-up is given before what is still running is killed.
    pub grace: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing { idle: Duration::from_secs(30 * 60), grace: Duration::from_secs(5) }
    }
}

/// How a chat ended: its exit code, or none when a signal ended it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Ended {
    pub code: Option<i32>,
}

/// A live chat, as `GET /api/chats` lists it and the socket's first message describes it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub run: String,
    pub label: String,
    pub about: Option<String>,
    pub started_at: String,
    pub attached: usize,
}

pub struct Chats {
    live: Mutex<HashMap<String, Arc<Live>>>,
    launch: Launch,
    timing: Timing,
}

struct Live {
    run: String,
    label: String,
    about: Option<String>,
    started_at: String,
    pty: Pty,
    screen: Mutex<Screen>,
    attached: AtomicUsize,
    /// Since when nobody has been attached; `None` while somebody is.
    alone_since: Mutex<Option<Instant>>,
    ended: watch::Receiver<Option<Ended>>,
}

/// What a chat has drawn lately, and the channel the rest goes out on. One lock over both, so a
/// page that attaches gets each byte exactly once: from the copy, or from the channel after it.
struct Screen {
    kept: Vec<u8>,
    out: broadcast::Sender<Bytes>,
}

impl Screen {
    fn push(&mut self, bytes: &[u8]) {
        self.kept.extend_from_slice(bytes);
        if self.kept.len() > SCROLLBACK {
            // Cut at a line where there is one, so the page starts drawing from a line's start.
            let over = self.kept.len() - SCROLLBACK;
            let cut = self.kept[over..].iter().position(|&b| b == b'\n').map_or(over, |at| over + at + 1);
            self.kept.drain(..cut);
        }
        let _ = self.out.send(Bytes::copy_from_slice(bytes));
    }
}

impl Chats {
    pub fn new(launch: Launch, timing: Timing) -> Arc<Chats> {
        Arc::new(Chats { live: Mutex::new(HashMap::new()), launch, timing })
    }

    /// The run's live chat, started if it has none. `about` is used only when starting one: a chat
    /// is about what it was opened on for as long as it lasts.
    fn attach_or_start(self: &Arc<Self>, run: &str, label: &str, about: Option<&str>, size: Size) -> Result<Arc<Live>, OttoError> {
        let mut live = self.live.lock().unwrap();
        if let Some(chat) = live.get(run) {
            return Ok(chat.clone());
        }
        let argv = (self.launch)(run, about)?;
        let (pty, child) = Pty::spawn(&pty::Command { argv: &argv, cwd: None, env: &[], size })
            .map_err(|e| OttoError::new(format!("could not start the chat: {e}"), 1))?;
        let (ended_tx, ended) = watch::channel(None);
        let chat = Arc::new(Live {
            run: run.to_string(),
            label: label.to_string(),
            about: about.map(str::to_string),
            started_at: crate::clock::Timestamp::now().to_string(),
            pty,
            screen: Mutex::new(Screen { kept: Vec::new(), out: broadcast::channel(1024).0 }),
            attached: AtomicUsize::new(0),
            alone_since: Mutex::new(Some(Instant::now())),
            ended,
        });
        live.insert(run.to_string(), chat.clone());
        tokio::spawn(pump(chat.clone(), child, ended_tx, Arc::downgrade(self), self.timing.grace));
        Ok(chat)
    }

    pub fn list(&self) -> Vec<Summary> {
        let mut all: Vec<Summary> = self.live.lock().unwrap().values().map(|c| c.summary()).collect();
        all.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        all
    }

    /// Hang up the run's chat, and kill it if the hang-up isn't heeded. False when it has none.
    pub fn end(&self, run: &str) -> bool {
        let Some(chat) = self.live.lock().unwrap().get(run).cloned() else { return false };
        end_chat(chat, self.timing.grace);
        true
    }

    /// End every chat nobody has been attached to for `idle`.
    pub fn reap(&self) {
        let idle: Vec<Arc<Live>> = self
            .live
            .lock()
            .unwrap()
            .values()
            .filter(|c| c.alone_since.lock().unwrap().is_some_and(|since| since.elapsed() >= self.timing.idle))
            .cloned()
            .collect();
        for chat in idle {
            end_chat(chat, self.timing.grace);
        }
    }

    /// End every chat, and wait for them to go — killing what outstays `grace`. For the server's
    /// own end, so each chat has journaled its end before the server exits.
    pub async fn hang_up_all(&self) {
        let all: Vec<Arc<Live>> = self.live.lock().unwrap().values().cloned().collect();
        for chat in &all {
            let _ = chat.pty.hang_up();
        }
        let grace = self.timing.grace;
        for chat in all {
            let mut ended = chat.ended.clone();
            if tokio::time::timeout(grace, ended.wait_for(Option::is_some)).await.is_err() {
                let _ = chat.pty.kill();
                let _ = tokio::time::timeout(grace, ended.wait_for(Option::is_some)).await;
            }
        }
    }

    /// Reap every minute, for as long as the server runs.
    pub fn keep_reaping(self: &Arc<Self>) {
        let chats = Arc::downgrade(self);
        let every = (self.timing.idle / 30).clamp(Duration::from_millis(50), Duration::from_secs(60));
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            loop {
                tick.tick().await;
                match chats.upgrade() {
                    Some(chats) => chats.reap(),
                    None => return,
                }
            }
        });
    }
}

fn end_chat(chat: Arc<Live>, grace: Duration) {
    let _ = chat.pty.hang_up();
    tokio::spawn(async move {
        let mut ended = chat.ended.clone();
        if tokio::time::timeout(grace, ended.wait_for(Option::is_some)).await.is_err() {
            let _ = chat.pty.kill();
        }
    });
}

/// Read everything the chat draws for as long as it runs — whether or not anyone is attached; see
/// `Pty::read` — then wait for it, and say it has ended.
async fn pump(chat: Arc<Live>, mut child: tokio::process::Child, ended: watch::Sender<Option<Ended>>, chats: Weak<Chats>, grace: Duration) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match chat.pty.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => chat.screen.lock().unwrap().push(&buf[..n]),
        }
    }
    // Nothing holds the terminal any more, so the chat has exited or is about to; anything that
    // closed its terminal and carried on is not the chat's to keep.
    let status = match tokio::time::timeout(grace, child.wait()).await {
        Ok(status) => status.ok(),
        Err(_) => {
            let _ = chat.pty.kill();
            child.wait().await.ok()
        }
    };
    if let Some(chats) = chats.upgrade() {
        let mut live = chats.live.lock().unwrap();
        if live.get(&chat.run).is_some_and(|c| Arc::ptr_eq(c, &chat)) {
            live.remove(&chat.run);
        }
    }
    let _ = ended.send(Some(Ended { code: status.and_then(|s| s.code()) }));
}

impl Live {
    fn summary(&self) -> Summary {
        Summary {
            run: self.run.clone(),
            label: self.label.clone(),
            about: self.about.clone(),
            started_at: self.started_at.clone(),
            attached: self.attached.load(Ordering::SeqCst),
        }
    }

    /// What it has drawn so far, and everything after it.
    fn subscribe(&self) -> (Vec<u8>, broadcast::Receiver<Bytes>) {
        let screen = self.screen.lock().unwrap();
        (screen.kept.clone(), screen.out.subscribe())
    }

    fn attach(self: &Arc<Self>) -> Attached {
        self.attached.fetch_add(1, Ordering::SeqCst);
        *self.alone_since.lock().unwrap() = None;
        Attached(self.clone())
    }
}

/// A page attached to a chat, for as long as this lives.
struct Attached(Arc<Live>);

impl Drop for Attached {
    fn drop(&mut self) {
        if self.0.attached.fetch_sub(1, Ordering::SeqCst) == 1 {
            *self.0.alone_since.lock().unwrap() = Some(Instant::now());
        }
    }
}

// ----- the routes

#[derive(Deserialize)]
pub struct SocketQuery {
    about: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Control {
    Resize { cols: u16, rows: u16 },
}

/// `GET /api/runs/{id}/chat`: the run's chat, on a websocket — started if it has none.
pub async fn socket(State(state): State<Arc<AppState>>, Path(id): Path<String>, Query(q): Query<SocketQuery>, ws: WebSocketUpgrade) -> Response {
    let chats = state.chats.clone();
    ws.on_upgrade(move |mut socket| async move {
        let size = Size { cols: q.cols.unwrap_or(80), rows: q.rows.unwrap_or(24) };
        let about = q.about.as_deref().map(str::trim).filter(|a| !a.is_empty()).map(str::to_string);
        let found = tokio::task::spawn_blocking(move || -> Result<(String, String), OttoError> {
            let run = crate::paths::resolve_run_id(&id)?;
            let label = crate::paths::label_of(&run);
            Ok((run, label))
        })
        .await
        .map_err(|e| OttoError::new(e.to_string(), 4))
        .and_then(|r| r)
        .and_then(|(run, label)| chats.attach_or_start(&run, &label, about.as_deref(), size));
        match found {
            Ok(chat) => talk(socket, chat).await,
            Err(e) => {
                let _ = socket.send(text(&serde_json::json!({ "error": e.message }))).await;
                let _ = socket.send(Message::Close(None)).await;
            }
        }
    })
}

fn text(value: &serde_json::Value) -> Message {
    Message::Text(value.to_string().into())
}

/// Carry one page's side of a chat until either goes.
async fn talk(mut socket: WebSocket, chat: Arc<Live>) {
    let _attached = chat.attach();
    let (seen, mut out) = chat.subscribe();
    let mut ended = chat.ended.clone();
    if socket.send(text(&serde_json::json!({ "chat": chat.summary() }))).await.is_err() {
        return;
    }
    if !seen.is_empty() && socket.send(Message::Binary(seen.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            // Output first: everything the chat drew is sent before the word that it ended.
            biased;
            drawn = out.recv() => match drawn {
                Ok(bytes) => {
                    if socket.send(Message::Binary(bytes)).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Too far behind to catch up byte by byte: start this page again from a copy.
                    let (seen, fresh) = chat.subscribe();
                    out = fresh;
                    if socket.send(text(&serde_json::json!({ "reset": true }))).await.is_err()
                        || socket.send(Message::Binary(seen.into())).await.is_err()
                    {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => {}
            },
            said = socket.recv() => match said {
                Some(Ok(Message::Binary(typed))) => {
                    if chat.pty.write_all(&typed).await.is_err() {
                        return;
                    }
                }
                Some(Ok(Message::Text(control))) => {
                    if let Ok(Control::Resize { cols, rows }) = serde_json::from_str(control.as_str()) {
                        let _ = chat.pty.resize(Size { cols, rows });
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
            // Cloned out at once: the watch's guard can't be held across the sends below.
            how = async { ended.wait_for(Option::is_some).await.ok().and_then(|how| how.clone()) } => {
                let _ = socket.send(text(&serde_json::json!({ "ended": how }))).await;
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
        }
    }
}

/// `POST /api/runs/{id}/chat/end`: hang up the run's chat.
pub async fn end(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let run = match tokio::task::spawn_blocking(move || crate::paths::resolve_run_id(&id)).await {
        Ok(Ok(run)) => run,
        Ok(Err(e)) => return ApiError(e).into_response(),
        Err(e) => return ApiError(OttoError::new(e.to_string(), 4)).into_response(),
    };
    Json(serde_json::json!({ "ended": state.chats.end(&run) })).into_response()
}

/// `GET /api/chats`: the chats that are live.
pub async fn list(State(state): State<Arc<AppState>>) -> Response {
    Json(state.chats.list()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size { cols: 80, rows: 24 };
    const QUICK: Timing = Timing { idle: Duration::from_millis(300), grace: Duration::from_millis(300) };

    /// Chats that run `script` in place of `otto chat`, with the run and `about` as `$1` and `$2`.
    fn chats(script: &'static str, timing: Timing) -> Arc<Chats> {
        Chats::new(
            Arc::new(move |run: &str, about: Option<&str>| {
                Ok(vec!["/bin/sh".into(), "-c".into(), script.into(), "sh".into(), run.into(), about.unwrap_or("").into()])
            }),
            timing,
        )
    }

    async fn ended(chat: &Live) -> Ended {
        let mut ended = chat.ended.clone();
        let how = tokio::time::timeout(Duration::from_secs(10), ended.wait_for(Option::is_some)).await.expect("it ended").unwrap().clone();
        how.unwrap()
    }

    async fn drawn_until(chat: &Live, want: &str) -> String {
        let wait = async {
            loop {
                let seen = String::from_utf8_lossy(&chat.subscribe().0).into_owned();
                if seen.contains(want) {
                    return seen;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), wait).await.unwrap_or_else(|_| panic!("never drew {want:?}"))
    }

    #[tokio::test]
    async fn one_chat_per_run_about_what_it_was_opened_on() {
        let chats = chats("echo \"about=$2\"; exec cat", QUICK);
        let first = chats.attach_or_start("r1", "1 one", Some("change 1/2"), SIZE).unwrap();
        let again = chats.attach_or_start("r1", "1 one", Some("something else"), SIZE).unwrap();
        assert!(Arc::ptr_eq(&first, &again), "a run has one chat");
        drawn_until(&first, "about=change 1/2").await;
        let other = chats.attach_or_start("r2", "2 two", None, SIZE).unwrap();
        assert!(!Arc::ptr_eq(&first, &other));
        let listed: Vec<_> = chats.list().into_iter().map(|s| (s.run, s.about)).collect();
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&("r1".to_string(), Some("change 1/2".to_string()))), "{listed:?}");
        assert!(chats.end("r1") && chats.end("r2"));
        ended(&first).await;
        ended(&other).await;
        assert!(chats.list().is_empty(), "an ended chat is no longer live");
        assert!(!chats.end("r1"), "nothing left to end");
    }

    #[tokio::test]
    async fn a_chat_that_exits_is_gone_with_its_code() {
        let chats = chats("echo bye; exit 3", QUICK);
        let chat = chats.attach_or_start("r1", "1", None, SIZE).unwrap();
        assert_eq!(ended(&chat).await, Ended { code: Some(3) });
        assert!(String::from_utf8_lossy(&chat.subscribe().0).contains("bye"), "what it drew is kept to the end");
        assert!(chats.list().is_empty());
    }

    /// The shape of `otto chat`: a leader ignoring SIGHUP, with its child not. Ending it lets the
    /// leader finish up — where `otto chat` journals `chat-ended` — and exit on its own.
    #[tokio::test]
    async fn ending_a_chat_lets_its_leader_finish_up() {
        let chats = chats("sleep 60 & trap '' HUP; echo ready; wait; echo finished-up; exit 0", QUICK);
        let chat = chats.attach_or_start("r1", "1", None, SIZE).unwrap();
        drawn_until(&chat, "ready").await;
        assert!(chats.end("r1"));
        assert_eq!(ended(&chat).await, Ended { code: Some(0) }, "not killed");
        assert!(String::from_utf8_lossy(&chat.subscribe().0).contains("finished-up"));
    }

    #[tokio::test]
    async fn a_chat_that_ignores_the_hang_up_is_killed() {
        let chats = chats("trap '' HUP; echo ready; sleep 60", QUICK);
        let chat = chats.attach_or_start("r1", "1", None, SIZE).unwrap();
        drawn_until(&chat, "ready").await;
        chats.end("r1");
        assert_eq!(ended(&chat).await, Ended { code: None }, "ended by a signal");
    }

    #[tokio::test]
    async fn a_chat_nobody_is_attached_to_is_ended_once_idle_and_not_before() {
        let chats = chats("echo ready; exec cat", QUICK);
        let chat = chats.attach_or_start("r1", "1", None, SIZE).unwrap();
        let attached = chat.attach();
        tokio::time::sleep(QUICK.idle * 2).await;
        chats.reap();
        assert_eq!(chats.list().len(), 1, "somebody is attached");
        drop(attached);
        chats.reap();
        assert_eq!(chats.list().len(), 1, "only just left");
        tokio::time::sleep(QUICK.idle * 2).await;
        chats.reap();
        ended(&chat).await;
        assert!(chats.list().is_empty());
    }

    #[tokio::test]
    async fn the_server_ending_waits_for_every_chat() {
        let chats = chats("sleep 60 & trap '' HUP; echo ready; wait; echo finished-up", QUICK);
        let a = chats.attach_or_start("r1", "1", None, SIZE).unwrap();
        let b = chats.attach_or_start("r2", "2", None, SIZE).unwrap();
        drawn_until(&a, "ready").await;
        drawn_until(&b, "ready").await;
        chats.hang_up_all().await;
        for chat in [a, b] {
            assert!(chat.ended.borrow().is_some(), "ended before hang_up_all returned");
            assert!(String::from_utf8_lossy(&chat.subscribe().0).contains("finished-up"));
        }
    }

    /// A page attaching later gets what was drawn, then what is drawn next, each byte once.
    #[tokio::test]
    async fn attaching_replays_what_was_drawn_then_follows() {
        let chats = chats("echo first; exec cat", QUICK);
        let chat = chats.attach_or_start("r1", "1", None, SIZE).unwrap();
        drawn_until(&chat, "first").await;
        let (seen, mut out) = chat.subscribe();
        assert!(String::from_utf8_lossy(&seen).contains("first"));
        chat.pty.write_all(b"second\n").await.unwrap();
        let mut after = String::new();
        while !after.contains("second") {
            let bytes = tokio::time::timeout(Duration::from_secs(10), out.recv()).await.unwrap().unwrap();
            after.push_str(&String::from_utf8_lossy(&bytes));
        }
        assert!(!after.contains("first"), "not sent twice: {after:?}");
        chats.end("r1");
    }

    #[test]
    fn the_kept_screen_is_bounded_and_cut_at_a_line() {
        let mut screen = Screen { kept: Vec::new(), out: broadcast::channel(4).0 };
        let line = format!("{}\n", "x".repeat(99));
        for _ in 0..(SCROLLBACK / 100 + 50) {
            screen.push(line.as_bytes());
        }
        assert!(screen.kept.len() <= SCROLLBACK, "{}", screen.kept.len());
        assert!(screen.kept.len() > SCROLLBACK - 100);
        assert!(screen.kept.starts_with(b"xxx") && screen.kept.ends_with(b"x\n"), "whole lines");
    }
}
