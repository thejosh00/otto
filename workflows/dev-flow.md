# dev-flow — ticket to landed code

**Version** 2 · **First phase** `intake`

Read a ticket, prepare a worktree, plan, get the plan reviewed, implement, verify, hand the result
back as a **change**, answer what comes back for as long as it takes, clean up.

**This run never touches the world.** It doesn't push, open a pull request, or merge: it hands its
branch back as a change (`otto state change submit`), the person approves it, and the lander lands
it — pushed to the base or as a pull request, as the repository's **process** says
(`otto config project <mainRepo>`). otto binds the approval to that version's commit and patch, so
an approval given on Monday does not land what the branch became on Wednesday. An agent that decides
on day three to merge because it looked ready is the failure everyone remembers; here it has no way
to.

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
| `process` | `intake` | The process the repo follows, by name (`otto config project <mainRepo>`): what approving the change will do |
| `testCmd` | `prepare` | The repo's test command, as `manage-pr` inferred it |
| `change` | `submit` | The change's number within this run: `<id>/<change>` is how `otto changes` names it |
| `landedSha` | `review` | The commit on the base once it landed, from the note that says so |

**Policy defaults** for this workflow: `maxAttempts: 3`, `maxTicksWithoutProgress: 14` (two weeks
of a change nobody looks at, at a tick a day). How much the person stays in the loop after the plan
is not this run's policy: it is the repository's process, set once for every change to it.

## Reconcile, per phase

Cheap and phase-scoped — never fetch a PR during `plan`. On every wake:

- `plan` onward: if `facts.jiraKey`, re-read *only* the ticket's `updated` field. If it differs
  from `ticketUpdatedAt`, the spec may be stale — re-read the ticket, rewrite
  `artifacts/spec.md`, and **open a human gate** rather than quietly acting on a changed
  request. Otherwise read `artifacts/spec.md`, not the ticket;
- `implement` onward: `git -C <worktree> status --porcelain` and
  `git -C <worktree> log --oneline origin/<base>..HEAD`. What is committed is the truth about
  progress; a memory of having written code is not.
- `review` onward: `otto changes show <id>/<change> --json` — its state, its versions, its pull
  request if it has one, and what happened to it. **It may have landed, been dropped, or had its
  pull request closed while you slept** — that possibility is the reason this phase re-derives rather
  than resumes. Landed, merged, dropped or closed goes straight to `cleanup`, whatever the plan said.
  Never fetch the pull request with `gh` yourself: what it says reaches you from the lander.

## Chaining

`intake → prepare → plan` and `implement → verify → submit → review` run **in one turn each**:
their gates are `continue`, which means proceed now, not stop. The run yields at exactly two places
— the `plan-review` gate, and `review`'s sleep, from which each note about the change wakes it. Stopping anywhere else leaves the run with nothing to wake it (see the skill's
wake loop): if you have to, open a gate first.

## The todo

`intake` writes `artifacts/todo.md` as its first act, one item per phase in order, and every phase
keeps its own item true. It is what a person sees of this run's progress, and what makes a skipped
phase visible: the run in DESIGN.md §19.1 went `land → done`, and nothing on disk said `cleanup`
never happened. Here its item stays `[ ]`, and otto flags a `done` run that leaves one open.

```markdown
# <key>: <the spec's one-line summary>
- [ ] intake — artifacts/spec.md, and the repo chosen
- [ ] prepare — a worktree on the branch, baseline green
- [ ] plan — artifacts/plan.md, approved at plan-review
- [ ] implement — committed, lint and tests green
- [ ] verify — artifacts/test-report.md, every criterion accounted for
- [ ] submit — handed back as a change
- [ ] review — landed, as the repo's process says
- [ ] cleanup — ticket update and summary written
```

