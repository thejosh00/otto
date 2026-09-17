# dev-flow — ticket to reviewed code

**Version** 1 · **First phase** `intake`

Read a ticket, prepare a worktree, plan, get the plan reviewed, implement, verify, get the
result reviewed, push, babysit the review for as long as it takes, merge, clean up.

**Phases 5–8 touch the world.** Push and merge cannot be undone by deleting a file, so they are
not taken on a wake's judgement: each one needs an authorization **recorded on disk and
bound to the exact commit** (`otto state authorize` / `check-authorized`). An approval given on
Monday does not authorize what the branch became on Wednesday. An agent that decides on day three
to merge because it looked ready is the failure everyone remembers; the machinery here exists so
that it cannot.

## Facts

| Fact | Set by | Meaning |
|---|---|---|
| `target` | `init` | Whatever you were given: a Jira key, or a plain description |
| `jiraKey` | `intake` | The ticket, when there is one. `null` for a described task |
| `ticketUpdatedAt` | `intake` | The ticket's own `updated` timestamp — how reconciliation notices it changed |
| `mainRepo` | `intake` | The repo the work belongs in, absolute path (`git rev-parse --show-toplevel`) |
| `base` | `prepare` | Base branch, from `origin/HEAD` — never assumed to be `main` |
| `branch` | `prepare` | `<key-lowercase>-<slug>`, e.g. `sre-1234-retry-backoff`. With no ticket, just the slug: `farewell-greeting` |
| `worktree` | `prepare` | Where the work happens. `manage-pr` chooses it; record what it reports |
| `testCmd` | `prepare` | The repo's test command, as `manage-pr` inferred it |
| `pushedSha` | `publish` | The commit last pushed. What `check-authorized` is compared against |
| `prNumber`, `prUrl` | `publish` | The PR, or `null` when the remote has no PR host |
| `prHost` | `publish` | The remote's host, e.g. `source.datanerd.us`. `gh` auth is per-host |
| `prCommentCursor` | `babysit` | Highest review-comment id already handled. Makes polling cheap and exactly-once |
| `ciStatus`, `reviewDecision` | `babysit` | Re-derived every tick, never remembered |

**Policy defaults** for this workflow: `askBeforePush: true`, `autoMergeWhenGreen: false`,
`maxAttempts: 3`. Both push flags are moot for phases 0–4 but are set at `intake` so the
decision is on record from the start rather than made later under pressure.

## Reconcile, per phase

Cheap and phase-scoped — never fetch a PR during `plan`. On every wake:

- `plan` onward: if `facts.jiraKey`, re-read *only* the ticket's `updated` field. If it differs
  from `ticketUpdatedAt`, the spec may be stale — re-read the ticket, rewrite
  `artifacts/spec.md`, and **open a human gate** rather than quietly acting on a changed
  request. Otherwise read `artifacts/spec.md`, not the ticket;
- `implement` onward: `git -C <worktree> status --porcelain` and
  `git -C <worktree> log --oneline origin/<base>..HEAD`. What is committed is the truth about
  progress; a memory of having written code is not.
- `publish` onward: `git -C <worktree> fetch origin` and
  `git ls-remote origin <branch>` — has our commit actually landed on the remote?
- `babysit` onward: `gh pr view <n> --json state,mergeable,reviewDecision,statusCheckRollup` plus
  a comment query newer than `prCommentCursor`. **The PR may have been merged, closed, or
  reassigned while you slept** — that possibility is the reason this phase re-derives rather than
  resumes. A merged or closed PR goes straight to `cleanup`, whatever the plan said.

## Chaining

`intake → prepare → plan` and `implement → verify` run **in one turn each**: their gates are
`continue`, which means proceed now, not stop. The run yields at exactly four places — the
`plan-review`, `result-review`, `push-confirm` and `merge-confirm` gates — plus each hourly
`babysit` tick. Stopping anywhere else leaves the run with nothing to wake it (see the skill's
wake loop): if you have to, open a gate first.

## Phase 0 — `intake`

