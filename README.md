<p align="left"><img src="assets/logo.svg" alt="otto" width="280" height="105"></p>

**Turn any Claude command into an agent that works for days.**

otto takes a thing Claude knows how to do — an existing Claude Code skill, a runbook, a prose
instruction, or just a goal — and makes it an agent that keeps working for days, without a session
that has to stay alive. It asks you when it needs a decision, puts what it finds in your inbox, and
keeps everything it makes where you can find it again.

- Explain one failing eval a day, forever.
- Carry a ticket from research through a design you approve to filed Jira tickets.
- Watch a channel and draft a reply to every question for you to send.
- Babysit a PR through a week of review.

otto contributes none of the doing. It contributes **durability**: state on disk, questions that
wait for your answer, an inbox, every version of the work kept, timers that survive a reboot, a
second look before anything is called done, and a mechanically enforced guarantee that every
stopping point can be resumed cold.

> Sessions end. The work doesn't.

<p align="center"><img src="assets/demo.png" alt="An agent's page in otto's web UI: what it is doing, a report waiting in the inbox, and its work — a week of daily reports with today's open" width="800"></p>
<p align="center"><sub>The web UI from <code>otto serve</code>: an agent that explains one failing eval a day — what it's doing, today's report waiting in your inbox, and every report it has made.</sub></p>

## How it works

A **run** is a goal plus what it wraps plus state on disk. A **wake** is one non-interactive
`claude` process that orients from the run directory, does a stretch of work, reaches a stopping
point, and exits. There is no long-lived session and no daemon that owns a run.

```
otto run ──> wake 1 ──> opens a gate, exits
                          │
             otto answer ─┘──> wake 2 ──> sleeps, exits
                                            │
                          otto poke (every 5 min) ──> wake 3 ──> done
```

Three things start a wake: its period coming round, a person answering a gate, or you asking.
Each wake starts cold and re-derives the world, because between two wakes `main` moved, CI reran,
and someone force-pushed.

A wake reaches you three ways: a **question** (a gate) it stops on until you answer, a **message**
in your inbox — a finding, a draft for you to send, news — and an **output**, the work itself, kept
on the run's page with every version. The web UI calls a run an *agent*, and says all of this in
plain words. [Concepts](docs/concepts.md) has the whole picture.

## Install

```bash
./install.sh             # add --service to also run the web UI from login
```

Builds `otto` into `~/.local/bin`, symlinks the skills otto ships into `~/.claude/skills/`, and
registers the launchd reviver that wakes sleeping runs. macOS, Rust and `claude` required; tmux
recommended. The first `otto run` asks where wakes should run (say, `~/work`) and remembers it —
or set it up front with `otto config workdir ~/work`.

## A first run

```bash
otto run --instructions test/fixtures/hello-flow.md \
         --goal "Smoke-test otto's machinery" \
         --until "artifacts/farewell.md exists and the run is done"

otto inbox                    # questions waiting on you, and what runs have sent you
otto outputs <id>             # what a run has made: its reports and documents, every version
otto ls                       # every run, and a line on what each is doing
otto pause <id> --for 2d      # hold back its schedule; `otto resume <id>` lifts it
otto show                     # the one that needs you, and its question in full
otto answer --choice Approve  # answer it
otto logs <id> -f             # the journal, readably (the run's number, or its name)
otto serve --open             # or all of the above in a browser
```

Real work looks like:

```bash
otto run --perpetual --period 1d \
         --goal "Each day, pick one failing LLM eval, work out why it fails, and send me a report"
otto run --target PROJ-4411 \
         --goal "A change design doc for PROJ-4411: research, then a design, plan and CDD I approve, then its Jira tickets"
otto run --perpetual --period 10m \
         --goal "Watch #ask-ai for @aeon-hero mentions; explain each question to me and draft a reply for me to send"
otto run --skill manage-pr --goal "get branch feat/x reviewed and merged" --repo ~/work/svc
otto run --instructions workflows/dev-flow.md --goal "PROJ-123 to merged" --target PROJ-123
```

## Documentation

- **[Concepts](docs/concepts.md)** — runs, wakes, the handoff, checking done, the period, gates,
  notes, messages and the inbox, outputs, pausing, check scripts, recovery.
- **Guides** — [living with a run](docs/guides/everyday.md) ·
  [wrapping work](docs/guides/wrapping.md) · [check scripts](docs/guides/check-scripts.md) ·
  [sandboxing](docs/guides/sandboxing.md) · [running as a service](docs/guides/running-as-a-service.md)
- **Reference** — [CLI](docs/reference/cli.md) · [`config.json`](docs/reference/config.md) ·
  [`run.json`](docs/reference/run-json.md) · [the journal](docs/reference/journal.md)
- **[DESIGN.md](DESIGN.md)** — why it's built this way.

## Development

```bash
cargo test                    # the whole engine: no tmux, no model, no money
test/live.sh                  # live tests against a real claude, side by side: ~5 min, real tokens
test/live.sh live_devflow     # dev-flow from ticket to merge, and stopped at ready-to-push: ~10 min
```

Everything is unit-testable because the two things that touch the outside world are injected:
`Exec` (spawning) and `Liveness` (is a wake running). What `cargo test` can't cover is whether the
harness prompt, read by a real model, produces the right behaviour — the live tests are for that,
and it is where this project's worst bugs have been found.

```
src/
  wake/          the executor: launcher argv, the harness prompt, the contract, token accounting
  core.rs        what each person-facing command does, as data — shared by the CLI and the web
  human.rs       those commands, rendered for a terminal
  server/        `otto serve`: a JSON API over core, and the embedded page
  state/         the only writer of run state — atomic write, journal, per-run lock
  messages.rs    what a run sends you; inbox.rs gathers them, with its questions, across runs
  outputs.rs     what a run makes, kept: keys, versions, stages, drafts, your reviews
  story.rs       a run in plain words: its line, its week, what happened lately
  poke.rs        the reviver
  config.rs      config.json: launchers
  launchd.rs     the reviver and web service as launchd agents
web/             the page (plain HTML/CSS/JS, compiled into the binary — no build step)
harness/wake.md  the operating procedure every wake is given
workflows/       prose workflows to wrap with --instructions
docs/            the documentation; docs/reference/cli.md is generated by `cargo test`
```

`docs/reference/cli.md` is generated from the CLI's own help; after changing a flag, regenerate
it with `OTTO_UPDATE_DOCS=1 cargo test cli_reference`.
