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
| `id` | The run's id, `<date>-<slug>` unless `--id` was given |
| `wraps` | `{kind, ref}`: `skill`, `instructions` or `goal_only`, and the skill name or file path |
| `goal` | Verbatim from `--goal`. No wake ever rewrites it |
| `doneCondition` | What makes the run finished; `null` until stated or agreed at the first gate |
| `phase` | A free-form label a wake sets, for people and the wrapped instructions. otto doesn't validate it |
| `facts` | The wake's scratch space: whatever the work re-derives and records (`branch`, `prNumber`…). `target` is set from `--target` |

## Where it stands

| Field | Meaning |
|---|---|
| `status` | `running`, `sleeping`, `awaiting_human`, `blocked`, `done`, `failed` or `stopped` |
| `blocked` | When `status` is `blocked`: `{cause, detail, at}`, cause one of `wake-failures`, `budget`, `stall`, `instructions` |
| `gate` | The open gate: `{id, slug, file, askedAt, answeredAt, expiresAt}`, or `null` |
| `nextWakeAt` | When poke next wakes the run, or `null` |
| `armedWakeAt` | The time a wake last asked for with `arm-timer --in`/`--at`. While it equals `nextWakeAt`, the sleep is the wake's own: a person's check doesn't stand in front of it and `otto period` doesn't move it |
| `wake` | The current or last wake: `{n, startedAt, deadlineAt, launcher, pid, session, outcome}`. `session` is the claude session id its usage is read from |
| `incompleteWakes` | Wakes in a row that didn't finish; reset by one that does |
| `ticksWithoutProgress` | Polling ticks in a row that changed nothing |

## How it runs

| Field | Meaning |
|---|---|
| `launcher` | `{kind, detach, repos}`: the launcher's name, `tmux` or `none`, and the `--repo` directories |
| `permission` | `{mode, allowedTools, disallowedTools}`, passed to claude on every wake |
| `budget` | `{wakes, hours, spentWakes}` — 0 means unlimited. (`usd`/`spentUsd` are vestigial and always 0) |
| `policy` | `periodMinutes`, `maxWakeMinutes`, `maxIncompleteWakes`, `maxTicksWithoutProgress`, `gateStaleAfterHours`, `handoffMaxBytes`, `autoMergeWhenGreen`, `perpetual`, plus any other `--policy` key. See [budgets and policy](../concepts.md#budgets-and-policy) |
| `check` | The [check script](../guides/check-scripts.md), if any — see below |
| `notes` | Notes still to be delivered, and standing ones: `{id, file, standing, addedAt, givenToWake}` |

### `check`

| Field | Meaning |
|---|---|
| `script` | Path relative to the run directory |
| `pinned` | `true` for a person's check (`otto check`), which belongs to the run; `false` for one a wake armed for one sleep |
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