| | |
|---|---|
| **goal** | Turn the request into a spec this run can be driven from, and choose the repo |
| **preconditions** | `facts.target` is set |
| **actions** | If the target looks like a Jira key (`[A-Z]+-\d+`), fetch it with the Jira MCP (`getJiraIssue`) and record `jiraKey` + `ticketUpdatedAt`; otherwise treat the target as the request itself. Distil into `artifacts/spec.md`: the problem, the acceptance criteria, what is explicitly out of scope, and open questions. Resolve the repo (see below) and `record-fact mainRepo=<abs path>`. Set policy explicitly |
| **durableOutput** | `artifacts/spec.md`, `facts.jiraKey`, `facts.ticketUpdatedAt`, `facts.mainRepo` |
| **exitCondition** | `artifacts/spec.md` exists with acceptance criteria, and `facts.mainRepo` is a git repo |
| **gate** | `continue` — **unless** the repo is ambiguous or the acceptance criteria cannot be stated, then **ask-human** |
| **idempotency** | If `artifacts/spec.md` exists and `mainRepo` is set, adopt them; only re-read the ticket if reconciliation says it changed |

**Resolving the repo.** In order: an explicit repo in the request; a repo named in the ticket
(component, fix-version, or a path in the description); a single obvious match among
`ls ~/workspace`. If two or more are plausible, **ask** — guessing the repo wastes a whole
plan-and-review cycle, and it is the single most expensive wrong turn available here.

**Jira is read-only** in this environment (every write tool is denied), which is exactly right
for phases 0–4: nothing here should touch the ticket. If a future phase needs to comment or
transition, that is an allowed-list change, not a code change.

## Phase 1 — `prepare`

| | |
|---|---|
| **goal** | A worktree on a fresh branch where lint and tests already pass, before any of our code exists |
| **preconditions** | `facts.mainRepo` is a git repo; `artifacts/spec.md` exists |
| **actions** | Lock the repo, create the branch if absent, then **invoke the `manage-pr` skill** for it. Once it reports the branch green, **unlock** — this phase is the only reason the lock was held |
| **durableOutput** | `facts.branch`, `facts.base`, `facts.worktree`, `facts.testCmd`; `artifacts/prepare-notes.md` (what `manage-pr` set up, and any repo friction) |
| **exitCondition** | `git -C <worktree> rev-parse --abbrev-ref HEAD` is `facts.branch`, `manage-pr` reported lint and tests **green on the empty branch**, and the repo lock is released |
| **gate** | `continue` — or **ask-human** if the baseline is red (see below) |
| **idempotency** | `otto state lock` is re-entrant, `git branch` is skipped when the branch exists, `manage-pr` reuses an existing worktree, and `unlock` is a no-op if already released. So re-entering this phase after a crash is safe and nearly free |

**Lock first, think second.** `otto state lock` is the *first* command of this phase, before
reading anything: it costs nothing, and the window between deciding to use a repo and claiming it
is exactly when another run takes it. (Measured on a real run: 15 minutes of deliberation
happened before the lock was acquired. Nothing went wrong; nothing had to go wrong for that to
be a bad habit.)

```
otto state lock <id> --repo <mainRepo>       # §9: two runs must never share a worktree
git -C <mainRepo> fetch origin               # best-effort; a stale ref is not a failure
git -C <mainRepo> symbolic-ref refs/remotes/origin/HEAD   # → facts.base, never assumed
git -C <mainRepo> branch --no-track <branch> origin/<base>  # only if it does not exist
→ invoke the manage-pr skill: "manage branch <branch> in <mainRepo>"
otto state unlock <id>                       # worktree + branch exist now; nothing left to protect
```

**Unlock as soon as the worktree exists, not one phase later.** Everything this lock protects —
`git fetch`, `git branch`, `manage-pr`'s `git worktree add` — is done by the time `manage-pr`
reports green. `plan`, `implement` and `verify` all work entirely inside this run's own worktree
directory and never touch another run's; holding the lock through them (and through whatever
human gate or sleep comes after) blocks every other run against this repo for no reason. That is
exactly the failure mode `skills/otto/SKILL.md`'s locking rule warns about — *"a run that sleeps
holding a lock blocks every other run against that repo for as long as it sleeps"* — and it is
what happened to a real run: it sat at the `result-review` gate for over a day while a second
run against the same repo waited on its lock the whole time. `publish`, `babysit` and `land` each
re-acquire the lock for the few commands that actually touch shared repo state (the remote push,
the upstream rebase, the merge) and release it again immediately after — see those phases.

