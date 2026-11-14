# Check scripts

A polling run — babysit a PR, watch a deploy, wait for a ticket to move — spends most of its
wakes finding nothing new, and every wake is a cold model session. A check script answers "is
there anything new?" with a shell script instead, so the model only runs when there is.

**Usually the run writes its own.** Give it a goal like "answer any PR questions that appear" and
the first wake does the work that's there, then writes a check script and makes it the run's
standing check (see [the run's own check](#the-runs-own-check)). You write one yourself when it
didn't, or when you want a different one — yours always wins.

## How it works

When the run's wake comes due, poke runs your script **instead of** waking the run:

| The script | What happens |
|---|---|
| exits `0` — nothing new | No wake. The run sleeps another period, and nothing is journaled (the [check history](#check-history) has it) |
| exits `1` — something changed | A real wake, as if there were no check |
| exits anything else, crashes, or runs past 10s | A real wake too — otto fails open rather than trust a broken script |

So the run's **period is how often it looks**, and the check decides whether looking needs a
model. To look every 15 minutes but only spend tokens when something changed:

```bash
otto period my-run 15m
otto check my-run --script ./pr-changed.sh
```

There is no separate check interval: the check runs exactly when the run would have woken.

## The safety net

A script that is wrong *without failing* — it exits 0 when something did change — would keep a
run asleep forever. So after **24 "nothing new" results in a row**, the run wakes anyway. Every
real wake starts the count again.

```bash
otto check my-run --script f --wake-after 96   # a looser net: with a 15m period, once a day
otto check my-run --script f --wake-after 0    # no net: only a change or an error wakes it
```

## Writing one

A check script is a small executable that looks at the world and exits:

```sh
#!/bin/sh
# Wake the run when the branch has a commit newer than the last one it handled.
set -e
found=$(git ls-remote https://github.com/acme/svc.git refs/heads/main)
head=${found%%[!0-9a-f]*}
[ -n "$head" ] || { echo "no refs/heads/main there"; exit 2; }
cursor="$OTTO_HOME/runs/$OTTO_RUN_ID/artifacts/seen-head"
seen=""
[ -f "$cursor" ] && seen=$(cat "$cursor")
if [ "$head" = "$seen" ]; then
  echo "no new commits"
  exit 0
fi
echo "main is at $head"
exit 1
```

- **Start with a `#!` line.** Poke runs the script directly; `otto check` refuses one without.
- **Exit deliberately.** Use `set -e` and an explicit final `exit`, rather than whatever the last
  command happened to return.
- **Use absolute paths** for anything outside `/usr/bin` and Homebrew — the script runs under
  launchd's minimal `PATH`, not your shell's, even when `otto check` runs it from your terminal.
- **Be read-only.** The script has no `otto state` access and never will. If the run needs to
  record something (a cursor, a ledger line), that's the wake's job once the check says
  "changed" — as above, where the wake writes `seen-head` after handling the commit.
- **Keep it fast.** Poke gives it 10 seconds before treating it as an error.
- **Let a failed read fail.** A read that fails should end the script with something other than 0,
  which wakes the run to look, as `set -e` does above when `git` can't reach the remote. Hidden with
  `|| true`, it reads as "nothing new" and the run sleeps on.

Its environment is the same whoever runs it, poke or `otto check`, and holds only: `HOME`,
`USER`, `LOGNAME`, `SHELL`, `TMPDIR` and `SSH_AUTH_SOCK` as you have them; `PATH` set to
`/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:~/.local/bin`; `OTTO_HOME`;
`OTTO_RUN_ID`; `OTTO_WORKDIR`, the run's working directory; and `OTTO_REPO` if the run has a
`repo` fact. It runs in the working directory, as the wakes do — not beside the copy of it otto
keeps in the run directory, so a script that finds files from its own path (`dirname "$0"`) finds
the run directory instead. Use `$OTTO_WORKDIR`.
The first 200 characters of what it prints are kept as the "last result" `otto show` displays.

## A sandboxed run's check

A run with a [sandbox record](sandboxing.md#the-record-decides) has its check run under that
record's profile, whoever runs it: poke in front of a due wake, or `otto check` on the spot. The
script gets the same reads and writes as the run's wakes and nothing more. A run with no record has
its check run as it always was.

What that leaves the script:

- **`$OTTO_HOME` is readable, all of it**, so `otto state list`, `otto show` and reading another
  run's `run.json` still work. Only the run's own directory there is writable.
- **What the record lists** under `read` and `write`, and the network.
- **The same environment** as above, and the same 10 seconds, which now include the sandbox
  starting.

What it takes away:

- **`gh`**, even for a public repository, and anything else that signs in from `~/.config/gh` or
  the Keychain.
- **ssh.** `SSH_AUTH_SOCK` is still set, but the agent's socket is closed and so is `~/.ssh`. A
  `git fetch` or `git ls-remote` over ssh fails; over `https` to a public repository it works.
- **Credentials**: `~/.netrc`, `~/.git-credentials`, `~/.aws` and the rest of
  [what the sandbox closes](sandboxing.md#whats-closed).
- **Other projects.** Every project in `config.json`, and the working directory itself, are closed
  to reads and writes unless the record lists them. The script still starts in the working
  directory, but can't read it.
- **Claude.** A check gets no sign-in and no Claude folder: it runs no model.

So a check that works for a run today can stop working the day the run gets a record, and how it
stops matters:

- **A read the sandbox refuses is a failed command in your script.** If the script lets it fail,
  it exits with something other than 0 or 1, and the run is woken to look: every period, until the
  script or the record is changed. If the script hides it and exits 0, that is "nothing new", and
  the run sleeps until the [safety net](#the-safety-net) wakes it. Don't hide it.
- **A check the sandbox won't start is an error, never "nothing new".** The run is woken, and the
  result says `its check couldn't start under its sandbox`.
- **A run that [disagrees with its record](sandboxing.md#a-run-that-disagrees-with-its-record)
  doesn't have its check run at all**, with a sandbox or without. Poke starts a wake instead, which
  is refused as any wake of that run would be, leaving it blocked with a question that says what
  differed. `otto check` says what differed and runs nothing. A record that can't be read counts
  the same.

## Setting, seeing and removing it

```bash
otto check my-run --script ./check.sh   # copies it to the run as check.sh, and runs it once now
otto check my-run                       # the script, its last result, what it has saved
otto check my-run --off                 # remove it; every wake is a full one again
```

When you set a check, otto runs it once on the spot and tells you what it returned — without
acting on it — so a broken script shows immediately rather than at the next wake. The web UI's
run page shows the same information in its **Check script** panel.

## Check history

Every pass poke runs the script adds a line to `check-history.jsonl` in the run's directory, "nothing
new" included: the time, the exit code, whether it timed out, the result and the start of what it
printed. The newest 700 are kept, about a week at a 15-minute period, so "why didn't it wake?" still
has an answer days later. The trial run when you set a check and `otto poke --dry-run` add nothing,
and none of it counts as the run making progress. The format is in
[`run.json`'s `check` section](../reference/run-json.md#check).

## The run's own check

A wake that sees the run is mostly watching sets a standing check itself, following its
instructions ([harness/wake.md](../../harness/wake.md)):

```bash
otto state set-check <id> --script artifacts/check.sh   # stands in front of every period wake
otto state set-check <id> --off                         # removes it
```

It behaves exactly like yours — every period wake, the same safety net — and stays until a wake
replaces or removes it, which a wake does when the work changes shape. `otto show` and the run page
say it was **set by the run**. To take over, set your own with `otto check --script`: it replaces
the run's, and from then on a wake can neither replace nor remove it. `otto check --off` removes
either kind.

For a one-off wait, a wake arms a check with a single sleep instead:

```bash
otto state arm-timer <id> --in 600 --check-script artifacts/ci-done.sh
```

That check stands in front of that sleep's wake only, and on "nothing new" asks again after the
same 600 seconds — a free poll every ten minutes until CI finishes. The next wake ends it.

## Which wakes a check stands in front of

- **A standing check** — yours, or the run's own — stands in front of every wake the **period**
  brings. It does *not* stand in front of a timer a wake armed for its own reason
  (`arm-timer --in 600` because CI takes ten minutes): that wake knew something the script doesn't,
  so it goes ahead, and the check resumes with the next period wake. A one-sleep `--check-script`
  never replaces a standing check.
- **A one-sleep check** stands in front of the sleep it was armed with, and is gone after it.
- **Never** in front of the retry of a wake that crashed or was killed — that work is half done
  whatever the script thinks — and never while a gate is open.
