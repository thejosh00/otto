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
| exits `0` — nothing new | No wake. The run sleeps another period, and nothing is journaled |
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
# Wake the run when the PR has a review comment newer than the last one it handled.
set -e
latest=$(/opt/homebrew/bin/gh pr view 123 --repo acme/svc --json comments \
  --jq '[.comments[].createdAt] | max // ""')
seen=$(cat "$HOME/.otto/runs/$OTTO_RUN_ID/artifacts/seen-comment" 2>/dev/null || true)
if [ "$latest" = "$seen" ]; then
  echo "no new comments"
  exit 0
fi
echo "new comment at $latest"
exit 1
```

- **Start with a `#!` line.** Poke runs the script directly; `otto check` refuses one without.
- **Exit deliberately.** Use `set -e` and an explicit final `exit`, rather than whatever the last
  command happened to return.
- **Use absolute paths** for anything outside `/usr/bin` and Homebrew — the script runs under
  launchd's minimal `PATH`, not your shell's.
- **Be read-only.** The script has no `otto state` access and never will. If the run needs to
  record something (a cursor, a ledger line), that's the wake's job once the check says
  "changed" — as above, where the wake writes `seen-comment` after handling the comment.
- **Keep it fast.** Poke gives it 10 seconds before treating it as an error.

It gets two environment variables: `OTTO_RUN_ID`, and `OTTO_REPO` if the run has a `repo` fact.
The first 200 characters of what it prints are kept as the "last result" `otto show` displays.

## Setting, seeing and removing it

```bash
otto check my-run --script ./check.sh   # copies it to the run as check.sh, and runs it once now
otto check my-run                       # the script, its last result, what it has saved
otto check my-run --off                 # remove it; every wake is a full one again
```

When you set a check, otto runs it once on the spot and tells you what it returned — without
acting on it — so a broken script shows immediately rather than at the next wake. The web UI's
run page shows the same information in its **Check script** panel.

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
