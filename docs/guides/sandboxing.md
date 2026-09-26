# Sandboxing wakes

A wake is unattended, so nobody is there to answer a permission prompt. That makes **what runs the
model** the real safety decision for anything unattended — and the place to make it is a
**launcher**.

## Launchers

A launcher is the command a wake runs under. `claude` is built in and is the default; any other —
typically a sandbox that wraps claude — is defined per machine in `~/.otto/config.json`
(`$OTTO_HOME/config.json`):

```json
{
  "launchers": [
    { "name": "nono (sandbox)", "command": "nono run --profile nolabs-ai/claude -- claude", "grantFlag": "--allow" }
  ]
}
```

Then pick it per run:

```bash
otto run --launcher nono --goal "…"      # a unique prefix of the name will do
```

The web UI's new-run form lists every launcher in the config. [The config
reference](../reference/config.md) has every field.

### How otto builds the command

otto appends the wake's prompt and claude's own flags to your `command`, so the command must end in
`claude`, or in something that hands its trailing arguments to claude. For the launcher above, a
wake runs:

```
caffeinate -i -s nono run --profile nolabs-ai/claude --allow ~/.otto --allow ~/work/svc -- claude '<prompt>' --add-dir ~/.otto --add-dir ~/work/svc --session-id … --permission-mode … …
```

- **`caffeinate`** keeps the Mac awake for exactly as long as the wake runs (macOS only).
- **`grantFlag`** is how the sandbox opens a directory. otto passes it once for `$OTTO_HOME` —
  the wake must be able to read `run.json` and write its handoff — and once per `--repo`, placed
  before the command's first `--` so it reaches the sandbox rather than claude. Leave it out and
  those grants are your sandbox profile's job.
- **`--add-dir`** still tells claude about the same directories.

`otto wake <id> --dry-run` prints the exact command for a run.

### What the sandbox profile has to allow

Beyond the directories otto grants, whatever a wake needs belongs to the sandbox's own profile:

- `~/.claude`, so `--skill` can find its skill and claude can write its session transcript (otto
  reads token usage from it; without it, a wake's cost shows as unknown, which is harmless).
- The network hosts the work needs — your git host, `gh`'s API host, package registries.
- Anything a wrapped skill writes outside its repo, such as a tool cache.

A wake that can't reach something fails the way it would anywhere: it retries, and after enough
failures the run is `blocked` with a gate saying what went wrong.

### Changing launchers

A run records the launcher's **name**, not its command. Editing a launcher in `config.json`
changes the next wake of every run using it; removing one makes those runs' wakes refuse up front
with a message listing the launchers that exist.

## Permission mode

`--permission-mode` (default `bypass-permissions`) is passed to claude for every wake, along with
`--allow-tool` / `--deny-tool` lists. With nobody attached, a prompt can never be answered, and
what each stricter mode actually denies in a non-interactive wake hasn't been measured — so the
default stays permissive, and **the guardrail is the launcher**: under a sandbox, the kernel
decides what the wake can touch. The stricter modes (`plan`, `manual`) are useful for a
*supervised* run, to see what a wrapped skill would do without letting it act.

Anything outward-facing — pushing, merging, commenting on a ticket — additionally needs an
[authorization](../concepts.md#outward-actions-and-repo-locks) bound to one commit, whatever the
permission mode.

## Why there's no `claude --bg` strategy

`claude --bg` / `claude attach` need a process-identity probe that runs the setuid `/bin/ps`,
which macOS's sandbox blocks unconditionally. A backgrounding strategy that can't work inside a
sandbox isn't offered; wakes run in tmux or as plain background processes instead (see
[where wakes run](everyday.md#where-wakes-run)).
