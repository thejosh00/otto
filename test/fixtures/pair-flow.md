# pair-flow — one change that settles two items in otto's queue

One wake's work. `tests/live_feedback_pair.rs` starts this in a run whose change is already out,
with two items in otto's feedback queue that the one change settles. It measures whether a wake can
tie both items to that change, so **do not say here what to do if otto refuses**.

```
otto run --instructions test/fixtures/pair-flow.md \
         --goal "Mark what this run's change settles as being built" \
         --until "every item the change settles is marked building against it, and the run is done"
```

## The work

This run builds otto from its feedback queue. Its change is out already, waiting for the person:
change 1 of this run (`otto changes`). Don't touch the change or its repository.

1. Read the queue: `otto feedback --all`. The change settles **both** items there: it makes the
   greeting say hello, and it makes the greeting say goodbye. Neither repeats the other.
2. An item whose change is out is marked so, where `<run>` is this run's id:

   ```
   otto feedback building <item> --change <run>/1
   ```

   Mark what the change settles.
3. Write what otto printed each time, exactly, to `artifacts/marked.md`.
4. Then the run is done.
