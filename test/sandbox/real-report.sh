#!/bin/bash
# `otto sandbox report` against the real log: two scratch runs put under observation, a shell under one's
# generated profile that lists ~/.ssh and writes outside its run, and what the report says for each; then the
# same run enforced. A second scratch otto home holds a run with the same id as the first's: neither home's
# report, nor the real home's, shows the other's. Then a window that ends before it starts, and `observe` and
# `enforce` from inside the enforced profile. A throwaway OTTO_HOME, the real HOME and sign-in (the wrapper wants one), no model, and
# no real run touched. Lists ~/.ssh and never prints what's in it.
#
#   test/sandbox/real-report.sh <path to otto> <evidence dir>
set -u
OTTO=${1:?path to otto}; OUT=${2:?evidence dir}; HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd -P)
exec > >(tee "$OUT/transcript.txt") 2>&1
unset TMUX TMUX_PANE OTTO_RUN_ID OTTO_RUN_DIR OTTO_CHAT OTTO_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT CLAUDE_CODE_SESSION_ID ANTHROPIC_API_KEY
# Not a temporary directory, which every wake can write: what the record doesn't list is what's being tested.
mkdir -p "$HOME/.work-trees"; S=$(mktemp -d "$HOME/.work-trees/otto-scratch-report.XXXXXX"); S=$(cd "$S" && pwd -P)
trap 'rm -rf "$S"' EXIT
mkdir -p "$S/bin" "$S/work-a" "$S/work-b" "$S/outside" "$S/skills-home"
cp "$OTTO" "$S/bin/otto"; OTTO=$S/bin/otto; export PATH="$S/bin:$PATH"
export OTTO_HOME=$S/home OTTO_NOTIFY=off OTTO_BIN=$OTTO
mkdir -p "$OTTO_HOME"
HOME_REAL=$HOME
HOME=$S/skills-home "$OTTO" install --repo "$(cd "$HERE/../.." && pwd)" >/dev/null || { echo "otto install failed"; exit 1; }
HOME=$HOME_REAL
[ -x "$OTTO_HOME/sandbox/wrapper" ] || { echo "no wrapper"; exit 1; }
printf '{"launchers":[{"name":"claude","command":"claude"}]}\n' >"$OTTO_HOME/config.json"
ids() { "$OTTO" state list --json | python3 -c "import json,sys;print(next(r['id'] for r in json.load(sys.stdin) if r['name']=='$1'))"; }
"$OTTO" run --goal a --name report-a --launcher claude --perpetual --period 1h --detach none --workdir "$S/work-a" >/dev/null 2>&1
"$OTTO" run --goal b --name report-b --launcher claude --perpetual --period 1h --detach none --workdir "$S/work-b" >/dev/null 2>&1
A=$(ids report-a); B=$(ids report-b)
echo "== scratch runs: A is $A, B is $B"
FAILED=0
# A second otto home, whose first run gets the same id as A: `in2 <command>` runs a command as that home.
HOME1=$OTTO_HOME; HOME2=$S/home2
in2() { ( export OTTO_HOME=$HOME2; "$@" ); }
mkdir -p "$HOME2" "$S/work-c"
HOME=$S/skills-home OTTO_HOME=$HOME2 "$OTTO" install --repo "$(cd "$HERE/../.." && pwd)" >/dev/null || { echo "otto install failed for the second home"; exit 1; }
HOME=$HOME_REAL
printf '{"launchers":[{"name":"claude","command":"claude"}]}\n' >"$HOME2/config.json"
in2 "$OTTO" run --goal c --name report-a --launcher claude --perpetual --period 1h --detach none --workdir "$S/work-c" >/dev/null 2>&1
A2=$(in2 ids report-a)
echo "== second otto home: its run is $A2"
ok() { if [ "$2" = yes ]; then echo "PASS $1"; else echo "FAIL $1"; FAILED=1; fi; }
yes_if() { if "$@" >/dev/null 2>&1; then echo yes; else echo no; fi; }
no_if() { if "$@" >/dev/null 2>&1; then echo no; else echo yes; fi; }
ok "the second home's run has the same id as A ($A2 and $A)" "$([ "$A2" = "$A" ] && echo yes || echo no)"
# The real otto home's report for that id, which only reads. It may have no such run: then it shows nothing either.
real_report() { env -u OTTO_HOME "$OTTO" sandbox report "$A" --since 10m >"$1" 2>&1; }
# A shell under a run's generated profile, the way a wake gets one: through the wrapper, as that run.
SESSION=0a0a0a0a-0000-4000-8000-00000000000
under() { OTTO_RUN_ID=$1 OTTO_SESSION_ID=$SESSION$2 "$OTTO" sandbox wrap -- /bin/sh -c "$3"; }
# The same two things each time: list ~/.ssh (names thrown away), and make a file the record doesn't list.
ACTS='ls "$HOME/.ssh" >/dev/null 2>&1; echo "ls ~/.ssh: exit $?"; echo x 2>/dev/null >"$1/made-$2"; echo "write outside: exit $?"'
acts() { under "$1" "$2" "set -- '$S/outside' '$3'; $ACTS"; }
# The log takes a few seconds to catch up: ask until the report has what's wanted, or 15 seconds have gone.
report_until() { # <run> <file> <pattern>
  for _ in 1 2 3 4 5 6 7 8; do
    "$OTTO" sandbox report "$1" >"$2" 2>&1; grep -q "$3" "$2" && return 0; /bin/sleep 2
  done; return 1
}