**`--no-track` is not incidental.** Without it the new branch tracks `origin/<base>`, and a
later `git push` with the usual `push.default` aims at the *base* branch. Nothing in phases 0–4
pushes, so this cannot bite here — which is exactly why it has to be right here, before
`publish` exists and the mistake becomes a push to `main`.

**Why `manage-pr` and not our own worktree code.** It already knows the things that only hurt
once: that a worktree contains no gitignored files, so a missing `.env` breaks tests in a way
that never happens in the developer's checkout; that `pre-commit` must run at its pinned revs
rather than the venv's drifted ones; and the per-repo `build-info` memory of how this
particular repo builds. Otto's job is durable state and gates, not relearning that. Record the
worktree path it chose — it lives under `.work-trees/manage-pr/`, which is its directory, not
ours.

**A red baseline is information, not a failure.** If lint or tests fail on a branch containing
none of our work, the repo was already broken (or the environment is). Do not "fix" it as part
of this ticket: gate it, quoting the failure, and offer to proceed anyway, to fix the baseline
first as separate work, or to abandon.

## Phase 2 — `plan`

| | |
|---|---|
| **goal** | A plan specific enough to implement from, and to review meaningfully |
| **preconditions** | `facts.worktree` exists; `artifacts/spec.md` exists |
| **actions** | **Delegate the reading** — one or more `Agent` subagents explore the worktree and report which files change and why, existing patterns to follow, and the test surface. The wake writes `artifacts/plan.md` from their findings, never from its own file-by-file reading |
| **durableOutput** | `artifacts/plan.md` |
| **exitCondition** | `artifacts/plan.md` names the files to change, the approach, the tests that will prove it, and the risks |
| **gate** | **ask-human** — plan review |
| **idempotency** | If `artifacts/plan.md` exists and no gate is open, re-open the review gate rather than re-planning. A plan already approved (`gates/*plan-review*.md` answered "approve") is adopted and the phase advances |

`artifacts/plan.md` carries, in this order: the goal in one sentence; the files to change with
what changes in each; the approach, and one alternative rejected with the reason; how it will be
tested; what could go wrong; and anything the reviewer must decide. A plan that could describe
any ticket is not a plan.

**Scale the effort to the change.** One subagent and a five-line plan for a one-function change;
several subagents and a page for something that touches a subsystem. Measured on a real run: 31
minutes to plan adding one function beside an existing one — which produced a genuinely good
plan, and was still four times more deliberation than the change deserved. Judge the size from
the spec *before* dispatching, and say in the plan why you sized it that way.

Gate question — `open-gate --slug plan-review`, then stop:

> `<key>`: plan ready for review at `artifacts/plan.md` (N files, M tests). One-line summary of
> the approach. Base `<base>`, branch `<branch>`, worktree green.
>
> - **Approve** *(default)* — implement it as written.
> - **Revise** — say what to change; `plan` re-runs with your note (attempt N of 3).
> - **Different approach** — the rejected alternative, or one you name.
> - **Abandon** — the run ends `failed`, worktree left for you to inspect.

## Phase 3 — `implement`

| | |
|---|---|
| **goal** | The approved plan, committed, with lint and unit tests green |
| **preconditions** | The plan-review gate is closed and approved; the worktree is on `facts.branch` |
| **actions** | **Delegate the writing** to `Agent` subagents (or a `Workflow` script for independent parts), one coherent change per subagent, each given the plan section and the repo patterns it must follow. Commit per logical step in the worktree. Then invoke `manage-pr` again for lint + tests at pinned versions |
| **durableOutput** | Commits on `facts.branch`; `artifacts/implementation-notes.md` (what was built, what deviated from the plan and why) |
| **exitCondition** | `git log origin/<base>..<branch>` is non-empty, the working tree is clean, and `manage-pr` reported lint and tests green |
| **gate** | `continue` |
| **idempotency** | Re-derive from `git log` which plan steps are already committed and continue from there — never from a memory of what was written. A clean tree with commits and green checks means this phase is already done: adopt it |

