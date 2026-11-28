# otto

README.md (Development) and ARCHITECTURE.md say how otto is built and tested; DESIGN.md says why.

## Running the tests

- **The whole suite: `cargo nextest run`.** About 95s, against about 5 minutes for `cargo test`,
  which runs the 54 test binaries one after another while the unit tests queue on
  `paths::ENV_MUTEX`. nextest runs every test in its own process, all at once. If `cargo nextest`
  isn't installed (`brew install cargo-nextest`), `cargo test` runs the same tests.
- **While working, only what the change touches:** `cargo nextest run -E 'test(chat::)'` for a
  module's unit tests, `cargo nextest run --test landing` for one integration file.
- **The whole suite before a change goes back or is pushed.** No subset is safe for that: every
  file in `tests/` drives the real `otto` binary, so a change anywhere in `src/` can break any of them.
- `OTTO_UPDATE_DOCS=1 cargo test cli_reference` regenerates `docs/reference/cli.md` when a
  command's help text changes.
- Live tests (`tests/live_*.rs`, ignored by default) cost real tokens: run them with `test/live.sh`,
  never by un-ignoring them.