echo "== (c) both observed"
"$OTTO" sandbox observe "$A" >"$OUT/observe-a.txt" 2>&1; ok "otto sandbox observe A" "$([ $? = 0 ] && echo yes || echo no)"
"$OTTO" sandbox observe "$B" >"$OUT/observe-b.txt" 2>&1; ok "otto sandbox observe B" "$([ $? = 0 ] && echo yes || echo no)"
in2 "$OTTO" sandbox observe "$A2" >"$OUT/observe-home2.txt" 2>&1; ok "otto sandbox observe, second home" "$([ $? = 0 ] && echo yes || echo no)"
cp "$OTTO_HOME/sandbox/runs/$A.json" "$OUT/record-a-observed.json"
/bin/sleep 1
acts "$A" 1 observed 2>&1 | tee "$OUT/observed-shell.txt"
under "$B" 2 'true' >"$OUT/observed-shell-b.txt" 2>&1
ok "observed: listing ~/.ssh was not blocked" "$(yes_if grep -q '^ls ~/.ssh: exit 0$' "$OUT/observed-shell.txt")"
ok "observed: the write outside the run was not blocked" "$([ -e "$S/outside/made-observed" ] && echo yes || echo no)"
report_until "$A" "$OUT/report-a-observed.txt" "outside/made-observed"
echo "-- otto sandbox report $A"; cat "$OUT/report-a-observed.txt"
"$OTTO" sandbox report "$B" >"$OUT/report-b-observed.txt" 2>&1
echo "-- otto sandbox report $B"; cat "$OUT/report-b-observed.txt"
WOULD=$(sed -n '/^Would be denied/,/^$/p' "$OUT/report-a-observed.txt")
ok "report A names ~/.ssh as would be denied, with a count" "$(echo "$WOULD" | grep -Eq "^ +[0-9]+×.* $HOME/\.ssh" && echo yes || echo no)"
ok "report A names the write outside as would be denied, with a count" "$(echo "$WOULD" | grep -Eq "^ +[0-9]+×.*file-write.* $S/outside/" && echo yes || echo no)"
LINES=$(wc -l <"$OUT/report-a-observed.txt" | tr -d ' ')
ok "report A is a few lines ($LINES, fewer than 40)" "$([ "$LINES" -lt 40 ] && echo yes || echo no)"
ok "report B shows none of A's" "$(grep -q "\.ssh\|outside/made" "$OUT/report-b-observed.txt" && echo no || echo yes)"
ok "report A says it guides and proves nothing" "$(yes_if grep -q 'guides a decision and proves nothing' "$OUT/report-a-observed.txt")"
ok "report B says it guides and proves nothing" "$(yes_if grep -q 'guides a decision and proves nothing' "$OUT/report-b-observed.txt")"
ok "the record is as observe wrote it after the reports" "$(yes_if cmp -s "$OTTO_HOME/sandbox/runs/$A.json" "$OUT/record-a-observed.json")"

