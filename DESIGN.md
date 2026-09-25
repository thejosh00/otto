# Otto — durable long-horizon agent runs

> **v2.** Supersedes the workflow-host design. v1 is preserved in git history; §20 keeps the
> decisions from it that are still load-bearing. The one-sentence diff: **otto no longer
> contains workflows. It makes anything survive days.**

## 1. What otto is for

Take a thing an agent knows how to do — an existing Claude Code skill, a runbook, a prose
instruction, or just a goal — and run it for days without a session that has to stay alive.

Carry a Jira ticket to merged. Babysit a PR through a week of review. Propose one improvement a
day, forever. Grind twenty repos through a dependency bump. Otto contributes none of the doing.
It contributes **durability**: state on disk, human gates, timers that survive a reboot, and a
mechanically enforced guarantee that every stopping point can be resumed cold.

The premise, stated so it can be falsified: **the thing being wrapped needs to know nothing
about otto.** §10 is how that works and where it frays; §11 is what you gain by writing *for*
otto on purpose, as `dev-flow` does.

## 2. Why nothing else spans it

| Mechanism | Lifetime | Why it isn't enough |
|---|---|---|
| A session's context | hours | Compacts; days of coding plus polling exhausts even 1M |
| `CronCreate` | one session | Jobs live only in that session; recurring ones expire at 7 days; fire only while the REPL idles |
| `ScheduleWakeup` | one session | Clamped to ≤1h; dies with the session |
| `Workflow` script | one invocation | Deterministic fan-out, no human gates, no multi-day suspend |
| `claude --bg` | until it exits or the machine reboots | Real session management (`attach`/`logs`/`stop`/`rm`), but no durable state, no gates, no revival after a reboot |
| A skill's own prose | one session, implicitly | The gap otto fills. Nearly every skill assumes continuous context and never says so |

The last row is the whole opportunity. There are hundreds of skills. Most were written for one
sitting, and *silently* assume it — which is exactly the assumption that breaks at hour six.

### Non-negotiables

1. **A run is driven from the CLI.** `otto run …` from a shell, not `/otto start` typed into a
   session. Gates are answered with `otto answer`. You can watch a wake in tmux if you want to;
   nothing requires you to.
2. **No session outlives a wake.** One wake, one process, and it exits.
3. **Human gates are questions with answers on disk.** No Slack, no inbox, no side channel, and
   no terminal that has to be read to notice one is open.

## 3. The three principles

> **The session holds the conversation. The run directory holds the truth.**

A session that lives for days is convenient; a session that is *irreplaceable* is a bug. In v1
that was an aspiration enforced by prose. In v2 no session lives longer than one wake, so it is
structurally true: anything not written down is gone within the hour, by design.

> **Re-derive, don't remember.**

Every wake begins by re-reading the world — git, `gh`, the ticket — never by trusting
recollection. Between two wakes master moved, CI reran, someone force-pushed, a reviewer
resolved their own comment.

> **The wake is the unit, and the contract is what otto enforces.**

Otto does not validate phases, plans or progress; with an arbitrary skill there is no table to
validate against. It validates exactly one thing, at exactly one moment — process exit (§5.2).
Everything else is guidance. Guidance can be ignored; a validator cannot.

## 4. Architecture

```
  you ──── otto run / ls / show / answer / logs / stop ──── the whole interface
   │                                                              │
   │  (optional) tmux attach · claude logs · a session manager     │
   ▼                                                              ▼
  ┌──────────────────────────────────────────────────────────────────┐
  │ otto wake <id>          ← one process, one wake, then gone        │
  │                                                                  │
  │   write wake-started, deadlineAt                                 │
  │   exec  caffeinate claude <harness prompt> --session-id <uuid>    │
  │           ├─ orient:  run.json · handoff.md · journal tail        │
  │           ├─ load:    the wrapped skill / instructions / goal     │
  │           ├─ work:    re-derive, act, delegate to subagents       │
  │           └─ stop at: gate | sleep | done                         │
  │   VALIDATE the contract  →  wake-complete | wake-incomplete       │
  └───┬──────────────────────────────────────────────────────────────┘
      │ reads/writes
      ▼
  ~/.otto/runs/<run-id>/
    run.json        ← machine truth, rewritten atomically
    handoff.md      ← the ONLY thing the next wake gets for free (capped)
    journal.jsonl   ← append-only, never rewritten
    gates/NNN-*.md  ← question + verbatim answer
    notes/NNN.md    ← a person's note to the run, verbatim (§7)
    artifacts/      ← plan.md, ledger.md, test-report.md, …
      ▲
      │ spawn due · reap exited · kill past deadline
  otto poke (launchd, ~5 min) ── makes no decision about the work
```

Two components, and the second one is 40 lines of judgement-free bookkeeping. There is no
daemon that owns a run, no conductor that has to stay alive, and nothing that reads a terminal
to find out what is going on.

## 5. The wake

### 5.1 Lifecycle

A wake is one process. It rehydrates, works until it reaches a legal stopping state, writes its
handoff, and exits. Three things cause one:

| Cause | Path |
|---|---|
| `nextWakeAt` passed | `otto poke` spawns it |
| A human answered a gate | `otto answer` spawns it (`--no-wake` to defer to poke) |
| You asked | `otto wake <id>` |

**A wake is not one model turn.** It runs as long as the work runs — several turns, a chain of
phases, subagents — and ends only at a gate, a sleep, or completion. Ephemerality is about the
*session*, not about slicing the work: a wake pays rehydration once, so a ten-step chain inside
one wake costs one cold start, not ten. The rule "stop only at a gate" is about never leaving a
run with nothing to wake it (§7), not about yielding between steps.

**Execution is non-interactive, but not `-p`.** Print mode bills SDK credits rather than the
subscription, which is the wrong budget for an unattended run that wakes forever, so otto does
not pass it. A wake is still non-interactive without it: given stdin at `/dev/null` and stdout on
a pipe, `claude '<prompt>'` runs the prompt and exits on its own. Nothing drives a TUI, types
into a composer, or has to kill anything at the end — the parent spawns, waits, and reads the
exit code exactly as before.

What `-p` took with it was the measurement. `--output-format json` only works with `--print`, and
that document was where cost, turn count and permission denials came from. The replacement is the
session transcript: otto passes `--session-id`, so afterwards it can read
`~/.claude/projects/<cwd-slug>/<id>.jsonl` for real token counts. Dollars are simply not in there,
and on a subscription there is no per-wake dollar figure to find — see §6.

Because nobody is attached, no permission prompt can ever be answered: the run's permission
posture is decided once, at `otto run`, recorded in `run.json`, and passed as
`--permission-mode` / `--allowed-tools` / `--disallowed-tools`. That is a real security
decision and it is explicit on purpose.

**Detachment is a strategy, not the design.** `otto wake <id>` is the unit; how it gets
backgrounded is swappable, and every entry point agrees on it — `otto run`, `otto answer`,
`otto wake` and `otto poke` all resolve the run's recorded `detach`, so where a run's wakes go is a
property of the run rather than of the command you happened to type. (`otto wake --watch` overrides
it for one wake without rewriting the run's preference, and the child `spawn_background` starts is
told `--foreground` — otherwise it would resolve the same preference again and background itself
inside the container just made for it, forever.)

