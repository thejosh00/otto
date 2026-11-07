# Ideas

Ideas to consider for v4. Nothing here is decided or scheduled. Commands and fields marked
*(proposed)* don't exist yet. This file lives outside `docs/` for the same reason as `v3.md`:
`src/doc_examples.rs` runs every `otto` command in `docs/`.

## Checked sequence diagrams on changes

**From:** a trial of [PR Lens](https://github.com/coldteadotai/pr-lens) on three landed otto-build
changes, 2026-11-07, against `4363745`. [The diagrams and notes](https://claude.ai/artifact/GoPtPt1uD5nnco1uhuf7Ug)
are on a private page.

A change can carry a small picture of the order things now happen in, and otto checks each step
against the code.

### What the trial showed

| Change | Size | Sequence diagram | Architecture overview |
|---|---|---|---|
| 12/11 Poke never kills a finished wake | +21 −1 | Very useful | Clear |
| 12/16 A kill says who held the lock | +547 −12 | Useful | Tangled at 15 cards, readable at 9 |
| 12/20 `otto why` | +1120 −2 | Correct, adds little | Tangled at both 19 and 11 cards |

- Sequence diagrams help most when a change is about order. In 12/11, a 21-line diff hides a race
  that the diagram makes plain.
- A diagram can pass validation and still be wrong. PR Lens checks structure, and both of these
  mistakes passed:
  - 12/16 put the holder comparison before the kill. The code does it after.
  - 12/20 had `account` printing the result. `why()` does that.
- PR Lens itself doesn't fit otto well:
  - Changes land on main with no pull request, so the GitHub App and Action have nothing to run on.
  - otto's evidence refuses SVG because "an svg can carry script" (`src/changes.rs:3176`).
  - Its instructions are about 74 KB, which is a lot to add to a wake.

These ratings are one reviewer's judgement of still screenshots, on three changes drawn by hand
rather than by a wake.

### How it would work

A wake adds a small file when it submits a change where order matters *(proposed)*:

```json
{
  "actors": {
    "wake": "src/wake/mod.rs:213-300",
    "decide": "src/poke.rs:93-160"
  },
  "steps": [
    { "from": "wake", "to": "wake.lock", "says": "take the lock", "at": "src/wake/mod.rs:219" },
    { "from": "decide", "to": "kill_wake", "says": "kill: this hit the new wake", "was": true },
    { "from": "decide", "to": "skip", "says": "skip until next pass", "new": true, "at": "src/poke.rs:125" }
  ]
}
```

Otto then does three things:

- **Checks it.** The cited lines must exist at that commit, and a step marked `new` must cite a line
  the change touched. Each step's line must sit inside its sender, and a sender's steps must follow
  line order.
- **Draws it.** The web page draws the diagram from this data, so it never accepts an uploaded SVG.
- **Shows it in the terminal.** `otto changes show` prints the same steps as a plain list.

These checks would be warnings on the change, not blocks, because loops, callbacks and threads can
legitimately break line order. They catch steps that disagree with the code. They can't prove a
diagram is right.

On paper, they catch both mistakes above: the 12/16 steps would be out of line order, and the
`print!` in 12/20 is outside `account`'s line range. This hasn't been run.

### What to leave out

- Architecture overviews, which were tangled on two of the three changes.
- Animation, a canvas and sample payloads.
- A requirement on every change. 12/20 shows many changes don't benefit. A short note in
  `harness/wake.md` could say when to draw one.

### The same drawing for wakes

`otto why` already puts the journal, poke's lines and check results in one order. The same drawing
code could show that timeline as a sequence across poke, the check and the wake. It would come from
what otto recorded, so it needs no model time. This would come second, because it reuses the drawing
code.

### How to test it

1. Write these files for 12/11, 12/16 and 12/20, including the two original mistakes, and confirm
   otto flags both. If it doesn't, the idea loses its main advantage, so stop there.
2. Add the file format, checks and web drawing, then have otto-build use it on its next 10 or so
   changes where order matters.
3. Keep it only if the diagrams change how the change is reviewed. Track how often the warnings find
   real mistakes versus false alarms.

### Open questions

- Does this overlap with the change-witness work (`architect/change-witness-M1-otto`)? Not read yet.
  The name suggests independent checking of changes, so this may belong inside it.
- How big is it? Not sized against the code.
- How well does a wake draw these, and what does it cost? The trial's diagrams weren't drawn by a wake.
- Should it live with evidence? Evidence proves a change works and a diagram only explains it, so it
  may need its own slot.