- **`[~]` while a phase is under way, including while its gate waits on a person; `[x]` once the
  run moves past it.** What a later wake or a person will want from a phase (the branch, the
  change's reference, its pull request, the landed sha) goes on an indented line under its item,
  not in a new item.
- **A phase the run does not go through is `[-]`, with why.** A change that was dropped leaves
  `review` `[-]`, with what the person said.
- **Ticks don't touch it.** `review` stays `[~]` for as long as review takes. A tick that
  rewrote the todo every hour would be paying to say nothing.
- **It reports progress; it doesn't decide it.** Git, the gates on disk and the artifacts are the
  truth, and a re-entered phase re-derives from them as it always has, then corrects its item if
  the two disagree. A todo that says `implement` is done is not a commit.

## Phase 0 — `intake`

| | |
|---|---|
| **goal** | Turn the request into a spec this run can be driven from, and choose the repo |
| **preconditions** | `facts.target` is set |
| **actions** | Write the todo (above). If the target looks like a Jira key (`[A-Z]+-\d+`), fetch it with the Jira MCP (`getJiraIssue`) and record `jiraKey` + `ticketUpdatedAt`; otherwise treat the target as the request itself. Distil into `artifacts/spec.md`: the problem, the acceptance criteria, what is explicitly out of scope, and open questions. Resolve the repo (see below) and `record-fact mainRepo=<abs path>`; read its process (`otto config project <mainRepo>`) and `record-fact process=<name>`. Set policy explicitly |
| **durableOutput** | `artifacts/todo.md`, `artifacts/spec.md`, `facts.jiraKey`, `facts.ticketUpdatedAt`, `facts.mainRepo`, `facts.process` |
| **exitCondition** | `artifacts/spec.md` exists with acceptance criteria, and `facts.mainRepo` is a git repo |
| **gate** | `continue` — **unless** the repo is ambiguous or the acceptance criteria cannot be stated, then **ask-human** |
| **idempotency** | If `artifacts/todo.md`, `artifacts/spec.md` and `mainRepo` exist, adopt them; only re-read the ticket if reconciliation says it changed |

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
otto config project <mainRepo>               # its process: when its "starting" says otherwise, follow it
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
what happened to a real run: it sat at a review gate for over a day while a second run against
the same repo waited on its lock the whole time. Nothing after `prepare` touches shared repo state
— the push, the rebase, the merge are the lander's, under its own lock — so this run never locks
again.

**`--no-track` is not incidental.** Without it the new branch tracks `origin/<base>`, and a stray
`git push` with the usual `push.default` aims at the *base* branch. This run never pushes — but a
person who picks up its worktree might, and the mistake would be a push to `main`.

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
| **gate** | `continue` — the change `submit` hands back is the result review, asked better than a gate could |
| **idempotency** | If `artifacts/test-report.md` exists and the branch head has not moved since it was written, adopt it; if the head moved, the report is stale — re-run |

Say what is untested. A report that claims everything is covered is the one nobody can act on,
and the person is about to decide whether this lands.

**Criteria you can't meet** are not a reason to submit anyway, nor to stop silently: open a gate
quoting the criterion and what you found, with options to waive it, to go back to `implement`, or
to abandon.

## Phase 5 — `submit`

| | |
|---|---|
| **goal** | The verified branch, handed back to the person as a **change** |
| **preconditions** | `verify` is complete; tree clean; `git log origin/<base>..<branch>` non-empty |
| **actions** | Write `artifacts/change.md` — the review — then `otto state change submit`, and record the change's number |
| **durableOutput** | `facts.change`; `artifacts/change.md` |
| **exitCondition** | `otto changes show <id>/<change> --json` has a version at the branch's head |
| **gate** | `continue` → `review` |
| **idempotency** | Submitting the same commit again is not a new version — otto says it is unchanged — so re-entering this phase after a crash is safe. With `facts.change` set, submit the same `--branch`: a new head is its next version, never a second change |

```
otto state change submit <id> --repo <mainRepo> --branch <branch> --title "<key>: <what it does>" \
     --summary-file artifacts/change.md --tests "<what the suite said, in a line>"
otto state record-fact <id> change=<n>       # the number submit printed
```

**`artifacts/change.md` is what the person reviews**, so it is written for someone who wasn't
here: what it does and why, first (the ticket, the approach the plan took); then each acceptance
criterion and its evidence from `artifacts/test-report.md`; then what is **not** covered and what
could go wrong. Name the plan gate it implements, and anything you deviated from.

**Nothing leaves this machine here, or anywhere in this run.** Pushing, opening a pull request,
merging: the lander does them, once the person approves, and only as the repository's process
says. The approval is bound to this version's commit and patch by otto itself — a later commit is
a new version the person sees before it lands.

## Phase 6 — `review`

Where the days are spent: the change is with the person, the lander, and — as a pull request —
its reviewers. This run waits, and does what comes back to it.

| | |
|---|---|
| **goal** | The change landed (or merged), or dropped |
| **preconditions** | `facts.change` is set |
| **actions** | Act on each note about the change (below); then sleep until the next one |
| **durableOutput** | New versions of the change, replies drafted, questions asked; `facts.landedSha` once it lands |
| **exitCondition** | `otto changes show <id>/<change> --json` says `landed` or `merged` → `cleanup`; `dropped` or `closed` → `cleanup`, reporting that it didn't land |
| **gate** | `sleep` — `otto state arm-timer <id> --in 86400 --waiting-for "change <change> to land"` (a day). A request for changes, the lander bouncing it, and its landing each wake the run; a drop is heard at the next wake, which is what the timer is for |
| **idempotency** | The change's state and its timeline are the ledger, not a memory: a request already answered by a later version is done. Notes are read once; re-derive from `otto changes show` when unsure |

What reaches the run is a note that starts `About change <n>@v<k>`:

- **The person asked for changes**, or **the lander couldn't land it** (a conflict, tests red after a
  rebase): in the same worktree, make the change as `implement` does — commit, lint and tests green
  — then `submit` it again with the same branch. Back to `implement` → `verify` → `submit` when it
  is more than a small fix. A request that changes the approach the plan took goes back to `plan`,
  and its gate.
