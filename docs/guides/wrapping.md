# Wrapping work

otto contributes none of the doing. It takes something an agent already knows how to do and makes
it survive days. This guide covers what you can wrap, how a run knows it's finished, and what
makes a workflow run well across many wakes.

## Three kinds of thing

```bash
# a Claude Code skill, by name
otto run --skill manage-pr --goal "get branch feat/x reviewed and merged" --repo ~/work/svc

# any prose file: a runbook, a workflow table, a ticket
otto run --instructions workflows/dev-flow.md --goal "PROJ-123 to merged" --target PROJ-123

# nothing but a goal
otto run --goal "Find and fix flaky tests in this repo, one per day" --perpetual
```

`--goal` is always required: it is the one thing that survives every wake unaltered, and the only
thing that can say when to stop. `--repo` grants a wake access to a repository (repeatable).
`otto run --dry-run` prints the run's id and its first wake's command, and creates nothing.

## How a skill that has never heard of otto survives

Every wake is given a harness ([harness/wake.md](../../harness/wake.md)) that sits between the
wrapped instructions and the run directory, and translates what a skill assumes about a
continuous session into something durable:

| The wrapped thing says | The wake does |
|---|---|
| "ask the user", "confirm with them" | Opens a [gate](../concepts.md#gates) and exits |
| "wait", "poll", "check back in an hour" | Exits; the period brings a wake back |
| "remember", "keep track of" | Writes it — the handoff for one wake, an artifact for longer |
| Anything assuming earlier steps are in context | Reads the handoff and artifacts; there is no earlier context |

These are instructions to a model, so they are occasionally ignored. The backstop is the
[contract](../concepts.md#the-contract): a wake that ignores them doesn't leave the run in a
state anything will bring it back from, and otto turns it into a retry. The failure mode is a
wasted wake, not a lost run.

## When is it done?

A skill has no idea when "done" is. otto makes it explicit:

- `--until "<condition>"` states it outright — the most reliable choice.
- Otherwise the **first wake proposes a done-condition and asks you** to confirm it, before any
  work (it may skip the question when the goal already says what finished means).
- `--perpetual` says there is no done — only retirement with `otto stop`.

The done-condition is re-read every wake and never changed without a gate.

## What otto can't do

- **A skill that wants a person mid-loop becomes one gate per question.** Correct, and slower — a
  five-question skill is five wakes. Fine over days, absurd over minutes.
- **Short one-shot skills gain nothing.** Wrapping something that finishes in one session is pure
  overhead.
- **It can't make vague instructions rigorous.** It guarantees resumability, not competence; a
  skill that was ambiguous in one session is ambiguous across forty wakes, at forty times the
  cost. The done-condition question is the cheapest place to catch that.

## Writing a workflow that runs well

None of this is required — a run that ignores all of it still works — but it is what a workflow
built for long horizons does. In rough order of value:

1. **Phases with exit conditions, and idempotency.** Every phase is re-entered, after a crash or a
   retry, so its first act is to check whether its own exit condition already holds and adopt
   existing work (an existing worktree, an existing PR) rather than redo it.
2. **Gates written as decisions.** Named options so `otto answer --choice` works, each with its
   consequence, a stated default, and a pointer to what to read.
3. **A ledger when the handoff won't fit.** The handoff is capped and rewritten; anything that
   accumulates — one line per target, per proposal, per cycle — goes in an append-only artifact.
   It doubles as the position marker: a crash at target 12 of 20 resumes at 12.
4. **Cheap ticks.** A polling phase should be one cheap look and `otto state tick`, not a full
   re-plan — and where a shell script can do the look, a [check script](check-scripts.md).
5. **Outward actions behind authorization.** Push, merge and comment only with a recorded
   authorization bound to the commit ([concepts](../concepts.md#outward-actions-and-repo-locks)).

Perpetual runs change a few habits: retire with `stopped` rather than `failed`; set
`--policy maxTicksWithoutProgress=0`, since ticks are *expected* to find nothing; take repo locks
per cycle, never for the life of the run; and give each cycle its own artifact directory.

[DESIGN.md §11](../../DESIGN.md#11-authoring-for-otto) has the reasoning behind each of these.

## Workflows that ship with otto

| File | What it does |
|---|---|
| [workflows/dev-flow.md](../../workflows/dev-flow.md) | A ticket (or a described task) to reviewed, merged code: plan, review, implement, verify, push, babysit the review, merge |
| [workflows/improve-flow.md](../../workflows/improve-flow.md) | Propose one improvement a day to a repository, perpetually |
| [workflows/upgrade-flow.md](../../workflows/upgrade-flow.md) | Carry a dependency bump across many repositories |
| [test/fixtures/hello-flow.md](../../test/fixtures/hello-flow.md) | The cheapest end-to-end check of otto itself: one gate, no real work |

`otto install` (run by `./install.sh`) also symlinks the skills otto ships, such as `manage-pr`,
into `~/.claude/skills/`.