How it gets backgrounded:

| Strategy | Why |
|---|---|
| tmux `otto-<run-id>` *(default)* | otto names the session, any launcher works via `--command`, and a session manager lists it beside everything else |
| `claude --bg` + `claude attach/logs/stop` | No tmux dependency — which matters because tmux is denied inside most sandboxed Claude Code environments, so this is the strategy that makes otto testable from inside one |
| Foreground | Debugging, and the first wake of `otto run` when you want to watch it — `--watch` on either `run` or `wake` |

**Otto never `--resume`s.** The flag exists, and using it would give cross-wake context reuse —
along with the unbounded context growth and compaction that v1 spent its whole design budget
defending against. Every wake starts cold. That is the cost being paid for the guarantee, and
§12 is how the cost is kept down.

### 5.2 The contract

At process exit, `otto wake` checks two things and nothing else:

1. **The run is in a legal stopping state:**

| Stop state | What brings it back |
|---|---|
| `awaiting_human`, gate open | `otto answer`, or the gate's expiry |
| `sleeping`, `nextWakeAt` set | poke |
| `running`, nothing pending, clean exit, fresh handoff | the parent sleeps it for `policy.periodMinutes` (§8), then poke |
| `blocked`, gate open | a human |
| `done` / `failed` / `stopped` | nothing; the run is over |

2. **`handoff.md` was written by this wake** (mtime ≥ `wake.startedAt`) and is within its cap.

Pass → `wake-complete`. Fail, for any reason including a crash, a kill at deadline, or a model
that simply stopped talking → **`wake-incomplete`**: increment `incompleteWakes`, arm
`nextWakeAt` with backoff, and let the next wake re-enter cold. Consecutive failures past
`policy.maxIncompleteWakes` → `blocked` plus a human gate carrying what the wakes were doing.

Every way into `blocked` records why — `run.json`'s `blocked: {cause, detail, at}`, with `cause`
one of `wake-failures`, `budget`, `stall`, `instructions` — because the remedy depends on it:
the logs and a retried wake for failures, a plain answer to the gate for a stall where nothing
failed. otto's own paths set it; a wake setting `blocked` itself says `--because`, or otto infers
`stall` from the tick counter. `transaction` clears the record whenever the status is no longer
`blocked`, so it can never describe a state the run has left.

Two checks, because two are enforceable against anything. A validator that also demanded a
declared `exitCondition` would only work on skills written for otto, and then the premise in §1
would be false.

**The stall v1 hunted is now unrepresentable.** v1's worst failure was a conductor that ended a
turn `running` with no gate and no wake time: nothing waiting, nothing scheduled, status still
`running`, and nobody notices for a day. It took prose, a table of legal stop states, and a
`poke` heuristic to defend against. In v2 that state ends a wake, so the validator catches it
immediately and turns it into a retry.

### 5.3 What a wake may not do

- **Hand-edit `run.json`, `journal.jsonl`, or a gate file.** Every write goes through
  `otto state` (atomic write + journal append + per-run lock). An agent hand-editing JSON across
  days will eventually corrupt it, and the corruption surfaces three days later.
- **Exceed its deadline.** `wake.deadlineAt` is written at spawn; poke kills past it. A killed
  wake is `wake-incomplete`, never a clean exit — reaping a finished session is free, and
  pretending a killed one finished is how work silently disappears.
- **Assume anything from the last wake except `handoff.md` and disk.** There is nothing else.

## 6. State model

```
~/.otto/runs/2026-09-12-nr-41022/        ($OTTO_HOME overrides ~/.otto)
  run.json          machine truth
  handoff.md        what the next wake gets for free — capped, rewritten every wake
  journal.jsonl     append-only
  gates/001-plan-review.md
  notes/001.md
  artifacts/        plan.md · ledger.md · test-report.md · cycles/<date>/…
```

```jsonc
{
  "schemaVersion": 2,
  "id": "2026-09-12-nr-41022",
  "wraps": { "kind": "skill", "ref": "nr:handle-trivy-tickets-skill" },
  "goal": "…verbatim from --goal; never paraphrased, never rewritten…",
  "doneCondition": "…what makes this run finished (§10.2)…",
  "status": "awaiting_human",     // running | awaiting_human | sleeping | blocked
                                  // | done | failed | stopped
  "phase": "implement",           // a free-form label the wake sets, for humans.
                                  // The engine does NOT validate it — with an arbitrary
                                  // skill there is no table to validate against.
  "gate": { "id": "001", "file": "gates/001-plan-review.md", "askedAt": "…",
            "expiresAt": null, "answeredAt": null },
  "nextWakeAt": null,
  "wake": { "n": 14, "startedAt": "…", "deadlineAt": "…",
            "detach": "tmux", "session": "otto-2026-09-12-nr-41022",
            "pid": 51234, "outcome": null },
  "incompleteWakes": 0,
  "budget":  { "wakes": 200, "hours": 336, "usd": 40,
               "spentWakes": 14, "spentUsd": 3.12 },
  "permission": { "mode": "acceptEdits",
                  "allowedTools": ["Bash(git *)", "Edit", "Read"],
                  "disallowedTools": [] },
  "policy": { "autoMergeWhenGreen": false,
              "gateStaleAfterHours": 48, "maxWakeMinutes": 45,
              "maxIncompleteWakes": 5, "maxTicksWithoutProgress": 24,
              "handoffMaxBytes": 8192 },
  "facts": { "repo": "/Users/joshuahill/workspace/foo", "base": "main",
             "branch": "nr-41022-bump", "prNumber": null, "prCommentCursor": null },
  "createdAt": "…", "updatedAt": "…"
}
```

### `facts` belongs to the wake; engine state does not go in it

A wake re-records the facts it read — that is the intended pattern, and §5.3 encourages batching
them — so anything otto keeps in `facts` will eventually be rewritten by a wake being helpful.
Observed exactly that while wrapping `manage-pr`: poke's `spawnAttempts` lived in `facts`, a wake
wrote `1` over otto's `0`, and poke's spawn backoff was silently corrupted. Poke's bookkeeping
(`spawnAttempts`, `lastSpawnedAt`) now sits at the top level of `run.json` beside `incompleteWakes`.

The rule generalizes, and it is worth stating as a rule: **`facts` is the wake's scratch space.
Engine state kept within reach of a wake is engine state a wake will overwrite.**

### `handoff.md` — one file, not two

v1 had `brief.md` ("rolling human-readable summary, ~40 lines") and would have grown a handoff
record beside it. v2 merges them, for the reason v1's own journal-hygiene rule gives about
duplicate log lines: *two accounts of the same event, and a week later nobody can tell which was
true.* One file, fixed sections, hard cap:

```markdown
# <run id> · <phase> · wake <n>
## Goal            — copied verbatim from run.json, never restated in other words
## Where this is   — 2–4 lines. What a person needs to not be lost
## Decided         — each decision and the gate that authorized it
## This wake did   — what actually changed on disk
## Next wake must  — the first concrete action, not a direction
## Needs           — the open question, the blocker, or "nothing"
## Don't re-derive — pointers to artifacts. Paths, not contents
```

