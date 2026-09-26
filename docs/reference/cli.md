# CLI reference

<!-- Generated from otto's own --help by `OTTO_UPDATE_DOCS=1 cargo test cli_reference`. Do not edit by hand. -->

Every command and flag, exactly as `--help` prints it. For what the commands are *for*, start with [concepts](../concepts.md) and the [guides](../README.md#guides).

Every `<ID>` takes the full run id, a unique prefix of it, or a unique prefix of its slug (`otto show verify`). Exit codes: `0` ok, `1` usage, `2` state conflict, `3` no such run.

## `otto run`

```text
Start a run: wrap a skill, a runbook, or a bare goal, and take the first wake

Usage: otto run [OPTIONS] --goal <GOAL>

Options:
      --dry-run
          Print the id the run would get and the command its first wake would run, and create nothing

  -h, --help
          Print help

What to run:
      --goal <GOAL>
          What this run is trying to achieve. The only thing that survives every wake unaltered, and the only thing that can say when to stop

      --skill <SKILL>
          Wrap a Claude Code skill by name (`manage-pr`)

      --instructions <INSTRUCTIONS>
          Wrap a prose file — a runbook, a workflow table, a ticket

      --target <TARGET>
          What the run is about — a Jira key, a PR — recorded as `facts.target` and used for the id

      --repo <DIR>
          A repository the wake may work in (repeatable). Granted to claude with `--add-dir`, and opened in the sandbox too when the launcher has a `grantFlag`

When it is done:
      --until <UNTIL>
          The done-condition, stated outright. Otherwise the first wake proposes one and gates it

      --perpetual
          Never: the run is retired with `otto stop`, not finished

      --period <AGE>
          How often it wakes when a wake doesn't ask for something else: `30m`, `1h`, `1d`. Measured from the start of one wake to the start of the next [default: 1h]

      --budget-wakes <N>
          Block with a gate after this many wakes (0: unlimited)
          
          [default: 0]

      --budget-hours <H>
          Block with a gate after this many hours since creation (0: unlimited)
          
          [default: 0]

Where it runs:
      --launcher <NAME>
          What runs the model: plain `claude`, or a launcher named in $OTTO_HOME/config.json — a sandbox is the right choice for anything unattended. A unique prefix of the name will do
          
          [default: claude]

      --workdir <DIR>
          The directory every wake runs in, whoever starts it [default: `workdir` in $OTTO_HOME/config.json]

      --detach <DETACH>
          Where each wake is backgrounded: a tmux session named otto-<id>, or none (your terminal)
          
          [default: tmux]
          [possible values: none, tmux]

      --watch
          Watch every wake in this terminal instead of detaching it (persists for the run)

What it may do:
      --permission-mode <PERMISSION_MODE>
          claude's permission mode for every wake. A wake is unattended, so no prompt can be answered; `bypass-permissions` is the only mode measured to work, and the real guardrail is a sandboxing `--launcher` (docs/guides/sandboxing.md)
          
          [default: bypass-permissions]
          [possible values: accept-edits, auto, bypass-permissions, manual, dont-ask, plan]

      --allow-tool <TOOL>
          Tool claude may use without asking, in claude's own syntax (`Read`, `Bash(git *)`); repeatable

      --deny-tool <TOOL>
          Tool claude may not use (`WebFetch`); repeatable

Advanced:
      --id <ID>
          The run id, instead of the generated <date>-<slug>

      --slug <SLUG>
          The slug for the generated id (default: from --target, else the goal)

      --phase <PHASE>
          The phase label the run starts in — free-form, for the wrapped instructions and `otto ls`
          
          [default: start]

      --fact <K=V>
          A fact the first wake starts with (`branch=feat/x`); repeatable. Values parse as JSON when they can

      --policy <K=V>
          A policy knob (repeatable): maxWakeMinutes=45, maxIncompleteWakes=5, maxTicksWithoutProgress=24 (0 disables), gateStaleAfterHours=48, handoffMaxBytes=8192, autoMergeWhenGreen=false. Any other key is kept for the wrapped instructions to read
```

## `otto ls`

```text
Every live run, and what each is waiting on

Usage: otto ls [OPTIONS]

Options:
      --all
          Include runs that have finished

  -h, --help
          Print help
```

## `otto show`

```text
Where one run stands, and the open question in full

Usage: otto show [ID]

Arguments:
  [ID]
          The run: its id, a prefix of it, or its slug. Omitted, the one run waiting on you — or the only live run, if there is just one

Options:
  -h, --help
          Print help
```

## `otto answer`

```text
Answer the open gate, then continue the run

Usage: otto answer [OPTIONS] [ID]

Arguments:
  [ID]
          The run: its id, a prefix of it, or its slug. Omitted, the one run waiting on you

Options:
      --choice <CHOICE>
          The option you are choosing, by name — checked against the gate's own options when it lists any, and rejected (naming the valid ones) if it matches none

      --text <TEXT>
          Prose, when the decision needs more than an option name

      --file <FILE>
          Read the answer from a file

      --no-wake
          Record the answer but don't wake the run yet

  -h, --help
          Print help
```

## `otto note`

```text
Leave a note for the run, gated or not; its next wake reads it, verbatim

Usage: otto note [OPTIONS] <ID> [TEXT]

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

  [TEXT]
          What to tell it, verbatim. Guidance on how to do the work — not an answer to a gate

Options:
      --file <FILE>
          Read the note from a file

      --standing
          Give it to every wake until dropped, not just the next one to complete

      --now
          Wake the run now to read it, rather than waiting for its next wake

      --list
          Show the run's standing notes and the ones not yet delivered

      --drop <NOTE>
          Withdraw an undelivered note, or retire a standing one, by number

  -h, --help
          Print help
```

## `otto check`

```text
Give a run a check script poke runs instead of a wake, see it, or remove it

Usage: otto check [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --script <FILE>
          A script poke runs directly, no model involved, whenever the run's period comes due: exit 0 = nothing new (no wake is spent; it sleeps another period), 1 = changed (a wake), anything else = a wake too. It starts with a #! line, and runs under launchd's PATH — use absolute paths for anything outside /usr/bin and Homebrew

      --wake-after <N>
          The safety net: wake anyway after this many "nothing new" results in a row, in case the script is wrong without failing. 0 turns it off
          
          [default: 24]

      --off
          Remove the run's check script; every wake is a full one again

  -h, --help
          Print help
```

## `otto logs`

```text
The run's journal, readably

Usage: otto logs [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
  -n, --lines <LINES>
          Trailing lines to show, like `tail -n` [default: 40, or everything with --since]

      --since <AGE|WHEN>
          Only lines from this long ago (`45m`, `2h`, `3d`) or since a date/time (`2026-09-15`, `2026-09-15T07:00Z`)

      --event <NAMES>
          Only these events, comma-separated (`gate-opened,gate-closed`)

      --decisions
          Only the turning points: gates, status and phase changes, budgets, wakes that failed

  -f, --follow
          Keep printing as the run writes more, like `tail -f`

  -h, --help
          Print help
```

## `otto usage`

```text
What wakes have used, in tokens, across every run — by run or by day

Usage: otto usage [OPTIONS]

Options:
      --since <SINCE>
          From when: an age (`24h`, `30d`), a date (`2026-09-01`) or a timestamp [default: 7d]

      --all
          Every wake ever, not just the last week

      --by <BY>
          One row per run, or per day
          
          [default: run]
          [possible values: run, day]

      --json
          Print the report as JSON, with exact counts

  -h, --help
          Print help
```

## `otto attach`

```text
Watch the wake that is running right now

Usage: otto attach <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
  -h, --help
          Print help
```

## `otto period`

```text
See or change how often a run wakes

Usage: otto period <ID> [DURATION]

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

  [DURATION]
          How often it wakes when a wake doesn't ask for something else: `30m`, `4h`, `1d`. Measured from the start of one wake to the start of the next. Omit it to see the current period

Options:
  -h, --help
          Print help
```

## `otto stop`

```text
Retire a run

Usage: otto stop [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --reason <REASON>
          Why, recorded in the journal

      --failed
          The run ended because it could not do its job, rather than simply no longer being wanted

  -h, --help
          Print help
```

## `otto resume`

```text
Bring a stopped or failed run back, and wake it

Usage: otto resume [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --reason <REASON>
          Why, recorded in the journal

      --no-wake
          Put it back on its schedule without waking it now

  -h, --help
          Print help
```

## `otto wake`

```text
Run one wake: spawn the model, then validate what it left behind

Usage: otto wake [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --answer <ANSWER>
          A person's answer to the open gate, handed to the wake verbatim

      --dry-run
          Print the command that would run, and change nothing

      --watch
          Run this wake in your terminal instead of backgrounding it

      --detach <DETACH>
          How to background this wake, overriding the run's own setting for this wake only
          
          [possible values: none, tmux]

  -h, --help
          Print help
```

## `otto poke`

```text
Start the wakes that are due, and clean up after the ones that are over. Run this from launchd (see `otto agent start`) every ~5 minutes, or by hand

Usage: otto poke [OPTIONS]

Options:
      --dry-run
          Decide, print what it would do, and change nothing — no wake, no check script, no kill

      --verbose
          Say what it decided for every run, not only the ones where it did something

      --max-starts <MAX_STARTS>
          Start at most this many wakes in one pass; the rest wait for the next pass
          
          [default: 2]

      --grace <MINUTES>
          Minutes past a run's wake time before poke starts it. 0: as soon as it is due
          
          [default: 0]

      --max-attempts <MAX_ATTEMPTS>
          Spawn attempts that produce no wake before poke gives up on a run and says so once
          
          [default: 5]

      --deadline-grace <MINUTES>
          Minutes past a wake's deadline before poke kills it as stuck rather than finishing
          
          [default: 5]

  -h, --help
          Print help
```

## `otto agent`

```text
The background reviver that wakes runs whose timer has passed

Usage: otto agent <COMMAND>

Commands:
  start   Install (if missing) and start it
  stop    Stop it
  status  Is it loaded, when did it last poke, and where is its plist
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### `otto agent start`

```text
Install (if missing) and start it

Usage: otto agent start

Options:
  -h, --help
          Print help
```

### `otto agent stop`

```text
Stop it

Usage: otto agent stop

Options:
  -h, --help
          Print help
```

### `otto agent status`

```text
Is it loaded, when did it last poke, and where is its plist

Usage: otto agent status

Options:
  -h, --help
          Print help
```

## `otto serve`

```text
The web UI: everything above, in a browser, on 127.0.0.1

Usage: otto serve [OPTIONS]

Options:
      --port <PORT>
          Port on 127.0.0.1 to listen on
          
          [default: 7878]

      --open
          Open the page in your browser once it is listening

  -h, --help
          Print help
```

## `otto service`

```text
The web UI as a launchd service: running from login, restarted if it exits

Usage: otto service <COMMAND>

Commands:
  start   Install (if missing) and start `otto serve` under launchd
  stop    Stop it; it stays stopped until the next `otto service start`
  status  Is it loaded, on which port, and where are its plist and log
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### `otto service start`

```text
Install (if missing) and start `otto serve` under launchd

Usage: otto service start [OPTIONS]

Options:
      --port <PORT>
          Port on 127.0.0.1 to listen on
          
          [default: 7878]

  -h, --help
          Print help
```

### `otto service stop`

```text
Stop it; it stays stopped until the next `otto service start`

Usage: otto service stop

Options:
  -h, --help
          Print help
```

### `otto service status`

```text
Is it loaded, on which port, and where are its plist and log

Usage: otto service status

Options:
  -h, --help
          Print help
```

## `otto install`

```text
Symlink otto's Claude Code skills into ~/.claude/skills/

Usage: otto install [OPTIONS]

Options:
      --repo <REPO>
          path to the otto checkout (default: $OTTO_REPO, or detected from the current directory)

  -h, --help
          Print help
```

## `otto config`

```text
See or change otto's settings in $OTTO_HOME/config.json

Usage: otto config <COMMAND>

Commands:
  workdir  The default directory wakes run in, for runs that don't pass --workdir
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### `otto config workdir`

```text
The default directory wakes run in, for runs that don't pass --workdir

Usage: otto config workdir [DIR]

Arguments:
  [DIR]
          Set it to this directory (`~` is expanded; it must exist). Omit to print it

Options:
  -h, --help
          Print help
```

## `otto state`

```text
The only writer of a run's durable state

Usage: otto state <COMMAND>

Commands:
  init              Create a run directory
  get               Print run.json, or one dotted field
  list              One line per run
  set-phase         Enter a phase
  set-status        Change status
  record-fact       Write re-derived facts
  open-gate         Write the gate file and stop the run on it
  close-gate        Record the answer verbatim and clear the gate
  arm-timer         Record nextWakeAt. This is the whole timer: poke reads it, nothing else is needed
  set-check         Give the run a standing check script, or remove the one a wake gave it
  tick              Account for one timer tick
  due               Runs whose nextWakeAt has passed
  authorize         Record that a specific commit may go outward
  check-authorized  Refuse an outward action that nobody approved
  lock              Claim a repo for this run
  unlock            Release this run's repo lock(s)
  locks             Who holds which repo
  handoff           Rewrite handoff.md — the only thing the next wake gets for free (capped)
  log               Append one journal line
  tail              Last N journal lines
  help              Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### `otto state init`

```text
Create a run directory

Usage: otto state init [OPTIONS] --goal <GOAL>

Options:
  -h, --help
          Print help

What to run:
      --goal <GOAL>
          What this run is trying to achieve. The only thing that survives every wake unaltered, and the only thing that can say when to stop

      --skill <SKILL>
          Wrap a Claude Code skill by name (`manage-pr`)

      --instructions <INSTRUCTIONS>
          Wrap a prose file — a runbook, a workflow table, a ticket

      --target <TARGET>
          What the run is about — a Jira key, a PR — recorded as `facts.target` and used for the id

      --repo <DIR>
          A repository the wake may work in (repeatable). Granted to claude with `--add-dir`, and opened in the sandbox too when the launcher has a `grantFlag`

When it is done:
      --until <UNTIL>
          The done-condition, stated outright. Otherwise the first wake proposes one and gates it

      --perpetual
          Never: the run is retired with `otto stop`, not finished

      --period <AGE>
          How often it wakes when a wake doesn't ask for something else: `30m`, `1h`, `1d`. Measured from the start of one wake to the start of the next [default: 1h]

      --budget-wakes <N>
          Block with a gate after this many wakes (0: unlimited)
          
          [default: 0]

      --budget-hours <H>
          Block with a gate after this many hours since creation (0: unlimited)
          
          [default: 0]

Where it runs:
      --launcher <NAME>
          What runs the model: plain `claude`, or a launcher named in $OTTO_HOME/config.json — a sandbox is the right choice for anything unattended. A unique prefix of the name will do
          
          [default: claude]

      --workdir <DIR>
          The directory every wake runs in, whoever starts it [default: `workdir` in $OTTO_HOME/config.json]

      --detach <DETACH>
          Where each wake is backgrounded: a tmux session named otto-<id>, or none (your terminal)
          
          [default: tmux]
          [possible values: none, tmux]

What it may do:
      --permission-mode <PERMISSION_MODE>
          claude's permission mode for every wake. A wake is unattended, so no prompt can be answered; `bypass-permissions` is the only mode measured to work, and the real guardrail is a sandboxing `--launcher` (docs/guides/sandboxing.md)
          
          [default: bypass-permissions]
          [possible values: accept-edits, auto, bypass-permissions, manual, dont-ask, plan]

      --allow-tool <TOOL>
          Tool claude may use without asking, in claude's own syntax (`Read`, `Bash(git *)`); repeatable

      --deny-tool <TOOL>
          Tool claude may not use (`WebFetch`); repeatable

Advanced:
      --id <ID>
          The run id, instead of the generated <date>-<slug>

      --slug <SLUG>
          The slug for the generated id (default: from --target, else the goal)

      --phase <PHASE>
          The phase label the run starts in — free-form, for the wrapped instructions and `otto ls`
          
          [default: start]

      --fact <K=V>
          A fact the first wake starts with (`branch=feat/x`); repeatable. Values parse as JSON when they can

      --policy <K=V>
          A policy knob (repeatable): maxWakeMinutes=45, maxIncompleteWakes=5, maxTicksWithoutProgress=24 (0 disables), gateStaleAfterHours=48, handoffMaxBytes=8192, autoMergeWhenGreen=false. Any other key is kept for the wrapped instructions to read
```

### `otto state get`

```text
Print run.json, or one dotted field

Usage: otto state get [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --field <FIELD>
          e.g. status, facts.branch, gate.file

  -h, --help
          Print help
```

### `otto state list`

```text
One line per run

Usage: otto state list [OPTIONS]

Options:
      --json
          Print the runs as a JSON array instead of a table

      --status <STATUS>
          Only runs in this status
          
          [possible values: running, awaiting_human, sleeping, blocked, done, failed, stopped]

  -h, --help
          Print help
```

### `otto state set-phase`

```text
Enter a phase

Usage: otto state set-phase [OPTIONS] --phase <PHASE> <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --phase <PHASE>
          The phase label to enter — free-form, for people and the wrapped instructions

      --status <STATUS>
          The status to enter with it. `running` also clears any scheduled wake
          
          [default: running]
          [possible values: running, awaiting_human, sleeping, blocked, done, failed, stopped]

      --note <NOTE>
          Why, recorded in the journal

  -h, --help
          Print help
```

### `otto state set-status`

```text
Change status

Usage: otto state set-status [OPTIONS] --status <STATUS> <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --status <STATUS>
          The status to set. A terminal one (done, failed, stopped) also clears any scheduled wake
          
          [possible values: running, awaiting_human, sleeping, blocked, done, failed, stopped]

      --reason <REASON>
          Why, recorded in the journal (and, for blocked, shown to the person)

      --because <BECAUSE>
          With `--status blocked`: why. Omitted, otto infers `stall` when the tick counter is what tripped, else `instructions`. Ignored for any other status

          Possible values:
          - wake-failures: `policy.maxIncompleteWakes` wakes in a row did not finish
          - budget:        A ceiling set when the run was created (`--budget-wakes`, `--budget-hours`) was reached
          - stall:         `policy.maxTicksWithoutProgress` ticks in a row changed nothing
          - instructions:  The wrapped instructions decided the run could not proceed

  -h, --help
          Print help (see a summary with '-h')
```

### `otto state record-fact`

```text
Write re-derived facts

Usage: otto state record-fact <ID> <K=V>...

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

  <K=V>...
          Facts to write into `facts` (`branch=feat/x`). Values parse as JSON when they can

Options:
  -h, --help
          Print help
```

### `otto state open-gate`

```text
Write the gate file and stop the run on it

Usage: otto state open-gate [OPTIONS] --slug <SLUG> <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --slug <SLUG>
          A short name for the question, used in the gate file's name (`plan-review`)

      --question <QUESTION>
          The question, inline

      --question-file <QUESTION_FILE>
          Read the question from this file

      --stdin
          Read the question from stdin

      --expires-at <EXPIRES_AT>
          ISO time after which no answer counts as none. Only use an expiry where silence has a sane meaning: "no answer, so don't do it" is sane; "no answer, so push it" is not

      --expires-in <SECONDS>
          same, relative to now

  -h, --help
          Print help
```

### `otto state close-gate`

```text
Record the answer verbatim and clear the gate

Usage: otto state close-gate [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --answer <ANSWER>
          The answer, inline, recorded verbatim

      --answer-file <ANSWER_FILE>
          Read the answer from this file

      --stdin
          Read the answer from stdin

      --expired
          close an expired gate as unanswered, instead of with an answer

      --status <STATUS>
          The status the run continues in
          
          [default: running]
          [possible values: running, awaiting_human, sleeping, blocked, done, failed, stopped]

  -h, --help
          Print help
```

### `otto state arm-timer`

```text
Record nextWakeAt. This is the whole timer: poke reads it, nothing else is needed

Usage: otto state arm-timer [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --at <AT>
          ISO-8601 wake time. Give neither this nor `--in` to sleep for the run's own period

      --in <SECONDS>
          seconds from now — a one-off override of the run's period, for this sleep only

      --status <STATUS>
          The status to sleep in
          
          [default: sleeping]
          [possible values: running, awaiting_human, sleeping, blocked, done, failed, stopped]

      --note <NOTE>
          Why, recorded in the journal

      --check-script <CHECK_SCRIPT>
          Path, relative to the run dir, to an opt-in script poke runs directly when this sleep comes due, instead of waking the run — DESIGN.md §8.1. "Nothing new" sleeps again for the same length of time; anything else wakes the run

  -h, --help
          Print help
```

### `otto state set-check`

```text
Give the run a standing check script, or remove the one a wake gave it

Usage: otto state set-check [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --script <PATH>
          Path, relative to the run dir, to an executable script poke runs whenever the run's period comes due, instead of waking it: exit 0 = nothing new (no wake), anything else = wake

      --wake-after <N>
          The safety net: wake anyway after this many "nothing new" results in a row; 0 turns it off
          
          [default: 24]

      --off
          Remove the standing check a wake set; every period wake is a full one again

  -h, --help
          Print help
```

### `otto state tick`

```text
Account for one timer tick

Usage: otto state tick [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --progress
          something happened: reset the counter

      --note <NOTE>
          What this tick saw, recorded in the journal

  -h, --help
          Print help
```

### `otto state due`

```text
Runs whose nextWakeAt has passed

Usage: otto state due

Options:
  -h, --help
          Print help
```

### `otto state authorize`

```text
Record that a specific commit may go outward

Usage: otto state authorize [OPTIONS] --action <ACTION> --head <HEAD> <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --action <ACTION>
          The outward action being authorized
          
          [possible values: push, merge, comment, delete-branch]

      --head <HEAD>
          the commit this authorizes, full or short sha

      --gate <GATE>
          the answered gate that authorizes it

      --policy <POLICY>
          the policy flag that authorizes it, e.g. autoMergeWhenGreen

      --item <ITEM>
          which target, for a run that fans out over several

      --quote <QUOTE>
          the human's words, verbatim

  -h, --help
          Print help
```

### `otto state check-authorized`

```text
Refuse an outward action that nobody approved

Usage: otto state check-authorized [OPTIONS] --action <ACTION> --head <HEAD> <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --action <ACTION>
          The outward action about to be taken
          
          [possible values: push, merge, comment, delete-branch]

      --head <HEAD>
          the commit you are about to act on

      --item <ITEM>
          which target, for a run that fans out over several

  -h, --help
          Print help
```

### `otto state lock`

```text
Claim a repo for this run

Usage: otto state lock [OPTIONS] --repo <REPO> <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --repo <REPO>
          The repository to claim

      --force
          break a lock you are sure is dead

  -h, --help
          Print help
```

### `otto state unlock`

```text
Release this run's repo lock(s)

Usage: otto state unlock [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --repo <REPO>
          default: every lock this run holds

  -h, --help
          Print help
```

### `otto state locks`

```text
Who holds which repo

Usage: otto state locks

Options:
  -h, --help
          Print help
```

### `otto state handoff`

```text
Rewrite handoff.md — the only thing the next wake gets for free (capped)

Usage: otto state handoff [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --file <FILE>
          Read the new handoff from this file

      --stdin
          Read the new handoff from stdin

  -h, --help
          Print help
```

### `otto state log`

```text
Append one journal line

Usage: otto state log [OPTIONS] --event <EVENT> <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --event <EVENT>
          The event name (`error`, `decision`, anything the instructions use)

      --message <MESSAGE>
          A one-line message

      --data <K=V>
          Extra fields (repeatable). Values parse as JSON when they can

  -h, --help
          Print help
```

### `otto state tail`

```text
Last N journal lines

Usage: otto state tail [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its id, a prefix of it, or its slug

Options:
      --lines <LINES>
          How many of the last journal lines to print
          
          [default: 20]

  -h, --help
          Print help
```
