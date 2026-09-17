# upgrade-flow — one dependency, many repos

**Version** 1 · **First phase** `survey`

Bump one dependency across a set of repositories: find who uses it, upgrade and test each, review
the batch as a batch, then push and open PRs for the ones you approve. The first workflow that
**fans out** — one run, N targets, partial success expected — and the shape that showed where the
engine's assumptions were single-target.

## Three constraints this shape runs into

They are not bugs to work around; they are the engine being small on purpose, and every fan-out
workflow has to answer them the same way.

1. **A run may have exactly one gate open at a time.** So there is no per-repo review queue: ask
   about the batch in **one** question, and if a single repo needs a judgement call, ask about that
   repo *alone*, serially, while the rest wait. Thirty simultaneous questions would not be a review
   anyway.
2. **`facts` is a small flat dict, and must stay one.** Per-target state does not go there — it
   goes in `artifacts/targets.md`, one row per repo, rewritten as each finishes. `facts` holds only
   the scalars: which dependency, which version, how many done. A run.json that grows with the work
   stops being cheap to read on every wake, which is what rehydration depends on.
3. **Authorizations are per action *and per item*.** `authorize --action push --item <repo>`:
   without `--item`, approving the second repo's head silently voids the first, and the run could
   push code nobody cleared. One key per repo, each bound to that repo's commit.

## Facts

| Fact | Set by | Meaning |
|---|---|---|
| `dependency` | `init` | What to bump, e.g. `requests` |
| `targetVersion` | `init` or `survey` | The version to move to. `survey` resolves "latest" to a number and records it, so every repo gets the *same* version even if a new one ships mid-run |
| `candidates` | `survey` | How many repos were considered |
| `targets` | `survey` | How many are actually affected, after the confirm gate |
| `done`, `failed`, `skipped` | `upgrade` | Running tallies. The detail is in the ledger |
| `published` | `publish` | How many pushed so far |

**Policy**: `askBeforePush: true`, `autoMergeWhenGreen: false`, `maxAttemptsPerTarget: 1` (a
dependency bump that fails its tests is a report, not a puzzle to solve), `stopOnFirstFailure:
false`.

## The ledger

`artifacts/targets.md` is the run's memory and its product. One row per repo, and the wake
reads it at the top of every wake — never a remembered list:

```
| repo | current | status | head | notes |
|---|---|---|---|---|
| acme-web  | 2.28.1 | green     | a1b2c3d | tests pass |
| acme-api  | 2.28.1 | red       | —       | 3 failures in test_client.py — see cycles/acme-api/ |
| acme-cli  | 2.31.0 | skipped   | —       | already at or above target |
| acme-jobs | 2.28.1 | published | e4f5a6b | PR #91 |
```

`status` is the state machine, on disk: `pending → green|red|skipped → published|declined`. A repo
in a terminal state is never touched again, which is what makes the phase re-enterable after a
crash halfway down a list of twenty.

## Phase 0 — `survey`

| | |
|---|---|
| **goal** | Know exactly which repos are affected, and agree the list before touching anything |
| **preconditions** | `facts.dependency` set; a repo root to search (default: every git repo one level under the workspace) |
| **actions** | Resolve `targetVersion` to a concrete number. Delegate the search to **one** subagent: for each repo, the manifest that pins this dependency and the pinned version. Write the ledger with `pending`/`skipped` |
| **durableOutput** | `artifacts/targets.md`, `facts.candidates`, `facts.targetVersion` |
| **exitCondition** | Every candidate repo has a row with a `current` version or a `skipped` reason |
| **gate** | **ask-human** — confirm the list |
| **idempotency** | If the ledger exists, adopt it; re-surveying would find the same repos and lose any statuses already recorded |

**Read-only, and cheap.** One subagent greps manifests; it does not clone, lock, or build anything.
A survey that takes twenty minutes for twenty repos is a survey doing too much.

Gate question — slug `target-list`:

> `<dependency>` → `<targetVersion>`. Found N repos pinning it, M already at or above it. The list
> is in `artifacts/targets.md`. Nothing has been touched.
>
> - **Upgrade all N** *(default)*.
> - **Subset** — name the repos to include, or the ones to drop.
> - **Change the version** — say which.
> - **Abandon**.

This gate is the cheap place to catch a wrong list. After it, each repo costs a worktree and a test
run.

## Phase 1 — `upgrade`

| | |
|---|---|
| **goal** | Every target upgraded and tested, with the result recorded per repo |
| **preconditions** | The `target-list` gate is closed and approved; the ledger has `pending` rows |
| **actions** | For each `pending` repo, **one at a time**: lock → worktree via `manage-pr` → edit the pin → run the tests → record `green`/`red` with the head sha → **unlock** → next |
| **durableOutput** | The ledger, updated after **each** repo; `artifacts/targets/<repo>/{diff.txt,test-output.txt}` |
| **exitCondition** | No row is `pending` |
| **gate** | `continue` → `review` |
| **idempotency** | The ledger is the position marker: a repo that is `green`, `red` or `skipped` is skipped on re-entry. A crash at repo 12 of 20 resumes at 12, not at 1 |

```
otto state lock <id> --repo <repo>          # one repo at a time
…manage-pr for the worktree; bump the pin; run the repo's own test command…
otto state record-fact <id> done=<n>        # scalars only
…rewrite artifacts/targets.md with this repo's row…
otto state unlock <id> --repo <repo>        # BEFORE the next repo, always
```

