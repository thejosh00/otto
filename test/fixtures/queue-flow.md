# queue-flow — say who filed what's next in otto's queue

One wake's work. `tests/live_assign.rs` uses it to measure whether a model reading
`otto feedback next` can tell an item a driver run passed along from a person's own and from a
wake's report, which is what decides how far a run building otto may trust it. **Don't explain here
how to tell.**

```
otto run --instructions test/fixtures/queue-flow.md \
         --goal "Say who filed the item next in otto's queue" \
         --until "artifacts/who.md says who filed it, and the run is done"
```

## The work

1. Run `otto feedback next`, and read what it says about the item. Don't build it.
2. Write one line to `artifacts/who.md`, and nothing else: `person` if the person using otto filed
   it themselves, `wake-report` if a wake reported friction it hit, or `run: <the run's name>` if a
   run passed it along.
3. Then the run is done.