**Rewritten, never appended**, and enforced at `policy.handoffMaxBytes` (8KB default) — the
write is refused past the cap rather than truncated, because truncation silently drops whatever
the wake put last, which is usually what mattered most.

The reason is orientation, not economy. This file is the whole of what the next wake knows before
it starts, so it has to be readable cold by a competent stranger; a handoff that has grown into a
history of the run buries the one thing that wake needs to do first. When it stops fitting, the
answer is a ledger artifact (§11.4) — the journal is the history.

*(An earlier draft claimed handoff size was "the dominant recurring cost in the whole system".
Measurement says otherwise — see §12 — and the cap is worth keeping on its own merits.)*

### Budget

`budget` has two dimensions, `wakes` and `hours`, and both are measured. At 80% of either:
journal it, and poke sends a desktop notification once (§7, Notifications). At 100%: `blocked` plus a human gate. An indefinite run
needs an outer bound.

**There is no dollar dimension.** Measuring per-wake cost needed `-p --output-format json`, and
`-p` bills the wrong budget (§4), so nothing left can price a wake — and on a subscription there
is no price to report. `--budget-usd` is therefore refused at `otto run` rather than accepted and
quietly ignored: a ceiling whose spend counter can only ever be zero reads as healthy right up to
the moment the real limit is blown past. Tokens *are* measured, from the transcript, and journaled
per wake as `inputTokens` / `outputTokens` / `cacheRead` / `cacheCreation`; they are reporting, not
a ceiling. The honest in-process bound on a runaway wake is `maxWakeMinutes`, which the parent has
always enforced itself.

## 7. Gates

**There is exactly one kind of gate: a human one.** v1 had a timer gate too, and then
`improve-flow` discovered it shouldn't be used — *"a timer gate would have to be closed and
reopened every cycle, 365 gate files a year documenting the same sentence."* v2 takes the hint:
waiting is a **status** (§8), not a gate. A gate is a question with an answer, and every gate
file has both.

```
wake:   write the question   → gates/NNN-<slug>.md
        otto state open-gate <id> --slug <slug> --question-file <f> [--expires-in S]
        → status: awaiting_human, then EXIT. The question is the last thing it does.
you:    otto show <run>                       # the question, cold-readable
        otto answer <run> --choice approve    # or --text "…" or --file notes.md
otto:   records the answer VERBATIM, clears the gate, spawns the next wake
```

**`AskUserQuestion` is gone, and with it a whole subsystem.** v1 *required* every human gate to
end inside that tool, because a session manager decided a session needed you by reading its
pane, and an open selection prompt was the only thing that read as "waiting". That forced: the
mandate itself, the "disk leads the pane by one action" subtlety, a captured-pane fixture, and a
tag contract with wrangler. In v2 the gate is a row in `otto ls` and a non-zero exit from
`otto answer --check`. Nothing reads a terminal to find out a run needs you.

**A gate question must be answerable cold.** You arrive eight hours later with no memory of the
transcript, and there is no transcript — the session is gone. So the gate file carries: two
sentences of where the run is, the specific decision, 2–4 concrete options *with consequences*, a
stated default, and paths to anything to read. "Does the plan look OK?" costs a round trip and
gets "yes", which is not a review.

Name the options, so `--choice approve` works and the answer is unambiguous a week later in the
journal. `--text` stays available for prose a review actually needs.

**Record verbatim before acting.** `close-gate` writes the answer into the gate file and journal
character-for-character. Paraphrasing a decision into the handoff and acting on the paraphrase
two days later is how runs drift from what was approved. Ambiguous answer → do not guess; open a
new gate citing what was unclear.

### Expiring gates

`--expires-in 79200` sets `nextWakeAt` to the deadline. Status stays `awaiting_human` and a late
answer still wins, but a wake comes to deal with the silence: if `expiresAt` has passed,
`close-gate --expired` records "no answer by …" and journals `gate-expired`.

**Only where silence has a sane meaning.** "No answer, so don't do it" is sane. "No answer, so
push it" is not. `--expired` refuses to close a gate that has no expiry, so the two cannot be
confused.

### Escalation

A gate open longer than `policy.gateStaleAfterHours` (default 48, `0` disables) → journal and
notify **once**.

### Notifications

A gate is useless if nobody knows it is open, so poke tells you. On every pass, after its spawn
decisions, it posts a macOS notification (`osascript`) for each of these, **once**:

| Key | When |
|---|---|
| `gate:<id>` | A gate is open |
| `blocked:<gate id>` | The run is `blocked` — replaces the gate's own notice, and names the cause |
| `gate-stale:<id>` | The gate has waited past `gateStaleAfterHours` |
| `budget` | A budget dimension is between 80% and 100% spent |

Poke sends them, not the wake, because a wake under yolo may not be able to reach the
notification centre, and because no wake is around to see a gate go stale. What was sent is kept
in `run.json`'s top-level `notified` (not `facts`, §6) and journaled as `notified`; a key that
no longer applies is dropped, so the next gate is news again. A failed send still counts as sent —
retrying a broken `osascript` every five minutes would only fill the journal.

### Notes

A gate is the run asking. A **note** is the person telling, whenever they like, gated or not:
`otto note <run> "skip the e2e suite, it's broken on main"`. Without one, the only way to steer a
sleeping run was to wait for it to ask, or to stop and resume it.

- **Verbatim, on disk first.** `notes/NNN.md`, `note-added` in the journal, and a `notes` entry in
  `run.json` — the same rule as a gate answer, for the same reason.
- **Delivered by the contract.** The next wake's prompt carries every pending note, and otto
  records which (`givenToWake`). Only that wake *completing* delivers a one-off note
  (`notes-delivered`, and it leaves `run.json`). A wake that crashes or is killed never completes,
  so the next wake is given the note again: at-least-once, with no ack for a wake to forget. A
  note added while a wake runs was never in its prompt, and waits for the next one.
- **Standing notes** (`--standing`) are given to every wake until `--drop`ped, for guidance that
  must hold for the life of the run. Otherwise it would live in the handoff, which the model
  rewrites every wake — a paraphrase every wake. Together they are capped at 4KB, like the handoff.
- **A note is not an answer and not an authorization.** It steers *how*; it never rewrites the
  goal, moves `doneCondition` (§10.2), or names a source for an outward action (§14). The harness
  tells a wake to open a gate quoting any note that asks for one of those. A note left while a gate
  is open waits for the wake after the answer, and `otto note` says so.
- **When.** By default the next wake that would happen anyway; `--now` starts one if nothing is
  running and no gate is open.

## 8. Waiting

`otto state arm-timer <id> --in 3600` sets `status: sleeping` and `nextWakeAt`. That is the
entire mechanism. Poke wakes it.

**Every run has a period** — `policy.periodMinutes`, set by `otto run --period` (default `1h`).
It is not a second mechanism, only a default for the first: when a wake exits 0, not killed, with
a freshly rewritten handoff, and leaves the run `running` with no gate and no `nextWakeAt`, the
parent writes `nextWakeAt = wake.startedAt + period` (never in the past) and sets `sleeping`.
Anchoring on the start keeps an hourly run hourly instead of drifting by each wake's length.
Poke is unchanged; it only ever reads `nextWakeAt`.

