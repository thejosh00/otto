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
caffeinate -i -s env -u ANTHROPIC_API_KEY -u CLAUDECODE … nono run --profile nolabs-ai/claude --allow ~/.otto --allow ~/work --allow ~/work/svc -- claude '<prompt>' --add-dir ~/.otto --add-dir ~/work --add-dir ~/work/svc --session-id … --permission-mode … …
```

- **`caffeinate`** keeps the Mac awake for exactly as long as the wake runs (macOS only).
- **`env -u`** clears what a wake must not inherit from whatever started it: an
  `ANTHROPIC_API_KEY`, which claude would bill instead of your subscription, and the variables of
  a Claude Code session the wake was started from. Your launcher runs after it, so a launcher that
  wants API billing can set the key again.
- **`grantFlag`** is how the sandbox opens a directory. otto passes it once for `$OTTO_HOME` —
  the wake must be able to read `run.json` and write its handoff — once for the run's working
  directory, which the wake starts in, and once per `--repo`, placed before the command's first
  `--` so it reaches the sandbox rather than claude. Leave it out and those grants are your sandbox
  profile's job.
- **`--add-dir`** still tells claude about the same directories.

`otto wake <id> --dry-run` prints the exact command for a run.

### What the sandbox profile has to allow

Beyond the directories otto grants, whatever a wake needs belongs to the sandbox's own profile:

- `~/.claude`, so `--skill` can find its skill and claude can write its session transcript (otto
  reads token usage from it; without it, a wake's cost shows as unknown, which is harmless). otto's own
  wrapper (below) gives claude a folder of its own instead, so `~/.claude` can stay closed to writes.
- The network hosts the work needs — your git host, `gh`'s API host, package registries.
- Anything a wrapped skill writes outside its repo. Tool caches it has already: its own (see below).

A wake that can't reach something fails the way it would anywhere: it retries, and after enough
failures the run is `blocked` with a gate saying what went wrong.

### Changing launchers

A run records the launcher's **name**, not its command. Editing a launcher in `config.json`
changes the next wake of every run using it; removing one makes those runs' wakes refuse up front
with a message listing the launchers that exist.

## otto's own sandbox

otto ships a sandbox built on macOS's Seatbelt: a launcher whose command is a wrapper, and a record for each
run saying what it may touch. **Nothing uses it yet.** `otto install` puts the wrapper in place, which opens
nothing and moves no run; a run is under it only once a person has given it both a launcher and a record, and
the commands for that come later (`plans/v3.md`, 1.7). A run with no record behaves exactly as it did.

```json
{ "name": "sandbox", "command": "~/.otto/sandbox/wrapper -- claude", "grantFlag": "--allow" }
```

Words in front keep working (`env LEGENDA_DIR=… ~/.otto/sandbox/wrapper -- claude`), and that variable arrives.

### The record decides

`$OTTO_HOME/sandbox/runs/<id>.json` is written by a person and can't be written by any wake:

```json
{
  "launcher": "sandbox",
  "workdir": "/Users/me/workspace",
  "read": ["/Users/me/workspace/lib"],
  "write": ["/Users/me/workspace/svc", "/Users/me/workspace/.work-trees/svc"],
  "mode": "enforce"
}
```

- **`workdir`** is where the wake starts. Being listed there opens nothing: `~/workspace` is every project.
  `claude` starts in a working directory it can't read, without its `CLAUDE.md` or `.claude/`. Those aren't
  read, so anything that a wake needs from them has to be in a repo the record lists.
- **`read`** and **`write`** are what's open, and writing implies reading. A path under `$OTTO_HOME` is allowed,
  which is how a system agent's writes into other runs are said.
- **`mode`** is `enforce`, the default, or `report`, which allows the same things and logs each (for 1.7).
- An unknown field is refused, so a mistyped key can't quietly change what's open.

The `--allow` otto passes come from `run.json`, which a wake can write, so the wrapper ignores them.

### What the wrapper does

Each time, outside the sandbox: read the record; generate `$OTTO_HOME/sandbox/profiles/<id>.<session>.sb`, never
reusing one, for the claude session otto names in `OTTO_SESSION_ID` (refusing if there's none); check that `sandbox-exec` takes it; read claude's access token from the Keychain and put it on descriptor 3;
clear what would carry a credential in (`GH_TOKEN`, `GITHUB_TOKEN`, `SSH_AUTH_SOCK`, `GIT_ASKPASS`, `GIT_CONFIG*`,
anything ending `_TOKEN`, `_SECRET`, `_PASSWORD`, `_PASSPHRASE`, `_API_KEY` or `_ACCESS_KEY`) and `TMUX`; point
the tools' caches at the run's own (below); lay out the run's own Claude folder and set `CLAUDE_CONFIG_DIR` to it
(below); exec `sandbox-exec` with the command after its `--`.

### Each run's own caches

The shared caches are read-only from inside: `~/.cache` (uv, pre-commit, oh-my-zsh and the rest), `~/.cargo/registry`
and `~/.cargo/git`, `~/.npm`, `~/.bun/install/cache`. An unsandboxed build later runs what's in them, so a wake that
changed a `build.rs` in the registry would run it outside. Instead each sandboxed run gets
`$OTTO_HOME/sandbox/caches/<id>/`, made by the wrapper and writable only by that run's wakes, and the wrapper sets
`CARGO_HOME`, `npm_config_cache`, `BUN_INSTALL_CACHE_DIR`, `UV_CACHE_DIR`, `PIP_CACHE_DIR`, `GOCACHE` and
`XDG_CACHE_HOME` to directories in it. Nothing outside the sandbox reads it; later wakes of the same run reuse it.

### Each run's own Claude folder

Every claude session, yours included, sources its shell snapshot before each Bash command, sources the files in its
`session-env`, and reads its memory under `~/.claude/projects` as it starts. So nothing in `~/.claude` (or
`~/.claude.json`) is written from inside, whatever the record says. Instead claude starts with
`CLAUDE_CONFIG_DIR=$OTTO_HOME/sandbox/claude/<id>`, a folder of the run's own that its wakes and chats share and
nothing else can write. What claude writes as it works lands there: transcripts (`projects/`, where otto reads
usage from first), shell snapshots, `session-env`, `sessions`, `backups`, `.claude.json` and the rest.

Before each wake or chat, outside the sandbox, the wrapper lays it out:

- **Links to yours**, for each of `settings.json`, `CLAUDE.md`, `skills`, `commands`, `agents`, `plugins` and
  `hooks` that you have in `~/.claude`, so each wake sees your current settings and skills. They're read-only from
  inside: a wake can't replace one, write through one, or add one you don't have (nor `settings.local.json`), in
  any case (`Skills` is `skills` on disk). So no wake changes what the next one runs.
- **`.claude.json`**, written afresh each time: onboarding done, and the record's `workdir` and `write` trusted.
  Nothing of yours is copied: no MCP servers, no account, no credentials. Signing in is unchanged: the token
  arrives on descriptor 3, and claude never looks in the Keychain.
- Nothing there is followed: the folder is opened without following a link, a link standing where the folder
  should be is removed rather than entered, and each step is made relative to it. A wake can't move the folder.

The cost: a run's wakes get memory of their own, starting empty, rather than yours (the files stay readable), and
MCP servers configured in your `~/.claude.json` aren't there.

The cost: a run's first build downloads every crate and package again, and `cargo build --offline` fails until its
cache has them. Moving `CARGO_HOME` doesn't lose the toolchain (rustup keeps that in `RUSTUP_HOME`) or
`~/.cargo/config.toml` for a project under your home directory (cargo walks up to it); `cargo install` from inside
puts binaries in the run's cache, not on your `PATH`. Nothing deletes a run's caches yet: remove
`$OTTO_HOME/sandbox/caches/<id>` by hand when a run is gone.

### A run that disagrees with its record

A wake has to write its own `run.json`, so nothing in it can be allowed to choose the next wake's sandbox. Before
`otto wake` or `otto chat` starts anything, it compares the run to its record, on the same state its command line
is built from: the launcher as `config.json` resolves it, the working directory the wake would actually start in
(the config's default, for a run that doesn't record one), and each repo, which has to be listed in `read` or
`write`. Paths compare as the kernel sees them, so a trailing slash or `~` doesn't matter and a link counts as
where it goes. `id` is compared too, since `OTTO_RUN_ID` comes from it and is how the wrapper finds a record.

`permission.*` isn't compared: it goes after the wrapper's `--`, so whatever it says reaches only what is already
inside the profile. Nor is `launcher.detach`, an enum choosing how `otto wake` itself starts, outside the sandbox,
with the wrapper still in front of its child. A run with no record isn't compared at all; a record that can't be
read is a refusal.

If they differ, nothing starts, and:

- the journal has a [`sandbox-refused`](../reference/journal.md) line saying what differed;
- the run is `blocked (sandbox)` with a question, so poke leaves it alone and spends no retries on it;
- an older binary reads all of this, seeing `blocked (instructions)` with the same reason.

**The way back** is to make them agree, by putting `run.json` back or changing the record. Then answer the question,
or `otto wake` it. To take a run out of the sandbox, delete the record and put its launcher back to one that isn't
the wrapper. Answering while they still differ gets the same refusal and the same question.

**In the wrapper, too, anything wrong and nothing starts.** No record, a bad record, no sign-in, a profile that won't compile: it says
why on stderr and in the journal as [`sandbox-refused`](../reference/journal.md), and never runs the command
outside the sandbox. Keeping the token fresh isn't built yet (1.8), so a run left alone long enough stops with
a sign-in message.

### Links in a run directory

A wake can write its own run directory, and otto, outside the sandbox, writes there too. So otto never writes
through a link there: if the file, or any folder between the run directory and it, is a link — `journal.jsonl`,
`run.json`, `handoff.md`, `gates/`, `notes/`, `changes/`, `outputs/`, `messages/`, `wake.log`, a lock — it
refuses with "a link was found in the run directory and was not followed" and writes nothing. The journal is
also opened with `O_NOFOLLOW`. The same goes for what otto reads to decide what to do: `run.json`, the journal,
gates, notes, check scripts, indexes. That error is the sign of a wake trying to aim otto at a file of the
person's (a journal linked to `~/.zshenv`); look at where the link points before removing it.

Three things otto only *shows* the person still follow a link, and only when, fully resolved, it lands in the
run's own directory, its workdir, or a repo it may write (the sandbox record's `write` list if it has one, else
`launcher.repos`): `artifacts/todo.md`, a message's attachments, and a file given to `otto state publish --file`.
A link anywhere else isn't shown.

Values a refusal quotes out of `run.json`, like the launcher's name, are quoted with escapes, so they can't carry
a newline into a question or a log.

### What's closed

| | |
|---|---|
| **Writes** | Closed by default. Open: the record's `write`, temporary directories, the run's own caches, the run's own Claude folder (not its links to `settings.json`, `CLAUDE.md`, `skills`, `commands`, `agents`, `plugins` or `hooks`), `~/Library/Caches`, its own run, repo locks, `feedback.jsonl` |
| **Your Claude folder** | `~/.claude` and `~/.claude.json`, closed to writes whatever a record says: shell snapshots, `session-env`, memory, transcripts, settings. Still readable |
| **Other projects** | Reads and writes, under the configured working directory and every configured project, unless the record lists them |
| **In anything writable** | `.git/hooks`, `.git/config`, `config.worktree`, `.claude/settings*.json`, `.claude/hooks`; moving a repository or its `.git`. No record opens these |
| **otto's** | `config.json`, `feedback-driver`, `sandbox/` (but for the run's own `sandbox/caches/<id>` and `sandbox/claude/<id>`), all closed to writes whatever a record says |
| **Shared caches** | Read-only: the shared tool caches (see above). `~/.cache/oh-my-zsh`, which your shell sources, and `/private/tmp/tmux-<uid>`, the real tmux server's directory, are closed to writes whatever a record says |
| **Credentials** | The Keychain and a list of files, listed in `src/sandbox.rs`; `gh` doesn't work, even for public reads. An app's own secrets (`.config/cloudgpu`, `.config/wrangler`, `.omnifactum/token`) are closed unless the record lists them |
| **Acting from outside** | `launchctl`, `open`, `osascript`, `security`, the services behind them, the real tmux server, the ssh agent, docker's socket, and **otto's own web page**, which approves changes for anyone who asks |

`tests/sandbox.rs` tries each of these against the real `sandbox-exec`, on a scratch home. `test/sandbox/real-wake.sh`
runs one real wake and one real chat under it, on a throwaway `OTTO_HOME` with your real home and sign-in, and prints
exit codes and byte counts only.

### What it doesn't close

Not a promise past the list above:

- **Reads are a denylist.** Anything readable and not named is readable, and the network is open, so it can be
  sent. A new credential file is open until someone adds it.
- **Other local services.** Only otto's page and docker's socket are closed. Anything else listening on the
  machine can be reached.
- **Reading `~/.claude/projects`**, other sessions' transcripts, some with secrets in tool output. Closed to writes
  now, and still readable.
- **Files in a writable repo that run later:** `.envrc`, `.vscode/`, `Makefile`, `.mcp.json`. Under a person's
  own tools these run with everything. Give a run its worktree and not the main checkout where that matters.
- **A worktree's `.git` file** can be pointed at a directory of a wake's own making, with hooks in it, which
  runs in whoever next uses git in that worktree (the lander, until 1.5).
- **`~/Library/Caches`** stays writable, as macOS frameworks keep caches there. pip's and Go's are moved to the
  run's own; anything else that caches there and runs it later isn't.
- **Signals.** A wake can kill other processes of yours.
- **Skills linked from a closed project** (`~/.claude/skills/x -> ~/workspace/otto/skills/x`) can't be read unless
  the record lists them.
- **What's listed under 1.3 to 1.5** is separate work.
- **A rollback past this comparison.** An older binary ignores the record, so `run.json` decides again until it's reinstalled.

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
