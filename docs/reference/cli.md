# CLI reference

<!-- Generated from otto's own --help by `OTTO_UPDATE_DOCS=1 cargo test cli_reference`. Do not edit by hand. -->

Every command and flag, exactly as `--help` prints it. For what the commands are *for*, start with [concepts](../concepts.md) and the [guides](../README.md#guides).

Every `<ID>` takes the run number (`otto show 7`), its name, or a unique prefix of the name (`otto show leg`). Exit codes: `0` ok, `1` usage, `2` state conflict, `3` no such run.

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

      --name <NAME>
          What to call the run — short, unique, and what you type to mean it (`otto show legenda`). Change it later with `otto rename` [default: the target, the wrapped skill or file, else the goal's first words]

When it is done:
      --until <UNTIL>
          The done-condition, stated outright. Otherwise the first wake proposes one and gates it

      --perpetual
          Never: the run is retired with `otto stop`, not finished

      --period <AGE>
          How often it wakes when a wake doesn't ask for something else: `30m`, `1h`, `1d`. Measured from the start of one wake to the start of the next [default: 1h]

      --at <TIME>
          With a period of whole days, the local time its wakes are due: `3:00`, `22:30`, `3am` — for heavy work you want done while you aren't using the machine. The first wake still starts now

      --budget-wakes <N>
          Block with a gate after this many wakes (0: unlimited)
          
          [default: 0]

      --budget-hours <H>
          Block with a gate after this many hours since creation (0: unlimited)
          
          [default: 0]

      --budget-tokens <N>
          Block with a gate once its wakes have used this many tokens, counted as `otto usage` counts them — input, output and cache: `500k`, `50M` (0: unlimited)
          
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
          The run id, instead of the next run number

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
          The run: its number or name (a unique prefix of the name will do). Omitted, the one run waiting on you — or the only live run, if there is just one

Options:
  -h, --help
          Print help
```

## `otto inbox`

```text
What every run has put in front of you: questions to answer, and messages to read

Usage: otto inbox [OPTIONS]
       otto inbox <COMMAND>

Commands:
  read  Print one message in full: what the run sent, any draft to send on, what it attached
  done  Mark a message done — with a reply, the run reads your words on its next wake
  help  Print this message or the help of the given subcommand(s)

Options:
      --all
          Include questions already answered and messages already done, newest first

      --json
          Print the inbox as JSON

  -h, --help
          Print help
```

### `otto inbox read`

```text
Print one message in full: what the run sent, any draft to send on, what it attached

Usage: otto inbox read <ID> [MESSAGE]

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  [MESSAGE]
          The message's number. Omitted, the run's newest open one

Options:
  -h, --help
          Print help
```

### `otto inbox done`

```text
Mark a message done — with a reply, the run reads your words on its next wake

Usage: otto inbox done [OPTIONS] <ID> [MESSAGE]

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  [MESSAGE]
          The message's number. Omitted, the run's only open one

Options:
      --reply <REPLY>
          Your reply, handed to the run as a note: "dig into the grader next", "looks right"

      --now
          With --reply: wake the run now to read it, rather than at its next wake

  -h, --help
          Print help
```

## `otto outputs`

```text
What a run has made: its reports, documents and drafts, every version kept

Usage: otto outputs [OPTIONS] <ID> [KEY]

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  [KEY]
          One output's key: print it in full. Omitted, list them all

Options:
      --version <N>
          Which version of it. Omitted, the latest

      --full
          The whole of it, when it has a summary. Omitted, the summary

      --mark <MARK>
          Mark it done (you sent its draft, you acted on it), waiting again, or dropped

          Possible values:
          - waiting: It waits on the person: a draft to send, something to act on
          - done:    The person has dealt with it — sent the draft, acted on it
          - dropped: Not needed any more — someone else answered, it was overtaken

      --json
          Print as JSON

  -h, --help
          Print help (see a summary with '-h')
```

## `otto changes`

```text
Code runs have handed back: what waits on your review, what is landing, what landed

Usage: otto changes [OPTIONS]
       otto changes <COMMAND>

Commands:
  show     One change: what its author said, its files, what happened to it — and with --patch, the patch
  approve  Approve changes to land as their repository's process says, and wake the lander to land them
  revise   Send a change back to its author, saying what to change, and wake it to do it
  drop     Drop a change: it won't land, its author is told, and the lander cleans up after it
  merge    Merge a change's pull request, now it is approved and passing — the lander does it
  post     Post the replies to reviewers a change's author drafted — the lander does it
  answer   Answer what a change's author asked you about it
  allow    What the babysitter may change on a change's pull request without you, in words it judges by — "trivial", "small", "anything but security" — or "none"
  help     Print this message or the help of the given subcommand(s)

Options:
      --all
          Every change, not only those open or finished this week

      --due
          What waits on the lander, as JSON — `[]` when nothing does. For a check script

      --watch
          Every open pull request with news for the babysitter, as JSON — `[]` when none has. For a check script

      --json
          Print it as JSON

  -h, --help
          Print help
```

### `otto changes show`

```text
One change: what its author said, its files, what happened to it — and with --patch, the patch

Usage: otto changes show [OPTIONS] <CHANGE>

Arguments:
  <CHANGE>
          The change: <run>/<number>, as `otto changes` lists it

Options:
      --version <N>
          Which version of it. Omitted, the latest

      --patch
          Print the patch too

      --json
          Print it as JSON

  -h, --help
          Print help
```

### `otto changes approve`

```text
Approve changes to land as their repository's process says, and wake the lander to land them

Usage: otto changes approve [OPTIONS] <CHANGES>...

Arguments:
  <CHANGES>...
          The changes: <run>/<number>, one or more

Options:
      --note <NOTE>
          For the lander and the record, in a sentence

      --pr
          As a pull request, whatever its process says: the lander opens it and keeps it moving, and what reviewers ask goes to its author

      --push
          Pushed to its base branch, whatever its process says

      --merge-when-ready
          As a pull request merged once it is approved and passing, without asking you again

      --no-wake
          Record it, and don't wake the lander yet

  -h, --help
          Print help
```

### `otto changes revise`

```text
Send a change back to its author, saying what to change, and wake it to do it

Usage: otto changes revise [OPTIONS] --text <TEXT> <CHANGE>

Arguments:
  <CHANGE>
          The change: <run>/<number>

Options:
      --text <TEXT>
          What to change, for its author: it reads your words

      --no-wake
          Record it, and don't wake the author yet

  -h, --help
          Print help
```

### `otto changes drop`

```text
Drop a change: it won't land, its author is told, and the lander cleans up after it

Usage: otto changes drop [OPTIONS] <CHANGE>

Arguments:
  <CHANGE>
          The change: <run>/<number>

Options:
      --text <TEXT>
          Why, for its author

  -h, --help
          Print help
```

### `otto changes merge`

```text
Merge a change's pull request, now it is approved and passing — the lander does it

Usage: otto changes merge <CHANGE>

Arguments:
  <CHANGE>
          The change: <run>/<number>

Options:
  -h, --help
          Print help
```

### `otto changes post`

```text
Post the replies to reviewers a change's author drafted — the lander does it

Usage: otto changes post [OPTIONS] <CHANGE>

Arguments:
  <CHANGE>
          The change: <run>/<number>

Options:
      --comment <COMMENT>
          Only the reply to this comment. Omitted, every reply drafted for it

  -h, --help
          Print help
```

### `otto changes answer`

```text
Answer what a change's author asked you about it

Usage: otto changes answer --text <TEXT> <CHANGE>

Arguments:
  <CHANGE>
          The change: <run>/<number>

Options:
      --text <TEXT>
          Your answer: one of its options, or what you'd tell it in your own words

  -h, --help
          Print help
```

### `otto changes allow`

```text
What the babysitter may change on a change's pull request without you, in words it judges by — "trivial", "small", "anything but security" — or "none"

Usage: otto changes allow <CHANGE> <WORDS>

Arguments:
  <CHANGE>
          The change: <run>/<number>

  <WORDS>
          What may change without you; "none" for nothing

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
          The run: its number or name (a unique prefix of the name will do). Omitted, the one run waiting on you

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
          The run: its number or name (a unique prefix of the name will do)

  [TEXT]
          What to tell it, verbatim. Guidance on how to do the work — not an answer to a gate

Options:
      --file <FILE>
          Read the note from a file

      --standing
          Give it to every wake until dropped, not just the next one to complete

      --now
          Wake the run now to read it, rather than waiting for its next wake

      --about <KEY>
          The output the note is about — its key, or key@version — so the run knows which one you mean

      --list
          Show the run's standing notes and the ones not yet delivered

      --drop <NOTE>
          Withdraw an undelivered note, or retire a standing one, by number

  -h, --help
          Print help
```

## `otto chat`

```text
Ask a run about its work, in a conversation. It reads everything the run has done and changes none of it: what you decide, it leaves as a note

Usage: otto chat [OPTIONS] <ID> [QUESTION]

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  [QUESTION]
          What to ask first. Without one, the chat opens by saying where the run stands. With no terminal — from a script, or into a file — it is answered once, and the chat ends

Options:
      --dry-run
          Print the command the chat would run, and start nothing

  -h, --help
          Print help
```

## `otto check`

```text
Give a run a check script poke runs instead of a wake, see it, or remove it

Usage: otto check [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

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
          The run: its number or name (a unique prefix of the name will do)

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
What wakes and chats have used, in tokens, across every run — by run or by day

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

## `otto feedback`

```text
What wakes have said about otto itself: where it got in their way

Usage: otto feedback [OPTIONS]

Options:
      --all
          Include the reports already handled

      --done <N>...
          Mark these reports handled — read, and fixed or set aside. They stay in the file

      --json
          Print the reports as JSON

  -h, --help
          Print help
```

## `otto attach`

```text
Follow what the running wake is saying and doing, as it happens

Usage: otto attach <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

Options:
  -h, --help
          Print help
```

## `otto period`

```text
See or change how often a run wakes

Usage: otto period [OPTIONS] <ID> [DURATION]

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  [DURATION]
          How often it wakes when a wake doesn't ask for something else: `30m`, `4h`, `1d`. Measured from the start of one wake to the start of the next. Omit it, and --at, to see the current period

Options:
      --at <TIME>
          With a period of whole days, the local time its wakes are due: `3:00`, `22:30`, `3am` — for heavy work you want done while you aren't using the machine

      --any-time
          Drop the time of day: it wakes whenever the period comes round

  -h, --help
          Print help
```

## `otto policy`

```text
See or change a run's policy: how long a wake may take, and the rest of what `--policy` sets

Usage: otto policy <ID> [KEY=VALUE]...

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  [KEY=VALUE]...
          What to set, as at `otto run --policy`: maxWakeMinutes=240, maxIncompleteWakes=5. Omit it to see the run's policy. The period is `otto period`'s

Options:
  -h, --help
          Print help
```

## `otto rename`

```text
Give a run a new name

Usage: otto rename <ID> <NAME>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  <NAME>
          Its new name — letters, digits and hyphens, not just a number, and not another run's

Options:
  -h, --help
          Print help
```

## `otto renumber`

```text
Give every run from before run numbers a number, keeping its old id as an alias

Usage: otto renumber

Options:
  -h, --help
          Print help
```

## `otto pause`

```text
Hold back a run's scheduled wakes — for a while, or until you resume it

Usage: otto pause [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

Options:
      --for <AGE>
          For this long: `2h`, `3d`

      --until <WHEN>
          Until then: a time today or tomorrow (`09:00`), a date (`2026-10-19`, that morning at 09:00), or a full timestamp. Omitted with --for too: until you `otto resume` it

      --reason <REASON>
          Why, recorded in the journal

  -h, --help
          Print help
```

## `otto stop`

```text
Retire a run

Usage: otto stop [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

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
Lift a pause, or bring a stopped or failed run back and wake it

Usage: otto resume [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

Options:
      --reason <REASON>
          Why, recorded in the journal

      --no-wake
          Put it back on its schedule without waking it now

  -h, --help
          Print help
```

## `otto delete`

```text
Delete runs for good, after stopping any still going. It asks first, and whether to keep their data until `otto cleanup`

Usage: otto delete [OPTIONS] <IDS>...

Arguments:
  <IDS>...
          The runs: each its number or name (a unique prefix of the name will do)

Options:
      --data <DATA>
          Their data in $OTTO_HOME — the journal, questions and answers, notes, messages, outputs: kept in archive/ until `otto cleanup`, or deleted now. Asked when not given; kept with --yes

          Possible values:
          - keep:   Moved to `archive/`, where `otto cleanup` finds it — the default, because it can be undone by hand and deleting it cannot
          - delete: Removed, now

  -y, --yes
          Don't ask first. With no terminal there is nobody to ask, so it is needed there

  -h, --help
          Print help (see a summary with '-h')
```

## `otto cleanup`

```text
What otto keeps that no agent uses — deleted agents' data, and anything else left over — with its size and when it was last used, to delete

Usage: otto cleanup [OPTIONS]

Options:
      --older-than <AGE>
          Only what nothing has used for at least this long: `12h`, `30d`

  -y, --yes
          Delete everything listed, without asking

  -h, --help
          Print help
```

## `otto wake`

```text
Run one wake: spawn the model, then validate what it left behind

Usage: otto wake [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

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
  start    Install (if missing) and start `otto serve` under launchd
  stop     Stop it; it stays stopped until the next `otto service start`
  restart  Restart it on the port it was installed with — after `./install.sh`, to serve the new binary
  status   Is it loaded, on which port, and where are its plist and log
  help     Print this message or the help of the given subcommand(s)

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

### `otto service restart`

```text
Restart it on the port it was installed with — after `./install.sh`, to serve the new binary

Usage: otto service restart

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
  lander   The run that lands approved changes — the only one that pushes them
  process  How a change goes from started to landed: make one, change one, or list them
  project  Which process a repository follows: what approving its changes does
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

### `otto config lander`

```text
The run that lands approved changes — the only one that pushes them

Usage: otto config lander [OPTIONS] [RUN]

Arguments:
  [RUN]
          Make this run the lander. Omit to print which run is

Options:
      --off
          There is no lander: approving a change is refused until there is one

  -h, --help
          Print help
```

### `otto config process`

```text
How a change goes from started to landed: make one, change one, or list them

Usage: otto config process [OPTIONS] [NAME]

Arguments:
  [NAME]
          The process to show, make or change. Omit to list every process and who follows it

Options:
      --mode <MODE>
          What approving a change does: push it to its base, or open a pull request. Needed to make one

          Possible values:
          - push: Onto its base branch, and pushed
          - pr:   As a pull request, which the lander keeps moving: reviewers' asks go to its author

      --merge-when-ready [<BOOL>]
          With --mode pr: merge it once it is approved and passing, without asking again (or `false`)
          
          [possible values: true, false]

      --start <START>
          How a change begins, in words, for the run that makes it: a branch or a worktree, a ticket first

      --land <LAND>
          How an approved change lands, in words, for the lander: a ship script, a merge method

      --allow-changes <WORDS>
          With --mode pr: what the babysitter may change on a pull request without you, in words it judges by — "trivial", "small", "anything but security" — or "none"

      --delete
          Remove it. One a repository follows is refused

      --json
          Print it as JSON

  -h, --help
          Print help (see a summary with '-h')
```

### `otto config project`

```text
Which process a repository follows: what approving its changes does

Usage: otto config project [OPTIONS] [REPO]

Arguments:
  [REPO]
          The repository: any checkout of it, or a directory inside one. Omit to list the repositories given a process

Options:
      --process <PROCESS>
          Have it follow this process; `standard` is the one every repository follows until told otherwise

      --json
          Print it as JSON

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
  verify            Answer a done claim: confirm the run is done, or send it back to work
  record-fact       Write re-derived facts
  open-gate         Write the gate file and stop the run on it
  close-gate        Record the answer verbatim and clear the gate
  message           Send the person a message — a report, a draft to send on, an update — to their inbox
  publish           Keep something the run made where the person can find it: a new key is a new output, the same key again its next version
  stages            Declare the stages the run's work moves through, in order
  output-state      Mark an output waiting, done, or not needed any more
  change            Code handed back: a wake submits it, and the lander lands it
  arm-timer         Record nextWakeAt. This is the whole timer: poke reads it, nothing else is needed
  set-check         Give the run a standing check script, or remove the one a wake gave it
  tick              Account for one timer tick
  feedback          Tell otto's maker what got in your way: about otto itself, never the work. Always exits 0
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

      --name <NAME>
          What to call the run — short, unique, and what you type to mean it (`otto show legenda`). Change it later with `otto rename` [default: the target, the wrapped skill or file, else the goal's first words]

When it is done:
      --until <UNTIL>
          The done-condition, stated outright. Otherwise the first wake proposes one and gates it

      --perpetual
          Never: the run is retired with `otto stop`, not finished

      --period <AGE>
          How often it wakes when a wake doesn't ask for something else: `30m`, `1h`, `1d`. Measured from the start of one wake to the start of the next [default: 1h]

      --at <TIME>
          With a period of whole days, the local time its wakes are due: `3:00`, `22:30`, `3am` — for heavy work you want done while you aren't using the machine. The first wake still starts now

      --budget-wakes <N>
          Block with a gate after this many wakes (0: unlimited)
          
          [default: 0]

      --budget-hours <H>
          Block with a gate after this many hours since creation (0: unlimited)
          
          [default: 0]

      --budget-tokens <N>
          Block with a gate once its wakes have used this many tokens, counted as `otto usage` counts them — input, output and cache: `500k`, `50M` (0: unlimited)
          
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
          The run id, instead of the next run number

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
          The run: its number or name (a unique prefix of the name will do)

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
          The run: its number or name (a unique prefix of the name will do)

Options:
      --phase <PHASE>
          The phase label to enter — free-form, for people and the wrapped instructions

      --status <STATUS>
          The status to enter with it. `running` also clears any scheduled wake. Omitted: `running`, unless a gate is open, when the status is left as it is
          
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
          The run: its number or name (a unique prefix of the name will do)

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
          - unverified:    It said it was done more than `policy.maxDoneReopens` times, and each time a fresh wake checking the claim found it wasn't

  -h, --help
          Print help (see a summary with '-h')
```

### `otto state verify`

```text
Answer a done claim: confirm the run is done, or send it back to work

Usage: otto state verify --reason <REASON> <--confirm|--reopen> <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

Options:
      --confirm
          The claim holds: the run is done

      --reopen
          The claim does not hold: the run goes back to work, starting now

      --reason <REASON>
          With --confirm, what you checked and what showed it; with --reopen, what is missing. Recorded verbatim, and a reopen's reason is the first thing the next wake is told

  -h, --help
          Print help
```

### `otto state record-fact`

```text
Write re-derived facts

Usage: otto state record-fact <ID> <K=V>...

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

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
          The run: its number or name (a unique prefix of the name will do)

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

      --about <KEY>
          The output the question is about — its key, or key@version — when you need it approved: the answer is kept on that version as the person's review of it

  -h, --help
          Print help
```

### `otto state close-gate`

```text
Record the answer verbatim and clear the gate

Usage: otto state close-gate [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

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

### `otto state message`

```text
Send the person a message — a report, a draft to send on, an update — to their inbox

Usage: otto state message [OPTIONS] --kind <KIND> --title <TITLE> <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

Options:
      --kind <KIND>
          What it is: a report to read, a draft for the person to send on, or an update worth knowing

          Possible values:
          - report: Something the run made for the person to read: a report, a finding, a summary
          - draft:  Text written for the person to send to someone else — a reply, a comment. The run never sends it itself
          - update: News the person would want: a task finished, a PR merged, something worth knowing

      --title <TITLE>
          What it is in a few words, for a list: "Why refund-policy-qa fails"

      --body <BODY>
          The body, inline: written for a person reading it cold

      --body-file <BODY_FILE>
          Read the body from this file

      --stdin
          Read the body from stdin

      --draft <DRAFT>
          Text for the person to send on to someone else, verbatim — kept apart so it can be copied whole

      --draft-file <DRAFT_FILE>
          Read the draft from this file

      --attach <PATH>
          A file in the run directory the message points at, relative to it (repeatable)

      --output <KEY>
          The output the message is about — its key, or key@version: the inbox shows it as a card that opens it, so the message can be the short news and the output the work

  -h, --help
          Print help (see a summary with '-h')
```

### `otto state publish`

```text
Keep something the run made where the person can find it: a new key is a new output, the same key again its next version

Usage: otto state publish [OPTIONS] --key <KEY> --title <TITLE> <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

Options:
      --key <KEY>
          What it is, for good: a new key is a new output (a report a day: `2026-10-16-refund-policy-qa`); the same key again is the next version of it (`design`)

      --title <TITLE>
          What it is in a few words, for a list: "refund-policy-qa: a stale index, not the model"

      --file <PATH>
          The text of this version — a path, or one relative to the run directory (artifacts/design.md)

      --stdin
          Read the text from stdin

      --link <URL>
          Or where the work lives, for work kept elsewhere: a ticket, a PR, a page

      --stage <STAGE>
          Which of the run's declared stages it belongs to (`otto state stages`)

      --about <ABOUT>
          What it responds to, usually a link: the thread a question was asked in, the ticket

      --draft <DRAFT>
          Text for the person to send on, verbatim — a reply to the question it is about

      --draft-file <PATH>
          Read the draft from this file

      --summary-file <PATH>
          A summary of the file, for a document over a page (500 words): the page opens on it, with the whole a click away. The same file again with a summary adds it to that version

      --state <STATE>
          Whether it waits on the person; an output with a draft waits until they deal with it

          Possible values:
          - waiting: It waits on the person: a draft to send, something to act on
          - done:    The person has dealt with it — sent the draft, acted on it
          - dropped: Not needed any more — someone else answered, it was overtaken

  -h, --help
          Print help (see a summary with '-h')
```

### `otto state stages`

```text
Declare the stages the run's work moves through, in order

Usage: otto state stages <ID> <STAGES>...

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  <STAGES>...
          The steps the work moves through, in order: Research Design Plan

Options:
  -h, --help
          Print help
```

### `otto state output-state`

```text
Mark an output waiting, done, or not needed any more

Usage: otto state output-state [OPTIONS] <ID> <KEY> <STATE>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  <KEY>
          The output's key

  <STATE>
          waiting, done, or dropped (not needed any more)

          Possible values:
          - waiting: It waits on the person: a draft to send, something to act on
          - done:    The person has dealt with it — sent the draft, acted on it
          - dropped: Not needed any more — someone else answered, it was overtaken

Options:
      --note <NOTE>
          Why, in a few words: "Sam answered it in the thread"

  -h, --help
          Print help (see a summary with '-h')
```

### `otto state change`

```text
Code handed back: a wake submits it, and the lander lands it

Usage: otto state change <COMMAND>

Commands:
  submit     Hand back code committed on a branch, for the person to review and the lander to land. The same branch (or --key) again is its next version. It doesn't stop the run
  withdraw   Take back a change you no longer want: overtaken, or the wrong approach
  reply      Draft a reply to a reviewer's comment on its pull request, for the person to post
  ask        Ask the person something about a change — a judgement that is theirs — without stopping the run
  due        The lander: what waits on it, across every run — approved changes to land, then cleaning up
  watch      The babysitter: every open pull request that has moved since it last looked
  fix        The babysitter: it changed the code on a pull request itself, as the person allowed — the lander pushes it
  claim      The lander: take an approved change in hand, and print what lands
  check      The lander: refuse to push a commit that isn't what the person approved — exit 2 means DO NOT ACT
  landed     The lander: it is on its base branch now. otto looks, and tells its author
  rebased    The lander: it was rebased onto a base that moved and its patch changed, so it goes back to the person
  bounce     The lander: it couldn't land — a conflict, tests red after a rebase — so it goes back to its author
  cleaned    The lander: its worktree and branch are gone (otto looks), or say what was left and why
  pr-pushed  The lander: it opened the change's pull request (--url), or pushed its next version to it
  pr-seen    The babysitter: what its pull request says now — CI, review, whether it merges — and when it said it
  relay      The babysitter (or the lander): hand what a reviewer or CI asked of its pull request to the change's author
  merged     The lander or the babysitter: its pull request was merged. otto looks, and tells its author
  closed     The lander or the babysitter: its pull request was closed without merging
  posted     The lander: it posted a reply the person said to post
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### `otto state change submit`

```text
Hand back code committed on a branch, for the person to review and the lander to land. The same branch (or --key) again is its next version. It doesn't stop the run

Usage: otto state change submit [OPTIONS] --repo <REPO> --branch <BRANCH> --title <TITLE> --summary-file <PATH> <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

Options:
      --repo <REPO>
          The repository: its main checkout, or any worktree of it

      --branch <BRANCH>
          The branch the work is committed on

      --title <TITLE>
          What it does, in a few words, for a list: "Answering a stopped run's question points to otto resume"

      --summary-file <PATH>
          What you'd tell the person reviewing it: what it does and why first, then what could go wrong

      --tests <TESTS>
          What the tests said, in a line: "cargo test: 429 passed"

      --base <BASE>
          The branch it lands on. Omitted, the remote's default branch

      --key <KEY>
          What this run calls it, when not its branch: the same key again is its next version

  -h, --help
          Print help
```

### `otto state change withdraw`

```text
Take back a change you no longer want: overtaken, or the wrong approach

Usage: otto state change withdraw --reason <REASON> <ID> <CHANGE>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  <CHANGE>
          The change's number

Options:
      --reason <REASON>
          Why, in a sentence: the person reads it on the change

  -h, --help
          Print help
```

### `otto state change reply`

```text
Draft a reply to a reviewer's comment on its pull request, for the person to post

Usage: otto state change reply --comment <COMMENT> --text <TEXT> <ID> <CHANGE>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

  <CHANGE>
          The change's number

Options:
      --comment <COMMENT>
          The comment it answers: its id, or its link

      --text <TEXT>
          The reply, as it would be posted

  -h, --help
          Print help
```

### `otto state change ask`

```text
Ask the person something about a change — a judgement that is theirs — without stopping the run

Usage: otto state change ask [OPTIONS] --question-file <PATH> <ID> <CHANGE>

Arguments:
  <ID>
          The run that wrote it: its number or name (a unique prefix of the name will do)

  <CHANGE>
          The change's number

Options:
      --question-file <PATH>
          The question, written to be answered cold: where it stands, the decision, options under `## Options`

      --by <BABYSITTER>
          The babysitter asking, rather than its author: the answer comes to it

  -h, --help
          Print help
```

### `otto state change due`

```text
The lander: what waits on it, across every run — approved changes to land, then cleaning up

Usage: otto state change due [OPTIONS] <LANDER>

Arguments:
  <LANDER>
          The lander run: its number or name

Options:
      --json
          Print it as JSON

  -h, --help
          Print help
```

### `otto state change watch`

```text
The babysitter: every open pull request that has moved since it last looked

Usage: otto state change watch [OPTIONS] <BABYSITTER>

Arguments:
  <BABYSITTER>
          The babysitter run: its number or name

Options:
      --json
          Print it as JSON

  -h, --help
          Print help
```

### `otto state change fix`

```text
The babysitter: it changed the code on a pull request itself, as the person allowed — the lander pushes it

Usage: otto state change fix [OPTIONS] --head <HEAD> --summary-file <PATH> --why <WHY> <BABYSITTER> <CHANGE>

Arguments:
  <BABYSITTER>
          The babysitter run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --head <HEAD>
          The commit with the fix, on top of what its pull request has

      --summary-file <PATH>
          What it changed and why, for the person reading the change

      --tests <TESTS>
          What the tests said, in a line

      --why <WHY>
          Why this is within what the person allowed: the words you went by, or their answer

  -h, --help
          Print help
```

### `otto state change claim`

```text
The lander: take an approved change in hand, and print what lands

Usage: otto state change claim <LANDER> <CHANGE>

Arguments:
  <LANDER>
          The lander run: its number or name

  <CHANGE>
          The change: <run>/<number>, as `otto state change due` lists it

Options:
  -h, --help
          Print help
```

### `otto state change check`

```text
The lander: refuse to push a commit that isn't what the person approved — exit 2 means DO NOT ACT

Usage: otto state change check --head <HEAD> <LANDER> <CHANGE>

Arguments:
  <LANDER>
          The lander run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --head <HEAD>
          The commit you are about to put on the base branch

  -h, --help
          Print help
```

### `otto state change landed`

```text
The lander: it is on its base branch now. otto looks, and tells its author

Usage: otto state change landed [OPTIONS] --sha <SHA> <LANDER> <CHANGE>

Arguments:
  <LANDER>
          The lander run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --sha <SHA>
          The commit now on the base branch

      --note <NOTE>
          Anything its author should know, in a sentence: "the main checkout has uncommitted changes, so it wasn't updated"

  -h, --help
          Print help
```

### `otto state change rebased`

```text
The lander: it was rebased onto a base that moved and its patch changed, so it goes back to the person

Usage: otto state change rebased --head <HEAD> <LANDER> <CHANGE>

Arguments:
  <LANDER>
          The lander run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --head <HEAD>
          The rebased commit

  -h, --help
          Print help
```

### `otto state change bounce`

```text
The lander: it couldn't land — a conflict, tests red after a rebase — so it goes back to its author

Usage: otto state change bounce --reason <REASON> <LANDER> <CHANGE>

Arguments:
  <LANDER>
          The lander run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --reason <REASON>
          What stopped it, specifically enough to fix: "a.txt conflicts with 3f2a1c0 on main"

  -h, --help
          Print help
```

### `otto state change cleaned`

```text
The lander: its worktree and branch are gone (otto looks), or say what was left and why

Usage: otto state change cleaned [OPTIONS] <LANDER> <CHANGE>

Arguments:
  <LANDER>
          The lander run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --left <LEFT>
          What you left on purpose, and why: "the worktree, which has uncommitted work in it"

  -h, --help
          Print help
```

### `otto state change pr-pushed`

```text
The lander: it opened the change's pull request (--url), or pushed its next version to it

Usage: otto state change pr-pushed [OPTIONS] --head <HEAD> <LANDER> <CHANGE>

Arguments:
  <LANDER>
          The lander run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --head <HEAD>
          The commit its pull request's branch has now

      --url <URL>
          The pull request's address, when you just opened it

      --auto-merge
          You turned on GitHub's auto-merge for it (`gh pr merge --auto`)

  -h, --help
          Print help
```

### `otto state change pr-seen`

```text
The babysitter: what its pull request says now — CI, review, whether it merges — and when it said it

Usage: otto state change pr-seen [OPTIONS] --updated <UPDATED> <BABYSITTER> <CHANGE>

Arguments:
  <BABYSITTER>
          The babysitter run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --updated <UPDATED>
          The pull request's `updatedAt`, as `gh pr view --json updatedAt` gave it

      --ci <CI>
          pending, passing or failing

      --review <REVIEW>
          approved, changes-requested, waiting, or none (nobody has to review it)

      --mergeable <MERGEABLE>
          clean, behind or conflicting

      --what <WHAT>
          What happened on it, in a line, for the change's history: "CI passing; Sam approved"

  -h, --help
          Print help
```

### `otto state change relay`

```text
The babysitter (or the lander): hand what a reviewer or CI asked of its pull request to the change's author

Usage: otto state change relay --who <WHO> --text <TEXT> <BABYSITTER> <CHANGE>

Arguments:
  <BABYSITTER>
          The babysitter run (or the lander's): its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --who <WHO>
          Who: "a reviewer (sam)", "CI"

      --text <TEXT>
          What they said, quoted — the comment's words, or the failing job and its first error lines

  -h, --help
          Print help
```

### `otto state change merged`

```text
The lander or the babysitter: its pull request was merged. otto looks, and tells its author

Usage: otto state change merged [OPTIONS] --sha <SHA> <LANDER> <CHANGE>

Arguments:
  <LANDER>
          The lander run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --sha <SHA>
          The commit now on the base branch

      --note <NOTE>
          Anything its author should know, in a sentence: "the main checkout has uncommitted changes, so it wasn't updated"

  -h, --help
          Print help
```

### `otto state change closed`

```text
The lander or the babysitter: its pull request was closed without merging

Usage: otto state change closed --reason <REASON> <LANDER> <CHANGE>

Arguments:
  <LANDER>
          The lander run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --reason <REASON>
          What stopped it, specifically enough to fix: "a.txt conflicts with 3f2a1c0 on main"

  -h, --help
          Print help
```

### `otto state change posted`

```text
The lander: it posted a reply the person said to post

Usage: otto state change posted --comment <COMMENT> <LANDER> <CHANGE>

Arguments:
  <LANDER>
          The lander run: its number or name

  <CHANGE>
          The change: <run>/<number>

Options:
      --comment <COMMENT>
          The comment the reply answered

  -h, --help
          Print help
```

### `otto state arm-timer`

```text
Record nextWakeAt. This is the whole timer: poke reads it, nothing else is needed

Usage: otto state arm-timer [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

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

      --waiting-for <WHAT>
          What this sleep waits on, in a few words: `CI on 3f2a1c0`. Re-arm the same wait with the same words — it keeps when it started, and its limit

      --give-up-after <AGE>
          How long that wait may take in all: `2h`, `1d`. Past it, otto refuses another sleep for it and the next wake must decide

  -h, --help
          Print help
```

### `otto state set-check`

```text
Give the run a standing check script, or remove the one a wake gave it

Usage: otto state set-check [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

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
          The run: its number or name (a unique prefix of the name will do)

Options:
      --progress
          something changed that otto can't see — a reply posted, a ticket moved — so this wake moved the run. otto sees commits, outputs, messages, the todo, facts and the phase for itself

      --note <NOTE>
          What this tick saw, recorded in the journal

  -h, --help
          Print help
```

### `otto state feedback`

```text
Tell otto's maker what got in your way: about otto itself, never the work. Always exits 0

Usage: otto state feedback [OPTIONS] <ID>

Arguments:
  <ID>
          The run: its number or name (a unique prefix of the name will do)

Options:
      --body <BODY>
          In your own words: what got in the way, what you did about it, and what would have helped

      --body-file <BODY_FILE>
          Read it from this file

      --stdin
          Read it from stdin

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
          The run: its number or name (a unique prefix of the name will do)

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
          The run: its number or name (a unique prefix of the name will do)

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
          The run: its number or name (a unique prefix of the name will do)

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
          The run: its number or name (a unique prefix of the name will do)

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
          The run: its number or name (a unique prefix of the name will do)

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
          The run: its number or name (a unique prefix of the name will do)

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
          The run: its number or name (a unique prefix of the name will do)

Options:
      --lines <LINES>
          How many of the last journal lines to print
          
          [default: 20]

  -h, --help
          Print help
```
