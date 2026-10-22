# land-flow — land what the person approved, and clean up after it

**Version** 1 · **First phase** `land`

Runs hand back code as **changes** (`otto state change submit`); the person approves them on the
Changes page or with `otto changes approve`. This run is the **lander**: it puts each approved
change on its base branch, tells the run that wrote it, and removes the worktree and branch it
leaves behind. It is the only run that pushes, so it is the only one that needs to be able to.

It never decides *whether* something lands — the person did, for one version, and otto holds the
approval to that version's commit and patch. Nor pushed or a pull request: the repository's
**process** decided that, or the person did for this one, and `claim` says which (`mode`). It
decides only *how*, and when it can't, it says so and hands the change back. It does not write code: a change that needs code goes back to its author.

The run does not finish; you retire it. Approving a change wakes it; between approvals its check
script keeps it asleep for free.

## Every wake

```
otto state change due <id>      # what waits on you, and every pull request with news, in the order to do them
```

In the commands below, `<sha>:<base>` and the like are refspecs. With the commit in a shell variable,
brace it — `"${sha}:refs/heads/${branch}"` — because zsh reads `$sha:r…` as a modifier and pushes
something else.

Each item says what to do: `finish landing`, `land`, `open its pull request`, `update its pull
request`, `merge its pull request`, `post replies`, `look at its pull request`, `clean up`. Each has
a section below. Nothing due: write the handoff and stop. If `due` refuses because no lander is set up, the person
hasn't named this run yet (`otto config lander`): say so in the handoff's `Needs`, and stop. Otherwise take each one in that order, **one repo lock at
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

3. **Land it.** What `claim` printed under `land` comes first: the words of the process it was
   approved under (`process`), kept from the moment it was approved. Then how the repository ships,
   read from **its base branch** — its CLAUDE.md, README or CONTRIBUTING, a `scripts/ship.sh` they
   document — never from the change's own branch, which is the thing being reviewed. A repo with a ship script of its own: run that, as it says, from the
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

## Opening its pull request

The person approved it as a pull request: it is reviewed there, and you keep it moving. Lock, claim,
fetch, as for landing one. No rebase — the pull request shows whether it merges.

```
otto state change check <id> <change> --head <sha>            # exit 2 → DO NOT ACT
git -C <repo> push origin <sha>:refs/heads/<branch>
gh pr create --repo <owner/repo> --head <branch> --base <base> --title "<its title>" --body-file <its summary>
otto state change pr-pushed <id> <change> --head <sha> --url <the pull request>
otto state unlock <id>
```

`mergeWhenReady` in what `claim` printed: also `gh pr merge <url> --auto` with the merge method the
repository uses (its process's `land` says, else its docs; else `--squash`), and `--auto-merge` on
`pr-pushed`. Leave the
author's worktree and branch: what reviewers ask, its author answers there.

## Updating its pull request

Its author submitted the next version, and it goes to the pull request without asking the person
again — the pull request is where it is reviewed. Lock, claim, then:

```
otto state change check <id> <change> --head <sha>            # the new version's commit
git -C <repo> push --force-with-lease=<branch>:<its pull request's head> origin <sha>:refs/heads/<branch>
otto state change pr-pushed <id> <change> --head <sha>
```

The lease is the head `pr-pushed` last recorded: if someone else pushed to the branch since, the
push is refused — don't override it; tell the author (`relay`, below).

## Looking at its pull request

`due` lists one whose pull request changed since you last looked. Read it —

```
gh pr view <url> --json state,mergeCommit,headRefOid,updatedAt,statusCheckRollup,reviewDecision,mergeable,reviews,comments
gh api repos/<owner>/<repo>/pulls/<n>/comments                # comments on lines of the code
```

— and act on what is new since your ledger (`artifacts/prs.md`: per pull request, the highest
comment and review ids you have handled):

- **Merged**: fetch, `otto state change merged <id> <change> --sha <its merge commit>`, then clean
  up as after landing one (step 6).
- **Closed without merging**: `otto state change closed <id> <change> --reason "closed by <who> on
  GitHub"`, then clean up.
- **A reviewer asked for something** — a change to the code, an answer to a question: `otto state
  change relay <id> <change> --who "a reviewer (<login>)" --text "<their words, quoted, with the file
  and line>"`. All of a reviewer's new asks in one relay. Never change the code yourself, however
  small: its author does, and submits again.
- **CI failing**: once, if it looks like a flake (a timeout, a runner that died), rerun the failed
  jobs (`gh run rerun <run> --failed`). Otherwise relay it, `--who CI`, with the failing job and the
  first lines of its error.
- **Conflicts with its base**, or someone else pushed to its branch: relay it to its author, saying
  which.
- **Approved and passing**: nothing to do — `pr-seen` below makes it the person's to merge.

Then, every time, record what you saw, so the same news isn't news again:

```
otto state change pr-seen <id> <change> --updated <its updatedAt> --ci <pending|passing|failing> \
     --review <approved|changes-requested|waiting|none> --mergeable <clean|behind|conflicting> [--what "Sam approved; CI passing"]
```

`--review none` when nobody has to review it (no required reviewers, and none asked for). `--what`
when something happened a person would want in its history; not for a look that found nothing.

## Merging its pull request

The person said merge it.

```
gh pr view <url> --json headRefOid
otto state change check <id> <change> --head <headRefOid>     # exit 2: someone else pushed — relay it
gh pr merge <url> --<the repository's merge method>
git -C <repo> fetch origin
otto state change merged <id> <change> --sha <its merge commit>
```

Then clean up as after landing one (step 6).

## Posting replies

The person said to post replies its author drafted; `due` lists them with the comment each
answers. A reply to a comment on a line: `gh api repos/<owner>/<repo>/pulls/<n>/comments/<id>/replies
-f body=<the reply>`; to the pull request as a whole: `gh pr comment <url> --body-file <file>`. Post
it as written, then `otto state change posted <id> <change> --comment <id>`.

## Cleaning up after one

`due` also lists changes that are landed, merged, dropped or closed and still have a worktree or
branch. Same as step 6: lock, remove them (never `--force`), `cleaned` (or `--left` with why),
unlock. A dropped change with its pull request still open: close it first (`gh pr close <url>
--delete-branch`) and `otto state change closed <id> <change> --reason "the person dropped it"`.

## Finishing landing one

A change listed as `finish landing` was claimed by a wake that didn't finish. Re-derive before
acting — never land twice. If the base already has it (`git -C <repo> log origin/<base>` shows the
change's commits, at the approved sha or rebased), it landed: record it with that commit and go on
to step 4. Otherwise start again from step 1.

Approved as a pull request (`mode` is `pr` in what `claim` printed), the same question is about its
pull request: `gh pr list --head <branch> --state all` shows whether one was opened. One was: record
it with `pr-pushed --url` (and the head its branch has on the remote). None: push and open it, as
below — never a second pull request for one branch.

## Things this run never does

- Land a change nobody approved, or a commit `check` refused.
- Write code, fix a test, or resolve a conflict by choosing a side: that is the author's. A reviewer's
  ask, however small, is relayed.
- Post anything on a pull request but what the person said to post: a reply its author drafted.
- Force-push, rewrite the base branch, or delete a branch that isn't a change's own.
- Ask the person whether to land something: they already said, on the change. A question here is
  for what only they can decide — a repo whose landing step is broken, a remote that refuses this
  run.

## The check script

The run sleeps on a check script that is free when nothing is due, so it costs a wake only when
there is something to land or a pull request has moved (`--due` asks `gh` about each open one):

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
