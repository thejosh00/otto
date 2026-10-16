# design-flow — research, a design I approve, then the finished piece

A tiny piece of work in three steps, with my approval between the second and third. It exists to
check what a wake does with work whose results a person reviews and wants to find again
(`harness/wake.md`, "Outputs"): whether each document is kept where I can find it, whether a
revised design is a new version of the same document rather than a new document, whether the
question asking me to approve it is about that document, and whether the finished piece is pointed
out to me. So it **never names a command**. That is what `tests/live_outputs.rs` measures.

```
otto run --instructions test/fixtures/design-flow.md \
         --goal "Research, design and write a one-line greeting, with my approval of the design" \
         --until "the greeting is written, I have been told, and the run is done"
```

**Do not add work to this, and do not mention outputs, publishing, gates or commands in it.** Any
of those stops it measuring the harness.

## The steps

The work moves through three steps: **Research**, **Design**, **Greeting**.

1. **Research.** Read this run's own journal and write a short research note — two or three
   sentences for someone who has never heard of this run — on what has happened in it so far. I'll
   want to find this again.
2. **Design.** Write a short design for a one-line greeting to whoever reads this run next: who it
   is for, its tone, and the line itself. Then ask me whether the design is right, with two options
   — approve it, or say what to change. Do not go on until I answer.
3. If I ask for changes, revise the design as I say — it is the same design, improved — and ask me
   again, the same way. Do this until I approve it.
4. **Greeting.** Once I approve, write the greeting exactly as the approved design says, as a short
   document I'll want to find again, and let me know it is done.
5. Then the run is done.