**Deviating from an approved plan.** A small, obvious correction (a better function name, a
missed import) is fine — record it in `artifacts/implementation-notes.md`. A change in approach
is **not**: it invalidates the review the human gave. Stop and open a gate quoting the plan step
and what you found instead. What was approved is what is on disk in `gates/`, not what seems
sensible now.

**Failure classification applies here more than anywhere.** A denied host or a missing `.env` is
environmental: retry, do not burn an attempt. A genuinely failing test burns one and gets fixed.
Three real attempts and it is `blocked` plus a human gate carrying every diff tried.

## Phase 4 — `verify`

| | |
|---|---|
| **goal** | Evidence the change does what the spec asked, beyond "the unit tests pass" |
| **preconditions** | `implement` is complete: commits present, tree clean, checks green |
| **actions** | Re-read `artifacts/spec.md`'s acceptance criteria and check each one explicitly. Run the fuller suite the repo offers (integration/E2E per `build-info`), delegated to a subagent so its output never enters this wake's context. Write the report |
| **durableOutput** | `artifacts/test-report.md`: each acceptance criterion with its evidence, the commands run and their results, what was **not** covered, and the diffstat |
| **exitCondition** | Every acceptance criterion is marked met, waived (with the reason), or not-covered (with why) |
| **gate** | **ask-human** — result review |
| **idempotency** | If `artifacts/test-report.md` exists and the branch head has not moved since it was written, adopt it and re-open the gate; if the head moved, the report is stale — re-run |

Say what is untested. A report that claims everything is covered is the one nobody can act on,
and the reviewer is about to decide whether to push this.

Gate question — `open-gate --slug result-review`, then stop:

> `<key>`: implemented and verified. `artifacts/test-report.md` — K/N criteria met, lint+tests
> green, X commits, diffstat. **Nothing has left this machine yet**; the next phase asks again
> before it does.
>
> - **Go to publish** *(default)* — `publish` confirms the push separately, so this is not yet a
>   decision to push.
> - **More work** — say what; `implement` re-runs (attempt N of 3).
> - **Stop here** — the run finishes at *ready to push*: `artifacts/ready-to-push.md` gets the
>   commands to push by hand, and the worktree is left for you.
> - **Abandon** — the run ends `failed`, worktree left in place.

**Stop here** is worth keeping: it is the whole of build phase 3's behaviour, still available for a
change you would rather push yourself. On that answer, write `artifacts/ready-to-push.md` (worktree,
branch, base, commits, verification summary, the exact push and PR commands), a defensive bare
`unlock` (a no-op by now — `prepare` already released it once the worktree was green), and
`set-status --status done --reason "ready to push, by request"`.

## Phase 5 — `publish`

The first outward-facing act of the whole run.

| | |
|---|---|
| **goal** | The branch on the remote, and a PR open on it |
| **preconditions** | `result-review` closed and approved; tree clean; checks green; `git log origin/<base>..<branch>` non-empty |
| **actions** | Check `gh` for the remote's host (B0, below), confirm with a human, authorize the exact head, **lock, push, unlock**, then open the PR |
| **durableOutput** | `facts.pushedSha`, `facts.prNumber`, `facts.prUrl`, `facts.prHost`; `artifacts/publish-notes.md` |
| **exitCondition** | `git ls-remote origin <branch>` returns `pushedSha`, either a PR exists or `prNumber: null` is recorded with the reason, and the repo lock is released |
| **gate** | **ask-human confirm** — unless `policy.askBeforePush` is `false`, and even then the authorization must be recorded (from the policy, naming it) |
| **idempotency** | If the remote already has this sha, the push is done — adopt it (and skip the lock/push block entirely — nothing to unlock either). If `gh pr list --head <branch>` returns a PR, **adopt that PR**; never open a second one for one branch. `otto state lock` is re-entrant; `otto state unlock` (no `--repo`) is a true no-op — but `unlock --repo <repo>` errors if this run isn't currently holding it, so only call it right after the matching `lock`, never speculatively |

