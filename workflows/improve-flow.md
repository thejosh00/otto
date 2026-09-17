# improve-flow — one improvement a day, forever

**Version** 1 · **First phase** `propose`

Wake once a day, propose **one** improvement to a repository, and ask. Approved, it gets
implemented like a `dev-flow` ticket. Turned down, the idea goes in the ledger so tomorrow's
proposal is a different one. The run does not finish; you retire it.

This is the first workflow with **no terminal phase**, which changes three habits:

- **`done` never arrives.** Retiring it is `status: stopped` — the run was healthy, it just
  isn't wanted any more. `failed` would be a lie in the audit trail.
- **A rejected idea is progress.** Something durable happened: the ledger grew, and tomorrow's
  proposal will be better informed. So ticks call `--progress`, and `maxTicksWithoutProgress` is
  **0** (disabled). Otherwise 24 polite rejections would mark the run `blocked`.
- **The ledger is the memory.** `artifacts/ledger.md` — one line per proposal and verdict — is
  read *before* proposing, every cycle. Skip it and tomorrow you re-propose today's rejected
  idea, which is the single most annoying way this workflow can fail.

## Facts

| Fact | Set by | Meaning |
|---|---|---|
| `mainRepo` | `init` | The repo to improve, absolute path. Required — this workflow does not guess |
| `cadenceSeconds` | `init` | Between proposals. Default `86400` |
| `cycle` | `propose` | How many proposals have been made. Also names the artifact directory |
| `accepted` / `rejected` | `propose` | Running tallies, for `handoff.md` |
| `stopAfterCycles` | `init` | `0` (default) runs until you retire it; `30` stops itself after 30 |
| `worktree`, `branch`, `base` | `implement` | Only while a cycle is implementing; cleared after |

**Policy**: `maxTicksWithoutProgress: 0`, `askBeforePush: true`, `autoMergeWhenGreen: false`,
`proposalTtlSeconds: 79200` (22h — an unanswered proposal expires *before* the next wake, so the
two never collide).

## The cycle

```
propose ──approved──> implement ──> wait ──(tomorrow)──> propose
   └─────rejected / unanswered─────> wait ──────────────────┘
```

Three phases, no exit. `wait` is where it lives most of its life.

## Phase `propose`

| | |
|---|---|
| **goal** | One concrete, worthwhile improvement, and a decision on it |
| **preconditions** | `facts.mainRepo` is a git repo; `artifacts/ledger.md` exists (create it empty on the first cycle) |
| **actions** | Read the ledger. Delegate a survey of the repo to a subagent. Pick **one** improvement that is not in the ledger. Write it up. Open an expiring human gate |
| **durableOutput** | `artifacts/cycles/<date>-<slug>/proposal.md`, a `proposed` line in the ledger, `facts.cycle` incremented |
| **exitCondition** | The gate is closed — answered or expired — and the ledger carries the verdict |
| **gate** | **ask-human, expiring** (see below) |
| **idempotency** | If a gate is open, re-ask it verbatim; never propose twice in a cycle. If this cycle's `proposal.md` exists but no gate is open and the ledger has no verdict, re-open the gate for the existing proposal |

```
otto state record-fact <id> cycle=<n+1>
… write artifacts/cycles/<date>-<slug>/proposal.md, append "proposed" to the ledger …
otto state tick <id> --progress --note "cycle <n+1> proposed"
otto state open-gate <id> --slug proposal-<n+1> \
    --expires-in <policy.proposalTtlSeconds> --question-file <that proposal>
→ then stop
```

**One proposal at a time.** Never open a second while one is pending: thirty stacked gates is
not a review queue, it is an inbox you will declare bankruptcy on. That is what the expiry is
for.

**What makes a proposal worth reading.** One sentence of what to change and where; why it is
worth doing *now* (a bug it prevents, a cost it removes, a confusion it ends); the size in files
and hours; and what could go wrong. Rank candidates by value-per-risk and propose the best one —
not the easiest one, and not a grab-bag. Say explicitly what you considered and passed over, in
one line each: that is what stops tomorrow repeating today.

Gate question — the run is asking for a *decision*, so give it the shape of one:

