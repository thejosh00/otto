# otto documentation

New to otto? Read [concepts](concepts.md) first — runs, wakes, the handoff, the period, gates and
notes, and how they fit together. The guides assume it.

## Guides

| Guide | For when you want to… |
|---|---|
| [Living with a run](guides/everyday.md) | see what needs you, answer gates, leave notes, change the period, read the journal, use the web UI |
| [Wrapping work](guides/wrapping.md) | run a skill, a runbook or a bare goal; decide when it's done; write a workflow that runs well over days |
| [Check scripts](guides/check-scripts.md) | stop paying for wakes that find nothing new |
| [Sandboxing wakes](guides/sandboxing.md) | run unattended wakes under a sandbox: launchers in `config.json`, permission modes |
| [Running as a service](guides/running-as-a-service.md) | keep the reviver and the web UI running with launchd; notifications; troubleshooting |

## Reference

| Page | Covers |
|---|---|
| [CLI](reference/cli.md) | Every command and flag — generated from `--help` |
| [`config.json`](reference/config.md) | Launchers, and the environment variables otto reads |
| [`run.json`](reference/run-json.md) | Every field of a run's state |
| [The journal](reference/journal.md) | Every event otto writes |

## Elsewhere

- [DESIGN.md](../DESIGN.md) — why otto is built the way it is: the reasoning, the measurements,
  and the alternatives that were tried.
- [harness/wake.md](../harness/wake.md) — the operating procedure every wake is given. Written for
  the model, not for people, but it is the authority on what a wake does.
- [workflows/](../workflows) — prose workflows to wrap with `--instructions`.

## Keeping these docs honest

Each fact is explained in one place and linked from everywhere else, and `cargo test` checks
what can be checked:

- The [CLI reference](reference/cli.md) is generated from `--help`, and the test fails when it is
  out of date. Regenerate it with `OTTO_UPDATE_DOCS=1 cargo test cli_reference`.
- Every `otto …` command in a code block — here, in the README, in `harness/wake.md` and in the
  workflows — must parse against the real CLI. `<placeholders>` stand for any value, and
  `[--optional flags]` are taken as given.
- Every `config.json` example must load through otto's own config parser.
- Every journal event otto writes, and every field it carries, must appear in
  [the journal reference](reference/journal.md).

These live in `src/doc_examples.rs`.