```
HEAD=$(git -C <worktree> rev-parse HEAD)
# 1. confirm  — gate slug `push-confirm`, quoting the diffstat and the exact remote
# 2. authorize — bind the approval to this commit, and nothing else
otto state authorize <id> --action push --gate <NNN> --head $HEAD --quote "<verbatim>"
# 3. refuse to act unauthorized, every time, even when you are sure
otto state check-authorized <id> --action push --head $HEAD || stop
otto state lock <id> --repo <mainRepo>       # only for the push itself
git -C <worktree> push -u origin <branch>
otto state unlock <id>                       # release before gh pr create — that's not a git-shared-state op
otto state record-fact <id> pushedSha=$HEAD
gh pr create --repo <owner/repo> --base <base> --head <branch> --title … --body-file …
```

**`gh` first (the B0 check `manage-pr` uses).** Derive the host from `git remote get-url origin`,
then `gh auth status --hostname <host>`. Auth is per-host — a token for one GHE instance says
nothing about `github.com` — and it must be established **outside** the sandbox, because the
`~/.config` grant is read-only. No `gh` for this host means: push if authorized, record
`prNumber: null` with the reason, and **gate** rather than pretending a PR exists. A remote with
no PR host at all (a plain git remote) is the same case, and is normal for a scratch repo.

**Ordering matters.** Push *after* the authorization check and *before* `gh pr create`: a pushed
branch with no PR is recoverable by a human in one command, while a PR whose branch never arrived
is confusing to everyone who sees it.

**"There is no PR" and "I cannot see the PR" are different, and confusing them is dangerous.**
Decide which one you are in, and record it as `facts.prNumber` plus a reason:

| Situation | What it means | Next phase |
|---|---|---|
| The remote has no PR host at all (a path, a bare repo, a plain git server) | There is no review to wait for, and nobody will ever open one | **`land`** — skip `babysit`, and say plainly in the merge gate that nobody reviewed this |
| A PR host exists, `gh` cannot authenticate or reach it | A review may be happening that you cannot see | **`blocked`** + a human gate. Do **not** proceed: merging code whose review state you cannot read is exactly the mistake the gates exist to prevent |
| A PR host exists and `gh` works | Normal | `babysit` |

The first row is what a scratch repo looks like, and it is why `tests/live_devflow.rs` can prove
these phases without touching a real PR.

Gate question — slug `push-confirm`:

> `<key>`: ready to push `<branch>` → `<remote host>/<owner/repo>`, N commits, `<diffstat>`, onto
> `<base>`. This is the first thing this run does that other people can see. `artifacts/test-report.md`
> has the verification; `artifacts/plan.md` was approved at gate `<NNN>`.
>
> - **Push and open a PR** *(default)* — then I watch the review hourly.
> - **Push only** — no PR; you open it when you want. The run then waits at a gate rather than
>   guessing whether a review happened.
> - **Not yet** — say what to change first; back to `implement`.
> - **Abandon** — nothing is pushed; the branch stays local.

## Phase 6 — `babysit`

Where the days are spent. Ticks must be cheap: this runs hourly for as long as review takes.

| | |
|---|---|
| **goal** | Keep the PR fresh, responsive and green until it is approved and mergeable |
| **preconditions** | `facts.prNumber` is set — with no PR there is nothing to babysit, and `publish` will have sent you to `land` instead; the worktree still exists |
| **actions** | **Lock**, one `manage-pr` **babysit tick** per wake (rebase + confidently-actionable comments), push what it changed, **unlock** — see below |
| **durableOutput** | Commits answering review; `facts.prCommentCursor` advanced; `facts.ciStatus`, `facts.reviewDecision` refreshed |
| **exitCondition** | `reviewDecision == APPROVED` **and** CI green **and** mergeable → `land`. PR merged or closed by someone else → `cleanup` |
| **gate** | `sleep(1h, until: approved && green)` — chain a one-shot cron per tick, per §5 |
| **idempotency** | The cursor is the ledger: a comment at or below `prCommentCursor` is already handled, so a re-entered tick cannot answer the same comment twice. Advance it **only after** the work is committed |

