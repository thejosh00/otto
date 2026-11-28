# Plan: chat in the web page

**Status:** not started. Built interactively, outside `otto-driver`; `plans/v3.md` carries on beside it.
**Written:** 2026-11-28, against `61313af`. Line references go stale, so check them against the code.

## What and why

A **Chat** button in the top nav opens a resizable panel on the right of every page. Typing in it
talks to `otto chat` about whatever the page is showing, so "what is this working on?" on an agent
page means that agent.

DESIGN.md §7 keeps chat out of the web, for two reasons that still hold:

1. **No `-p`.** A page can only drive claude through print mode, and print mode bills SDK credits
   instead of the subscription (`src/wake/launcher.rs` module doc).
2. **Approvals need a person at a prompt.** A chat runs in `manual` mode; everything but otto's own
   reads asks first, and approving `otto note` is how the person agrees to a note's wording.

So the panel is **a terminal, in the web**: xterm.js in the page, a websocket to `otto serve`, and
on the server a pseudo-terminal running the same `otto chat <run>` a person would type. claude sees
a real terminal, so billing, approval prompts, the read-only guards, notes as the only way out, and
the `chat-started` / `chat-ended` journal entries are all unchanged. Nothing about what a chat *is*
changes; only where its terminal lives.

## Decisions

- **The child is `otto chat`, not claude.** The server spawns its own executable as
  `otto chat <run> [--about …]` on the pty. All of `chat.rs` (sandbox check, opening, argv,
  journaling) is reused, and `attended` comes out true because stdin and stdout are a tty.
- **A chat is about one run, for its whole life.** Its working directory, launcher and sandbox are
  that run's. The panel header names the run. Moving to another page leaves the chat where it is
  and offers "Start one about *that* run?".
- **The page decides the run, and adds what you're looking at.**

  | Page | Run | `--about` |
  |---|---|---|
  | `#/run/<id>` | `<id>` | — |
  | `#/run/<id>/work/<w>[/v<n>]` | `<id>` | `work item <w>[, version <n>]` |
  | `#/inbox/<id>/<q…|m…>` | `<id>` | `inbox item <q…|m…>` |
  | `#/changes/<id>/<n>` | `<id>` | `change <id>/<n>` |
  | anything else | none | button opens the panel with "Open an agent to ask about it" |

- **One live chat per run, kept by the server.** Closing the panel or reloading the page detaches;
  it doesn't end the chat. Reattaching replays the scrollback. A chat ends on `/exit`, the panel's
  **End chat** button, the server stopping, or 30 minutes with nobody attached.
- **The server now holds something.** `src/server/mod.rs` says it holds no state. That stays true
  of runs; it now also holds live chats, which journal themselves, so losing them loses a
  conversation and nothing about any run.
- **No new crate for the pty.** `libc::openpty` + `setsid` + `TIOCSCTTY` in `pre_exec`, the master
  fd under `tokio::io::unix::AsyncFd`, as `live_tests.rs` and `landing/install.rs` already use
  `setsid`. axum gains its `ws` feature.
- **xterm.js is vendored**, compiled in like everything else in `web/` (MIT; licence beside it),
  set in the bundled JetBrains Mono.

## Steps

Each step is one or more commits to `main`, tests passing, pushed after rebasing on whatever the
lander has landed.

### 1. `otto chat --about` and surviving hang-up — S

- `ChatArgs` gains `--about <text>`: one line in the opening message, "The person is looking at
  <text>." (`Opening::text`).
- `otto chat` ignores SIGHUP as it ignores SIGINT (`IgnoreInterrupt`). When the pty closes, the
  hang-up reaches otto as session leader; without this it dies and `chat-ended` is never written.
  claude still gets the hang-up, exits, and otto journals the end.
- Tests: the opening carries `--about`; `--dry-run` shows it. `docs/reference/cli.md`.

### 2. A pty that runs a command — S/M

`src/server/pty.rs`: open a pty, spawn a command on it with a size, read output as a stream, write
input, resize (`TIOCSWINSZ`), hang up (close the master, then SIGHUP the process group if it
lingers), reap. Unit tests against `/bin/sh -c` (echo, `stty size` after resize, exit on hang-up).

### 3. Live chats on the server — M

- `src/server/chat.rs`: a registry keyed by run id. Each entry: the pty, a ring buffer of the last
  256 KB of output, the `--about` it started with, attached sockets, last-detached time.
- Routes:
  - `GET /api/runs/{id}/chat` — websocket upgrade. Starts the chat if none is live (`?about=`,
    `?cols=&rows=`), else attaches and replays the buffer. Binary frames are terminal bytes both
    ways; text frames are JSON control (`{"resize":[c,r]}` in; `{"ended":…}` out).
  - `POST /api/runs/{id}/chat/end` — hang it up.
  - `GET /api/chats` — which runs have a live chat (for the nav dot).
- **Guard:** a websocket upgrade is a GET, and `check_request` lets GETs through without looking
  at Origin. Browsers don't apply CORS to websockets, so any site could otherwise open one to
  127.0.0.1 and type into a chat. Upgrades must carry an Origin, and it must be this page. Test in
  `mod.rs` beside the existing guard tests.
- Idle reaper: a tokio task that ends chats detached for 30 minutes. Server shutdown hangs up every
  chat.
- Tests: router-level tests with a stub command in place of `otto chat` (start, reattach replays,
  end, second socket shares the session, cross-origin upgrade refused).

### 4. The panel — M

- `web/vendor/xterm.js`, `xterm.css`, `addon-fit.js` and their licence, served from `assets.rs`.
- Nav: a **Chat** button by the inbox button, with a dot while any chat is live (`/api/chats`).
- Panel: fixed on the right, drag handle on its left edge, width kept with `store()`; an overlay
  under ~900px wide. Header: "Asking about <run>" (+ the `--about`), **End chat**, close.
  Shortcut to toggle it.
- Context: `route()` already parses every hash above; a `chatContext()` beside it returns
  `{run, about}` or null. Opening the panel attaches to that run's live chat or starts one;
  changing page with a chat open shows the "start one about this run?" bar when the run differs.
- On the agent page, "Ask it something" (`renderChat`) becomes an **Open chat** button, with the
  command still there to copy for a real terminal.
- Checked by hand in the browser: start, approve a note, reload mid-chat, `/exit`, End chat.

### 5. Docs — S

- DESIGN.md §7: "A terminal, not the web" becomes "A terminal, in the web" — why not `-p`, why a
  pty, a chat per run held by the server, the Origin rule for upgrades.
- `src/server/mod.rs` module doc (what the server holds now), ARCHITECTURE.md (the server's row:
  it now spawns `otto chat` on a pty), README, `docs/guides/everyday.md`.

### 6. Live test — S, optional

`tests/live_web_chat.rs`: start `otto serve` on a free port, open the websocket, ask one question
of a real run, see an answer and `chat-ended` in the journal after End chat.

## Risks

- **A pty endpoint runs commands as the person.** It is no worse than the POSTs that start wakes,
  as long as the Origin rule on upgrades holds; step 3's test is the guard.
- **v3 Phase 4 reshapes the kernel and apps.** If it moves `src/server` while this is in flight,
  rebase onto it rather than around it.
- **claude's TUI in xterm.js.** Expected to work (xterm.js is what VS Code's terminal is), but key
  handling and resize are checked by hand in step 4, not assumed.
