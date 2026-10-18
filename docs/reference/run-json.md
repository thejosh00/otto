# `run.json`

`~/.otto/runs/<id>/run.json` is the machine truth about one run. **Never edit it by hand**: every
write goes through `otto state` (atomic, journaled, under the run's lock), and a hand edit can
race a wake. Read it with `otto state get <id>`, or one field with `--field facts.branch`.

This page describes each field for someone reading the file; [concepts](../concepts.md) explains
what the ideas mean.

## Identity and purpose

| Field | Meaning |
|---|---|
| `schemaVersion` | The file's format version |
| `id` | The run's number (`7`), unless `--id` was given; a run from before numbering keeps its `<date>-<slug>` id until `otto renumber` |
| `name` | What a person calls it: unique, set by `--name` or made up, changed by `otto rename`. Absent on a run from before names |
| `formerId` | The `<date>-<slug>` id it had before `otto renumber`; still resolves. Absent otherwise |
| `wraps` | `{kind, ref}`: `skill`, `instructions` or `goal_only`, and the skill name or file path |
| `goal` | Verbatim from `--goal`. No wake ever rewrites it |
| `doneCondition` | What makes the run finished; `null` until stated or agreed at the first gate |
| `phase` | A free-form label a wake sets, for people and the wrapped instructions. otto doesn't validate it |
| `facts` | The wake's scratch space: whatever the work re-derives and records (`branch`, `prNumber`…). `target` is set from `--target` |

## Where it stands

| Field | Meaning |
|---|---|
| `status` | `running`, `sleeping`, `awaiting_human`, `blocked`, `done`, `failed` or `stopped` |
| `blocked` | When `status` is `blocked`: `{cause, detail, at}`, cause one of `wake-failures`, `budget`, `stall`, `instructions`, `unverified` |
| `doneClaim` | A wake said the run is done and no other wake has checked yet: `{byWake, at, reason, leftOpen}`. The next wake is the check — see [checking done](../concepts.md#checking-done). Gone once the run is over |
| `lastVerdict` | What the last check of a done claim found: `{confirmed, reason, wake, at}` — on a done run, what was checked; on a reopened one, what was missing |
| `doneReopens` | Times a check has sent a claim back since the run last asked the person; past `maxDoneReopens` it blocks |
| `gate` | The open gate: `{id, slug, file, askedAt, answeredAt, expiresAt, about}`, or `null`. `about` is the output version it asks about (`design@1`), when it asks about one |
| `nextWakeAt` | When poke next wakes the run, or `null` |
| `armedWakeAt` | The time a wake last asked for with `arm-timer --in`/`--at`. While it equals `nextWakeAt`, the sleep is the wake's own: a person's check doesn't stand in front of it and `otto period` doesn't move it |
| `limitedUntil` | The wake time otto set when a wake ran into the account's usage limit: a minute past the reset. While it equals `nextWakeAt` the run is waiting out the limit, which no check script stands in front of |
| `wake` | The current or last wake: `{n, startedAt, deadlineAt, launcher, pid, session, outcome}`. `session` is the claude session id its usage is read from |
| `incompleteWakes` | Wakes in a row that didn't finish; reset by one that does |
| `startFailures`, `startFailure` | Wakes in a row that never got going — claude exited at once, having done nothing — and the first line it printed the last time. Retried for as long as it takes, never counted toward blocking the run; reset by a wake that gets going |
| `wait` | What a wake said the run is waiting on, with a limit (`arm-timer --waiting-for`): `{for, since, giveUpAt}`. Re-arming the same wait keeps both times; past `giveUpAt`, otto refuses another sleep for it |
| `paused` | While a person has paused it (`otto pause`): `{at, until, reason}`, `until` absent for "until resumed". Poke starts no scheduled wake meanwhile |
| `ticksWithoutProgress` | Wakes in a row that changed nothing otto could see and didn't say `tick --progress`; at `policy.maxTicksWithoutProgress`, otto blocks the run (`stall`) and asks |
| `progressMark` | otto's mark of the run's work after the last wake that finished: phase, facts, gates, outputs, messages, `artifacts/`, and the `--repo`s' branches and worktrees. A wake that leaves it as it was changed nothing |
| `saidProgress` | The wake that last said `tick --progress`: it moved the run somewhere otto can't see |

## How it runs

| Field | Meaning |
|---|---|
| `launcher` | `{kind, detach, repos, workdir}`: the launcher's name, `tmux` or `none`, the `--repo` directories, and the absolute directory every wake starts in (absent on runs from before working directories, which use the configured default) |
| `permission` | `{mode, allowedTools, disallowedTools}`, passed to claude on every wake |
| `budget` | `{wakes, hours, tokens, spentWakes, spentTokens}` — 0 means unlimited. Tokens are counted as `otto usage` totals them: input, output, cache read and written. (`usd`/`spentUsd` are vestigial and always 0) |
| `policy` | `periodMinutes`, `maxWakeMinutes`, `maxIncompleteWakes`, `maxTicksWithoutProgress`, `gateStaleAfterHours`, `handoffMaxBytes`, `autoMergeWhenGreen`, `perpetual`, `verifyDone`, `maxDoneReopens`, plus any other `--policy` key. See [budgets and policy](../concepts.md#budgets-and-policy) |
| `check` | The [check script](../guides/check-scripts.md), if any — see below |
| `notes` | Notes still to be delivered, and standing ones: `{id, file, standing, addedAt, givenToWake}` |
| `messages` | Messages the run has sent that the person has not marked done: `{id, kind, title, file, sentAt}`, `file` the message's `messages/NNN-<slug>.json` |

### `check`

| Field | Meaning |
|---|---|
| `script` | Path relative to the run directory |
| `pinned` | `true` for a standing check, which belongs to the run and stands in front of every period wake; `false` for one a wake armed for one sleep, which the next wake ends |
| `setByWake` | `true` when a wake set it (`set-check`, or `arm-timer --check-script`) rather than a person (`otto check`). A person's check can't be replaced or removed by a wake |
| `retrySeconds` | How long to sleep again after "nothing new"; absent means the period |
| `wakeAfter` | The safety net: wake anyway after this many "nothing new" in a row; 0 is off |
| `consecutiveNoChange` | "Nothing new" results since the last real wake |
| `noChangeTotal` | Every "nothing new" result since the check was set — each a wake not spent |
| `lastResult`, `lastAt`, `lastNote` | The last result (`no-change`, `changed`, `error`), when, and the start of what the script printed |

## Engine bookkeeping

Kept at the top level rather than in `facts`, because a wake re-recording its facts would
otherwise overwrite them.

| Field | Meaning |
|---|---|
| `spawnAttempts`, `lastSpawnedAt` | Poke's attempts to start a wake that hasn't started yet — drives its backoff |
| `budgetWarnedAt` | When the 80% budget warning was sent, so it's sent once |
| `notified` | Which notifications have been sent, keyed by what they were about (`gate:003`) |
| `authorizations` | Recorded outward-action authorizations, each bound to a commit |
| `createdAt`, `updatedAt` | When the run was created and last written |
