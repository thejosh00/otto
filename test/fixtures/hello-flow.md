# hello-flow — the engine smoke test

Two steps and one gate, deliberately doing no real work. Its job is to prove otto's machinery
end to end: a run directory, a wake that orients from disk, a gate round-trip through
`otto answer`, a verbatim answer, a handoff, and a terminal status.

Wrap it as an instructions file:

```
otto run --instructions test/fixtures/hello-flow.md \
         --goal "Smoke-test otto's machinery: greet, gate, farewell." \
         --until "artifacts/farewell.md exists and the run is done" \
         --watch
```

**Do not add work to this.** If it needs a repo, a network call or a subagent, it has stopped
being a test of the engine and become a test of something else.

## Step 1 — `greet`

Write `artifacts/greeting.md` containing:

- this run's id,
- the goal, copied from `run.json`,
- how many wakes have happened so far (count `wake-started` in the journal),
- the current timestamp.

Then set the phase to `greet`, write the handoff, and **open a gate** asking whether the
greeting is approved. Use exactly `--slug greet-approval`, so a test can name it. Stop there.

The gate question must be answerable by someone who has never seen this run:

> Run `<id>` (hello-flow, the engine smoke test) wrote `artifacts/greeting.md`. Nothing
> outward-facing has happened and nothing will — this only exercises otto's machinery.
>
> - **Approve** *(default)* — record it and finish the run.
> - **Revise** — say what to change; `greet` runs again with your note.
> - **Abandon** — the run ends, with your reason in the journal.

*Already done?* If `artifacts/greeting.md` exists, adopt it — do not rewrite it. Note in the
journal that it was adopted and go straight to the gate.

## Step 2 — `farewell`

Only after the gate is closed with an approval.

Write `artifacts/farewell.md` quoting the answer **verbatim** from the gate file, along with
when it was answered. Then set the phase to `farewell`, write the handoff, and set the status
to `done`. Report the run directory path and stop.

**If the answer was Revise:** delete `artifacts/greeting.md` (so step 1's adopt-check does not
pick up the rejected one), set the phase back to `greet` with the attempt count incremented,
record the note, and open no gate — just arm a short timer so the next wake re-runs `greet`
with the note in hand. Past `maxAttempts`, set `blocked` and open a gate instead of looping.

**If the answer was Abandon:** set the status to `failed` with the reason, and stop.

## What a green run looks like

The journal, in order: `run-created`, `wake-started`, `gate-opened` (001), `gate-closed` (001,
carrying the answer verbatim), `wake-complete`, `wake-started`, `status-changed` (→ done),
`wake-complete`.

`run.json`: `status: done`, `gate: null`, `nextWakeAt: null`. Both artifacts present. The
handoff rewritten by each wake and under its cap.