Each tick, in order:

2. **Reconcile first, decide second.** `gh pr view --json state,mergeable,reviewDecision,statusCheckRollup`.
   Merged or closed → `set-phase cleanup` and stop. That outcome is not a failure; it is Tuesday.
   (No lock needed yet — this is a read.)
3. **Lock**, then **invoke the `manage-pr` babysit tick** — not its schedule. Otto owns the clock
   (`arm-timer` + one-shot cron); `manage-pr` owns the work: its B0 `gh` check, the merged check,
   the stale/overlap **rebase gate** (rebase only if >24h behind or the base's commits overlap our
   files), its **confidently-actionable comments** rule (apply concrete unambiguous changes; skip
   questions, opinions and anything needing judgement), and keeping the branch green. Do **not**
   let otto's timer drive it and do not let it stop-on-change — otto decides what happens next.
4. **Push what changed** — the one thing `manage-pr` will not do. New commits mean a new head, so
   the old authorization is stale by design: re-authorize. Routine review-comment fixes on an
   already-approved push are authorized by `policy.askBeforePush == false`; with
   `askBeforePush: true` (the default) each new push needs its own confirm gate. That is the
   trade-off you chose at intake.
5. **Unlock — every path out of this tick, including "nothing happened".** The tick's git work
   (rebase, push) is done by now whether or not anything changed; a tick that finds no work must
   not fall through to `sleep(1h)` still holding the lock, or `babysit`'s hourly cadence becomes
   the exact multi-day lock-hold this fix exists to avoid. Lock only for steps 3–4, per cycle,
   never for the sleep in between.
6. **Comments needing judgement** → open a human gate quoting the comment and the file, and stop
   (after unlocking). Guessing at a reviewer's intent is worse than waiting for them.
7. **Nothing happened** → `otto state tick <id>` (no `--progress`), re-arm, stop. One journal
   line. This is the common case and it must stay nearly free.
8. **Something happened** → `otto state tick <id> --progress`, then re-arm.

`policy.maxTicksWithoutProgress` (default 24) turns a month of silence into a human gate rather
than a slow token leak. A review that is genuinely just slow is not "no progress" — the count
resets whenever anything moves.

## Phase 7 — `land`

| | |
|---|---|
| **goal** | The change on the base branch |
| **preconditions** | Approved, green, mergeable, and the merge authorized for **this** head |
| **actions** | Confirm (or invoke the policy), authorize, check, **lock**, merge, **unlock** |
| **durableOutput** | `facts.mergedSha`, `facts.mergedAt`; `artifacts/merge-notes.md` |
| **exitCondition** | The PR state is `MERGED`, or (no PR) `git ls-remote origin <base>` contains our head; repo lock released either way |
| **gate** | **ask-human confirm** — unless `policy.autoMergeWhenGreen` is `true`, which authorizes it *by name* in the journal |
| **idempotency** | Already merged → adopt and go to `cleanup`, skipping the lock/merge block entirely. This check is the phase's first act, because "merge" is the one action you must never do twice. `lock` is re-entrant; call bare `unlock` (no `--repo`) right after the merge, never speculatively — `unlock --repo <repo>` errors if this run isn't holding it |

```
HEAD=$(git -C <worktree> rev-parse HEAD)
otto state authorize <id> --action merge --gate <NNN> --head $HEAD   # or --policy autoMergeWhenGreen
otto state check-authorized <id> --action merge --head $HEAD || stop
otto state lock <id> --repo <mainRepo>                     # the merge itself touches <base>
gh pr merge <n> --<strategy> --repo <owner/repo>          # with a PR
git -C <worktree> push origin HEAD:<base>                 # no PR host: a plain fast-forward
otto state unlock <id>
```

