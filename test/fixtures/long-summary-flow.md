# long-summary-flow — hand a change back with far too much to say about it

One wake's work. It exists to check what a wake does when what it tells the reviewer runs well over a
page (`tests/live_long_summary.rs`), so it **never says how to split it up**. Whether otto's own reply
gets the wake to something the reviewer can read in a minute, before any version exists for them to
approve, is what that test measures.

**Do not add work to this, and do not describe what otto wants instead.** Either one stops it measuring
what otto tells the wake.

## The work

The goal names a repository and a change to make in it.

1. Make the change in a worktree of your own, with a test beside the existing ones, and run the
   repository's tests (`make test`). Don't touch the main checkout.
2. Write `artifacts/review-notes.md` for the reviewer: what the change does, why, every alternative you
   can think of and why you didn't take it, what could go wrong, and how the test covers it. It must be
   at least 700 words. Check with `wc -w`.
3. Hand the change back for review, giving those notes as its summary, with no other document beside
   them. Don't write anything shorter ahead of time. Don't push it, merge it or open a PR.
4. If otto wants something else, do what it says.
5. Write `artifacts/submitted.md`: what otto said each time you handed it back, and what the reviewer
   now sees. Then the work is finished. It doesn't have to wait for the change to land.