- **On its pull request, a reviewer (or CI) said…**: a change to make is the same as above; the
  lander pushes the next version to the pull request — without asking the person again only when
  they allow changes to it without them (`allowChanges`); otherwise they approve it again first. A question,
  or an ask the code already meets — look before you assume it doesn't — is a reply drafted for
  the person to post: `otto state change reply <id> <change> --comment <its id> --text "…"`.
- **A judgement that is the person's** — a reviewer wants it done another way, the scope grew — is
  asked about the change, not guessed: `otto state change ask <id> <change> --question-file <file>`.
  It doesn't stop the run; the answer comes as a note.
- **It landed** (or its pull request merged): `record-fact landedSha=<sha>` and go to `cleanup`.
- **It was dropped** (or its pull request closed): go to `cleanup`; the run finishes without it.

Each wake that did something is `otto state tick <id> --progress`; one that found nothing new is a
bare `tick`. `policy.maxTicksWithoutProgress` turns a change nobody looks at into a human gate
rather than a slow token leak. The same request coming back `policy.maxAttempts` times is
`blocked`, with a gate carrying each version and what was said about it.

**Never push, rebase onto the base, merge, or comment on the pull request yourself** — not even to
"just fix CI". Those are the lander's; the run's part is the code, and the words it drafts for the
person.

## Phase 7 — `cleanup`

> **Only this phase sets `status: done`.** A real run once went straight from merging to `done` and
> left its work half finished — nothing on disk said cleanup never happened. The todo is what makes
> that visible now: `cleanup` would still be `[ ]`, and `set-status done` names it to the wake that
> tried, while it can still do the work.

| | |
|---|---|
| **goal** | Leave nothing behind but the record |
| **preconditions** | The change is landed, merged, dropped or closed |
| **actions** | `unlock` as a defensive no-op; write the ticket update; mark `cleanup` `[x]`; rewrite `handoff.md`; `set-status done` |
| **durableOutput** | `artifacts/summary.md`: what landed and where (`facts.landedSha`), or that it was dropped and why |
| **exitCondition** | `status: done`, and no lock held |
| **gate** | — |
| **idempotency** | Every step is a no-op when already done |

**The worktree and branch are the lander's to remove**, once the change is landed or dropped —
leave them. A dropped change still finishes the run: say so plainly in the summary and the ticket
update, rather than reporting success.

**The ticket is read-only here.** Every Jira write tool is denied in this environment, so
`cleanup` writes what it *would* have posted to `artifacts/ticket-update.md` and says so in its
report. Wiring it up is an allowed-list change plus one line in this file — not a code change.

## What a green run looks like

`journal.jsonl`, in order: `run-created`, `phase-changed`(→intake), facts, `phase-changed`(→prepare),
`lock-acquired`, facts, `lock-released`, `phase-changed`(→plan), `gate-opened`/`gate-closed`(plan-review),
`phase-changed`(→implement), `phase-changed`(→verify), `phase-changed`(→submit), `change-submitted`,
facts(change), `phase-changed`(→review), then a quiet wait — a wake per note about the change, and a
`change-submitted` for each version it takes — then facts(landedSha), `phase-changed`(→cleanup),
`status-changed`(→done).

One gate of this run's own, the plan. One review of the change, on the Changes page or its pull
request. A `lock-acquired` / `lock-released` pair around `prepare` only, and *nothing* holding the
lock across a gate or a sleep. Nothing pushed by this run, ever. And a todo with every item `[x]`.
