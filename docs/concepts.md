# Concepts

How otto thinks about a long-running piece of work, and how the parts fit together. Read this
once; the [guides](README.md#guides) assume it. For *why* each piece is shaped the way it is, see
[DESIGN.md](../DESIGN.md).

- [Runs and wakes](#runs-and-wakes)
- [The handoff](#the-handoff)
- [The todo](#the-todo)
- [Activity](#activity)
- [The contract](#the-contract)
- [Checking done](#checking-done)
- [Statuses](#statuses)
- [When a run wakes: the period and timers](#when-a-run-wakes-the-period-and-timers)
- [Check scripts](#check-scripts)
- [Gates](#gates)
- [Notes](#notes)
- [Chats](#chats)
- [Messages and the inbox](#messages-and-the-inbox)
- [Outputs: what it made, kept](#outputs-what-it-made-kept)
- [Changes: code handed back](#changes-code-handed-back)
- [Pausing](#pausing)
- [When things go wrong](#when-things-go-wrong)
- [Budgets and policy](#budgets-and-policy)
- [Outward actions and repo locks](#outward-actions-and-repo-locks)
- [The reviver and notifications](#the-reviver-and-notifications)
- [What a wake costs](#what-a-wake-costs)
- [Feedback from wakes](#feedback-from-wakes)
- [Where everything lives](#where-everything-lives)

## Runs and wakes

A **run** is a goal, the thing it wraps (a Claude Code skill, a prose file, or nothing but the
goal), and a directory of state on disk. A run can last minutes or months.

A **wake** is one process: otto starts `claude` non-interactively, the model orients from the run
directory, does a stretch of work, reaches a stopping point and exits. A wake is not one model
turn — it runs as long as the work does, across many turns and subagents — but it always ends.
There is no long-lived session, and nothing survives a wake except what it wrote to disk.

```
otto run ──> wake 1 ──> opens a gate, exits
                          │
             otto answer ─┘──> wake 2 ──> sleeps, exits
                                            │
                          otto poke (every 5 min) ──> wake 3 ──> done
```

Three things start a wake:

| Cause | Who starts it |
|---|---|
| The run's wake time has come | `otto poke`, the reviver launchd runs every ~5 minutes |
| A person answered a gate | `otto answer` (or the web UI) |
| You asked | `otto wake <id>`, or `otto note <id> "…" --now` |

Every wake starts in the run's **working directory** — `~/work`, say, set once as the default in
`config.json` or per run with `--workdir` — whoever starts it, so every wake sees the same
`CLAUDE.md` and project settings. Every wake starts cold and re-derives the world — git, `gh`,
the ticket — because between two wakes `main` moved, CI reran, and someone force-pushed. That is the price of durability, and the
reason a run survives a reboot, a crash or a week away.

## The handoff

`handoff.md` in the run directory is the only thing the next wake gets for free. Every wake
**rewrites** it (never appends) in fixed sections — where the run is, what was decided, what this
wake did, what the next wake must do first, what it needs — capped at 8KB. Anything that has to
accumulate goes in an artifact the handoff points to. `otto show` prints it.

Two of its sections are written for you as well. The runs list shows **Where this is** under each
run, and the run's page shows **Needs** whenever it says anything but "nothing": what the run would
like from you without stopping to ask, like a key that would help.

Those two are checked: `otto state handoff` refuses a handoff whose `Where this is` or `Needs`
talks in otto's own words — wake, gate, poke, handoff, check.sh, `otto state`, `artifacts/` paths,
timestamps — and says which, so the wake rewrites them in the work's own terms before it stops.
Each refusal is journaled as `handoff-refused`.

## The todo

When a run's work comes in pieces — twenty repos to bump, four things to build, a cleanup that
mustn't be forgotten — the wake keeps `artifacts/todo.md`: a markdown checklist, one line per
piece, marked as it goes.

```markdown
## Upgrade
- [x] acme-web — green at a1b2c3d
- [~] acme-api
- [ ] acme-jobs
- [-] acme-cli — skipped: already at 2.31.0
- [!] acme-old — failed: tests red, see artifacts/targets/acme-old/
```

`[ ]` to do, `[~]` doing, `[x]` done, `[-]` skipped and `[!]` failed, the last two saying why.
otto reads it and never writes it: `otto show` has a `todo` line counting it, and the web page shows
a progress bar in the runs list and the whole list on the run page. A run that keeps no todo — one
that only watches, say — just shows none.

A run marked `done` with items still `[ ]` or `[~]` is allowed, but never silent: `set-status`
says so to the wake that did it, the `status-changed` line in the journal lists them, and `otto
show` and the web page flag the run. A skipped or failed item is accounted for; only one left open
is work nothing says happened.

## Activity

A run that watches or repeats — an hourly queue check, a daily curation — has nothing to tick off,
so its progress is what each wake did. A wake that did something journals one line about it:

```bash
otto state log <id> --event cycle --message "added 1 talk; still no feedback since Oct 5"
```

Everything a wake journals this way (any event that isn't one of otto's own) is the run's
**activity**: the newest line shows under the run in the web list and as `last` in `otto show`, and
the run page lists the last 20. A wake that found nothing writes nothing, so a quiet run's feed
stays quiet. `otto logs` still has the whole journal.

## The contract

The only thing otto enforces about a wake, and what makes wrapping *anything* possible. When a
wake's process exits, otto checks two things:

1. **The run is somewhere it will be brought back from** — a gate open, a wake time set, or
   finished.
2. **This wake rewrote `handoff.md`**, within its cap.

A wake that exits cleanly with nothing pending doesn't have to do anything special: otto puts
the run to sleep for its [period](#when-a-run-wakes-the-period-and-timers).

Pass, and the wake is `wake-complete`. Fail for any reason — a crash, a kill at its deadline, a
model that simply stopped — and it is `wake-incomplete`: see [When things go
wrong](#when-things-go-wrong). The check happens in the parent process, because a wake that
crashed cannot report on itself.

## Checking done

`done` is the one stop nothing ever comes back from, so otto doesn't take it on the word of the wake
that did the work. When a wake says the run is done (`otto state set-status --status done`), that is
recorded as a **claim**, and the next wake starts straight away to check it — cold, with none of
the claiming wake's context, the way a reviewer who wasn't there would. It reads the done condition
and what the run wraps, looks at the world itself (the commit on `main`, the worktree gone, the
output published, every todo item accounted for), and then either:

- **confirms** it (`otto state verify --confirm`): the run is done, and what was checked is kept
  (`otto show` prints it as `checked`); or
- **reopens** it (`otto state verify --reopen`): the run goes straight back to work, and the next
  wake is told exactly what was missing before anything else.

The wake that made a claim can't confirm it, and a checking wake that ends without a verdict (or a
question for you) counts as a wake that didn't finish. A run that keeps saying it's done when it
isn't stops after `maxDoneReopens` reopens (2 by default) and asks you whether to keep going.

While a claim waits, the list says the run is waiting on *a check that it's done*. Checking costs one
wake per finished run. To trust a run's `done` as said, start it with `--policy verifyDone=false`.

## Statuses

| Status | Meaning | What brings it back |
|---|---|---|
| `running` | A wake is working, or about to | — |
| `sleeping` | Waiting for its wake time | poke, when `nextWakeAt` passes |
| `awaiting_human` | A gate is open | `otto answer`, or the gate expiring |
| `blocked` | otto stopped trying and opened a gate explaining why | a person |
| `done` | The goal is met, and a wake that didn't do the work [checked](#checking-done) | nothing — the run is over |
| `failed` | It could not do its job | `otto resume` |
| `stopped` | Retired by a person | `otto resume` |

`blocked` always records a cause: `wake-failures` (wakes kept failing), `budget` (a budget was
spent), `stall` (wake after wake changed nothing), `instructions` (the wrapped instructions decided
it couldn't go on), or `unverified` (it kept saying it was done, and each check found it wasn't).

**A stall is otto's call, not the wake's.** After every wake, otto looks at what the run has to show
for it: the phase and facts, gates and their answers, outputs and messages, every file under
`artifacts/` (the todo among them), and the branches and worktrees of its `--repo`s. When
`maxTicksWithoutProgress` wakes in a row (24 by default) leave all of that as it was, otto blocks the
run and asks whether to keep going. Waiting is fine; waiting forever, a wake an hour, is the most
expensive way for a run to fail. A wake whose work lands somewhere otto can't see — a reply posted
on a PR, a ticket moved — says so with `otto state tick <id> --progress`. A perpetual run is never
stalled: watching and finding nothing is its job.

## When a run wakes: the period and timers

Every run has a **period** — how often it wakes when nothing else says otherwise. It's an hour
unless the run was started with `--period`, and `otto period <id> 4h` changes it later.

When a wake finishes cleanly with nothing pending, otto sleeps the run until **one period after
that wake started** (so an hourly run stays hourly rather than drifting by each wake's length).

A wake can say something different for one sleep: `otto state arm-timer --in 600` because CI
takes ten minutes, or `--at` for tomorrow morning. That lasts one sleep; after it the run is back
on its period. An open gate takes precedence over both.

A wait can carry a limit: `arm-timer --in 600 --waiting-for "CI on 3f2a1c0" --give-up-after 2h`.
Re-armed with the same words it keeps when it started and its limit, so the limit can't creep
later one sleep at a time. Once it passes, otto refuses another sleep for it and tells the next
wake first thing to decide — usually by asking you. `otto show` says what a run is waiting on. A
sleep for anything else ends the wait, and so does your answer to a question: "keep waiting"
starts it afresh.

A run whose machine was asleep, or whose reviver was off, gets **one** catch-up wake when it comes
back — never a replay of every wake it missed.

## Check scripts

A wake costs a cold model session even when there is nothing to do. A **check script** lets a
shell script answer "anything new?" first: when the run's wake comes due, poke runs the script
instead — no model, no tokens — and only wakes the run if it says something changed. Exit 0 means
nothing new, and the run sleeps another period; anything else wakes it. A safety net wakes it
anyway after 24 "nothing new" results in a row.

The period then means "how often to look", and the check decides whether looking needs a model.
A run whose goal is mostly watching usually sets up its own check on its first wake; you can
always set one yourself, and yours wins. [Check scripts](guides/check-scripts.md) covers both.

## Gates

A **gate** is the run asking a person something. A wake writes the question to
`gates/NNN-<slug>.md`, opens the gate, and exits; the run is `awaiting_human` until someone
answers with `otto answer` (or a button in the web UI). The answer is recorded verbatim and the
next wake starts.

A gate question is written to be answered cold — you arrive hours later with no transcript — so
it carries where the run is, the specific decision, named options with their consequences, and a
default. `otto answer --choice <name>` picks an option; `--text` is for anything that needs
prose.

- **Expiring gates.** A gate can expire (`open-gate --expires-in`), for questions where silence
  has a sane meaning ("no answer, so don't do it"). When it expires a wake comes to deal with it.
- **Stale gates.** A gate left unanswered past `gateStaleAfterHours` (48 by default) gets one
  more notification.

## Notes

A gate is the run asking; a **note** is you telling, whenever you like, gated or not:
`otto note <id> "skip the e2e suite, it's broken on main"`. The next wake gets it verbatim in its
prompt. It is delivered once that wake *completes* — a wake that crashes never delivered it, so
the next one gets it again.

- `--standing` gives a note to every wake until you `--drop` it, for guidance that should hold
  for the life of the run.
- `--now` starts a wake to read it, if none is running and no gate is open.
- A note you leave while a wake is working reaches that wake before it asks you anything or says
  it's done: otto refuses the question or the done once, with your note in the refusal.
- A note steers *how* the run works. It never changes the goal or the done-condition, and never
  authorizes a push; a wake opens a gate for anything like that.

## Chats

A gate is the run asking and a note is you telling, but neither answers a question *you* have —
why it skipped the e2e suite, what it found in the logs, what is actually left. A **chat** does:

```bash
otto chat 7                                  # it says where the run stands, then it's your turn
otto chat 7 "why did wake 12 skip e2e?"      # or start with a question
```

A chat is a `claude` session you hold, started in the run's working directory under its launcher.
It answers from the record: `otto show`, the handoff, the journal, the questions it asked and your
answers, what it sent and made, its `artifacts/` — and each wake's own session transcript, which is
the one place that says *why* a wake did something rather than what it wrote down. When the question
is about now, it looks at the world, the way a wake would.

**A chat is not a wake, and it changes nothing.** It has no wake number, isn't checked when it ends,
counts toward no budget and is no tick for the [stall](#statuses) count, so it is fine beside a wake
that is working. It can't do the run's work either, because a change made there is one no wake would
know about — no lock, no handoff, no approval on record. Edit and Write are refused, every other tool
but otto's own reads (`otto show`, `otto logs`…) asks you first, and `otto state` refuses its writes.

**What you decide leaves as a note.** When the conversation lands on something the run should do
differently, the chat drafts a [note](#notes) for a wake that wasn't there, and leaves it with `otto
note` once you say so — `--now` to act on it now. A note is the only way a chat reaches the run. If a
gate is open you can answer it from the chat too, and the answer is recorded as yours, verbatim.

From a script — stdin or stdout not a terminal — `otto chat 7 "what's left?"` answers once and ends.
With nobody there to approve a command, it reads files rather than running them, and leaves a note
only when the question asks for one.
The journal has `chat-started` and `chat-ended`, with what it used and the notes left while it was
open; what was said stays in its transcript. A chat's tokens count toward the run's total in `otto
usage` and `otto show`, beside its wakes. The web page has the command to copy, for a terminal.

## Messages and the inbox

A gate is the run asking; a **message** is the run delivering. When the work produces something
for you — the report you asked for, a reply drafted for someone else, news you'd want — a wake
sends it to your inbox:

```bash
otto state message <id> --kind report --title "Why refund-policy-qa fails" --body-file artifacts/report.md
```

`report` is something it made for you to read; `draft` is text for you to send on to someone else
(the run never sends it itself); `update` is news. Each lands in your inbox, beside the questions
waiting on you, and poke notifies you of it. It stays until you mark it done:

```bash
otto inbox                              # everything waiting for you, across every run
otto inbox read <id> 2                  # one message in full, with any draft and what it attached
otto inbox done <id> 2 --reply "dig into the grader next"
```

A reply becomes a note to the run, delivered like any other. The message's body is
`messages/NNN-<slug>.md`, verbatim, and the rest of it — kind, title, draft, attachments, when you
marked it done and what you replied — `messages/NNN-<slug>.json`. The web UI has the same inbox,
with a count in the top bar.

## Outputs: what it made, kept

A message is news; it leaves your inbox when you're done with it. The work itself — the day's
report, a design as it is revised, an explanation and the reply drafted for it, the tickets it filed
— is an **output**, kept on the run's page under "Its work", every version:

```bash
otto state publish <id> --key 2026-10-16-refund-policy-qa --title "refund-policy-qa: a stale index, not the model" --file artifacts/report.md
otto state publish <id> --key design --title "Design: per-key rate-limit tiers" --file artifacts/design.md --summary-file artifacts/design-summary.md --stage Design
otto state publish <id> --key thread-1729072990 --title "Priya asked why refunds say 14 days" --file artifacts/explain.md --about https://example.slack.com/archives/C1/p1729072990 --draft-file artifacts/reply.md
otto state publish <id> --key proj-4412 --title "Add rate_tier to key_metadata" --link https://example.atlassian.net/browse/PROJ-4412
```

There are no agent types to choose; the page reads the shape of the work from the outputs:

- **The key says what it is.** A new key is a new output — a report a day is a dated list. The same
  key again is its next version — a design revised after review shows v1 and v2 and what changed
  between them. Publishing what the latest version already says makes no new one.
- **A summary** comes with anything over a page (500 words). The page opens on it, with a toggle to
  the whole document; `otto outputs <id> <key>` prints it, and `--full` the whole. A run writes one
  without being asked, a new one for each version.
- **Stages** (`otto state stages <id> Research Design Plan`) put outputs on a track, for work that
  moves through steps.
- **A draft and a state** make an output something to deal with: a card with the reply to copy,
  waiting until you mark it sent (or the run marks it not needed — someone else answered).
- **A question `--about` an output** (`otto state open-gate … --about design`) is shown beside that
  version, and your answer is kept on it as your review.
- **A message `--output` an output** shows it in your inbox as a card that opens it.

```bash
otto outputs <id>                  # what it has made, newest first, and where its stages stand
otto outputs <id> design --version 1   # one, with its draft and your review — its summary, if it has one
otto outputs <id> design --full        # the whole of it, not its summary
otto outputs <id> <key> --mark done    # you sent its draft
otto note <id> "shorter, please" --about design   # a note the run knows is about that output
```

On disk, `outputs/index.json` holds everything but the text, `outputs/<key>/v<n>.md` each
version's text, verbatim, and `outputs/<key>/v<n>.summary.md` its summary. `artifacts/` stays the run's scratch space: an output is published from
it on purpose, so the page shows the work and not the workings.

## Changes: code handed back

When a run's work changes code, it doesn't commit to `main` or push: it commits on a branch of its
own and hands that back as a **change**. You review it on one page, and a run of its own — the
**lander** — puts it on the base branch once you approve.

```bash
otto state change submit <id> --repo ~/work/svc --branch svc-retry --title "Retry flaky reads" --summary-file artifacts/change.md --tests "unit: 12 passed"
```

```bash
otto changes                       # what waits on your review, what is in hand, what landed this week
otto changes show 3/2 --patch      # one change: what its author said, its files, its patch, what happened
otto changes approve 3/2 7/1       # land them: the lander is woken to do it
otto changes revise 3/2 --text "keep the old flag working"   # back to its author, in your words
otto changes drop 3/2              # it won't land; the lander cleans up after it
```

- **A change doesn't stop its run.** Submitting isn't a question: the run goes on with its next piece
  of work, and what happens to the change reaches it as a note — you asked for changes, it landed, it
  was dropped. A run that hands back a lot of code doesn't wait on you between pieces.
- **What you approve is what lands.** The patch is kept as it was submitted, and your approval names
  that version, its commit and its `git patch-id`. Just before it pushes, the lander checks the commit
  it is about to put on the base (`otto state change check`): the approved commit, or a clean rebase of
  the same patch onto a base that moved. If the code around it changed, it comes back to you as a new
  version; if it conflicts, it goes back to its author.
- **Every change says whose move it is**: yours, its author's, the lander's, or no one's once it has
  landed or been dropped and cleaned up after. The Changes page lists them that way — what waits on
  you first — and flags a change over a few hundred lines, or two open changes that touch the same
  file. The inbox doesn't list them; it says how many wait, with a link.
- **Only the lander lands.** It is named in `config.json` (`otto config lander <run>`), and otto refuses
  the landing commands from any other run. So only the lander needs to be able to push; the runs that
  write code can run under a launcher that can't. otto checks what it says, too: a change is landed
  only once its base branch contains it, and cleaned up only once its worktree and branch are gone.
- **What comes after landing stays with its author.** Installing it, marking a ticket, telling someone:
  the author is told it landed, and woken to do it.

The [README](../README.md#landing-changes) sets up a lander. A change's files live with the run that
wrote it: `changes/index.json`, and each version's patch and summary beside it.

## Pausing

`otto pause <id> --for 2d` (or `--until 09:00`, `--until 2026-10-19`, or neither for "until I say")
holds back everything scheduled: the period's wakes, timers, check scripts, expiring gates. A wake
already running finishes, questions stay open, and you can still answer, note or `otto wake` it by
hand. `otto resume <id>` lifts the pause; a wake it missed meanwhile comes at the next poke — one,
not a backlog.

## When things go wrong

A crash mid-wake is the normal case, and recovery needs nobody:

| What happens | What brings it back |
|---|---|
| The wake crashes, or is killed | Marked incomplete; retried after a backoff (5m, 10m, 20m… up to an hour) |
| A wake hangs | Poke kills it at its deadline (`maxWakeMinutes`, 45 by default); incomplete path. Long before that, once it has said nothing for 5 minutes, `otto ls`, `otto show` and the web page say so — "Working now, though nothing new from it for 12 minutes" — without stopping it: a long build is quiet too. `otto attach` shows what it last did |
| The machine reboots | launchd starts poke at login; any run that's due wakes |
| A wake hits your usage limit | Not counted as a failure: the run sleeps until the limit resets (claude records when, in the session transcript), and `otto ls` says `usage limit` |
| A wake never gets going — claude exits at once, having done nothing (an expired login, say) | Not counted as a failure either: it is the machine, not the run. Retried on the same backoff for as long as it takes; after three in a row you are notified, with what claude printed, and the list and the health line say so |
| Five wakes in a row don't finish | `blocked`, with a question saying what kept happening |

A retry is never held back by a check script, and nor is the wake after a usage limit — the work is
half done whatever the world outside has been doing.

## Budgets and policy

**Budgets** are outer bounds for runs that could go on for a long time: `--budget-tokens N`,
`--budget-wakes N` and `--budget-hours H`. At 80% of any, otto notifies you once; at 100%, the run
is `blocked` with a gate. Tokens are counted the way `otto usage` totals them — input, output, and
cache read and written — so `--budget-tokens 50M` is a number you can check against that report; a
wake whose transcript otto can't read (a sandbox hiding `~/.claude`) adds nothing. There is no
dollar budget — nothing can price a wake on a subscription (see [cost](#what-a-wake-costs)).

**Policy** knobs tune the engine per run (`otto run --policy key=value`):

| Key | Default | What it does |
|---|---|---|
| `periodMinutes` | 60 | The period (prefer `--period` / `otto period`) |
| `maxWakeMinutes` | 45 | A wake's deadline; poke kills it after |
| `maxIncompleteWakes` | 5 | Failed wakes in a row before `blocked` |
| `maxTicksWithoutProgress` | 24 | Wakes in a row that change nothing before otto blocks the run and asks ([stalls](#statuses)); 0 disables. A perpetual run ignores it: watching and finding nothing is its job |
| `gateStaleAfterHours` | 48 | When an unanswered gate is notified again; 0 disables |
| `handoffMaxBytes` | 8192 | The handoff's cap |
| `verifyDone` | true | A wake's `done` is a claim the next wake [checks](#checking-done); `false` trusts it as said |
| `maxDoneReopens` | 2 | How many times a check may send a claim back before the run asks you; 0 is no limit |
| `autoMergeWhenGreen` | false | A policy flag a workflow may cite as authorization to merge |

Any other key is kept in `policy` for the wrapped instructions to read.

## Outward actions and repo locks

Pushing, merging and commenting on someone's ticket can't be undone by deleting a file, so a wake
never takes them on its own judgement. It records an **authorization** — naming the answered gate
or policy flag that allows it, and bound to one commit — and checks it immediately before acting
(`otto state authorize` / `check-authorized`). A branch that moved since the approval asks again.

Two runs working in one repository would trample each other, so a wake claims a **repo lock**
(`otto state lock`) while it uses one, and releases it before it sleeps.

## The reviver and notifications

`otto poke` is the reviver. launchd runs it every ~5 minutes (`otto agent start` installs it). It
makes no decisions about the work, only about whether a wake should exist: start the runs that
are due, run their check scripts, kill wakes past their deadline, and record crashed ones.
**Without it, a sleeping run never wakes.** See [running as a
service](guides/running-as-a-service.md).

Poke also sends a macOS notification, once each, when a run opens a gate, sends a message, hands
back code for review, becomes blocked, leaves a gate unanswered past `gateStaleAfterHours`, can't
start three times in a row, or passes 80% of a budget.

## What a wake costs

otto runs `claude` without `-p`: print mode bills SDK credits instead of your subscription, the
wrong meter for something that wakes forever. For the same reason a wake never inherits an
`ANTHROPIC_API_KEY` from whatever started it, which claude would bill instead (a
[launcher](guides/sandboxing.md) can set one on purpose). So a wake's cost is measured in tokens, read back
from the session transcript claude leaves behind and journaled as `wake-spent` (turns, input and
output tokens, cache reads and creation). `otto usage` totals it across runs, by run or by day;
`otto show <id>` gives one run's total; `otto check <id>` shows the recent average per wake.

Every wake is a cold start: roughly 12k tokens of cache creation before anything useful happens,
and prompt caching does not carry across wakes. So fewer, longer wakes are cheaper than many
short ones, and a check script that answers "nothing new" for free is the biggest saving
available for a polling run.

## Feedback from wakes

Wakes are otto's real users, and they hit things no test thought of: the harness disagreeing with
the instructions a run wraps, a refusal that didn't say what to do instead. When otto itself gets in
a wake's way, it can say so — `otto state feedback <id> --body "…"` — and carry on. You read them
with `otto feedback`:

```bash
otto feedback                 # what wakes have said about otto, newest first
otto feedback --done 3 4      # read, and fixed or set aside
otto feedback --all --json    # every report, for reading in bulk
```

Each report is stamped by otto with the run, the wake, what the run wraps and which harness it was
given (one from before the current harness says so). They're about otto, not the run's work, so they
stay out of the inbox and nothing notifies you; the run's journal only notes that one was sent
(`feedback-sent`). A wake sends two at most, in its own words, and the command never fails.
[The README](../README.md#otto-improving-itself) sets up a run that works through them.

## Where everything lives

```
~/.otto/                         ($OTTO_HOME overrides)
  config.json                    the default workdir, launchers and lander — see reference/config.md
  runs/<id>/
    run.json                     machine truth — see reference/run-json.md
    handoff.md                   what the next wake gets for free
    journal.jsonl                every event, append-only — see reference/journal.md
    gates/NNN-<slug>.md          questions, and their answers verbatim
    notes/NNN.md                 notes, verbatim
    messages/NNN-<slug>.md|.json messages it sent you: the body verbatim, and the rest
    outputs/index.json           what it made, kept: keys, titles, versions, stages, states
    outputs/<key>/v<n>.md        each version's text, verbatim
    outputs/<key>/v<n>.summary.md its summary, for one over a page
    changes/index.json           code it handed back: branches, versions, decisions, what happened
    changes/<n>-<key>/v<k>.patch each version's patch, and v<k>.summary.md what its author said
    artifacts/                   whatever the work produces: todo.md, plans, ledgers, check.sh
  runs/.locks/                   repo locks
  archive/<id>/                  a deleted run's directory, kept until `otto cleanup`
  feedback.jsonl                 what wakes have said about otto itself — `otto feedback`
  logs/poke.log, serve.log       the launchd jobs' output
```

Never edit `run.json` by hand: every write goes through `otto state` (atomic, journaled, locked),
and the person-facing commands are built on the same calls.
