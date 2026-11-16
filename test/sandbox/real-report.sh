#!/bin/bash
# `otto sandbox report` against the real log: two scratch runs put under observation, a shell under one's
# generated profile that lists ~/.ssh and writes outside its run, and what the report says for each; then the
# same run enforced. A throwaway OTTO_HOME, the real HOME and sign-in (the wrapper wants one), no model, and
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
ok() { if [ "$2" = yes ]; then echo "PASS $1"; else echo "FAIL $1"; FAILED=1; fi; }
yes_if() { if "$@" >/dev/null 2>&1; then echo yes; else echo no; fi; }
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
echo "== $(grep -c '^PASS' "$OUT/transcript.txt") passed, $(grep -c '^FAIL' "$OUT/transcript.txt") failed"
exit $FAILED
