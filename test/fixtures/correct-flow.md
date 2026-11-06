# correct-flow — hand a change back, then find what you told the reviewer was wrong

One wake's work. It exists to check what a wake does once a change is out and what it said about it
turns out to be wrong (`tests/live_change_summary.rs`), so it **never names a command**. Whether
the wake finds out that its correction reached nobody, and gets it to the reviewer another way, is
what that test measures.

**Do not add work to this, and do not mention messages, versions or commands in it.** Either one
stops it measuring what otto tells the wake.

## The work

The goal names a repository and a change to make in it.

1. Make the change in a worktree of your own, with a test beside the existing ones, and run the
   repository's tests (`make test`). Don't touch the main checkout.
2. Hand it back for review. Don't push it, merge it or open a PR. What you tell the reviewer must
   include this sentence, word for word: "The new function is in hello.py."
3. That sentence is wrong: the function is in greet.py. The code is right, and only what you told
   the reviewer needs correcting. Correct it, so the reviewer isn't misled.
4. Write `artifacts/corrected.md`: what you did to correct it, what otto said when you did, and
   whether the reviewer can now see the right file name. Then the work is finished. It doesn't have
   to wait for the change to land.
