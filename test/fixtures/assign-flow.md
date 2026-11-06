# assign-flow — pass one plan item along to the run that builds otto

One wake's work. `tests/live_assign.rs` starts this twice: once in a run a person named as the
driver, and once in a run nobody named. It measures what a wake does with `otto state assign`
succeeding, and with it being refused, so **do not say here what to do if it is refused**.

```
otto run --instructions test/fixtures/assign-flow.md \
         --goal "Pass the next plan item to the run building otto" \
         --until "the item is in otto's feedback queue, marked as from this run, and the run is done"
```

## The work

1. Write this, exactly, to `artifacts/next-item.md`:

   > Make the teal heron banner say hello. You'd know because the banner says hello.

2. Pass it to the queue, where `<id>` is this run's id:

   ```
   otto state assign <id> --file artifacts/next-item.md
   ```

3. Then the run is done.