- `arm-timer --in`/`--at` is a one-shot override. Starting a wake clears `nextWakeAt`, so the
  override never outlives the sleep it was armed for. `arm-timer` with neither uses the period.
- A gate takes precedence: an open gate means no `nextWakeAt` is filled. The answer starts a
  wake, and that wake's clean exit puts the run back on its period.
- A crash, a kill, a non-zero exit or a stale handoff is never given the period. It stays the
  stranded run of §5.2 and gets the short backoff, so a broken wake cannot hide behind an hour.

**Cron is gone.** v1 chained one-shot `CronCreate` jobs — with UTC→local conversion, a nudge off
`:00`, a warning never to use a recurring job because it expires at day 7, and the observation,
in `improve-flow`, that *"a session rarely survives 24 hours, so most days the wake comes from
`otto poke` anyway."* The fallback path was doing the primary path's job. v2 deletes the primary
path: `nextWakeAt` on disk, poke reads it, done. No expiry, no timezone arithmetic, no dropped
link to detect, and it survives a reboot — which no in-session timer ever did.

**Ticks must be cheap.** One `gh pr view --json` and a comment-cursor diff. Nothing new →
`otto state tick <id>`, re-arm, exit. Something moved → `tick --progress`, then work. Only a real
change spins up a subagent; that is what makes a week of hourly polling affordable instead of a
slow leak. Since every wake is a cold start, a cheap tick must be a genuinely small *prompt*,
not just a small amount of work.

`tick` prints `{"ticksWithoutProgress": n, "limit": l, "exhausted": bool}`. Exhausted → stop
sleeping: `blocked` (`set-status --status blocked --because stall`) plus a human gate. **Progress means durable state changed**, not that the
goal was reached: a rejected proposal recorded in a ledger is progress. `0` means disabled, for a
workflow whose ticks are *expected* to achieve nothing (§11.5).

**A missed window is one wake, never a replay.** However long the run was down — an hour, three
days — it gets one reconcile. A closed lid through six ticks is one catch-up, because a run has
one `nextWakeAt`, not a queue.

### 8.1 Check scripts — opt-in, and cheaper than a tick

A tick is still a wake: cold Claude session, ~$0.38 and 11 turns in the measured steady state
(§12), to run something as cheap as one `gh pr view` and a cursor diff. Poke already decides
*whether a wake should exist* for free, with no LLM involved (§9) — it just couldn't tell "nothing
changed" from "time's up" without spawning one to ask. A check script lets it.

```
wake:  writes artifacts/check.sh, then
       otto state arm-timer <id> --in 3600 --check-script artifacts/check.sh --check-every 300
poke:  runs the script directly every 300s; the run's real nextWakeAt is still 3600s out
       exit 0  → nothing changed. No spawn. `next-check` advances, silently.
       exit 1  → changed. Falls into the same spawn path a missed nextWakeAt takes.
       anything else, or a timeout → treated as changed too — fail open to a real wake, never
       fail closed into silence — but journaled as an error, not a change.
```

**Opt-in, and additive.** A run that never sets `--check-script` is unaffected — `check` is
`None` and poke's decision is exactly what it was before this existed. This is deliberately
narrow: it targets the cheap-tick case above, not a general poke plugin system.

**The script gets no `otto state` access.** Same reasoning as §5.3 — every write goes through
`otto state`, and a check script is not a wake. Its only channel out is its exit code and a
capped stdout snippet (journaled and shown in `otto show`, never accumulated). It cannot open a
gate, set a status, or touch `run.json` directly.

**The outer `nextWakeAt` is the only safety net, and that's deliberate.** It still fires a real
wake regardless of what the check script has been saying — the same re-derivation guarantee (§3)
that already exists, not a new mechanism. A script that's stale, wrong, or has started lying gets
caught the next time the real wake comes due, exactly as it would if there were no check script at
all. No second counter (a "force a heartbeat after N no-changes" rule) was added, because this
ceiling already does that job.

**Fail open, on purpose.** Anything other than exit 0/1 — a crash, a missing binary, a timeout —
spawns a wake rather than staying quiet. A check script is judgement-free plumbing exactly like
poke itself; the moment it can't answer cleanly, the answer is to hand the question to something
that can, not to guess "no change" and risk a run going stale unnoticed. This costs the same
$0.2–0.4 floor a wake always costs, but a broken script is a rare event, not a schedule.

**A check a person sets belongs to the run.** `otto check <run> --script f --every 1h --period 1d`
copies the script to `check.sh` in the run directory and marks the check `pinned`: poke keeps
running it across every wake, a wake's plain `arm-timer` keeps it rather than clearing it, and a
wake's own `--check-script` does not replace it. `--period` is what makes it pay — the check can
only skip wakes the period would otherwise spend, so `otto check` warns when the check runs no
more often than the period. The first run is on the next poke, so a broken script shows at once.
`otto show` and the run page show the script, its last result and output, and how many checks
have found nothing (`noChangeTotal`, each a wake not spent) — or, with no check, what a wake has
been costing instead.

**Every wake restarts the check's interval.** A wake is a real look at the world. Without this, a
change the wake could not clear (a task it failed to finish) reads as "changed" at the very next
poke, and a check meant to save wakes spawns one every five minutes. With it, the worst case is
one wake per check interval.

**§11.8** is how a phase declares one. See `harness/wake.md`'s Waiting section for what a wake
that's about to sleep actually writes.

## 9. Poke

launchd, every ~5 minutes, managed by `otto agent start` / `otto agent stop`. It makes **no
decision about the work** — only about whether a wake should exist:

| Condition | Action |
|---|---|
| `run.json` unreadable | skip — a person needs to look |
| Terminal status | skip, reaping any session left behind |
| A wake is running, past `wake.deadlineAt` (+ grace) | kill its process group, journal `wake-killed`, record the wake incomplete (§5.2) |
| A wake is running | skip — it is working |
| A gate is open and unexpired | skip — a person owns it |
| A wake started and died without finishing | **record it incomplete**, so the backoff applies |
| Nothing pending and no wake ever ran | spawn — the run is stranded |
| `nextWakeAt` in the future, an opt-in check script (§8.1) is due | **run it directly, no LLM** — no change: skip, silently; change or error: fall into the ordinary due-wake row below |
| `nextWakeAt` in the future, no check due | skip |
| `nextWakeAt` passed | spawn (with spawn-attempt backoff) — `GRACE_MINUTES` is 0; `--grace N` defers for N minutes |
| More than `MAX_STARTS` spawned this pass | defer |

**All inference is gone.** v1's poke had to guess whether anybody was home: `heartbeatAt`
freshness, pane classification (`Live` vs `Shell`), `stuck-alive` at 45 minutes overdue,
`stalled-busy` when a session looked genuinely occupied, and a rule against reviving into a live
session because that would double-run a conductor. All of it existed because a session could be
alive but useless. Now liveness is a lock somebody either holds or does not (§5.2, `liveness`), and
a wake that is alive but useless hits its deadline and gets killed. `heartbeat` is deleted.

