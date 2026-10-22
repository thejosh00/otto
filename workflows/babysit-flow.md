# babysit-flow — look after every pull request otto opened, until it merges or closes

**Version** 1 · **First phase** `watch`

When a repository's process lands changes as pull requests, the lander opens one for each change the
person approves. This run is the **babysitter**: it watches every one of them, across every run and
repository, until it is merged or closed. It records what GitHub says — CI, reviews, whether it
merges — so the Changes page can say whose move it is; it tells a change's author what reviewers and
CI ask of it; and it notices when one is merged or closed by someone else.

It never pushes, merges, or posts: the lander does those. It never decides whether something lands:
the person did. The run does not finish; you retire it. Its check script keeps it asleep for free
until a pull request moves.

## Every wake

```
otto state change watch <id>    # every open pull request that has moved since you last looked
```

Each line is a change: its pull request, what moved, and, in `--json`, where its author's run keeps
what it knew (`authorDir`: `handoff.md`, `artifacts/`) and whether that run can still hear a note
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
- **A reviewer asked for something** — a change to the code, an answer to a question: `otto state
  change relay <id> <change> --who "a reviewer (<login>)" --text "<their words, quoted, with the file
  and line>"`. All of a reviewer's new asks in one relay. Never change the code yourself: its author
  does, and submits again; the lander pushes that to the pull request.
- **CI failing**: once, if it looks like a flake (a timeout, a runner that died), rerun the failed
  jobs (`gh run rerun <run> --failed`). Otherwise relay it, `--who CI`, with the failing job and the
  first lines of its error.
- **Conflicts with its base**, or someone else pushed to its branch: relay it to its author, saying
  which.
- **Approved and passing**: nothing to do — `pr-seen` below makes it the person's to merge, unless it
  merges itself.

When its author has finished (`authorAlive` is false), a relay reaches nobody, and otto says so on
the change; the person sees it there.

Then, every time, record what you saw, so the same news isn't news again:

```
otto state change pr-seen <id> <change> --updated <its updatedAt> --ci <pending|passing|failing> \
     --review <approved|changes-requested|waiting|none> --mergeable <clean|behind|conflicting> [--what "Sam approved; CI passing"]
```

`--review none` when nobody has to review it (no required reviewers, and none asked for). `--what`
when something happened a person would want in its history; not for a look that found nothing.

## Things this run never does

- Push, merge, close, or post anything on a pull request. Rerunning failed CI jobs is the one thing
  it does on GitHub.
- Write code, or decide that a change lands.
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