echo "== (c2) the second home's run $A2, observed, and the real home"
in2 "$OTTO" sandbox report "$A2" >"$OUT/report-home2-before.txt" 2>&1
echo "-- OTTO_HOME=<home 2> otto sandbox report $A2 (before it did anything)"; cat "$OUT/report-home2-before.txt"
ok "home 2's report shows neither ~/.ssh nor the write, which were home 1's" "$(no_if grep -q "\.ssh\|outside/made" "$OUT/report-home2-before.txt")"
in2 acts "$A2" 4 home2-observed 2>&1 | tee "$OUT/observed-shell-home2.txt"
in2 report_until "$A2" "$OUT/report-home2-observed.txt" "outside/made-home2-observed"
echo "-- OTTO_HOME=<home 2> otto sandbox report $A2"; cat "$OUT/report-home2-observed.txt"
WOULD2=$(sed -n '/^Would be denied/,/^$/p' "$OUT/report-home2-observed.txt")
ok "home 2's report shows its own write as would be denied" "$(echo "$WOULD2" | grep -Eq "^ +[0-9]+×.*file-write.* $S/outside/made-home2-observed" && echo yes || echo no)"
ok "home 2's report shows none of home 1's write" "$(no_if grep -q "outside/made-observed" "$OUT/report-home2-observed.txt")"
"$OTTO" sandbox report "$A" >"$OUT/report-a-observed-after-home2.txt" 2>&1
ok "home 1's report still shows its own write" "$(yes_if grep -q "outside/made-observed" "$OUT/report-a-observed-after-home2.txt")"
ok "home 1's report shows none of home 2's write" "$(no_if grep -q "made-home2" "$OUT/report-a-observed-after-home2.txt")"
real_report "$OUT/report-real-home-observed.txt"
echo "-- otto sandbox report $A --since 10m, in the real otto home"; cat "$OUT/report-real-home-observed.txt"
ok "the real home's report for run $A shows neither scratch home's write" "$(no_if grep -q "outside/made" "$OUT/report-real-home-observed.txt")"

echo "== (d) A enforced"
"$OTTO" sandbox enforce "$A" >"$OUT/enforce-a.txt" 2>&1; ok "otto sandbox enforce A" "$([ $? = 0 ] && echo yes || echo no)"
/bin/sleep 1
acts "$A" 3 enforced 2>&1 | tee "$OUT/enforced-shell.txt"
ok "enforced: listing ~/.ssh was blocked" "$(grep -q '^ls ~/.ssh: exit 0$' "$OUT/enforced-shell.txt" && echo no || echo yes)"
ok "enforced: the write outside the run was blocked" "$([ -e "$S/outside/made-enforced" ] && echo no || echo yes)"
report_until "$A" "$OUT/report-a-enforced.txt" "outside/made-enforced"
echo "-- otto sandbox report $A"; cat "$OUT/report-a-enforced.txt"
DENIED=$(sed -n '/^Denied/,/^$/p' "$OUT/report-a-enforced.txt")
ok "report A shows ~/.ssh as denied" "$(echo "$DENIED" | grep -Eq "^ +[0-9]+×.* $HOME/\.ssh" && echo yes || echo no)"
ok "report A shows the write outside as denied" "$(echo "$DENIED" | grep -Eq "^ +[0-9]+×.*file-write.* $S/outside/made-enforced" && echo yes || echo no)"
LINES=$(wc -l <"$OUT/report-a-enforced.txt" | tr -d ' ')
ok "report A is still a few lines ($LINES, fewer than 40)" "$([ "$LINES" -lt 40 ] && echo yes || echo no)"
"$OTTO" sandbox report "$A" --json >"$OUT/report-a-enforced.json" 2>/dev/null
ok "the same as JSON parses" "$(yes_if python3 -c 'import json,sys; json.load(open(sys.argv[1]))["findings"]' "$OUT/report-a-enforced.json")"
grep -h sandbox- "$OTTO_HOME/runs/$A/journal.jsonl" | cut -c1-200 >"$OUT/journal-a.txt"
ok "the wrapper refused nothing" "$(grep -q sandbox-refused "$OUT/journal-a.txt" && echo no || echo yes)"