**Why "started and died" is its own row.** A wake clears poke's spawn-attempt counter the moment it
starts, because starting is the only proof the spawn worked. That leaves a hole: a wake that
reliably crashes *after* starting resets the backoff every time, so poke would restart it every
five minutes for ever and pay for each attempt. Found by `kill -9`ing a real wake. Recording the
crash as an incomplete wake routes it through the bounded retry that already exists, which ends at
`blocked` plus a gate rather than at an unbounded bill.

```bash
otto poke --dry-run --verbose      # what it would do, and why, for every run
otto logs <id>                     # what happened to one run
```

## 10. Wrapping anything

This is the premise. Three kinds of thing, one engine:

| Invocation | What the wake loads |
|---|---|
| `otto run --skill nr:handle-trivy-tickets-skill --goal "…"` | The skill, by name, via the Skill tool |
| `otto run --instructions runbook.md --goal "…"` | Any prose file — a runbook, a ticket, a v1 workflow table |
| `otto run --goal "…"` | Nothing but the goal. Otto is still useful |

`--skill` and `--instructions` may be combined; the goal is always required, because it is the
only thing that survives every wake unaltered and the only thing that says when to stop.

### 10.1 Translating a skill that has never heard of otto

The harness prompt sits between the wrapped instructions and the run directory, and applies five
translations. Each one converts an assumption about a continuous session into something durable:

| The wrapped thing says | The wake does |
|---|---|
| "ask the user" / "confirm with them" / names `AskUserQuestion` | Open a gate (§7) and exit. The answer arrives via `otto answer` |
| "wait", "poll", "check back in an hour", "keep watching" | Exit; the period brings a wake back. `arm-timer --in` for a different gap |
| "remember", "keep track of", "note for later" | Write it — `handoff.md` for one wake, an artifact for longer |
| Anything assuming earlier steps are in context | Read `handoff.md` and the artifacts. There is no earlier context |
| Reading a big diff, a whole suite, a long file | Delegate to a subagent; take back the conclusion, not the contents |

These are prose, so they will occasionally be ignored. The backstop is mechanical: a wake that
ignores them ends without a legal stopping state and the validator turns it into a retry (§5.2).
The failure mode is a wasted wake, not a lost run.

### 10.2 What makes a run finished

A workflow has a terminal phase. A skill has no completion criterion at all, and "runs
indefinitely" makes that worse — so this is a first-class field, not an inference:

- `--until "<condition>"` states it outright.
- Otherwise the **first wake proposes a done-condition and gates it.** One cheap question, before
  any work.
- `--perpetual` says there is no done condition, only retirement (§11.5). Explicit, so it is a
  choice rather than an omission.

**How reliable this is, measured.** Given a vague goal ("tidy the comments in X"), a wake did gate
it, and produced a genuinely good question — it noticed the file was markdown with no comments in
the code sense, cited the specific lines it thought were meant, and offered options with a default.
Given a concrete goal ("rebase if needed, run the checks, commit, do not push"), a wake *skipped*
the gate and journalled why: the goal already specified completion, so a round trip would have
bought nothing.

That is defensible judgement, and it is also not a guarantee. So this is guidance the wake applies,
not a check otto enforces — the contract stays at two (§5.2). The failure mode it guards against
only bites a run that spans wakes; a wake that can finish the goal outright has nothing to
negotiate. Where it genuinely matters, pass `--until` and remove the judgement.

`doneCondition` is re-read every wake and never rewritten without a gate. A run that revises its
own finish line is a run that will never cross it.

### 10.3 Honest limits

- **A `Workflow` script cannot span wakes.** It is one invocation. A wrapped skill that fires one
  must let it complete inside a single wake, which means it must fit inside `maxWakeMinutes`.
- **Skills that want a human mid-loop degrade to one gate per question.** Correct, and slower —
  a five-question skill becomes five wakes. Fine over days; absurd over minutes.
- **Short one-shot skills gain nothing.** Wrapping `/humanizer` in a durable run is pure
  overhead. Otto is for work whose horizon exceeds a session, and it should not pretend
  otherwise.
- **Otto cannot make a vague instruction rigorous.** It guarantees resumability, not competence.
  A skill that was ambiguous in one session is ambiguous across forty wakes, at forty times the
  price. The `doneCondition` gate in §10.2 is the cheapest place to catch that.

## 11. Authoring for otto

Everything here is **guidance, not requirement** — nothing in it is validated, and a run that
ignores all of it still works. It is what you write down when you are deliberately building a
long-horizon workflow, as `dev-flow` is (§11.6). What each habit buys, in rough order of value:

### 11.1 Declare phases with exit conditions and idempotency

The single biggest win, and the one the engine genuinely cannot supply for you. Per phase:

| Field | Purpose |
|---|---|
| `goal` | One sentence, for the handoff and gate questions |
| `preconditions` | Machine-checkable, re-derived — not "we did this last time" |
| `actions` | The work; delegated when token-heavy |
| `durableOutput` | What must be on disk before this phase counts as done |
| `exitCondition` | Machine-checkable where possible ("PR exists and CI is green") |
| `gate` | `continue` \| `ask-human` \| `sleep(interval, until)` |
| `idempotency` | How to detect already-done work and **adopt** it rather than redo it |

`gate: continue` means *proceed now, in this wake* — it is not a stopping point.

**Idempotency is what makes a phase re-enterable, and every phase is re-entered.** After a crash,
a retry, a kill at deadline. The rule: **a phase's first act is to check whether its own exit
condition already holds.** `prepare` adopts an existing worktree; `publish` adopts an existing PR
instead of opening a second one. No phase may leave the world in a state its own precondition
check cannot recognize.

### 11.2 Write gates as decisions

Name the options so `otto answer --choice` works, give each one its consequence, state the
default, and point at the artifact. §7 is the full rule; a phase table is where you write them
down in advance instead of improvising one at hour forty.

### 11.3 Say how big the work is

Measured on a two-file scratch repo, for a change adding one function, v1's `dev-flow` took
~50 minutes to reach its first human gate — 19 in `prepare`, 31 in `plan`. That is the premise of
the system, not a fault in it. But it says something a phase table should encode: **a
one-function change does not deserve half an hour of planning**, and the plan should say how it
was sized.

### 11.4 Keep a ledger when the handoff won't fit

`handoff.md` is capped (§6) and rewritten. Anything that must accumulate — one line per proposal,
per target, per cycle — goes in an append-only artifact, read at the top of every wake.

**The ledger is also the position marker.** A crash at target 12 of 20 resumes at 12 because the
statuses are on disk, not because anything remembered.

### 11.5 Perpetual runs change four habits

- **`done` never arrives.** Retirement is `status: stopped` — healthy, just no longer wanted.
  `failed` would be a lie in the audit trail.
- **Escalation off** (`maxTicksWithoutProgress: 0`), because ticks are *expected* to achieve
  nothing. Otherwise 24 polite rejections mark the run `blocked`.
- **Locks per cycle, never per run** (§15). A perpetual run holding a lock while it sleeps blocks
  every other run against that repo forever.
- **An artifact directory per cycle.** A run directory sized for one deliverable now holds
  hundreds.

### 11.6 Fan-out lives inside three limits

1. **One gate open at a time.** Ask about the batch in one question; if one target needs a
   judgement call, ask about that one alone while the rest wait. Thirty simultaneous questions is
   not a review.