**With no PR**, the merge is a push to the base — and it must **fast-forward**. If it will not
(`git merge-base --is-ancestor origin/<base> HEAD` fails), the base has moved: **lock**, rebase
per the stale/overlap gate, **unlock**, and re-ask — because the thing that was approved is no
longer what would land. This rebase is its own short lock/unlock, separate from the merge's; do
not hold the lock across the re-ask gate. Never force. And say it in the gate: with no PR, nobody
has reviewed this but you.

**The strategy is not yours to pick.** Squash, merge or rebase changes the base's history, and
repos disagree. Read it from `build-info` if it is recorded there; otherwise ask in the gate and
record the answer as `policy.mergeStrategy`.

Gate question — slug `merge-confirm`:

> `<key>`: `<PR url, or "no PR — plain remote">` is approved and green, mergeable, `<n>` commits at
> `<sha>`. Merging puts this on `<base>` for everyone. `<strategy>` is the strategy
> `<from build-info | you choose>`.
>
> - **Merge** *(default)* — and then clean up.
> - **Wait** — keep babysitting; ask again when something changes.
> - **Stop here** — leave the PR open; the run finishes and you merge when you like.

**`autoMergeWhenGreen` is opt-in per run and still leaves a record**: the authorization names the
policy, so the journal says exactly what allowed the merge and at which commit. Turn it on once
you trust the logs, not before.

## Phase 8 — `cleanup`

> **`land` must not set `status: done`.** Only this phase does. A real run went `land → done`
> directly and left its worktree behind — the journal never mentioned `cleanup` at all, and otto's
> contract could not notice, because `done` with a handoff written is a legal way to stop. If the
> merge is the last thing you do, the run is not finished; it is finished when the worktree is
> reaped and the ticket is updated.

| | |
|---|---|
| **goal** | Leave nothing behind but the record |
| **preconditions** | The PR is merged or closed |
| **actions** | Reap the worktree (`manage-pr`'s Step 0 rules: contained-in-base, or the squash-merge heuristic — **offer**, do not force); `unlock` as a defensive no-op (each earlier phase already released it after its own git work — this catches a lock left behind by an interrupted phase); write the ticket update; rewrite `handoff.md`; `set-status done` |
| **durableOutput** | `artifacts/summary.md`; worktree gone; no lock held (should already be true) |
| **exitCondition** | `status: done`, no lock held, and the worktree either removed or explicitly left with a reason |
| **gate** | — |
| **idempotency** | Every step is a no-op when already done: `unlock` is idempotent, and a worktree that is gone stays gone |

**The ticket is read-only here.** Every Jira write tool is denied in this environment, so
`cleanup` writes what it *would* have posted to `artifacts/ticket-update.md` and says so in its
report. Wiring it up is an allowed-list change plus one line in this file — not a code change.

**Never `--force` a dirty worktree away.** Uncommitted changes in it are someone's work, even if
you cannot see why. Say what you left and let a human decide.

## What a green run looks like

`journal.jsonl`, in order: `run-created`, `phase-changed`(→intake), facts, `phase-changed`(→prepare),
`lock-acquired`, facts, `lock-released`, `phase-changed`(→plan), `gate-opened`/`gate-closed`(plan-review),
`phase-changed`(→implement), `phase-changed`(→verify), `gate-opened`/`gate-closed`(result-review),
`phase-changed`(→publish), `gate-opened`/`gate-closed`(push-confirm), **`authorized`**(push),
`lock-acquired`, facts(pushedSha, prNumber), `lock-released`, `phase-changed`(→babysit), then per
tick either a bare `noop-tick` (never touches the lock) or `lock-acquired` … `lock-released` around
a `progress-tick` — for as many days as the review takes — then `phase-changed`(→land),
`gate-opened`/`gate-closed`(merge-confirm), **`authorized`**(merge), `lock-acquired`,
facts(mergedSha), `lock-released`, `phase-changed`(→cleanup), `status-changed`(→done).

Three or four human gates. Two `authorized` lines, each naming a commit. A `lock-acquired` /
`lock-released` pair around each phase that actually touches shared repo state, and *nothing*
holding the lock across a gate or a sleep. Everything else is the run doing its job.
