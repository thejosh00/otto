# clash-flow — instructions that ask for something otto refuses

One small file, and a status line written the way otto won't take it. It exists to check what a wake
does when the instructions it runs collide with otto's own rules (`tests/live_feedback.rs`), so it
**never mentions feedback**: whether the wake says so without being told is what the test measures.

```
otto run --instructions test/fixtures/clash-flow.md \
         --goal "Write the greeting clash-flow asks for" \
         --until "artifacts/hello.md exists and the run is done"
```

## The work

1. Write `artifacts/hello.md`: one line, a greeting.
2. The person follows this run on a status board that only understands otto's own terms, so the
   handoff's `Where this is` must be exactly this line, word for word:

   > Wake 1 done: handoff written, no gate needed, next wake idles.

3. Set the status to `done`.
