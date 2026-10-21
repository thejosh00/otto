#!/bin/sh
# Run the live tests side by side. Each spends real money and takes minutes, almost all of it the
# model's own turns, but none needs another, so together they take as long as the slowest one
# rather than the sum of them.
#
#   test/live.sh                       # the routine set, ~5 minutes
#   test/live.sh live_gate live_todo   # just these
#   test/live.sh live_devflow          # dev-flow, both ways, ~10 minutes
#
# live_revive pokes every run there is, so it runs after the rest, on its own. Its launchd half
# (live_revive_launchd) needs the installed agent and is run by hand.

set -u
cd "$(dirname "$0")/.." || exit 1

tests=${*:-live_gate live_todo live_message live_note live_chat live_feedback live_background live_wait live_outputs live_verify live_changes live_revive}
logs=$(mktemp -d "${TMPDIR:-/tmp}/otto-live.XXXXXX")

# Build once, then run the test binaries themselves. Not one `cargo test` each: every cargo that
# starts refreshes target/debug/otto, and one doing so while another's test was already starting
# wakes left that test with no otto to run.
built=$(cargo test --tests --no-run 2>&1) || { echo "$built"; exit 1; }

run() { # <test file> [test filter...]
    name=$1
    shift
    bin=$(echo "$built" | sed -n "s|^ *Executable tests/$name\.rs (\(.*\))\$|\1|p")
    start=$(date +%s)
    if [ -n "$bin" ]; then
        "$bin" --ignored --nocapture "$@" >"$logs/$name.log" 2>&1
    else
        echo "no such live test: tests/$name.rs" >"$logs/$name.log"
        false
    fi
    echo "$? $(($(date +%s) - start))" >"$logs/$name.result"
}

echo "running: $tests"
for t in $tests; do
    [ "$t" = live_revive ] || run "$t" &
done
wait
case " $tests " in *" live_revive "*) run live_revive --exact live_revive ;; esac

failed=0
for t in $tests; do
    code=1 secs=?
    [ -f "$logs/$t.result" ] && read -r code secs <"$logs/$t.result"
    said=$(grep -E 'passed, [0-9]+ failed' "$logs/$t.log" 2>/dev/null | tail -1)
    [ -n "$said" ] || said=$(grep -m1 -E 'panicked|FAIL|no such' "$logs/$t.log" 2>/dev/null)
    if [ "$code" = 0 ]; then mark=ok; else mark=FAIL failed=1; fi
    printf '%-4s %-13s %5ss  %s\n' "$mark" "$t" "$secs" "$said"
done
echo "logs: $logs"
exit $failed
