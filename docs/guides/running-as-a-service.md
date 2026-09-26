# Running as a service

otto has no daemon that owns a run. Two small launchd jobs keep it working while you're away, both
installed with one command and both safe to re-run.

| Job | What it does | Needed? |
|---|---|---|
| **The reviver** — `otto agent` | Runs `otto poke` every ~5 minutes: starts wakes that are due, runs check scripts, kills stuck wakes, sends notifications | **Yes.** Without it a sleeping run never wakes |
| **The web service** — `otto service` | Keeps `otto serve` (the web UI) running from login, restarting it if it exits | Optional — runs carry on without it |

`./install.sh` installs the reviver; `./install.sh --service` installs the web service too.

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
due"). To see what it *would* do and why, for every run:

```bash
otto poke --dry-run --verbose
```

`otto ls` warns when runs are sleeping but the reviver isn't loaded.

## The web service

```bash
otto service start               # install and start `otto serve` on 127.0.0.1:7878
otto service start --port 7979   # on another port
otto service status              # loaded? which URL? where are the plist and log?
otto service stop                # stop it; it stays stopped until the next start
```

The plist is `~/Library/LaunchAgents/com.joshuahill.otto-serve.plist` and the log is
`~/.otto/logs/serve.log`. If the server can't start — usually because something else has the port
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

## Troubleshooting

| Symptom | Look at |
|---|---|
| A sleeping run never wakes | `otto agent status` — is the reviver loaded, and when did it last poke? |
| A run is due but not waking | `otto poke --dry-run --verbose` says why for every run; `otto logs <id>` for its history |
| A run keeps failing | `otto logs <id> --event wake-incomplete,wake-killed,error`; after five in a row it becomes `blocked` with a gate explaining |
| The web UI isn't there | `otto service status`, then `~/.otto/logs/serve.log` |
| Wakes can't find `claude` or `gh` | launchd's `PATH` is `/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:~/.local/bin` — install tools there or use absolute paths |
