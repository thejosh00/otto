# Living with a run

What you actually do once a run exists: see what needs you, answer it, steer it, and look at what
happened. Everything here works from the terminal or the [web UI](#the-web-ui).

## Naming a run

Every run has a number and a name: `#7 legenda`. The number is handed out in order and never
reused; the name is yours — give it with `otto run --name legenda`, or let otto make one up from the
target, the wrapped skill or the first words of the goal. Names are unique, and `otto rename 7
picks` changes one at any time.

Every command that takes a run takes either: `otto show 7` (or `#7`), `otto show legenda`, or a
unique prefix of the name — `otto show leg`. A number only ever matches exactly, so `1` never means
run 12. `otto ls` shows both in its `ID` and `NAME` columns, and every command otto prints for you
to paste back uses the number.

Runs from before numbering keep their `<date>-<slug>` ids until you run `otto renumber`, which
gives each one a number, oldest first, and a short name from its old slug. It skips any run with a
wake in flight. The old id still works afterwards — it is kept as `formerId`.

## What needs me?

```bash
otto inbox       # every question waiting on you, and every message a run has sent, newest first
otto ls          # every live run: status, what it waits on, next wake, and a line on what it's doing
otto ls --all    # include finished runs
```

`otto inbox` is the short answer: questions to answer and [messages](../concepts.md#messages-and-the-inbox)
to read, from every run. Below `otto ls`'s table, a `needs you:` summary gives a ready-to-paste
answer command for each run waiting on a person.

## Reading what a run sent you

```bash
otto inbox read my-run          # its newest open message, in full — with any draft to send on
otto inbox read my-run 3        # message 3
otto inbox done my-run 3        # done with it
otto inbox done my-run 3 --reply "dig into the grader next" --now   # and tell the run, and wake it
otto inbox --all                # what you've answered and read, too
```

A draft is text for you to send on — a reply to a question in a channel, a comment on a ticket. The
run never sends it itself.

```bash
otto show        # with no id: the one run waiting on you, or the only live run
otto show my-run # where it stands, its handoff, and any open question in full
```

## Answering a gate

```bash
otto answer my-run --choice approve   # pick one of the gate's named options
otto answer my-run --text "…"         # or answer in your own words
otto answer my-run --file notes.md
otto answer                           # on a terminal: a numbered menu; Enter takes the default
```

The answer is recorded verbatim and the next wake starts right away (`--no-wake` records it and
leaves the run for poke). `--choice` is checked against the gate's own options.

## Steering without being asked

```bash
otto note my-run "skip the e2e suite, it's broken on main"   # the next wake reads it
otto note my-run "never touch legacy/" --standing            # every wake, until dropped
otto note my-run "rebase first" --now                        # and wake it to read it now
otto note my-run --list                                      # what's pending
otto note my-run --drop 2
```

A note changes how the run works, not what it's for — see [notes](../concepts.md#notes).

## Changing how often it wakes

```bash
otto period my-run        # show it
otto period my-run 4h     # change it
```

If the run is sleeping on its period, that sleep moves to the new period (sooner or later). A
timer a wake armed for its own reason is kept, and the new period applies after it. To spend
tokens only when something changed, add a [check script](check-scripts.md).

## Looking at what happened

```bash
otto logs my-run                # the journal: a status line, local times, a rule per day
otto logs my-run -f             # keep following
otto logs my-run --decisions    # just the turning points: gates, answers, status changes
otto logs my-run --since 2h --event gate-opened,gate-closed
otto attach my-run              # follow what the running wake is saying and doing, live
```

[The journal reference](../reference/journal.md) lists every event.

## How much is it using?

A wake's cost is tokens, read from the session transcript claude leaves behind:

```bash
otto usage                  # every run, the last 7 days: wakes, and tokens in, out, cache write, cache read
otto usage --by day         # the same, one row per day
otto usage --since 30d      # or a date: --since 2026-09-01
otto usage --all --json     # everything, exact counts, for a script
```

`otto show <id>` has a `tokens` line with what that run has used over its whole life, and the web
UI has the same on each run page and a **Usage** page with the table above. A wake whose transcript
couldn't be read — usually a sandbox that hides `~/.claude` — is counted and marked, rather than
quietly adding zero. For a polling run, a [check script](check-scripts.md) is the biggest saving
there is: `otto check <id>` shows how many wakes it has spared.

## Waking, stopping, resuming

```bash
otto wake my-run                # one wake now, backgrounded the way the run asks
otto wake my-run --watch        # this one in your terminal
otto wake my-run --dry-run      # print the command it would run
otto pause my-run --for 2d      # hold back its scheduled wakes; --until 09:00, or neither for "until resumed"
otto stop my-run                # retire it (stopped); --failed if it couldn't do its job
otto resume my-run              # lift a pause; or bring a stopped or failed run back, and wake it
otto resume my-run --no-wake    # back on its schedule, no wake now
```

A pause holds back everything scheduled — the period, timers, check scripts — while a wake already
running finishes and questions stay open. Answering, a note with `--now`, and `otto wake` still work.

Stopping a run kills any wake in progress and releases its repo locks.

`otto attach` (and the web UI's **Live** tab) follows the wake's session transcript as it's
written: what the model says (●), each tool call it makes (▸ — the command, the file, the query),
the first lines of what came back, and errors (✗). Ctrl-C stops watching; the wake carries on.
When the transcript can't be read — a sandbox that hides `~/.claude` — it falls back to the wake's
tmux session.

## Where wakes run

By default each wake runs in a tmux session named `otto-<run-id>`, which ends when the wake does, so
there is nothing to clean up. `otto run --detach none`
runs them as plain background processes instead, and `otto run --watch` keeps them in your
terminal. This is only about watching: the run behaves identically either way, and `otto run`,
`otto answer`, `otto wake` and `otto poke` all honour the run's setting. `otto wake --watch` or
`--detach` overrides it for a single wake.

## The web UI

```bash
otto serve --open          # http://127.0.0.1:7878/
otto service start         # or keep it running from login — see running-as-a-service.md
```

Everything above, in a browser, in plain words — a run is an *agent* there, a wake is *a run* of
it, and a gate is a *question*:

- **Agents**: one line each — what it's doing, a glyph for whether it is waiting on you, working,
  watching, paused or needs a look, and when it runs next.
- **The inbox**, with a count in the top bar: questions to answer and messages to read, from every
  agent. Answer with a click, copy a draft, reply, mark done.
- **An agent's page**: its question if it has one, what it would like from you, a strip of its last
  week (each run, whether it did something, when it waited on you), its checklist when it keeps one,
  what it has sent you, what happened lately in words, and one box to tell it something (pin it to
  keep it). Pause, run now and stop are at the top; the handoff, the log, a live view, cost and
  settings are under the hood.
- **New agent**: describe the job; everything `otto run` can set is folded under more options.
- **otto itself**: a health line in the top bar — the reviver, and agents that couldn't start —
  with the reviver's controls and **usage** a click away.

The server is a view over the run directory, not an owner of it — every button calls the same
code as the matching command, and runs carry on waking under poke whether it is running or not.
It listens on 127.0.0.1 only, with no login, and refuses requests from any other website. To
reach it from another machine, tunnel: `ssh -L 7878:127.0.0.1:7878 <mac>`.