> `<repo>` cycle N: **<one-line proposal>**. Details in
> `artifacts/cycles/<date>-<slug>/proposal.md` (M files, ~H hours). Also considered: X, Y.
>
> - **Approve** *(default)* — implement it now, ending at ready-to-push.
> - **Not this** — the idea goes in the ledger as rejected; say why if you like, and tomorrow's
>   proposal avoids that direction.
> - **Later** — worth doing, not now: recorded as deferred and eligible again in 30 days.
> - **Stop the run** — retire it (`status: stopped`).
>
> No answer by `<expiry>` counts as **Not this**, with "no answer" as the reason.

## Phase `implement`

| | |
|---|---|
| **goal** | The approved improvement, committed, tests green, ready to push |
| **preconditions** | The gate is closed with an approval; `facts.mainRepo` is unlocked or locked by us |
| **actions** | Exactly `dev-flow`'s `prepare` → `implement` → `verify`, minus the plan gate (the proposal *was* the plan, and it was approved): lock the repo, branch `improve-<date>-<slug>`, worktree via **`manage-pr`**, subagents write the code, `manage-pr` again for lint+tests, then the report |
| **durableOutput** | Commits on the branch; `artifacts/cycles/<date>-<slug>/{implementation-notes,test-report,ready-to-push}.md`; an `implemented` line in the ledger |
| **exitCondition** | Commits exist, the tree is clean, checks are green, and `ready-to-push.md` names the branch |
| **gate** | `continue` → `wait`. Nothing is pushed — same line `dev-flow` does not cross |
| **idempotency** | Re-derive progress from `git log`, not memory. A cycle whose `ready-to-push.md` exists is finished: go to `wait` |

**Hold the lock for a cycle, never for the run.** A perpetual run that keeps
`otto state lock` while it sleeps blocks every `dev-flow` against that repo for the rest of its
life. Acquire when implementing, `otto state unlock` before `wait` — *including* on the failure
paths.

**If the repo is locked by someone else**, the improvement waits: journal it, arm a one-hour
timer, and try again — the other run is doing something more specific than this one. After three
tries, gate it and let a human choose.

Failure classification is `dev-flow`'s. Three real attempts and the cycle is *abandoned*, not the
run: record `failed to implement` in the ledger with what was tried, release the lock, and go to
`wait`. One bad idea must not end a workflow that is supposed to outlive it.

## Phase `wait`

| | |
|---|---|
| **goal** | Sleep until tomorrow, cheaply, and survive anything that happens meanwhile |
| **preconditions** | No gate open; no lock held; the current cycle has a verdict in the ledger |
| **actions** | Rewrite `handoff.md`; check `stopAfterCycles`; arm the next wake |
| **durableOutput** | `nextWakeAt`, and a `timer-armed` line |
| **exitCondition** | `status: sleeping` with `nextWakeAt` a cadence away |
| **gate** | `sleep(cadenceSeconds, until: retired)` |
| **idempotency** | Arming twice is harmless — `nextWakeAt` is overwritten, not queued. There is no backlog to replay |

```
otto state unlock <id>                       # belt and braces: never sleep holding a lock
otto state handoff <id> --stdin              # capped; the history lives in the ledger
# stopAfterCycles reached?  → set-status --status stopped --reason "…N cycles"; and stop here
otto state arm-timer <id> --in <cadenceSeconds>
→ stop
```

**No `open-gate` for this wait.** `arm-timer` alone sets `sleeping` with a due time, which is
everything the reviver and `/otto status` need. A timer *gate* would have to be closed and
reopened every single cycle — 365 gate files a year documenting the same sentence. Keep gates for
questions that have an answer.

**The reviver is the only waker.** `arm-timer` writes `nextWakeAt` and `otto poke` starts a wake
once it has passed, which lands 15–20 minutes late (jitter grace plus poll interval). For a daily
cadence that is invisible; do not try to correct for it. (v1 chained in-session cron jobs here and
found they rarely fired, because a session seldom survived a day — v2 deleted them.)

## Retiring it

`/otto abandon <id>` — or "Stop the run" at a gate, or `stopAfterCycles`. All three land on
`status: stopped`, which is terminal: the reviver ignores it, `due` ignores it, and the ledger
stays as the record of what it did. Release the lock on the way out.

## What a healthy month looks like

`journal.jsonl`: per cycle a `gate-opened` (with `expiresAt`), then either `gate-closed` or
`gate-expired`, a `tick` with `progress`, `timer-armed`, and — on approved cycles —
`lock-acquired` … `lock-released`. Thirty cycles is roughly 200 lines. `artifacts/ledger.md` is
30 lines and is the only thing you need to read to know what the run has been doing.
