# Running as a service

otto has no daemon that owns a run. Two small launchd jobs keep it working while you're away, both
installed with one command and both safe to re-run.

| Job | What it does | Needed? |
|---|---|---|
| **The reviver** — `otto agent` | Runs `otto poke` every ~5 minutes: starts wakes that are due, runs check scripts, kills stuck wakes, sends notifications | **Yes.** Without it a sleeping run never wakes |
| **The web service** — `otto service` | Keeps `otto serve` (the web UI) running from login, restarting it if it exits | Optional — runs carry on without it |

`./install.sh` installs the reviver; `./install.sh --service` installs the web service too. While the
web service is running, `./install.sh` and `./install.sh --rollback` restart it, so it serves the
binary they leave in place. One you stopped stays stopped. To see what is installed, `otto --version`
names the commit it was built from.

**Checking a release kept your runs.** `scripts/migration-check.sh record ~/.otto before.txt` writes
what must survive a release (plans/v3.md, "What must survive"), one fact per line: each run's
names and aliases, goal and done condition by hash, schedule and pause, gates, notes, unread
messages, drafts, output versions, changes and their patches, `artifacts/`, handoff, journal
lines, check script and counters, the worktrees of each repo a run uses, `feedback.jsonl` and
`config.json`. It only reads the home. Record again after installing (or rolling back), then
`scripts/migration-check.sh compare before.txt after.txt` names every difference by run and
category and exits 1 if anything was lost or rewritten. A journal may only grow, and a launcher
may be added but not removed or changed. What moves on its own on a live home (`nextWakeAt`, lines
appended, counters going up, new notes or messages) is listed apart and doesn't fail. If you
answered a gate or read a message between the two records, that shows as a failure, so say why.

**Rehearsing a release.** `./install.sh` does this for you before an install over an existing
one (and so after every landing): `scripts/rehearse.sh` copies the otto home to a temp directory,
runs `otto ls`, `otto state list --json`, `otto changes --all --json` and `otto poke --dry-run
--verbose` there with the installed binary and then the new build, records the copy with
`migration-check.sh` between the two and after, and compares. The install **stops**, before anything
is replaced, if the new build's `otto ls` fails, another of those commands fails with the new build
but not the installed one, a run the installed build lists is missing from the new build's list, or
migration-check finds something lost or rewritten. The old `otto` and `config.json` are left as
they were. Differences in output are **only shown**, as are commands that fail with both builds.
Everything, diffs included, goes to `~/.otto/logs/rehearsal.log` (the previous one is kept as
`rehearsal.log.1`), and that log is all it writes to the real home. For a release that changes a
recorded fact on purpose, read the log, then run `./install.sh --skip-rehearsal`; it still checks
that the new build's `otto ls` works. To rehearse by hand, run `scripts/rehearse.sh --new
target/release/otto`. It compares against the `otto` on your PATH unless you give `--installed BIN`.
`--home DIR` and `--log FILE` are optional. Add `--snapshot <phase>` to first keep a tarball of the home as
`~/.otto-snapshots/<date>-<phase>.tar.gz` before a cut-over (it won't overwrite one).

### Rolling back an install

When an install replaces the binary with a different build, it keeps the old one as
`otto.previous` beside it and the otto home's `config.json` as it stood then as
`otto.previous.config.json`. Installing the same build again replaces neither. `./install.sh --rollback`
puts both back: an older binary refuses a `config.json` with keys it doesn't know, so the binary alone
might not start.

The config it restores is that snapshot, with one exception: **a launcher the current config has
and the snapshot doesn't is carried over unchanged** — a run moved into a `sandbox-*` launcher since,
or one you added, still finds the launcher it names. It prints one line for each. A launcher both
have takes the snapshot's definition, and every other key takes the snapshot's value. The config it
replaced is saved as `config.json.before-rollback` in the otto home, and the snapshot is deleted, as
`otto.previous` is.

The merge needs `jq`. Without it, or if the merge fails, the binary is still rolled back but
`config.json` is left as it is and the script exits non-zero. With no snapshot (an install from
before snapshots existed) it rolls back the binary only and says so.

## The reviver

```bash
otto agent start     # install (if missing) and start it
otto agent status    # is it loaded, and when did it last poke?
otto agent stop      # stop it
```

`otto agent start` writes `~/Library/LaunchAgents/com.joshuahill.otto-poke.plist` pointing at the
`otto` binary you ran it with, and loads it. Run it again after moving or rebuilding the binary,
or after changing `OTTO_HOME` — it rewrites the plist every time.

Its output goes to `~/.otto/logs/poke.log`: one line per pass, saying what it did (or "nothing
due"), and a line saying so when a pass comes more than 30 minutes after the last one, as it does
after a laptop has slept. At about 1MB the log rolls over to `poke.log.1`, with the older ones
shifting along to `poke.log.3`, which is what goes next. To see what it *would* do and why, for every run:

```bash
otto poke --dry-run --verbose
```

`otto ls` warns when runs are sleeping but the reviver isn't loaded.

## The web service

```bash
otto service start               # install and start `otto serve` on 127.0.0.1:7878
otto service start --port 7979   # on another port
otto service status              # loaded? which URL? where are the plist and log?
otto service stop                # stop it; it stays stopped until the next start, through ./install.sh too
otto service restart             # serve the binary now installed, on the same port
```

The plist is `~/Library/LaunchAgents/com.joshuahill.otto-serve.plist` and the log is
`~/.otto/logs/serve.log`, every line of it stamped with the time it was written (lines from before
that have no stamp). If the server can't start — usually because something else has the port
— launchd waits 30 seconds between attempts, and the reason is in the log.

## Notifications

On every pass the reviver posts a macOS notification, once each, when a run:

- opens a gate,
- becomes `blocked` (naming why),
- leaves a gate unanswered past its `gateStaleAfterHours` (48h by default), or
- passes 80% of a budget.

It's poke that sends them rather than the wake, because a sandboxed wake may not be able to reach
the notification centre. If you see none, check System Settings → Notifications for Script
Editor, which is what `osascript` posts as.

With `OTTO_NOTIFY=off` in poke's environment nothing is posted: each notice is still recorded as
sent, and poke's report marks it `[not sent: OTTO_NOTIFY=off]`. otto's tests set it, so their
scratch runs stay out of your notification centre. Don't set it for the reviver itself; a notice
it records but doesn't post is not posted later.

## Troubleshooting

| Symptom | Look at |
|---|---|
| A sleeping run never wakes | `otto agent status` — is the reviver loaded, and when did it last poke? |
| A run is due but not waking | `otto poke --dry-run --verbose` says why for every run; `otto logs <id>` for its history |
| You want to know why a wake started, or how it ended | `otto why <id> [--wake <n>]` puts the journal, poke's decisions and the check results in one order |
| A run keeps failing | `otto logs <id> --event wake-incomplete,wake-killed,error`; after five in a row it becomes `blocked` with a gate explaining |
| The web UI isn't there | `otto service status`, then `~/.otto/logs/serve.log` |
| Wakes can't find `claude` or `gh` | launchd's `PATH` is `/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:~/.local/bin` — install tools there or use absolute paths |
