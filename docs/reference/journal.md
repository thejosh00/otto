# The journal

`~/.otto/runs/<id>/journal.jsonl` is the run's history: one JSON object per line, append-only,
each with an `event` name and a `ts` timestamp. It is the only honest account of a run a week
later. Read it with `otto logs <id>` (formatted, with `--event`, `--since`, `--decisions` and
`-f`), or `otto state tail <id>` for the raw lines.

The events below are otto's own. Wrapped instructions can also journal anything they like with
`otto state log <id> --event <name>` — `error` for each real failure, by convention — so a journal
may contain names not listed here.

`otto logs --decisions` keeps the turning points: `run-created`, `phase-changed`,
`status-changed`, the gate events, the note events, `check-set`, `check-cleared`, `period-set`,
the budget events, and the wake failures.

## The run

| Event | Fields | When |
|---|---|---|
| `run-created` | `wraps`, `ref`, `goal`, `doneCondition`, `perpetual`, `phase`, `target`, `launcher` | `otto run` created it |
| `phase-changed` | `from`, `to`, `status`, `note` | A wake entered a new phase |
| `status-changed` | `from`, `to`, `reason`, `because` | The status changed; `because` is the cause when it became `blocked` |
| `facts-recorded` | `facts` | A wake recorded facts it re-derived |

## Wakes

| Event | Fields | When |
|---|---|---|
| `wake-started` | `wake`, `deadlineAt`, `launcher` | A wake began |
| `wake-spent` | `turns`, `spentWakes`, `inputTokens`, `outputTokens`, `cacheRead`, `cacheCreation`, `toolErrors` | What the wake used, read from its session transcript |
| `usage-unavailable` | `note` | The transcript couldn't be read (common under a sandbox); the wake still counts |
| `wake-reported-error` | `exitCode`, `timedOut`, `turns` | The wake's process exited non-zero or timed out |
| `wake-complete` | `status` | The wake passed the [contract](../concepts.md#the-contract) |
| `wake-incomplete` | `reason`, `consecutive` | It didn't; it will be retried after a backoff |
| `wake-killed` | `reason` | Poke killed a wake past its deadline |
| `spawn-abandoned` | `reason` | Poke gave up trying to start a wake that never started (said once) |

## Waiting

| Event | Fields | When |
|---|---|---|
| `timer-armed` | `nextWakeAt`, `note` | A sleep was scheduled — by a wake, the period (`note: period`), or a retry backoff |
| `tick` / `noop-tick` | `ticksWithoutProgress`, `note` | A polling wake found progress / found nothing |
| `period-set` | `periodMinutes`, `previousMinutes` | A person changed the period |
| `check-set` | `script`, `wakeAfter` | A person set a check script |
| `check-cleared` | `script` | A person removed it |
| `check-ran` | `result`, `consecutiveNoChange`, `note` | A check script reported a change or an error. "Nothing new" is never journaled — see `check` in `run.json` |

## People

| Event | Fields | When |
|---|---|---|
| `gate-opened` | `gate`, `slug`, `phase`, `expiresAt` | A wake asked a question |
| `gate-closed` | `gate`, `slug`, `askedAt`, `answeredAt`, `answer` | It was answered; `answer` is verbatim |
| `gate-expired` | same | Its expiry passed with no answer |
| `note-added` | `note`, `file`, `standing` | A person left a note |
| `notes-delivered` | `notes`, `wake` | A completed wake carried these one-off notes |
| `note-dropped` | `note`, `standing` | A person withdrew a note |
| `notified` | `key`, `title`, `message` | Poke sent a desktop notification |

## Budgets, locks and authorizations

| Event | Fields | When |
|---|---|---|
| `budget-warning` | `fractionSpent`, `budget` | 80% of a budget is spent (once) |
| `budget-exhausted` | `reason` | A budget ran out; the run is `blocked` |
| `lock-acquired` / `lock-released` | `repo` | A wake claimed or released a repo |
| `lock-broken` | `repo`, `previousOwner`, `reason` | A dead run's lock was broken |
| `lock-lost` | `repo`, `takenBy`, `reason` | This run's lock was broken by another |
| `authorized` | `action`, `item`, `head`, `by`, `source` | An outward action was authorized for one commit |
