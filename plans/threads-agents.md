# Plan: threads as otto agents, and chats that can record

**Status:** steps 8 and 9 checked (2026-11-28); nothing built yet. Built interactively, outside `otto-driver`; `plans/v3.md` carries on beside it.
**Written:** 2026-11-28, against `bfb3a32`. Line references go stale, so check them against the code.

## What and why

`~/workspace/threads` keeps one folder per open loop at work: `SUMMARY.md`, `log.md`, `refs.md`,
`decisions.md`, `open.md`, and a `CLAUDE.md` with the rules for keeping them. Sessions start cold
and re-read the folder, so a thread is already a run's state in all but name.

The person wants each thread to be an otto agent that:

1. **checks for news on its own** — the Slack threads, tickets and PRs in its `refs.md` — updates
   its files, and tells them when something needs them; and
2. **takes updates in a chat** — "met Priya, we're going with option B" — and records them in the
   thread's files then and there.

The first fits otto as it is, with two small gaps. The second doesn't: a chat today changes
nothing (DESIGN.md §7), deliberately. This plan adds **chats that may edit one declared folder**,
and keeps each reason chats were read-only answered another way.

## Decisions

- **One run per thread.** Perpetual; its working directory is the thread's folder, and that folder
  is its write grant. "Chat about this thread" is then just a chat about its run.
- **No sandbox for now.** Wakes have the person's MCP servers (Slack and the rest) as any claude
  session does. The guards below are backstops, not walls: an unsandboxed wake can already write
  anywhere, and so can a chat's Bash once the person approves the command. Sandboxing threads runs
  later needs an MCP allowlist (step 9's note).
- **The thread folder stays in the threads repo**, not in the run directory, so `cd thread &&
  claude`, the threads skills and its git history keep working.

### Why a chat couldn't write, and what answers each reason now

| Reason (DESIGN.md §7) | Answered by |
|---|---|
| No repo lock: a chat and a wake could edit the same files | The chat holds the run's wake lock while it is open (step 3) |
| No handoff: the next wake can't see why the world moved | otto diffs the folder at the chat's end and leaves the next wake a note naming the diff (steps 4, 5) |
| No authorization on record | The person declares the folder, outside `run.json` (step 1), and the diff and its undo are on record |
| "Just fix it" could reach anything | Edits outside the folder are refused by a hook; Bash still asks for every command (step 2) |

## Steps

Each step is one or more commits to `main`, tests passing, pushed after rebasing.

### 1. A folder a run's chats may edit — S

- `otto chat-edits <run> [<dir>]… [--clear]` — set, show or clear it. Kept at
  `~/.otto/chat-edits/<id>.json`, the person's, beside `approvals/`; never in `run.json`, which a
  wake rewrites, so a run can't widen what its own chats may touch once runs are sandboxed.
- Refused: a path that doesn't exist or isn't a directory; `/`, `$HOME` itself, anything under
  `~/.otto` or `~/.claude`; a folder over the snapshot bound (say 2,000 files or 20 MB, `.git`
  not counted).
- Paths stored canonical. `otto show` and the agent page say what a chat may edit.

### 2. The chat may edit there and nowhere else — M

- `launcher::chat_argv`, for a run with a folder:
  - Edit, Write and NotebookEdit come off `REFUSED_TOOLS` for it; `Edit(//<dir>/**)` and
    `Write(//<dir>/**)` join `--allowed-tools`, so edits there don't ask. (Verify claude's
    absolute-path rule syntax against a real chat; step 10.)
  - A PreToolUse hook passed with `--settings`: `otto chat-guard`, which reads the tool call,
    resolves `file_path` without following links, and denies it, with why, unless it is inside the
    folder. The allow rule makes edits quiet; the hook is what refuses.
  - `OTTO_CHAT` still makes `otto state` refuse every write.
- `harness/chat.md` gains a section for a chat that may edit: record what the person says in the
  folder by the folder's own rules (its `CLAUDE.md`); everything about how the run works is still a
  note; say what you changed. The opening message names the folder.

### 3. A chat that may edit holds the run's wake lock — S/M

- `otto chat` takes the wake lock (`liveness::WakeLock::acquire`) for its whole life when the run
  has a folder; `wake-holder.json` says it is a chat. Poke already skips a run whose lock is taken,
  so a wake due during the chat runs after it. flock is released on exit, so a killed chat holds
  nothing.
- A wake already working: the chat opens read-only, its folder not offered, and says so.
- `otto note --now` from inside such a chat can't start a wake against its own lock: it says the
  note will be read when the chat ends, and poke wakes the run then.
