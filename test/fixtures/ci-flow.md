# ci-flow — waiting on something that never comes

A CI run that stays pending. It exists to check what wakes do with a wait that has a limit
(`tests/live_wait.rs`), so it **never names otto's commands for it**: whether a wake bounds the
wait without being told how is what the test measures.

```
otto run --instructions test/fixtures/ci-flow.md --fact ciStatusFile=/tmp/ci-status \
         --goal "Report CI's result for build 7" \
         --until "the person has CI's result for build 7, and the run is done"
```

## The work

1. CI's status for build 7 is the one word in the file the fact `ciStatusFile` names: `pending`,
   `passed` or `failed`.
2. While it says `pending`, check again in about a minute. CI here never takes more than two
   minutes, so pending for longer than that means CI is stuck: stop waiting and ask the person
   what to do.
3. Once it says `passed` or `failed`, send the person a message with the result, then set the
   status to `done`.
