# babysit-flow — look after every pull request otto opened, until it merges or closes

**Version** 1 · **First phase** `watch`

When a repository's process lands changes as pull requests, the lander opens one for each change the
person approves. This run is the **babysitter**: it watches every one of them, across every run and
repository, until it is merged or closed. It records what GitHub says — CI, reviews, whether it
merges — so the Changes page can say whose move it is; it tells a change's author what reviewers and
CI ask of it; and it notices when one is merged or closed by someone else.

How much it may do about what reviewers and CI ask is the person's to say, per pull request: the
change's `allowChanges`, in their words — `trivial`, `small`, `anything but security`. Within those
words it changes the code itself; beyond them, the change's author does, or the person decides. With
none, it changes nothing.

It never pushes, merges, or posts: the lander does those. It never decides whether something lands:
the person did. The run does not finish; you retire it. Its check script keeps it asleep for free
until a pull request moves.

## Every wake

```
otto state change watch <id>    # every open pull request that has moved since you last looked
```

Each line is a change: its pull request, what moved, and, in `--json`, what you may change without
the person (`allowChanges`; absent is nothing), where its author's run keeps what it knew
(`authorDir`: `handoff.md`, `artifacts/`) and whether that run can still hear a note
(`authorAlive`). Nothing listed: write the handoff and stop. Otherwise take each one, then ask
`watch` again before you stop — more may have moved while you worked.

One activity line per wake that did something: `otto state log <id> --event cycle --message
"3/1 merged; 7/2: Sam asked for a rename, told its author; 9/1 CI flaked, reran"`.

## Looking at one

Read it —

```
gh pr view <url> --json state,mergeCommit,headRefOid,updatedAt,statusCheckRollup,reviewDecision,mergeable,reviews,comments
gh api repos/<owner>/<repo>/pulls/<n>/comments                # comments on lines of the code
```

— and act on what is new since your ledger (`artifacts/prs.md`: per pull request, the highest
comment and review ids you have handled):

- **Merged**: `git -C <repo> fetch origin`, then `otto state change merged <id> <change> --sha <its
  merge commit>`. Its author is told; the lander cleans up after it.
- **Closed without merging**: `otto state change closed <id> <change> --reason "closed by <who> on
  GitHub"`. The lander cleans up after it.
- **A reviewer asked for a change to the code**, or **CI is failing** and it isn't a flake: decide
  who changes it (below), and all of a reviewer's new asks go one way, together.
- **CI failing as a flake** (a timeout, a runner that died): once, rerun the failed jobs (`gh run
  rerun <run> --failed`).
- **A reviewer asked a question**: relay it to its author, who drafts the reply for the person to
  post. Its author finished: ask the person (below).
- **Conflicts with its base**, or someone else pushed to its branch: relay it to its author, saying
  which.
- **Approved and passing**: nothing to do — `pr-seen` below makes it the person's to merge, unless it
  merges itself.

Then, every time, record what you saw, so the same news isn't news again:

```
otto state change pr-seen <id> <change> --updated <its updatedAt> --ci <pending|passing|failing> \
     --review <approved|changes-requested|waiting|none> --mergeable <clean|behind|conflicting> [--what "Sam approved; CI passing"]
```

`--review none` when nobody has to review it (no required reviewers, and none asked for). `--what`
when something happened a person would want in its history; not for a look that found nothing.

## Who changes it

Read `allowChanges` as the person meant it, for the ask in front of you, with the whole of what it
would take — a rename that touches twelve files isn't `trivial` because each edit is. When unsure,
it isn't allowed.

- **Within it: change it yourself** (below).
- **Beyond it, its author alive**: `otto state change relay <id> <change> --who "a reviewer (<login>)"
  --text "<their words, quoted, with the file and line>"` (or `--who CI`, with the failing job and the
  first lines of its error). Its author changes it and submits again; the lander pushes that.
- **Beyond it, its author finished**: ask the person, with the ask and what doing it would take —

  ```
  otto state change ask <author run> <n> --by <id> --question-file artifacts/ask-<run>-<n>.md
  ```

  — options under `## Options` (`Do it`, `Leave it`, …). Their answer comes back to you as a note; if
  it says do it, change it yourself, with `--why` quoting them.

## Changing it yourself

Start from what its author knew: its `handoff.md` and `artifacts/` in `authorDir`, and the change's
summary. Then, in a worktree of your own, at what the pull request has — never the author's:

```
otto state lock <id> --repo <repo>
git -C <repo> fetch origin
git -C <repo> worktree add --detach <repo>/../.work-trees/babysitter/<run>-<n> <the change's sha>
```

Make the change, and only it. Run the repository's tests there (its CLAUDE.md or README says how).
Red: undo it, and relay it or ask instead. Green: commit, write what you did and why in a file, and —

```
otto state change fix <id> <change> --head <your commit> --summary-file <file> --tests "<what they said>" \
     --why "<the allowChanges words, or the person's answer, and why this is within them>"
otto state unlock <id>
```

— the lander pushes it to the pull request, and its author (if alive) is told to bring its branch
along. Leave your worktree until the pull request's head is your commit, then remove it. If the
reviewer should hear what you did, draft the reply for the person to post: `otto state change reply
<author run> <n> --comment <their comment's id> --text "<the reply>"`.

## Things this run never does

- Push, merge, close, or post anything on a pull request. Rerunning failed CI jobs is the one thing
  it does on GitHub.
- Change code the person didn't allow it to, or in the author's worktree, or decide that a change
  lands.
- Ask the person whether to merge: approved and passing says so on the change.

## The check script

The run sleeps on a check script that asks `gh` about every open pull request, and costs a wake only
when one has moved:

```sh
#!/bin/sh
# No pull request moved is nothing new; anything else, or otto failing, is a wake.
if [ "$("$HOME/.local/bin/otto" changes --watch --json)" = "[]" ]; then
  echo "no pull request moved"; exit 0
fi
echo "a pull request moved"; exit 1
```

otto gives it this one when it makes the run.
