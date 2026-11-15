# Approvals on record

`~/.otto/approvals/<run id>/<n>.json` (`$OTTO_HOME/approvals/…`) is your approval of one change:
change `<n>` of run `<run id>`. `otto changes approve` and the page's Approve write it, before the
run's own `changes/index.json` says the change is approved.

It is kept outside the run directory on purpose. A wake can write its own run's files, so an
approval that lived only there could be written by the wake that made the change. A sandboxed wake
cannot write `approvals/`, whatever its sandbox record lists (see
[sandboxing](../guides/sandboxing.md#whats-closed)).

`otto state change check`, which the lander runs before it lands a change, passes only on this
record. The run's index says what state a change is in; it is not the authority (see
[what the check reads](#what-the-check-reads)).

## Fields

| Field | What it is |
|---|---|
| `run`, `n` | The run's id and the change's number when you approved it |
| `version` | The version you approved |
| `sha`, `patchId` | That version's commit, and the `git patch-id --stable` of its whole patch, which is the same after a clean rebase |
| `repo`, `base`, `branch` | The repository's main checkout, the branch it lands on, and the change's branch, as they were when you approved |
| `mode` | `push` or `pr`: how it lands |
| `process` | The process its repository followed then |
| `land` | That process's words on how a change lands, when it lands the way the process says |
| `overridden` | `true` when you chose how it lands rather than its process |
| `mergeWhenReady` | As a pull request: merge it once it is approved and passing |
| `allowChanges` | As a pull request: what the babysitter may change without you, in your words |
| `note` | Your `--note`, if you gave one |
| `at` | When you approved |
| `remote` | The repository's `origin` address when you approved: where the landing step pushes it. Absent on an approval from before it was kept, for which the step reads the repository's own `remote.origin.url` |
| `mergeAsked` | `{at, head}`: when you said to merge its pull request (`otto changes merge`), and the commit the pull request had then |
| `answered` | `{at, version, by}`: when you answered what the babysitter asked about its pull request (`otto changes answer`), the latest version then, and who answered. It lets the babysitter's fix through as the one next version. A new approval clears it |

Fields that are empty or `false` are left out.

## What writes and removes it

| You do | The record |
|---|---|
| `otto changes approve`, or Approve on the page | Written. Approving a later version replaces it |
| `otto changes revise`, when the change has no pull request | Removed |
| `otto changes drop` | Removed |
| `otto changes allow` | `allowChanges` updated, if there is a record |
| `otto changes merge` | `mergeAsked` set, if there is a record |
| `otto changes answer`, to the babysitter | `answered` set, before the run's index; if it can't be written the command fails and says why |
| `otto renumber` | Moved with the run to its new id |

Only your own commands touch it. What a wake does to a change (a new version from its author, the
lander sending it back or rebasing it) leaves the record as it was: an old record names a version
and a commit that are no longer the change's latest.

## What the check reads

`otto state change check`, and the lander's `claim` and `due`, take three things from the run's
index: the change's state, its latest version's number, and for a pull request whether there is one
and its head. Everything else comes from the record: the version and commit you approved, the
patch id, how it lands, what may change without you, and the commit you said to merge.

The record must be for this run, the index's latest version, and that version's commit. As a pull
request only, a later version passes in two cases: the record has `allowChanges`; or it has
`answered` and the latest version is exactly the one after `answered.version`. A pull request's own
head from the index counts only when the record allows later versions.

| What is there | What happens |
|---|---|
| The index says approved or landing, and no record matches | `check` and `claim` exit 2: `DO NOT ACT: no approval on record`. `otto changes` lists it as yours: "its run says approved, with no approval of yours on record: yours to approve again" |
| The same, when the lander next asks what is due | The change goes back to review with its decision cleared, and its history says, by `otto`: "approved before approvals were kept outside the run: approve it again" |
| A record for an earlier version that allows no later one | When the lander next asks what is due, it goes back to you: "the approval on record is for vN, and allows no later version without the person: vM is theirs to approve" |

A change approved before records were kept has none, so it takes the second row once. That is
expected: approve it again.

## When it can't be written

Approving writes the record first. If that fails, the change is not approved: it stays in review,
nothing is decided, and the command says why. When several changes are approved at once and one
fails here, the error names the ones already approved, as it does for any failure partway.

Run from inside a sandboxed wake, `otto changes approve` fails this way, and says that approving is
the person's.