2. **`facts` stays scalar.** Counts and versions there, per-target state in a ledger. `run.json`
   is read every wake and must not grow with the work.
3. **One lock at a time.** Lock a target, finish it, unlock, next.

### 11.7 Worked example: `dev-flow`

The v1 workflow tables — `dev-flow`, `improve-flow`, `upgrade-flow`, `hello-flow`, `nap-flow` —
become skills otto wraps. They already have every habit above, which is what makes them the
regression test that matters: **if the v2 engine cannot carry `dev-flow`, the contract in §5.2 is
too thin** and §10's guarantees were imaginary.

| # | Phase | Durable output | Gate |
|---|---|---|---|
| 0 | `intake` | `artifacts/spec.md`, `facts.jiraKey` | `continue`, or ask if the ticket is genuinely ambiguous |
| 1 | `prepare` | worktree + branch, `.env`/fixtures | `continue` |
| 2 | `plan` | `artifacts/plan.md` | **ask-human** |
| 3 | `implement` | commits, lint + unit green | `continue` |
| 4 | `verify` | `artifacts/test-report.md` | **ask-human** |
| 5 | `publish` | pushed branch, `facts.prNumber` | **ask-human** — first outward action |
| 6 | `babysit` | commits answering review, cursor advanced | `sleep(1h, until: approved && green)` |
| 7 | `land` | merge commit | **ask-human**, unless `policy.autoMergeWhenGreen` |
| 8 | `cleanup` | worktree reaped, ticket updated, `done` | — |

Notes that still matter:

- **Phases 1, 6 and 8 are `manage-pr`** — worktree setup with the gitignored-`.env` gotcha,
  per-repo `build-info` memory, the stale/overlap rebase gate, pre-commit at pinned revs, the
  confidently-actionable-comments rule, merged-worktree reaping. `dev-flow` invokes that skill
  twice rather than reimplementing it. Under v2 this is just a wrapped skill delegating to
  another skill, which is the ordinary case rather than a special one.
- **`babysit` is where the days go**, and reconciliation there is the whole job: base moved →
  rebase; new comments past the cursor → apply what is confidently actionable and gate what is
  not; PR closed or merged out from under you → jump to `cleanup`.
- **`publish` and `land` are the only outward-facing steps.** §14.

### 11.8 Declare a check script for a poll-and-wait phase

Optional, the same way a phase table itself is optional (§1) — a `sleep(interval, until: …)` gate
works with no check script at all, exactly as it always has. Write one when the condition you're
polling for is genuinely cheap to ask outside an LLM: `gh pr view --json` and a cursor diff, a
`curl` against a status endpoint, `git fetch --dry-run` for a moved base. §8.1 has the mechanism;
this is when it's worth reaching for:

- **The check must be read-only.** It has no `otto state` access and can't be given any — its
  only channel out is an exit code and a short stdout note. If the poll needs to *do* something
  (advance a cursor, write a ledger line), that belongs in the wake `changed` triggers, not in the
  script.
- **Exit 0 for "no change", exit 1 for "changed," anything else is treated as an error** and
  spawns a wake anyway — write the script defensively (`set -e` and a clear final exit, not
  whatever the last command happened to return).
- **`--check-every` a fair bit shorter than the sleep itself.** A 15-minute check under a 1-hour
  sleep gives four free looks before the real wake would've fired anyway; a check every 55 minutes
  under a 1-hour sleep barely earns its keep.
- **`dev-flow`'s `babysit` phase (row 6 above) is the model case** for this — `manage-pr`'s hourly
  reconciliation loop is exactly the "cheap tick, mostly nothing happening" shape §12 measured at
  ~$9/day. A check script there turns most of those hours into a free shell call.

## 12. Context and cost

> **These dollar figures were measured while otto still ran `claude -p`, and they are kept as the
> record of what a wake costs in absolute terms.** otto no longer observes them: print mode bills
> SDK credits rather than the subscription (§4), so `-p` is gone and with it `total_cost_usd`. What
> a wake now reports is tokens, which is what every dollar figure below was derived from anyway —
> the turn counts and cache behaviour are unchanged, and they are the part that drives the
> conclusions. Read the money as "what this would cost if billed per token".

**What a wake actually costs, measured.** Real wakes doing trivial work (write one small file,
set a status) came in at **$0.18–$0.25 each, over 8–10 turns**. Every wake creates roughly 12k
cache tokens, every time — including two wakes of the same run whose prompt is byte-identical, so
**cross-wake prompt-cache reuse does not happen** and each wake pays a genuine cold start. The
very large `cache_read` a wake reports (500k–800k tokens) is reuse *within* the wake, across its
own turns, and happens regardless of anything otto does.

Two consequences, both the opposite of what this document first assumed:

- **Turns of work dominate, not prompt size.** A wake's bill is set by how many tool-using turns
  it takes, so the lever is delegating and not re-reading — rules 2 and 3 below — rather than
  shaving the handoff or the harness.
- **A floor of ~$0.2 per wake is structural.** A 200-wake run starts at ~$40 before doing
  anything useful. That is the price of durability, and the reason `budget` is measured and
  enforced (§6) rather than advisory. It is also an argument for fewer, longer wakes: a chain of
  `continue` phases inside one wake pays one cold start, not ten (§5.1).

`wake-spent` journals `cacheRead`/`cacheCreation` per wake, so this stays measured rather than
assumed — that accounting is what corrected the assumption.

Four rules:

1. **Cap the handoff** — for orientation, per §6. Not for economy; it is not where the money goes.
2. **A cheap tick must be a cheap tick.** One status check, one `tick`, re-arm, stop. What makes a
   tick expensive is turns, so the discipline is to do nothing else when nothing changed.

   Measured, wrapping `manage-pr`'s hourly babysit: **$0.96 for the setup wake (27 turns), then
   $0.38 per steady-state tick (11 turns)** — about **$9/day** for an hourly cadence, on a repo with
   nothing happening to it. A wake doing real work on real code cost **$1.09 over 16 turns**. So the
   trivial-wake floor of ~$0.2 is a floor and not an estimate. The lever that remains is cadence:
   bound a real perpetual run with `--budget-wakes` or `--budget-hours` and prefer the longest
   cadence the job tolerates. (`--budget-usd` is refused — see §6.) "Cheap" here means cheap
   *relative to a wake*, not cheap in absolute terms.
3. **Delegate anything token-heavy** — writing code, running a suite, reading a PR diff — to
   subagents. Give each the artifact path, the worktree, and one coherent job; ask what changed
   and what it proved, not for narration. **Never read a whole file to "check" a subagent** —
   check the durable output: `git log`, `git status`, the test result, the artifact. A wake that
   reads the diff it delegated has paid twice.
4. **Phase output is a file, not a message**, and once the ticket is distilled into
   `artifacts/spec.md`, read the spec — not the ticket — unless reconciliation says it changed.

## 13. Failure and recovery

**Classify before reacting:**

- **Environmental** (denied host, network, flaky CI, a missing `.env` in a fresh worktree): retry
  with backoff, **do not** burn an attempt. Multi-day runs hit these constantly.
- **Real** (tests genuinely fail, a rebase genuinely conflicts): burn an attempt, try to fix.
- **Ambiguous** (needs a judgement call): don't guess — open a gate.

