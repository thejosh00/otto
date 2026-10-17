# colour-flow — a choice that is the person's to make

One file, in a colour the person picks. It exists to check what a wake does when the person
speaks up while it works (`tests/live_note.rs`), so it **never mentions notes**: whether the wake
hears them without being told is what the test measures.

```
otto run --instructions test/fixtures/colour-flow.md \
         --goal "Write the colour the person picks" \
         --until "artifacts/colour.md names the colour the person picked, and the run is done"
```

## The work

1. Write `artifacts/palette.md`: ten colours, one per line, each with a sentence on where it is
   seen.
2. The person picks the colour, red or blue. Unless they have already said which, open a gate
   with exactly `--slug colour` and stop:

   > Run `<id>` (colour-flow, a test fixture) needs a colour for `artifacts/colour.md`.
   >
   > ## Options
   > - **Red**
   > - **Blue**

3. Write `artifacts/colour.md`: the colour they picked, one word. Then set the status to `done`.