echo "== (d2) the second home's run enforced, and the real home"
in2 "$OTTO" sandbox enforce "$A2" >"$OUT/enforce-home2.txt" 2>&1; ok "otto sandbox enforce, second home" "$([ $? = 0 ] && echo yes || echo no)"
/bin/sleep 1
in2 "$OTTO" sandbox report "$A2" >"$OUT/report-home2-enforced-before.txt" 2>&1
in2 acts "$A2" 6 home2-enforced 2>&1 | tee "$OUT/enforced-shell-home2.txt"
ok "second home, enforced: the write outside the run was blocked" "$([ -e "$S/outside/made-home2-enforced" ] && echo no || echo yes)"
in2 report_until "$A2" "$OUT/report-home2-enforced.txt" "outside/made-home2-enforced"
echo "-- OTTO_HOME=<home 2> otto sandbox report $A2"; cat "$OUT/report-home2-enforced.txt"
DENIED2=$(sed -n '/^Denied/,/^$/p' "$OUT/report-home2-enforced.txt")
ok "home 2's report shows its own write as denied" "$(echo "$DENIED2" | grep -Eq "^ +[0-9]+×.*file-write.* $S/outside/made-home2-enforced" && echo yes || echo no)"
ok "home 2's report shows ~/.ssh as denied" "$(echo "$DENIED2" | grep -Eq "^ +[0-9]+×.* $HOME/\.ssh" && echo yes || echo no)"
ok "home 2's report shows none of home 1's denied write" "$(no_if grep -q "outside/made-enforced" "$OUT/report-home2-enforced.txt")"
"$OTTO" sandbox report "$A" >"$OUT/report-a-enforced-after-home2.txt" 2>&1
ok "home 1's report still shows its own denied write" "$(yes_if grep -q "outside/made-enforced" "$OUT/report-a-enforced-after-home2.txt")"
ok "home 1's report shows none of home 2's denied write" "$(no_if grep -q "made-home2" "$OUT/report-a-enforced-after-home2.txt")"
real_report "$OUT/report-real-home-enforced.txt"
echo "-- otto sandbox report $A --since 10m, in the real otto home"; cat "$OUT/report-real-home-enforced.txt"
ok "the real home's report for run $A shows neither scratch home's denied write" "$(no_if grep -q "outside/made" "$OUT/report-real-home-enforced.txt")"

echo "== (e) a window that would end before it starts"
"$OTTO" sandbox report "$A" --until 1h >"$OUT/report-until-before-start.txt" 2>&1; UNTIL=$?
cat "$OUT/report-until-before-start.txt"
ok "report --until 1h with no --since exits non-zero ($UNTIL)" "$([ $UNTIL != 0 ] && echo yes || echo no)"
ok "and says the window would end before it starts" "$(yes_if grep -q 'window would end before it starts' "$OUT/report-until-before-start.txt")"
ok "and says how to give both" "$(yes_if grep -q -- '--since <when> --until <when>' "$OUT/report-until-before-start.txt")"

echo "== (f) observe and enforce from inside A's enforced profile, OTTO_RUN_ID and OTTO_CHAT cleared"
cp "$OTTO_HOME/sandbox/runs/$A.json" "$OUT/record-a-enforced.json"
SUMS=$(shasum "$OTTO_HOME/config.json" "$OTTO_HOME/runs/$A/run.json")
for WHAT in observe enforce; do
  under "$A" 7 "env -u OTTO_RUN_ID -u OTTO_CHAT '$OTTO' sandbox $WHAT $A; echo \"[exit \$?]\"" >"$OUT/inside-$WHAT.txt" 2>&1
  echo "-- inside the profile: otto sandbox $WHAT $A"; cat "$OUT/inside-$WHAT.txt"
  ok "inside: $WHAT exits non-zero" "$(grep -q '^\[exit [1-9]' "$OUT/inside-$WHAT.txt" && echo yes || echo no)"
  ok "inside: $WHAT says nothing was changed" "$(yes_if grep -q 'Nothing was changed' "$OUT/inside-$WHAT.txt")"
  ok "inside: $WHAT says to run it outside a sandbox" "$(yes_if grep -q 'outside a sandbox' "$OUT/inside-$WHAT.txt")"
  ok "inside: after $WHAT the record is byte for byte the same" "$(yes_if cmp -s "$OTTO_HOME/sandbox/runs/$A.json" "$OUT/record-a-enforced.json")"
done
ok "inside: config.json and run.json are unchanged" "$([ "$SUMS" = "$(shasum "$OTTO_HOME/config.json" "$OTTO_HOME/runs/$A/run.json")" ] && echo yes || echo no)"
grep -h sandbox- "$OTTO_HOME/runs/$A/journal.jsonl" | cut -c1-200 >"$OUT/journal-a-end.txt"
ok "the journal has nothing new from the refused commands" "$(yes_if cmp -s "$OUT/journal-a.txt" "$OUT/journal-a-end.txt")"
echo "== $(grep -c '^PASS' "$OUT/transcript.txt") passed, $(grep -c '^FAIL' "$OUT/transcript.txt") failed"
exit $FAILED
