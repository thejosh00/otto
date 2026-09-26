# Living with a run

What you actually do once a run exists: see what needs you, answer it, steer it, and look at what
happened. Everything here works from the terminal or the [web UI](#the-web-ui).

## Naming a run

Every command that takes a run takes its full id (`2026-09-11-verify-1789130664`), a unique prefix
of it, or a unique prefix of just the part after the date — `otto show verify` works as long as
no other run starts with `verify`. `otto ls` prints that shortest form in its `SHORT` column, and
every command otto prints for you to paste back uses it.

## What needs me?

```bash
otto ls          # every live run: status, what it waits on, next wake, period, wakes spent
otto ls --all    # include finished runs
```

Below the table, a `needs you:` summary gives a ready-to-paste answer command for each run
waiting on a person.

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
otto attach my-run              # watch the wake running right now (its tmux session)
```

[The journal reference](../reference/journal.md) lists every event.

## Waking, stopping, resuming

```bash
otto wake my-run                # one wake now, backgrounded the way the run asks
otto wake my-run --watch        # this one in your terminal
otto wake my-run --dry-run      # print the command it would run
otto stop my-run                # retire it (stopped); --failed if it couldn't do its job
otto resume my-run              # bring a stopped or failed run back, and wake it
otto resume my-run --no-wake    # back on its schedule, no wake now
```

Stopping a run kills any wake in progress and releases its repo locks.

## Where wakes run

By default each wake runs in a tmux session named `otto-<run-id>`, which ends when the wake does —
so `otto attach` can show it live and there is nothing to clean up. `otto run --detach none`
runs them as plain background processes instead, and `otto run --watch` keeps them in your
terminal. This is only about watching: the run behaves identically either way, and `otto run`,
`otto answer`, `otto wake` and `otto poke` all honour the run's setting. `otto wake --watch` or
`--detach` overrides it for a single wake.

## The web UI

```bash
otto serve --open          # http://127.0.0.1:7878/
otto service start         # or keep it running from login — see running-as-a-service.md
```

Everything above, in a browser:

- The **runs list**, with a *needs you* section whose option buttons answer a gate in one click.
- A **run page**: its state, handoff and open question; answering in your own words; notes;
  wake, stop, resume and change-period buttons; the check script; the journal with filters and
  live follow; and a read-only view of the wake running right now.
- A **new run** form that mirrors `otto run`'s flags, with a dry run.
- The **reviver**: its status, start/stop, and poke-now.

The server is a view over the run directory, not an owner of it — every button calls the same
code as the matching command, and runs carry on waking under poke whether it is running or not.
It listens on 127.0.0.1 only, with no login, and refuses requests from any other website. To
reach it from another machine, tunnel: `ssh -L 7878:127.0.0.1:7878 <mac>`.
