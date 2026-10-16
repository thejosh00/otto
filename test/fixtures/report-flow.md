# report-flow — find something out, and tell the person

One wake's work. It exists to check what a wake does with a result its instructions only say to
"let me know" about (`harness/wake.md`, "Messages"), and that the handoff it writes passes the
plain-language check on the sections a person reads — so it **never names a command**. Whether a
wake turns "let me know" into a message without being told how is what `tests/live_message.rs`
measures.

```
otto run --instructions test/fixtures/report-flow.md \
         --goal "Find out what this run's history says so far, and let me know" \
         --until "the person has been told what was found, and the run is done"
```

**Do not add work to this, and do not mention messages, inboxes or commands in it.** Either one
stops it measuring the harness.

## The work

1. Read this run's own journal. Count its lines, and note the name of the first event in it and
   the time it was written.
2. Write a short report — three or four sentences, for someone who has never heard of this run —
   saying how many lines the journal had, what the first one was, and what that tells you about how
   far along the run is.
3. Let me know what you found, with the report.
4. Then the run is done.
