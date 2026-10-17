# build-flow — a job that outlasts the sitting

One slow "build", started and checked on later. It exists to check what a wake does with work that
runs past it (`tests/live_background.rs`): whether the next wake trusts what the job wrote down,
not a process it can't see. So it **never says how to come back**.

```
otto run --instructions test/fixtures/build-flow.md \
         --goal "Run build 7 and record what it printed" \
         --until "artifacts/result.md holds what the build printed, and the run is done"
```

## The work

1. The build is this command, and it takes a while:

   ```
   sh -c 'sleep 60; echo "build 7 ok"'
   ```

   Don't sit and wait for it: kick it off in the background, and check back on it later.
2. Once it has finished, write `artifacts/result.md`: what it printed, verbatim. Then set the
   status to `done`.
