# Approvals on record

`~/.otto/approvals/<run id>/<n>.json` (`$OTTO_HOME/approvals/…`) is your approval of one change:
change `<n>` of run `<run id>`. `otto changes approve` and the page's Approve write it, before the
run's own `changes/index.json` says the change is approved.

It is kept outside the run directory on purpose. A wake can write its own run's files, so an
approval that lived only there could be written by the wake that made the change. A sandboxed wake
cannot write `approvals/`, whatever its sandbox record lists (see
[sandboxing](../guides/sandboxing.md#whats-closed)).

**Nothing acts on this file yet.** `otto state change check`, which the lander runs before it lands
a change, still reads the run's index. The next change makes the check read this record instead.
Until then the record is written and kept in step, and that is all.

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
| `mergeAsked` | `{at, head}`: when you said to merge its pull request (`otto changes merge`), and the commit the pull request had then |

Fields that are empty or `false` are left out.

## What writes and removes it

| You do | The record |
|---|---|
| `otto changes approve`, or Approve on the page | Written. Approving a later version replaces it |
| `otto changes revise`, when the change has no pull request | Removed |
| `otto changes drop` | Removed |
| `otto changes allow` | `allowChanges` updated, if there is a record |
| `otto changes merge` | `mergeAsked` set, if there is a record |
| `otto renumber` | Moved with the run to its new id |

Only your own commands touch it. What a wake does to a change (a new version from its author, the
lander sending it back or rebasing it) leaves the record as it was: an old record names a version
and a commit that are no longer the change's latest.

## When it can't be written

Approving writes the record first. If that fails, the change is not approved: it stays in review,
nothing is decided, and the command says why. When several changes are approved at once and one
fails here, the error names the ones already approved, as it does for any failure partway.

Run from inside a sandboxed wake, `otto changes approve` fails this way, and says that approving is
the person's.
