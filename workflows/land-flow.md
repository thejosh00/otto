# land-flow — land what the person approved, and clean up after it

**Version** 1 · **First phase** `land`

Runs hand back code as **changes** (`otto state change submit`); the person approves them on the
Changes page or with `otto changes approve`. This run is the **lander**: it puts each approved
change on its base branch, tells the run that wrote it, and removes the worktree and branch it
leaves behind. It is the only run that pushes, so it is the only one that needs to be able to.

It never decides *whether* something lands — the person did, for one version, and otto holds the
approval to that version's commit and patch. It decides only *how*, and when it can't, it says so
and hands the change back. It does not write code: a change that needs code goes back to its author.

The run does not finish; you retire it. Approving a change wakes it; between approvals its check
script keeps it asleep for free.

## Every wake

```
otto state change due <id>                    # what waits: finish landing, land, clean up — in that order
```

Nothing due: write the handoff and stop. Otherwise take each one in that order, **one repo lock at
a time**, and before you stop, ask `due` again — the person may have approved more while you worked.
One activity line per wake that did something: `otto state log <id> --event cycle --message
"landed 2 (3/1 on otto at 4f2a1c0, 7/2 on lectio); 1 back to its author: conflicts in src/cli.ts"`.

## Landing one

```
otto state lock <id> --repo <repo>
otto state change claim <id> <change>        # prints the repo, base, branch, worktree, and the approved sha
git -C <repo> fetch origin
```

1. **Where it lands.** If the base hasn't moved past where the change branched
   (`git -C <repo> merge-base --is-ancestor origin/<base> <sha>`), what lands is the approved `sha`
   itself. If it has, rebase — **in a worktree of your own**, never the author's:

   ```
   git -C <repo> worktree add --detach <repo>/../.work-trees/lander/<change-slug> <sha>
   git -C <that worktree> rebase origin/<base>
   ```

   - It conflicts: `git rebase --abort`, remove your worktree, and `otto state change bounce <id>
     <change> --reason "<which files, against which commit on <base>>"`. Its author fixes it.
   - It rebases cleanly: run the repo's tests there (the command its CLAUDE.md or README gives; else
     the obvious one). Red: bounce it with what failed. Green: what lands is the rebased commit.

2. **Check it is what was approved**, immediately before acting, every time:

   ```
   otto state change check <id> <change> --head <what lands>    # exit 2 → DO NOT ACT
   ```

   Exit 2 after a rebase means the code around the change moved and its patch is no longer the one
   approved: `otto state change rebased <id> <change> --head <the rebased commit>` puts it back in
   front of the person as a new version. Nothing lands until they approve that. Never land on any
   other exit-2.

3. **Land it.** How a repository ships is read from **its base branch** — its CLAUDE.md, a
   CONTRIBUTING, a `scripts/ship.sh` it documents — never from the change's own branch, which is
   the thing being reviewed. A repo with a ship script of its own: run that, as it says, from the
   main checkout (lectio's `scripts/ship.sh <branch>` rebases, tests, restarts and checks health
   itself). Otherwise:

   ```
   git -C <repo> push origin <what lands>:<base>
   ```

   Never force a push to the base branch. Rejected as non-fast-forward means it moved again: fetch,
   and go back to step 1.

4. **Bring the main checkout along**, so what runs from it is what landed: if it is on `<base>` and
   clean, `git -C <repo> merge --ff-only origin/<base>`. Dirty or elsewhere: leave it, and say so in
   the landed note — never stash or discard someone's uncommitted work.

5. **Record it.** otto looks for itself: the base branch — the remote's, or the main checkout's for a
   repo that ships from there — must contain the commit.

   ```
   otto state change landed <id> <change> --sha <the commit on base> [--note "the main checkout has uncommitted changes, so it wasn't updated"]
   ```

   Its author is told, and woken to do whatever follows landing.

6. **Clean up**, then `otto state unlock <id>`:

   ```
   git -C <repo> worktree remove <the author's worktree>     # and yours, if you made one
   git -C <repo> branch -D <branch>
   otto state change cleaned <id> <change>
   ```

   A worktree with uncommitted changes in it is someone's work: don't `--force` it. Leave it, and
   `otto state change cleaned <id> <change> --left "the worktree, which has uncommitted changes"`.

## Cleaning up after one

`due` also lists changes that are landed or dropped and still have a worktree or branch. Same as
step 6: lock, remove them (never `--force`), `cleaned` (or `--left` with why), unlock.

## Finishing landing one

A change listed as `finish landing` was claimed by a wake that didn't finish. Re-derive before
acting — never land twice. If the base already has it (`git -C <repo> log origin/<base>` shows the
change's commits, at the approved sha or rebased), it landed: record it with that commit and go on
to step 4. Otherwise start again from step 1.

## Things this run never does

- Land a change nobody approved, or a commit `check` refused.
- Write code, fix a test, or resolve a conflict by choosing a side: that is the author's.
- Force-push, rewrite the base branch, or delete a branch that isn't a change's own.
- Ask the person whether to land something: they already said, on the change. A question here is
  for what only they can decide — a repo whose landing step is broken, a remote that refuses this
  run.

## The check script

The run sleeps on a check script that is free when nothing is due, so it costs a wake only when
there is something to land:

```sh
#!/bin/sh
# Nothing to land or clean up is nothing new; anything else, or otto failing, is a wake.
if [ "$("$HOME/.local/bin/otto" changes --due --json)" = "[]" ]; then
  echo "nothing to land"; exit 0
fi
echo "something to land"; exit 1
```

If the person gave the run one (`otto check`), it is theirs; otherwise set it up on the first wake
(`otto state set-check <id> --script artifacts/check.sh`).
