# todo-flow — work in pieces, across a gate

Three trivial notes, with a gate between the second and the third. It exists to check what wakes do
with the run's todo (`harness/wake.md`, "The todo"), so it **never mentions one**. Work in pieces
that spans wakes is exactly when the harness says to keep a todo, and whether a wake does so without
being told is what `tests/live_todo.rs` measures.

```
otto run --instructions test/fixtures/todo-flow.md \
         --goal "Write the three notes todo-flow asks for" \
         --until "the run is done, with each note written or skipped at the go-on gate"
```

**Do not add work to this, and do not mention the todo in it.** Either one stops it measuring the
harness.

## The notes

Under `artifacts/notes/`, one at a time:

1. `alpha.md` — one sentence saying what this run is.
2. `beta.md` — how many wakes have happened so far (count `wake-started` in the journal).
3. `gamma.md` — the answer to the `go-on` gate, quoted verbatim.

Adopt a note that already exists rather than rewriting it.

## The gate

After `alpha.md` and `beta.md`, before `gamma.md`, open a gate with exactly `--slug go-on` and stop:

> Run `<id>` (todo-flow, a test fixture) has written `artifacts/notes/alpha.md` and `beta.md`.
> One note is left.
>
> ## Options
> - **Continue** *(default)* — write `gamma.md` and finish.
> - **Skip gamma** — finish without it.

On **Continue**, write `gamma.md` and set the status to `done`. On **Skip gamma**, don't write it,
and set the status to `done`.
