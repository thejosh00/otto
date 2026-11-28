//! tests/web_chat.rs — the web page's chat panel end to end (plans/web-chat.md): a real `otto serve`,
//! a real websocket, and a real `otto chat` on the server's pseudo-terminal, with a stub in place of
//! claude that says what it was given and repeats what it is typed.

mod support;

use futures_util::{SinkExt, StreamExt};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const OTTO: &str = support::OTTO;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for d in ["home", "repo", "projects"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        // claude, as a chat starts it: says whether the opening carried what the page was showing,
        // then repeats each line typed, until it is hung up.
        let stub = root.join("stub");
        std::fs::write(
            &stub,
            "#!/bin/sh\ncase \"$*\" in *'is looking at change 1/2'*) echo about-seen;; esac\n\
             echo \"stub-ready chat=$OTTO_CHAT\"\nwhile IFS= read -r line; do echo \"heard:$line\"; done\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        // Named `claude`, so nothing here can reach the real one.
        std::fs::write(
            root.join("home/config.json"),
            serde_json::json!({"launchers": [{"name": "claude", "command": stub.display().to_string()}]}).to_string(),
        )
        .unwrap();
        Scratch { _dir: dir, root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = support::otto_command();
        command
            .args(args)
            .current_dir(self.root.join("repo"))
            .env("OTTO_HOME", self.root.join("home"))
            .env("OTTO_CLAUDE_PROJECTS_DIR", self.root.join("projects"))
            .env("OTTO_BIN", OTTO)
            .env_remove("TMUX")
            .env_remove("OTTO_RUN_ID")
            .env_remove("OTTO_RUN_DIR")
            .env_remove("OTTO_CHAT");
        command
    }

    fn run(&self, name: &str) -> String {
        let repo = self.root.join("repo");
        let out = self
            .command(&["state", "init", "--goal", "g", "--name", name, "--perpetual", "--detach", "none", "--workdir", repo.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let list = self.command(&["state", "list", "--json"]).output().unwrap();
        let list: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
        list.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap()["id"].as_str().unwrap().to_string()
    }

    fn journal(&self, id: &str) -> String {
        std::fs::read_to_string(self.root.join("home/runs").join(id).join("journal.jsonl")).unwrap_or_default()
    }

    /// `otto serve` on a port of its own, and that port once it is listening.
    #[allow(clippy::zombie_processes)] // handed back, and every test waits on it
    fn serve(&self) -> (Child, u16) {
        let mut server = self.command(&["serve", "--port", "0"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut said = BufReader::new(server.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            if said.read_line(&mut line).unwrap() == 0 {
                let status = server.wait().unwrap();
                panic!("the server ended before it listened: {status:?}");
            }
            if let Some(at) = line.find("http://127.0.0.1:") {
                let port: String = line[at + 17..].chars().take_while(char::is_ascii_digit).collect();
                // Read on, so the server never blocks writing to a full pipe.
                std::thread::spawn(move || std::io::copy(&mut said, &mut std::io::sink()));
                return (server, port.parse().unwrap());
            }
        }
    }
}

async fn open(port: u16, path: &str, origin: &str) -> Result<Socket, tokio_tungstenite::tungstenite::Error> {
    let mut request = format!("ws://127.0.0.1:{port}{path}").into_client_request().unwrap();
    request.headers_mut().insert("origin", origin.parse().unwrap());
    tokio_tungstenite::connect_async(request).await.map(|(socket, _)| socket)
}

fn ours(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// What arrives until the terminal has drawn `want`: the terminal's bytes, and the control messages.
async fn until(socket: &mut Socket, want: &str) -> (String, Vec<serde_json::Value>) {
    let mut drawn = String::new();
    let mut control = Vec::new();
    let read = async {
        while !drawn.contains(want) {
            match socket.next().await {
                Some(Ok(Message::Binary(bytes))) => drawn.push_str(&String::from_utf8_lossy(&bytes)),
                Some(Ok(Message::Text(text))) => control.push(serde_json::from_str(text.as_str()).unwrap()),
                Some(Ok(_)) => {}
                other => panic!("the socket ended before {want:?}: {other:?}\ndrawn: {drawn:?}\ncontrol: {control:?}"),
            }
        }
    };
    if tokio::time::timeout(Duration::from_secs(20), read).await.is_err() {
        panic!("never drew {want:?}\ndrawn: {drawn:?}\ncontrol: {control:?}");
    }
    (drawn, control)
}

/// The control message that says the chat ended, skipping whatever is drawn first.
async fn ended(socket: &mut Socket) -> serde_json::Value {
    let read = async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let value: serde_json::Value = serde_json::from_str(text.as_str()).unwrap();
                    if value.get("ended").is_some() {
                        return value;
                    }
                }
                Some(Ok(_)) => {}
                other => panic!("the socket ended without saying so: {other:?}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), read).await.expect("the chat never ended")
}

fn post(port: u16, path: &str) -> String {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}

fn get(port: u16, path: &str) -> serde_json::Value {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let body = answer.split_once("\r\n\r\n").unwrap().1;
    serde_json::from_str(body).unwrap_or_else(|_| panic!("{answer}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_in_the_page_is_otto_chat_on_a_terminal_and_ends_in_the_journal() {
    let s = Scratch::new();
    let id = s.run("asked");
    let (mut server, port) = s.serve();

    let path = format!("/api/runs/{id}/chat?about=change%201%2F2&cols=100&rows=30");
    let mut page = open(port, &path, &ours(port)).await.expect("the page opens a chat");
    let (drawn, control) = until(&mut page, &format!("stub-ready chat={id}")).await;
    assert_eq!(control[0]["chat"]["run"], id.as_str(), "{control:?}");
    assert_eq!(control[0]["chat"]["about"], "change 1/2");
    // `otto chat` itself started it — as a chat of this run, with what the page was showing in
    // its opening — and journaled that it did.
    assert!(drawn.contains("about-seen"), "{drawn}");
    assert!(s.journal(&id).contains("\"chat-started\""), "{}", s.journal(&id));

    page.send(Message::Binary(b"hello there\n".to_vec().into())).await.unwrap();
    until(&mut page, "heard:hello there").await;

    // Another tab: the same chat, with what it has drawn so far.
    let mut other = open(port, &format!("/api/runs/{id}/chat"), &ours(port)).await.unwrap();
    let (replayed, _) = until(&mut other, "heard:hello there").await;
    assert!(replayed.contains("stub-ready"), "{replayed}");
    let live = get(port, "/api/chats");
    assert_eq!(live[0]["run"], id.as_str(), "{live}");
    assert_eq!(live[0]["attached"], 2, "{live}");

    // Another website can't open one.
    let refused = open(port, &format!("/api/runs/{id}/chat"), "http://evil.test").await;
    assert!(refused.is_err(), "a foreign page opened a chat");

    // End chat: both tabs are told, and otto chat journaled the end.
    let answer = post(port, &format!("/api/runs/{id}/chat/end"));
    assert!(answer.starts_with("HTTP/1.1 200") && answer.contains("\"ended\":true"), "{answer}");
    ended(&mut page).await;
    ended(&mut other).await;
    let journal = s.journal(&id);
    assert!(journal.contains("\"chat-ended\""), "a hung-up chat still journals its end: {journal}");
    assert_eq!(get(port, "/api/chats"), serde_json::json!([]));

    server.kill().unwrap();
    server.wait().unwrap();
}

/// The server stopping — Ctrl-C, or launchd stopping the service — ends each chat properly first.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_stops_ends_its_chats_in_the_journal() {
    let s = Scratch::new();
    let id = s.run("left-open");
    let (mut server, port) = s.serve();
    let mut page = open(port, &format!("/api/runs/{id}/chat"), &ours(port)).await.unwrap();
    until(&mut page, "stub-ready").await;
    drop(page); // detached: a closed tab doesn't end it
    assert_eq!(get(port, "/api/chats")[0]["attached"], 0);

    unsafe {
        libc::kill(server.id() as libc::pid_t, libc::SIGTERM);
    }
    let status = server.wait().unwrap();
    assert!(status.success(), "{status:?}");
    let journal = s.journal(&id);
    assert!(journal.contains("\"chat-ended\""), "{journal}");
}

/// A run that isn't there gets a word, not a hang.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_about_no_run_says_why() {
    let s = Scratch::new();
    let (mut server, port) = s.serve();
    let mut page = open(port, "/api/runs/nothing-here/chat", &ours(port)).await.unwrap();
    let said = tokio::time::timeout(Duration::from_secs(10), page.next()).await.unwrap().unwrap().unwrap();
    let said: serde_json::Value = serde_json::from_str(said.to_text().unwrap()).unwrap();
    assert!(said["error"].as_str().is_some_and(|e| !e.is_empty()), "{said}");
    server.kill().unwrap();
    server.wait().unwrap();
}