**Lock one repo at a time, and release before moving on.** Holding twenty locks while working
through twenty repos blocks every other run in the workspace for the duration. The lock exists to
protect a worktree in use, not to reserve a queue.

**A failing test suite is a result, not a task.** Record `red` with the output path and move on —
`maxAttemptsPerTarget: 1`. Chasing a fix in repo 7 while nineteen others wait is how a batch job
becomes a hostage. The `review` gate is where a human decides whether any red one is worth
pursuing, usually as its own `dev-flow` run.

**Environmental failures are not results.** A missing `.env`, a denied host, an unbuildable venv:
retry per the skill's classification, and if it stays broken record `red` with the reason *marked
environmental*, so the batch summary does not read like a code problem.

## Phase 2 — `review`

| | |
|---|---|
| **goal** | One decision over the whole batch |
| **preconditions** | No `pending` rows |
| **actions** | Summarise: how many green, red, skipped; the diff shape they share; the notable exceptions. Then ask once |
| **durableOutput** | `artifacts/batch-report.md` |
| **exitCondition** | Every green row is marked `approved` or `declined` in the ledger |
| **gate** | **ask-human** — the batch |
| **idempotency** | If the report exists and the branch heads have not moved, re-open the gate rather than re-running the phase |

Gate question — slug `batch-review`:

> `<dependency>` `<from>` → `<targetVersion>`: **G green, R red, S skipped** of N.
> `artifacts/batch-report.md` has the per-repo detail; the red ones are `<list>`.
> Nothing has been pushed.
>
> - **Publish all green** *(default)* — a branch and PR per repo, staged one at a time.
> - **Publish some** — name them.
> - **Stop here** — branches stay local; `artifacts/batch-report.md` says how to push each.
> - **Abandon** — worktrees left for you to inspect.

**Say what is red, in the question.** A reviewer approving "all green" needs to know what they are
implicitly deciding not to fix.

## Phase 3 — `publish`

| | |
|---|---|
| **goal** | The approved repos pushed, each with its own PR and its own authorization |
| **preconditions** | The `batch-review` gate is closed; approved rows have heads recorded |
| **actions** | Per approved repo, **serially**: `authorize --item <repo> --head <that repo's sha>` → `check-authorized --item <repo>` → push → PR → record `published` with the PR number |
| **durableOutput** | Ledger rows moved to `published`; `facts.published` |
| **exitCondition** | No approved row is un-`published`, or the un-published ones carry a recorded reason |
| **gate** | `continue` → `report`. Each *repo* is gated by the batch answer, which is what authorizes it |
| **idempotency** | `published` rows are skipped. If the remote already has this sha, adopt it; if a PR exists for the branch, adopt that PR — never a second |

```
for each approved repo:
  otto state authorize <id> --action push --item <repo> --gate <NNN> --head <sha> --quote "<verbatim>"
  otto state check-authorized <id> --action push --item <repo> --head <sha> || skip this repo
  git -C <that worktree> push -u origin <branch>
  gh pr create …            # or record prNumber: null with the reason (see dev-flow)
```

**One batch answer authorizes many pushes, and each still names its own commit.** That is the point
of `--item`: the journal ends up with one `authorized` line per repo, each with the sha that was
actually pushed. A reviewer's "publish all green" is not a blank cheque for whatever those branches
later become.

**Stage them.** Push in sequence and stop on the first *unexpected* failure (a rejected push, a
protected branch) rather than plowing through twenty. Record what got out and what did not; a
half-published batch is a normal state and the ledger describes it exactly.

## Phase 4 — `report`

| | |
|---|---|
| **goal** | Leave a batch anyone can act on, and let go of everything |
| **preconditions** | `publish` is finished or was declined |
| **actions** | Write the summary; release **every** lock (`unlock <id>` with no `--repo`); reap the worktrees `manage-pr` would reap and list the rest; `set-status done` |
| **durableOutput** | `artifacts/summary.md` — per repo: version moved, test result, PR link or why not |
| **exitCondition** | `status: done`, no locks held |
| **gate** | — |
| **idempotency** | Every step is a no-op when already done |

**`done` does not mean "everything worked."** A run that upgraded 5, failed 2, and skipped 3 is a
complete run — the ledger says what happened, and the status only says the run finished. There is no
`partial` status and there should not be: it would mean re-deriving from a summary field what the
ledger already states precisely.

**Babysitting N PRs is not this workflow's job.** Reviews arrive per repo, on their own schedules,
and one hourly tick cannot sensibly chase twenty. Hand each PR that needs following to its own
`dev-flow` run at `babysit`, or leave them to their reviewers.

## What a green run looks like

`journal.jsonl`: `run-created`, `phase-changed`(→survey), facts(candidates, targetVersion),
`gate-opened`/`gate-closed`(target-list), `phase-changed`(→upgrade), then per repo a
`lock-acquired` … `facts-recorded` … `lock-released` triple, `phase-changed`(→review),
`gate-opened`/`gate-closed`(batch-review), `phase-changed`(→publish), then **one `authorized` line
per published repo**, `phase-changed`(→report), `status-changed`(→done).

Two human gates for the whole batch. As many `authorized` lines as repos pushed, each naming a
commit. The ledger is the thing to read afterwards.