Attempts exhausted → `blocked` plus a gate carrying what was tried. Journal every failure with
`log --event error` as it happens; a week later the journal is the only honest account.

**Crash mid-wake is the normal case.** Recovery needs no human: the process dies, the validator
sees no legal stopping state, `wake-incomplete` is journalled, poke re-spawns after backoff, and
the next wake re-enters cold and adopts whatever partial work exists (§11.1). In v1 this path
needed `/otto resume` typed by a person or inferred by a heuristic. In v2 it is the ordinary
control flow.

| What happens | What brings it back |
|---|---|
| The wake process crashes | The validator marks it incomplete; poke re-spawns with backoff |
| `kill -9` on the session | Same path. There is no session state to lose |
| Reboot | launchd loads poke at login; any run whose `nextWakeAt` has passed is spawned |
| A wake hangs | `deadlineAt` passes, poke kills it, incomplete path |
| A closed lid through six ticks | One catch-up wake, never six replays |
| A gate answered 14 hours later | `otto answer` spawns the next wake. The question was on disk the whole time |
| Repeated incomplete wakes | Backoff, then `blocked` plus a gate. It does not retry forever |

## 14. Outward-facing actions

Pushing, merging, and commenting on someone's ticket cannot be undone by deleting a file. They
are not taken on a wake's judgement, and not on its recollection of an approval either — and
under v2 there is no recollection at all, which makes this stricter rather than looser:

```bash
otto state authorize <id> --action push --gate 003 --head <sha> --quote "<verbatim>"
otto state check-authorized <id> --action push --head <sha>    # exit 2 → DO NOT ACT
```

1. **An authorization names its source** — an answered gate, or a policy flag that is actually
   `true`. There is no third option, because an action decided on alone has no source to name.
2. **It binds to one commit.** Over a multi-day run the branch keeps moving, so a stale head
   re-asks. This is the common case, not an edge case.
3. **Check immediately before acting**, every time, even when you just recorded it. The check is
   cheap; the mistake is not.
4. **Exit 2 means stop and report.** If you believe an unauthorized action is warranted, that
   belief is exactly what the gate exists to test.

A run fanning out over targets adds `--item <target>` on both calls. Without it every target
shares one key, approving the second silently voids the first, and the run can push code nobody
cleared.

## 15. Repo locks

```bash
otto state lock <id> --repo <path>     # re-entrant for its own holder
otto state unlock <id>
otto state locks                        # who holds what, and what is breakable
```

**Hold a lock while using the repo and not one wake longer.** Release before any sleep and on
every failure path. Exit `2` means another run holds it — stop and report which. Do not
`--force`: a lock breaks automatically when its owner is terminal or has gone quiet past its own
wake time, and forcing past a live holder is how two wakes come to share one worktree.

A run parked at a gate for a day and a run holding a lock for a day are different failure
classes. Only the first is supposed to happen.

## 16. Command surface

```bash
# start
otto run --skill <name> | --instructions <path> [--goal "…"] \
         [--until "…" | --perpetual] [--repo P] [--period 1h] \
         [--budget-wakes N --budget-hours H] \
         [--permission-mode M] [--detach tmux|bg|none] [--watch]

# live with it
otto ls                        # status · goal · who is blocking · wakes spent
otto show <run>                # state, handoff, the open question, cold-readable
otto answer <run> --choice approve | --text "…" | --file f  [--no-wake]
otto note <run> "…" [--standing] [--now] | --list | --drop N
otto check <run> [--script f --every 1h --period 1d | --off]
otto logs <run> [-f]           # the journal, readable
otto attach <run>              # watch the live wake, if there is one
otto wake <run> [--watch | --detach tmux|none] [--dry-run]
                               # force one now, backgrounded as the run asks
otto stop <run> [--reason "…"]  # terminal: stopped

# machinery
otto poke [--dry-run --verbose]   # launchd, every ~5 min
otto start | stop                  # register / unregister the launchd agent
otto state <cmd>                   # the wake's writer surface (§5.3)
otto serve [--port 7878]           # the same human commands, in a browser
```

`otto state` is unchanged in role: the only writer, used by a wake, atomic per call. The human
commands above are thin formatters over the same operations — `otto answer` *is* `close-gate`
plus a spawn. Exit codes stay: `2` state conflict, `3` no such run; both mean stop and report.

`otto serve` is a second rendering of the same human commands, not a second system. The logic
lives once, in `core`; `human.rs` formats it for a terminal and `server/` serves it as JSON to an
embedded page. The server holds no state and owns no run — it reads and writes the run
directory exactly as the CLI does — so the "no daemon owns a run" principle survives it: stop the
server and nothing about any run changes. It binds 127.0.0.1 only and refuses cross-site
requests, because a POST to it can start a wake.

**`/otto` the skill is gone.** A run is not driven from inside a session any more, so there is no
conductor skill to invoke. What replaces it is the harness prompt (§10.1), which is otto's own
internal asset rather than something a human types.

## 17. What v2 deletes

*(Steps 1–3 landed all of this; the checklist stays as the record of why each went.)*

A checklist, because the deletions are most of the work and each one traces to a v1 workaround
for a session that outlived a wake:

| Deleted | It existed because |
|---|---|
| `CronCreate` chaining, `arm-timer --print-cron`, UTC→local, `:00` nudging, 7-day-expiry reasoning | In-session timers were the primary waker (§8) |
| `otto state heartbeat`, `heartbeatAt`, `heartbeat_of` | Liveness had to be inferred from a long-lived session (§9) |
| `stuck-alive`, `stalled-busy`, `STUCK_MINUTES`, revive backoff-into-live-session rules | A session could be alive and useless (§9) |
| Most of `tmux.rs`: `wait_ready`, `send_prompt`, `SETTLE_SECONDS`, `SessionState::{Live,Shell}` | Prompts were typed into a live composer (§5.1) |
| The `AskUserQuestion` mandate, "disk leads the pane", `test/fixtures/pane_gate_open.txt` | Gates were detected by reading a terminal (§7) |
| The wrangler tag contract as a *requirement* | Same. It stays supported, no longer load-bearing (§20) |
| `brief.md` | Merged into `handoff.md` (§6) |
| Timer gates and their gate files | Waiting is a status (§8) |
| `skills/otto/SKILL.md` as the conductor | Replaced by the harness prompt (§16) |
| `workflows/` as a first-class concept | They become wrapped skills (§11.7) |

## 18. Build order

1. **The wake: executor plus validator.** `otto wake <id>` — spawn, deadline, a non-print
   `claude`, validate the two checks, journal the outcome. The structural change everything rests on.
   Prove it with `hello-flow` reframed as a trivial wrapped skill.
2. **The CLI human surface**, and delete the pane machinery in the same pass — `otto run/ls/show/
   answer/logs/attach/stop` and the §17 tmux deletions are the same change seen from two sides.
3. **Poke rewrite.** due→spawn, exited→reap, past-deadline→kill, incomplete→backoff. It stops
   inferring and starts observing. This is where v1's 721 lines get much smaller.
4. **Goal, done-condition, budget, and the harness prompt.** Then wrap one *unmodified*
   third-party skill end to end. First real test of the premise in §1.
