<p align="left"><img src="assets/logo.svg" alt="otto" width="280" height="105"></p>

**Make any claude command survive days.**

otto takes a thing an agent knows how to do — an existing Claude Code skill, a runbook, a prose
instruction, or just a goal — and runs it for days without a session that has to stay alive.

Carry a Jira ticket to merged. Babysit a PR through a week of review. Propose one improvement a
day, forever. Grind twenty repos through a dependency bump. otto contributes none of the doing. It
contributes **durability**: state on disk, human gates, timers that survive a reboot, and a
mechanically enforced guarantee that every stopping point can be resumed cold.

> The run directory holds the truth. A wake is one process, and nothing survives it.

## Contents

- [How it works](#how-it-works)
- [Quick start](#quick-start)
- [Commands](#commands)
- [Web UI](#web-ui)
- [The contract](#the-contract)
- [Launchers](#launchers)
- [What a wake costs](#what-a-wake-costs)
- [Layout](#layout)
- [Tests](#tests)

## How it works

A **run** is a goal plus what it wraps plus state on disk. A **wake** is one process — `claude -p`,
non-interactive — that orients from the run directory, does a stretch of work, reaches a stopping
point, and exits. There is no long-lived session, no daemon that owns a run, and nothing that reads
a terminal to find out what is going on.

```
otto run ──> wake 1 ──> gate opens, process exits
                          │
             otto answer ─┘──> wake 2 ──> arms a timer, process exits
                                            │
                          otto poke (launchd) ──> wake 3 ──> done
```

Three things start a wake: a timer coming due, a person answering a gate, or you asking. Each wake
starts cold and re-derives the world — git, `gh`, the ticket — because between two wakes `main`
moved, CI reran, and someone force-pushed.

## Quick start

```bash
./install.sh
```

Builds the binary to `~/.local/bin/otto`, symlinks the skills otto ships into `~/.claude/skills/`,
and registers the launchd reviver. Without the reviver, a sleeping run never wakes.

```bash
# wrap a prose file
otto run --instructions test/fixtures/hello-flow.md \
         --goal "Smoke-test otto's machinery" \
         --until "artifacts/farewell.md exists and the run is done"

otto ls                       # which runs need you
otto show                     # where the one that needs you stands, and its question in full
otto answer --choice Approve  # answer it; name the run when more than one is waiting
otto logs <id> -f             # the journal, readably
```

Also valid, and equally first-class:

```bash
otto run --skill manage-pr --goal "get branch feat/x reviewed and merged" --repo ~/work/svc
otto run --goal "Find and fix flaky tests in this repo, one per day" --perpetual
```

## Commands

| | |
|---|---|
| `otto run` | Start a run and take the first wake. `--skill` \| `--instructions` \| neither. `--dry-run` prints the id and the first wake's command without creating anything |
| `otto ls` | Every live run and what each waits on, plus a `needs you:` summary of copy-pasteable answer commands. `--all` includes finished |
| `otto show [id]` | Where a run stands, plus the open gate question in full and the real `otto answer` command for each option. With no id: the one run waiting on you, or the only live run |
| `otto answer [id]` | `--choice X` (checked against the gate's own options), `--text "…"` or `--file f`, then continue the run. No flag on a TTY prompts with a numbered menu instead of failing; Enter takes the gate's stated default. With no id: the one run waiting on you |
| `otto logs <id>` | The journal, readably: a status line on top, local times, a rule per day, token counts rounded. `-f` to keep following, `-n` for how many (like `tail`), `--since 2h`/`--since 2026-09-15`, `--event gate-opened,gate-closed`, `--decisions` for just the turning points |
| `otto attach <id>` | Watch the wake running right now |
| `otto stop <id>` | Retire a run (`stopped`; `--failed` if it could not do its job) |
| `otto wake <id>` | Force one wake now, backgrounded the way the run asks for. `--watch` keeps it in your terminal, `--detach` overrides the run for this wake, `--dry-run` prints the command it would run |
| `otto poke` | The reviver: start wakes whose timer has passed. launchd runs this |
| `otto agent start\|stop\|status` | Manage the launchd reviver, or check whether it's loaded and when it last ran |
| `otto serve` | The web UI on `127.0.0.1:7878` (`--port`, `--open`): everything above except `attach`'s typing, in a browser |
| `otto state <cmd>` | The machine surface a wake writes through. Never hand-edit `run.json` |

Every `<id>` above takes the full id, a unique prefix of it, or a unique prefix of just its
slug — `otto show verify` resolves to `2026-09-11-verify-1789130664` as long as no other run
starts the same way, and names the candidates if it doesn't. `otto ls` prints that shortest
form in its `SHORT` column, and every command otto prints for you to paste back uses it.

## Web UI

```bash
otto serve --open        # http://127.0.0.1:7878/
```

Everything a person does from the CLI can be done from the page: the runs list with a **needs
you** section whose option buttons answer a gate in one click, a run's full state and question,
answering in your own words, waking and stopping a run, starting one from a form that mirrors
`otto run`'s flags (with a dry run), the journal with filters and live follow, a read-only view of
the wake running right now (its tmux pane, or `wake.log` for a detached wake), and the reviver:
its status, start/stop, and poke-now.

The server is a **view over the run directory, not an owner of it**. Every action calls the same
code the matching command does, so `otto ls` and the page always agree, and nothing needs the
server running: stop it and runs carry on waking under poke. A wake started from the page is
always backgrounded — `--detach none` means a detached process there, as it does for poke,
never a wake tied to an HTTP request.

It listens on 127.0.0.1 only, with no login, and refuses requests from any other site: a
foreign `Host` (DNS rebinding), a foreign `Origin`, or a POST that is not JSON is a 403. To reach
it from elsewhere, tunnel: `ssh -L 7878:127.0.0.1:7878 <mac>`.

## The contract

This is the only thing otto mechanically enforces, and it is what makes wrapping *anything*
possible. otto validates neither phases nor plans nor progress — with an arbitrary skill there is
no table to validate against. At the moment a wake's process exits, it checks two things:

1. **The run is in a state something will bring it back from** — a gate open, a `nextWakeAt` set,
   or terminal.
2. **This wake rewrote `handoff.md`**, within its cap.

Pass and it is `wake-complete`. Fail — for any reason, including a crash, a kill at its deadline,
or a model that simply stopped talking — and it is `wake-incomplete`: backoff, retry, and after
enough consecutive failures, `blocked` plus a gate explaining what has been happening.

Validation lives in the **parent** process, because a wake that crashed cannot report on itself.
That is what makes v1's worst failure — a conductor ending its turn `running` with no gate and no
wake time, stranded but still reporting `running`, invisible for a day — unrepresentable rather
than merely discouraged.

## Launchers

What runs the model is configurable, and separate from how the wake is backgrounded.

| `--launcher claude` | Plain `claude`. Portable, and the default |
| `--launcher yolo` | [yolo](https://source.datanerd.us/joshuahill/yolo)'s nono sandbox: kernel-enforced filesystem and egress limits. The right choice for unattended work |

otto deliberately does **not** pass `-p`. Print mode bills SDK credits rather than the
subscription, and an unattended run that wakes forever is the last thing that should be on the
wrong meter. A wake is non-interactive anyway: with stdin closed and stdout on a pipe, `claude
'<prompt>'` runs the prompt and exits by itself.

A wake is unattended, so no permission prompt can ever be answered. A prompt-worthy tool does not
hang the wake — a `manual`-mode wake with stdin at `/dev/null` was seen running a Bash command and
exiting cleanly — but what each mode now denies is genuinely unmeasured, because the old answer
(`acceptEdits` denies Bash, `dontAsk` denies Write/Edit/Bash) rested on `--permission-prompts none`,
which only works with `-p`. `bypassPermissions` stays the default until someone measures the rest,
which means **the real safety decision is the launcher, not the permission mode** — run under yolo
and the guardrail is the kernel. Anything outward-facing (push, merge, commenting on a ticket)
additionally needs an `authorize` record bound to one commit.

Under yolo, `--skill` needs `--skills-dir`: the sandbox cannot see `~/.claude`, so otto refuses such
a run up front rather than letting it fail mid-wake.

`--detach tmux` (default) puts each wake in a tmux session named `otto-<run-id>`; the session ends
when the wake does, so nothing needs reaping. `--detach none` (or `--watch`) runs it in your
terminal. Detachment is observability only — the run cannot tell which was used. There is no `bg`
strategy: `claude --bg` is disabled under yolo, because `claude attach` on a re-adopted worker needs
a process-identity probe that execs the setuid `/bin/ps`, which macOS Seatbelt blocks.

Where a run's wakes go is a property of the run, so **every way of starting one agrees**: `otto
run`, `otto answer`, `otto wake` and `otto poke` all honour the recorded `detach`. `otto wake
--watch` keeps one wake in your terminal without changing the run's setting, and `otto wake
--detach tmux|none` overrides it for that wake only — unlike `otto run --watch`, which persists so
that "watch this" does not silently mean "watch the first one and detach the rest". A wake that is
never going to happen (a finished run, `--dry-run`) always answers in your terminal rather than in a
session you would have to attach to.

## What a wake costs

**In tokens, not dollars.** Cost per wake came from `-p --output-format json`, and `-p` is gone for
the reason above; on a subscription there is no per-wake dollar figure to report either. So
`wake-spent` journals what can actually be measured — turns, input/output tokens, and
`cacheRead`/`cacheCreation` — read back from the session transcript claude leaves under
`~/.claude/projects/`. `--budget-usd` is refused rather than silently ignored: a ceiling whose
counter can only ever be zero is worse than no ceiling.

The dollar figures below were measured while otto still ran `-p`. They are kept because the *shape*
is what matters and the shape has not changed — the token counts behind them are the same:

A wake doing trivial work cost **$0.18–$0.25 over 8–10 turns**. Every wake creates ~12k cache
tokens; **cross-wake prompt-cache reuse does not happen**, including for two wakes of the same run
with a byte-identical prompt. (The very large `cache_read` a wake reports is reuse *within* the
wake, across its own turns.)

So a floor per wake is structural, and 200 wakes is 200 cold starts before anything useful happens.
That is the price of durability, and the reason `budget` is enforced rather than advisory —
`--budget-wakes` and `--budget-hours`, either of which blocks the run with a gate when spent, plus
the `maxWakeMinutes` policy to bound a single runaway wake. It is also an argument for fewer, longer
wakes: a chain of work inside one wake pays one cold start, not ten.

## Layout

```
src/
  wake/          the executor: launcher argv, the harness prompt, the contract validator,
                 and transcript.rs — what a wake used, read back from claude's session log
  core.rs        what each person-facing command does, as data — shared by the CLI and the web
  human.rs       the commands a person uses, rendered for a terminal
  server/        `otto serve`: JSON API + event streams over core, and the embedded page
  state/         the only writer of run state — atomic write + journal + per-run lock
  liveness.rs    is a wake running? (an flock, not an inference)
  exec.rs        spawn with a deadline, stdout and stderr kept apart
  detach.rs      tmux, reduced to: start detached, exists, kill
  poke/          the reviver
web/             the page (plain HTML/CSS/JS, compiled into the binary — no build step)
harness/wake.md  the operating procedure every wake is given
workflows/*.md   prose to wrap with --instructions (dev-flow, improve-flow, upgrade-flow)
~/.otto/runs/<id>/   run.json · handoff.md · journal.jsonl · gates/ · artifacts/   ($OTTO_HOME)
```

## Tests

```bash
cargo test     # the whole engine. No tmux, no model, no money
```

Everything is unit-testable because the two things that touch the outside world are injected: `Exec`
(spawning) and `Liveness` (is a wake running). A fake `Exec` can even write to the run directory
mid-spawn, which is how a wake's own behaviour is simulated.

What `cargo test` **cannot** cover is whether the harness prompt, read by a real model, produces
correct behaviour — that is what the live tests (`cargo test -- --ignored`) are for, and it is
where this project's worst bugs have been found. `test/fixtures/hello-flow.md` is the cheapest
end-to-end check: one gate, no real work.
