# nap-flow — the timer smoke test

**Version** 1 · **First phase** `nap`
**Facts** `tickSeconds` (default 120), `ticksRequired` (default 3), `ticksDone`
**Policy** `maxTicksWithoutProgress` (default 24)

Sleeps, wakes, and does nothing useful `ticksRequired` times, then finishes. Its job is to
prove the parts of the engine that only fail *over time*: a chained one-shot cron, a
`nextWakeAt` the reviver can act on, tick accounting, and survival of `kill -9` and a reboot.

Every tick is deliberately trivial — one `otto state tick`, no `git`, no `gh`, no subagent —
because a tick that costs real tokens is a workflow that cannot be left running for a week.
Keep it that way. `tests/live_revive.rs` drives this workflow with `tickSeconds=120`.

## Phase 0 — `nap`

| | |
|---|---|
| **goal** | Sleep for `tickSeconds`, wake, account for the tick, and repeat until `ticksDone == ticksRequired` |
| **preconditions** | `facts.ticksRequired` is set; `facts.ticksDone` is an integer (0 at intake) |
| **actions** | See the tick sequence below |
| **durableOutput** | `facts.ticksDone` incremented, and one `noop-tick` line per tick in the journal |
| **exitCondition** | `facts.ticksDone >= facts.ticksRequired` |
| **gate** | `sleep(tickSeconds, until: ticksDone == ticksRequired)` |
| **idempotency** | `ticksDone` on disk is the count, so a re-entry after a crash resumes at the right tick and can never double-count one. A wake that finds `ticksDone` already satisfied goes straight to `wake` |

**Each tick, in this order.** The order is the point: the state that survives is written before
the thing that might not happen.

1. (nothing to do — otto records the wake itself). The reviver compares this against
   `nextWakeAt` to tell a missed tick from one in progress.
2. Reconcile: nothing external to re-derive in this workflow. A real workflow does its
   `gh pr view` here.
3. `otto state record-fact <id> ticksDone=<n+1>` then `otto state tick <id>` (no `--progress`:
   nothing was achieved, which is the honest signal). Read the JSON it prints; if
   `exhausted` is true, `set-status --status blocked` and open a **human** gate instead of
   sleeping again.
4. If `ticksDone >= ticksRequired` → `set-phase <id> --phase wake` and continue to phase 1 in
   this same turn. Otherwise re-arm:
   - `otto state arm-timer <id> --in <tickSeconds>` — writes `nextWakeAt` and
     prints the matching one-shot cron expression.
   - Stop. Say which tick this was and when the next one is due.

## Phase 1 — `wake`

| | |
|---|---|
| **goal** | Record how the nap actually went and close the run out |
| **preconditions** | `facts.ticksDone >= facts.ticksRequired` |
| **actions** | Write `artifacts/nap-report.md`: ticks required vs done, each tick's timestamp from the journal, any `revived` entries (a `kill -9` or reboot recovery), and total elapsed; rewrite `handoff.md`; `set-status --status done` |
| **durableOutput** | `artifacts/nap-report.md` |
| **exitCondition** | `status` is `done` |
| **gate** | `continue` → report the path and stop |
| **idempotency** | If the report exists, adopt it and just ensure `status` is `done` |

## What a green run looks like

`journal.jsonl`: `run-created`, `phase-changed`(→nap), then per tick a `timer-armed` (carrying
its `cron`) and a `noop-tick` (carrying `ticksWithoutProgress`), then `phase-changed`(→wake)
and `status-changed`(→done). A run that was killed mid-nap also carries one `revived` line per
recovery — and its tick count still lands exactly on `ticksRequired`, because the count lives
on disk rather than in a session's memory.