5. **Regression: carry `dev-flow` through the new engine.** Not a formality — the experiment that
   says whether §5.2's two checks are load-bearing, or whether long-horizon runs need a declared
   plan after all (§19.1).

## 19. Risks and open questions

1. ~~**The contract may be too thin, and step 5 is how you find out.**~~ **Answered by running
   `dev-flow` end to end.** It carried a ticket to a merged commit in 5 wakes and 4 gates for
   $4.14, following the declared phase table faithfully: it chained `continue` phases within a
   wake, produced every declared artifact, acquired and released the repo lock *inside* `prepare`
   rather than across a gate, delegated exploration to subagents, distinguished "no PR host" from
   "cannot reach the PR" and skipped `babysit` deliberately, and took both outward actions only
   under an authorization bound to the exact commit and naming its gate.

   **And it silently skipped a phase.** `cleanup` (phase 8) never ran: the wake went `land → done`
   and left the worktree behind, with no mention in the journal or the handoff. The contract did
   not catch it, and *cannot*: `done` is a legal stopping state and the handoff was written, so
   both checks passed. Nothing verifies that a declared phase's `durableOutput` exists — that is
   what "two checks, deliberately" costs.

   So the conclusion is narrower than either "the contract suffices" or "it needs a phase table":

   - **For safety and resumability the contract is enough.** Nothing unsafe happened, nothing was
     stranded, and every irreversible action was authorized.
   - **For completeness it is not, and no generic check can be.** Whether declared work happened
     is only answerable against a declaration otto is not allowed to require (§1).
   - **What closes the gap is the live test, not the engine.** `tests/live_devflow.rs` asserts the
     worktree is reaped; the contract never will. A declared phase table earns its keep by making
     omissions *checkable by a test*, not by making them impossible.

   The generated-plan fallback stays unbuilt, because the failure it would address is not the one
   that showed up. Note also what a wrapped skill gets from being *well authored* (§11): every
   good behaviour above came from `dev-flow.md` declaring it, not from the engine.
2. **Translation (§10.1) is prose with a mechanical backstop.** It will sometimes be ignored. The
   cost is a wasted wake, and the incomplete-wake path bounds it. Worth measuring on real skills
   before trusting a number.
3. ~~**Cold-start cost is unmeasured.**~~ **Measured (step 1).** ~$0.2 and 8–10 turns for a
   trivial wake; ~12k cache-creation tokens every wake with no cross-wake reuse. The cost driver
   is turns, not prompt size — §12 carries the numbers and what changed because of them. Still
   open: what a *working* wake costs on real code, which step 5 will show.
4. **Unattended permission posture — reopened by dropping `-p`.** This was previously marked
   settled by measurement: `acceptEdits` denies Bash, `dontAsk` denies Write, Edit and Bash, so
   every mode short of `bypassPermissions` produced a wake that spent money achieving nothing.
   That measurement depended on `--permission-prompts none`, which only works with `--print` and
   is no longer passed (§4), so the finding no longer describes what otto runs.
   What is known now is one data point: a prompt-worthy tool does **not** hang the wake — a
   `manual`-mode wake with stdin at `/dev/null` was observed running a Bash command and exiting 0,
   so the deadline is not silently burned waiting for an answer nobody can give. One tool in one
   mode is not enough to say what each mode now denies, and the modes want re-measuring.
   `bypassPermissions` stays the default until that happens. The conclusion that survived intact
   is the more important one: **the real decision is the launcher, not the mode** — run under
   `--launcher yolo` and the guardrail is a kernel-enforced sandbox rather than a prompt nobody is
   there to answer. The stricter modes remain useful for a *supervised* run (`plan`, `manual`) to
   see what a wrapped skill would do without letting it act.
5. **Does a non-print wake expose everything a wake needs?** Skills resolve, subagents and MCP
   are configured through `--settings` / `--mcp-config`, and `--allowed-tools` scopes it. Verify
   the whole set — Skill, Agent, the MCP servers a wrapped skill expects — against a real wake in
   step 1, before building on it. Dropping `-p` reopens this rather than closing it: the flags
   above are not print-only, but nothing has yet confirmed the whole set behaves identically
   without `-p`.
   Also worth watching on the first real wakes: sessions now persist (`--no-session-persistence`
   was print-only too). otto never resumes one, so this is disk rather than correctness — but the
   harness prefix is meant to be byte-stable for cache reuse, and `cacheRead` in the journal is the
   first time otto can actually see whether it is.
6. **Detachment default.** tmux is the stated intent and what the existing code does. But tmux is
   denied inside most sandboxed Claude Code environments, which is why v1's live tests needed a
   real terminal — and `claude --bg` would make the suite runnable from inside one. Keep
   `--detach` swappable and revisit once step 1 can be tested both ways.
7. **Concurrency ceiling.** `MAX_STARTS` is 2. Rate limits or your own attention will move it;
   measure rather than raise it hopefully.

## 20. History

The decisions below were made under v1 and are still in force. They are kept because each one
records something real that a rewrite would otherwise relearn.

> **Separate from wrangler, 2026-09-11.** The contract turned out to be four tmux session tags:
> otto sets `@cc_session`, `@cc_owner otto`, `@cc_label` and `@otto_run`, and wrangler lists any
> session carrying `@cc_session` rather than any session matching its own prefix. The
> generalizing happened on wrangler's side, which was the point. Two couplings were removed and
> should not come back: otto used to read `~/.config/wrangler/config.json` for a session-name
> prefix (the coupling arriving through a config file rather than an API), and otto used to ask
> wrangler to tell a busy session from an idle one, because wrangler reads Claude Code's terminal
> UI.
>
> **Under v2 this contract survives but stops being load-bearing.** A wake is a process, so
> liveness is `wake.pid`, and a gate is a row in `otto ls`, so nothing needs a pane read. The
> tags stay because a run appearing in a session manager beside everything else is genuinely
> useful — just no longer necessary. The v1 reasoning for keeping the projects separate holds
> unchanged, and v2 strengthens it: a run and a session were 1:many before, and are now 1:many by
> construction, one per wake.

> **Rewritten in Rust, 2026-09-11.** Three Python scripts (`otto-state`, `otto-poke`,
> `otto-session`) became one binary with matching subcommands. Runtime data moved from the
> checkout's `runs/` to `~/.otto/` (`$OTTO_HOME`); the default launcher became the plain `claude`
> CLI, overridable with `--command`; `otto install` / `start` / `stop` replaced manual
> symlink-and-copy-a-plist steps. The legacy `cc-<name>` session-name fallback was dropped, a
> from-scratch rewrite being the natural point to retire a shim already marked for deletion.

> **Facts v1 established that v2 depends on.** Worth not rediscovering:
> a session rarely survives 24 hours, so an in-session timer is a fallback pretending to be a
> primary (§8); a daily cadence woken by the reviver lands 15–20 minutes late, and chasing that
> is wasted effort; a one-function change took ~50 minutes to reach its first gate, 19 of them in
> worktree setup (§11.3); and the most expensive failure available is a silent forever-loop,
> which is why escalation exists and why `0` must mean *deliberately disabled* rather than
> *unset*.
