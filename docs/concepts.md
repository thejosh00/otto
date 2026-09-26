# Concepts

How otto thinks about a long-running piece of work, and how the parts fit together. Read this
once; the [guides](README.md#guides) assume it. For *why* each piece is shaped the way it is, see
[DESIGN.md](../DESIGN.md).

- [Runs and wakes](#runs-and-wakes)
- [The handoff](#the-handoff)
- [The contract](#the-contract)
- [Statuses](#statuses)
- [When a run wakes: the period and timers](#when-a-run-wakes-the-period-and-timers)
- [Check scripts](#check-scripts)
- [Gates](#gates)
- [Notes](#notes)
- [When things go wrong](#when-things-go-wrong)
- [Budgets and policy](#budgets-and-policy)
- [Outward actions and repo locks](#outward-actions-and-repo-locks)
- [The reviver and notifications](#the-reviver-and-notifications)
- [What a wake costs](#what-a-wake-costs)
- [Where everything lives](#where-everything-lives)

## Runs and wakes

A **run** is a goal, the thing it wraps (a Claude Code skill, a prose file, or nothing but the
goal), and a directory of state on disk. A run can last minutes or months.

A **wake** is one process: otto starts `claude` non-interactively, the model orients from the run
directory, does a stretch of work, reaches a stopping point and exits. A wake is not one model
turn — it runs as long as the work does, across many turns and subagents — but it always ends.
There is no long-lived session, and nothing survives a wake except what it wrote to disk.

```
otto run ──> wake 1 ──> opens a gate, exits
                          │
             otto answer ─┘──> wake 2 ──> sleeps, exits
                                            │
                          otto poke (every 5 min) ──> wake 3 ──> done
```

Three things start a wake:

| Cause | Who starts it |
|---|---|
| The run's wake time has come | `otto poke`, the reviver launchd runs every ~5 minutes |
| A person answered a gate | `otto answer` (or the web UI) |
| You asked | `otto wake <id>`, or `otto note <id> "…" --now` |

Every wake starts in the run's **working directory** — `~/work`, say, set once as the default in
`config.json` or per run with `--workdir` — whoever starts it, so every wake sees the same
`CLAUDE.md` and project settings. Every wake starts cold and re-derives the world — git, `gh`,
the ticket — because between two wakes `main` moved, CI reran, and someone force-pushed. That is the price of durability, and the
reason a run survives a reboot, a crash or a week away.

## The handoff

`handoff.md` in the run directory is the only thing the next wake gets for free. Every wake
**rewrites** it (never appends) in fixed sections — where the run is, what was decided, what this
wake did, what the next wake must do first, what it needs — capped at 8KB. Anything that has to
accumulate goes in an artifact the handoff points to. `otto show` prints it.

## The contract

The only thing otto enforces about a wake, and what makes wrapping *anything* possible. When a
wake's process exits, otto checks two things:

1. **The run is somewhere it will be brought back from** — a gate open, a wake time set, or
   finished.
2. **This wake rewrote `handoff.md`**, within its cap.

A wake that exits cleanly with nothing pending doesn't have to do anything special: otto puts
the run to sleep for its [period](#when-a-run-wakes-the-period-and-timers).

Pass, and the wake is `wake-complete`. Fail for any reason — a crash, a kill at its deadline, a
model that simply stopped — and it is `wake-incomplete`: see [When things go
wrong](#when-things-go-wrong). The check happens in the parent process, because a wake that
crashed cannot report on itself.

## Statuses

| Status | Meaning | What brings it back |
|---|---|---|
| `running` | A wake is working, or about to | — |
| `sleeping` | Waiting for its wake time | poke, when `nextWakeAt` passes |
| `awaiting_human` | A gate is open | `otto answer`, or the gate expiring |
| `blocked` | otto stopped trying and opened a gate explaining why | a person |
| `done` | The goal is met | nothing — the run is over |
| `failed` | It could not do its job | `otto resume` |
| `stopped` | Retired by a person | `otto resume` |

`blocked` always records a cause: `wake-failures` (wakes kept failing), `budget` (a budget was
spent), `stall` (ticks kept changing nothing), or `instructions` (the wrapped instructions decided
it couldn't go on).

## When a run wakes: the period and timers

Every run has a **period** — how often it wakes when nothing else says otherwise. It's an hour
unless the run was started with `--period`, and `otto period <id> 4h` changes it later.

When a wake finishes cleanly with nothing pending, otto sleeps the run until **one period after
that wake started** (so an hourly run stays hourly rather than drifting by each wake's length).

A wake can say something different for one sleep: `otto state arm-timer --in 600` because CI
takes ten minutes, or `--at` for tomorrow morning. That lasts one sleep; after it the run is back
on its period. An open gate takes precedence over both.

A run whose machine was asleep, or whose reviver was off, gets **one** catch-up wake when it comes
back — never a replay of every wake it missed.

## Check scripts

A wake costs a cold model session even when there is nothing to do. A **check script** lets a
shell script answer "anything new?" first: when the run's wake comes due, poke runs the script
instead — no model, no tokens — and only wakes the run if it says something changed. Exit 0 means
nothing new, and the run sleeps another period; anything else wakes it. A safety net wakes it
anyway after 24 "nothing new" results in a row.

The period then means "how often to look", and the check decides whether looking needs a model.
A run whose goal is mostly watching usually sets up its own check on its first wake; you can
always set one yourself, and yours wins. [Check scripts](guides/check-scripts.md) covers both.

## Gates

A **gate** is the run asking a person something. A wake writes the question to
`gates/NNN-<slug>.md`, opens the gate, and exits; the run is `awaiting_human` until someone
answers with `otto answer` (or a button in the web UI). The answer is recorded verbatim and the
next wake starts.

A gate question is written to be answered cold — you arrive hours later with no transcript — so
it carries where the run is, the specific decision, named options with their consequences, and a
default. `otto answer --choice <name>` picks an option; `--text` is for anything that needs
prose.

- **Expiring gates.** A gate can expire (`open-gate --expires-in`), for questions where silence
  has a sane meaning ("no answer, so don't do it"). When it expires a wake comes to deal with it.
- **Stale gates.** A gate left unanswered past `gateStaleAfterHours` (48 by default) gets one
  more notification.

## Notes

A gate is the run asking; a **note** is you telling, whenever you like, gated or not:
`otto note <id> "skip the e2e suite, it's broken on main"`. The next wake gets it verbatim in its
prompt. It is delivered once that wake *completes* — a wake that crashes never delivered it, so
the next one gets it again.

- `--standing` gives a note to every wake until you `--drop` it, for guidance that should hold
  for the life of the run.
- `--now` starts a wake to read it, if none is running and no gate is open.
- A note steers *how* the run works. It never changes the goal or the done-condition, and never
  authorizes a push; a wake opens a gate for anything like that.

## When things go wrong

A crash mid-wake is the normal case, and recovery needs nobody:

| What happens | What brings it back |
|---|---|
| The wake crashes, or is killed | Marked incomplete; retried after a backoff (5m, 10m, 20m… up to an hour) |
| A wake hangs | Poke kills it at its deadline (`maxWakeMinutes`, 45 by default); incomplete path |
| The machine reboots | launchd starts poke at login; any run that's due wakes |
| Five wakes in a row don't finish | `blocked`, with a gate saying what the wakes were doing |

A retry is never held back by a check script — the work is half done whatever the world outside
has been doing.

## Budgets and policy

**Budgets** are outer bounds for runs that could go on for a long time: `--budget-wakes N` and
`--budget-hours H`. At 80% of either, otto notifies you once; at 100%, the run is `blocked` with a
gate. There is no dollar budget — nothing can price a wake on a subscription (see
[cost](#what-a-wake-costs)).

**Policy** knobs tune the engine per run (`otto run --policy key=value`):

| Key | Default | What it does |
|---|---|---|
| `periodMinutes` | 60 | The period (prefer `--period` / `otto period`) |
| `maxWakeMinutes` | 45 | A wake's deadline; poke kills it after |
| `maxIncompleteWakes` | 5 | Failed wakes in a row before `blocked` |
| `maxTicksWithoutProgress` | 24 | Polling ticks that change nothing before `blocked`; 0 disables |
| `gateStaleAfterHours` | 48 | When an unanswered gate is notified again; 0 disables |
| `handoffMaxBytes` | 8192 | The handoff's cap |
| `autoMergeWhenGreen` | false | A policy flag a workflow may cite as authorization to merge |

Any other key is kept in `policy` for the wrapped instructions to read.

## Outward actions and repo locks

Pushing, merging and commenting on someone's ticket can't be undone by deleting a file, so a wake
never takes them on its own judgement. It records an **authorization** — naming the answered gate
or policy flag that allows it, and bound to one commit — and checks it immediately before acting
(`otto state authorize` / `check-authorized`). A branch that moved since the approval asks again.

Two runs working in one repository would trample each other, so a wake claims a **repo lock**
(`otto state lock`) while it uses one, and releases it before it sleeps.

## The reviver and notifications

`otto poke` is the reviver. launchd runs it every ~5 minutes (`otto agent start` installs it). It
makes no decisions about the work, only about whether a wake should exist: start the runs that
are due, run their check scripts, kill wakes past their deadline, and record crashed ones.
**Without it, a sleeping run never wakes.** See [running as a
service](guides/running-as-a-service.md).

Poke also sends a macOS notification, once each, when a run opens a gate, becomes blocked, leaves
a gate unanswered past `gateStaleAfterHours`, or passes 80% of a budget.

## What a wake costs

otto runs `claude` without `-p`: print mode bills SDK credits instead of your subscription, the
wrong meter for something that wakes forever. So a wake's cost is measured in tokens, read back
from the session transcript claude leaves behind and journaled as `wake-spent` (turns, input and
output tokens, cache reads and creation). `otto check <id>` shows the recent average.

Every wake is a cold start: roughly 12k tokens of cache creation before anything useful happens,
and prompt caching does not carry across wakes. So fewer, longer wakes are cheaper than many
short ones, and a check script that answers "nothing new" for free is the biggest saving
available for a polling run.

## Where everything lives

```
~/.otto/                         ($OTTO_HOME overrides)
  config.json                    the default workdir and launchers — see reference/config.md
  runs/<id>/
    run.json                     machine truth — see reference/run-json.md
    handoff.md                   what the next wake gets for free
    journal.jsonl                every event, append-only — see reference/journal.md
    gates/NNN-<slug>.md          questions, and their answers verbatim
    notes/NNN.md                 notes, verbatim
    artifacts/                   whatever the work produces: plans, ledgers, check.sh
  runs/.locks/                   repo locks
  logs/poke.log, serve.log       the launchd jobs' output
```

Never edit `run.json` by hand: every write goes through `otto state` (atomic, journaled, locked),
and the person-facing commands are built on the same calls.
