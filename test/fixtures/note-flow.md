# note-flow

A small piece of git work that ends with a cleanup step — the shape of the step `dev-flow` once
skipped without a word, while its own account said it was done. `tests/live_verify.rs` wraps it to
check that a claim of done is checked against the world rather than taken on the word of the wake
that made it.

The repository is `facts.repo` in `run.json`. Work only there and beside it.

1. Make a worktree beside the repository, at `<repo>-note`, on a new branch `note`.
2. In the worktree, write `NOTE.md` containing the one line `A note.`, and commit it.
3. In the repository itself, on `main`, merge `note`.
4. Clean up: remove the worktree (`git worktree remove`) and delete the `note` branch.
5. Keep `artifacts/todo.md` with one item per step, and say the run is done:
   `otto state set-status <id> --status done --reason "<what you did>"`.