- `otto ls`, `otto show` and the agent page say "a chat is open; its wakes wait".
- `otto chat --read-only` for a chat that only asks, and so holds nothing.

### 4. Snapshot, diff and undo — M

- At the start: the folder (not `.git`) copied to `runs/<id>/chats/<n>/before/`.
- At the end: the files that changed, a unified diff at `runs/<id>/chats/<n>.diff`, and a hash of
  each file as left. `chat-ended` gains `changed: [...]`.
- `otto chat undo <run> [<n>]` puts `before/` back, refused if any file has changed since that
  chat left it (a wake, or another chat, has built on it).
- Keep the last 20 chats' snapshots per run.

### 5. The next wake is told — S

When a chat changed anything, otto leaves the run a note itself, `from: chat`, `about` the files:
"In a chat at 14:02 the person changed SUMMARY.md and decisions.md; the diff is
`chats/3.diff`. Re-read them before relying on what you knew." Notes are already delivered
at least once, so the next wake can't miss it.

### 6. The web panel — S/M

- The panel's header: "can edit: threads/srx-subagent", or "read-only: a wake is working".
- When the chat ends: "changed 3 files · View diff · Undo" (`GET /api/runs/{id}/chats/{n}/diff`,
  `POST …/undo`, under the guard like every POST).
- The agent page: the folder, and a way to set or clear it.

### 7. Hours for periods shorter than a day — S

`--on` and `--at` apply to whole-day periods only. Add `--hours 9-18` and allow `--on` with any
period: `otto period <run> 1h --on mon-fri --hours 9-18`. A wake due outside them waits for the
next opening.

### 8. Reading Slack, never posting — none, checked

Already configuration: `--deny-tool` with the Slack server's posting tools, and `otto state publish
--draft-file` for a reply the person sends themselves. **Checked:** `--deny-tool
mcp__blender__execute_blender_code` left exactly that tool out of a real wake's tool list (its
transcript, not its word). The names to deny depend on how Slack is connected at work: a local
server's tools are `mcp__<server>__<tool>`, a claude.ai connector's `mcp__claude_ai_<Name>__<tool>`
— list them from a wake there before writing the deny list.

### 9. MCP in a non-print wake — checked

**Checked (DESIGN.md §19, question 5):** an unsandboxed wake started without `-p` had the person's
user-scope MCP server (Blender) and their claude.ai connectors, and its one call went through. So
Slack reaches a thread run if it is configured at user scope, or as a claude.ai connector — not if
it is configured for one project directory other than the thread's. Confirm on the work machine
with the first thread run. (Sandboxed runs
get none today, `docs/guides/sandboxing.md`; sandboxing threads later needs a per-run MCP
allowlist passed with `--mcp-config`, and network for it.)

### 10. Tests and docs — M

- Unit: the record's refusals; the guard's in, out, `..` and link cases; the lock against a
  running wake; snapshot, diff, undo and undo refused; the note.
- `tests/web_chat.rs`: a stand-in that writes inside and outside the folder through the hook.
- A live test: a real chat told an update writes it into the folder, is refused outside it, and
  the next wake gets the note.
- DESIGN.md §7: a chat changes nothing *except* the run's declared folder, and on these terms.
  ARCHITECTURE.md: the actor row for a chat, an invariant per guard. Concepts and the everyday
  guide.

## Changes on the threads side (the person's, later)

- **`INDEX.md` written by every thread.** One run per thread means every run edits it, and it is
  outside every chat's folder. Generate it from the `SUMMARY.md` files (a script, or `/threads`)
  instead.
- **A watch workflow**: what a wake checks (the refs in `refs.md`), a "seen up to here" ledger per
  source, what is worth an inbox message, and that nothing is ever posted.
- **Plain `cd thread && claude` sessions don't take otto's lock.** For a thread with an agent, chat
  through otto; or the threads hook could check `otto state locks` and warn.
- **The Stop hook runs in wakes and chats too** (claude reads the person's `settings.json`). Wanted
  in chats; in wakes it adds a turn when refs are unfiled, which is fine, but it must not block a
  wake's handoff turn indefinitely — `stop_hook_active` already prevents a loop.

## Risks

- **Unsandboxed wakes read Slack as the person.** A message can carry instructions. The deny list
  stops the posting tools; it doesn't stop Bash. That is the case for sandboxing these runs later.
- **Cost.** A cold wake is about $0.38 (DESIGN.md §12). Three threads hourly, ten hours a day, is
  about $11 a day. Longer periods, or a check script once Slack can be read without a model.
- **A chat left open holds back its run's wakes.** The web panel ends chats after 30 minutes with
  nobody attached; a terminal chat holds the lock until it is closed, and `otto show` says so.
