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
2. Wait for `artifacts/proceed` to exist before going on: whoever is driving this fixture writes it
   once the palette is there, usually within seconds. Check for it with one shell loop that looks
   every second, for up to three minutes. If it never appears, open no question: set the status
   to `failed` with the reason "no proceed".
3. The person picks the colour, red or blue. Unless they have already said which, open a gate
   with exactly `--slug colour` and stop:

   > Run `<id>` (colour-flow, a test fixture) needs a colour for `artifacts/colour.md`.
   >
   > ## Options
   > - **Red**
   > - **Blue**

4. Write `artifacts/colour.md`: the colour they picked, one word. Then set the status to `done`.
