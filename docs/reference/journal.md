# The journal

`~/.otto/runs/<id>/journal.jsonl` is the run's history: one JSON object per line, append-only,
each with an `event` name and a `ts` timestamp. It is the only honest account of a run a week
later. Read it with `otto logs <id>` (formatted, with `--event`, `--since`, `--decisions` and
`-f`), or `otto state tail <id>` for the raw lines.

The events below are otto's own. Wrapped instructions can also journal anything they like with
`otto state log <id> --event <name>` — `error` for each real failure, by convention, and one line
per cycle for a run that repeats — so a journal may contain names not listed here. Those lines are
the run's [activity](../concepts.md#activity).

`otto logs --decisions` keeps the turning points: `run-created`, `phase-changed`,
`status-changed`, the done-claim events, the gate events, the note events, `check-set`, `check-cleared`, `period-set`,
the budget events, the wake failures, and `usage-limited`.

## The run

| Event | Fields | When |
|---|---|---|
| `run-created` | `wraps`, `ref`, `goal`, `doneCondition`, `perpetual`, `phase`, `target`, `launcher` | `otto run` created it |
| `phase-changed` | `from`, `to`, `status`, `note` | A wake entered a new phase |
| `status-changed` | `from`, `to`, `reason`, `because`, `leftOpen` | The status changed; `because` is the cause when it became `blocked`, and `leftOpen` the [todo](../concepts.md#the-todo) items still open when it became `done` |
| `done-claimed` | `wake`, `reason`, `leftOpen` | A wake said the run is done; the next wake [checks it](../concepts.md#checking-done) before it is. `reason` is what the wake said, `leftOpen` its todo items still open |
| `done-verified` | `reason`, `claimedBy` | The check found the claim holds: `reason` is what it checked, `claimedBy` the wake that claimed. A `status-changed` to `done` follows |
| `done-reopened` | `reason`, `reopens` | The check found the claim doesn't hold, and the run went back to work: `reason` is what was missing, `reopens` how many times in a row |
| `facts-recorded` | `facts` | A wake recorded facts it re-derived |
| `renamed` | `from`, `to` | A person renamed the run (`otto rename`) |
| `renumbered` | `from` | `otto renumber` gave a run from before numbering its number; `from` is its old id |

## Wakes

| Event | Fields | When |
|---|---|---|
| `wake-started` | `wake`, `deadlineAt`, `launcher` | A wake began |
| `wake-spent` | `turns`, `spentWakes`, `inputTokens`, `outputTokens`, `cacheRead`, `cacheCreation`, `toolErrors` | What the wake used, read from its session transcript |
| `usage-unavailable` | `note` | The transcript couldn't be read (common under a sandbox); the wake still counts |
| `wake-reported-error` | `exitCode`, `timedOut`, `turns` | The wake's process exited non-zero or timed out |
| `handoff-refused` | `reason` | `otto state handoff` refused a handoff — past its cap, or with otto's own words in `Where this is` or `Needs` — and the wake has to write it again |
| `wake-complete` | `status`, `progress` | The wake passed the [contract](../concepts.md#the-contract). `progress`: it changed something otto can see, or said `tick --progress` |
| `wake-incomplete` | `reason`, `consecutive` | It didn't; it will be retried after a backoff |
| `usage-limited` | `limit`, `resetsAt` | It ran into the account's usage limit (`limit` is claude's name for which, e.g. `five_hour`). Not counted as incomplete: the run sleeps until `resetsAt` |
| `wake-killed` | `reason` | Poke killed a wake past its deadline |
| `spawn-abandoned` | `reason` | Poke gave up trying to start a wake that never started (said once) |
| `start-failed` | `reason`, `consecutive` | A wake never got going — claude exited at once, having done nothing; `reason` is what it printed. Retried on a backoff for as long as it takes, never counted toward blocking the run |

## Waiting

| Event | Fields | When |
|---|---|---|
| `timer-armed` | `nextWakeAt`, `note` | A sleep was scheduled — by a wake, the period (`note: period`), or a retry backoff |
| `tick` / `noop-tick` | `ticksWithoutProgress`, `note` | A wake said it moved the run where otto can't see (`--progress`) / said what a look found. otto counts wakes that change nothing itself, so a plain tick adds nothing to the count |
| `period-set` | `periodMinutes`, `previousMinutes` | A person changed the period |
| `check-set` | `script`, `wakeAfter`, `by` | A standing check script was set, by a `person` (`otto check`) or a `wake` (`set-check`) |
| `check-cleared` | `script`, `by` | The standing check was removed, by a `person` or a `wake` |
| `check-ran` | `result`, `consecutiveNoChange`, `note` | A check script reported a change or an error. "Nothing new" is never journaled — see `check` in `run.json` |

## People

| Event | Fields | When |
|---|---|---|
| `gate-opened` | `gate`, `slug`, `phase`, `expiresAt`, `about` | A wake asked a question; `about` is the output version it asks about (`design@1`), whose review the answer becomes |
| `gate-closed` | `gate`, `slug`, `askedAt`, `answeredAt`, `answer` | It was answered; `answer` is verbatim |
| `gate-expired` | same | Its expiry passed with no answer |
| `note-added` | `note`, `file`, `standing` | A person left a note |
| `notes-delivered` | `notes`, `wake` | A completed wake carried these one-off notes |
| `feedback-sent` | `report`, `wake` | The wake told otto's maker what got in its way; the report is number `report` in `$OTTO_HOME/feedback.jsonl` (`otto feedback`) |
| `notes-given-late` | `notes`, `wake`, `before` | The wake was about to ask (`gate`), claim done (`done`) or confirm a claim (`confirm`) without these notes, added while it ran: refused once, and given them |
| `note-dropped` | `note`, `standing` | A person withdrew a note |
| `notified` | `key`, `title`, `message` | Poke sent a desktop notification |
| `message-sent` | `message`, `kind`, `title`, `file` | A wake sent the person a message (`otto state message`): a `report`, a `draft` or an `update` |
| `message-done` | `message`, `reply`, `note` | The person marked it done (`otto inbox done`); a `reply` became `note` |
| `output-published` | `output`, `version`, `title`, `stage`, `file`, `link` | A wake published a version of an output (`otto state publish`) — the first when `version` is 1. Its text is at `file`, or it lives at `link` |
| `output-state` | `output`, `state`, `by`, `note` | An output was marked `waiting`, `done` or `dropped` — by the `person` ("I sent it", `otto outputs --mark`) or a `wake` (`otto state output-state`) |
| `stages-set` | `stages` | A wake declared the steps its work moves through (`otto state stages`) |
| `paused` | `until`, `reason` | A person paused the run (`otto pause`) |
| `unpaused` | `by` | The pause ended: by a `person` (`otto resume`) or `time` |

## Budgets, locks and authorizations

| Event | Fields | When |
|---|---|---|
| `budget-warning` | `fractionSpent`, `budget` | 80% of a budget is spent (once) |
| `budget-exhausted` | `reason` | A budget ran out; the run is `blocked` |
| `lock-acquired` / `lock-released` | `repo` | A wake claimed or released a repo |
| `lock-broken` | `repo`, `previousOwner`, `reason` | A dead run's lock was broken |
| `lock-lost` | `repo`, `takenBy`, `reason` | This run's lock was broken by another |
| `authorized` | `action`, `item`, `head`, `by`, `source` | An outward action was authorized for one commit |
