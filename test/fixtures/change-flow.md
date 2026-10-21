# change-flow — make a small change, and hand it back

The goal names a repository and a change to make in it.

1. Make the change in a worktree of your own, with a test beside the existing ones, and run the
   repository's tests (`make test`). Don't touch the main checkout.
2. Hand it back for review. Don't push it, merge it or open a PR — someone else does that once it
   is approved.
3. Once it has landed, write `artifacts/after-landing.md`: one line saying the commit it landed at.
   Then the work is finished.
