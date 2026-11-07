#!/bin/bash
# A run edited past its sandbox record, against the real binary, a scratch OTTO_HOME and a private tmux server,
# with a stub for the model, so it spends nothing. Each edit a wake could make: what `otto wake` and `otto chat`
# say, what the journal says, what poke does. Then an older binary reading the result, and the way back.
#
#   test/sandbox/refusal.sh <path to otto> <path to an older otto>
set -u
OTTO=${1:?path to otto}; OLD=${2:?path to an older otto}
unset TMUX TMUX_PANE OTTO_RUN_ID OTTO_RUN_DIR OTTO_CHAT
S=$(mktemp -d "$HOME/.work-trees/otto-scratch-refusal.XXXXXX"); S=$(cd "$S" && pwd -P)
trap 'env -u TMUX TMUX_TMPDIR="$S/t" tmux kill-server 2>/dev/null; rm -rf "$S"' EXIT
mkdir -p "$S/t" "$S/home/sandbox/runs" "$S/work/repo" "$S/work/other"
export OTTO_HOME=$S/home TMUX_TMPDIR=$S/t
cat >"$S/stub" <<STUB
#!/bin/sh
echo "\$OTTO_RUN_ID\$OTTO_CHAT" >> "$S/started"
STUB
chmod +x "$S/stub"
cat >"$OTTO_HOME/config.json" <<JSON
{"launchers":[{"name":"under","command":"$S/stub"},{"name":"claude","command":"$S/stub"}]}
JSON
"$OTTO" run --goal g --name r --launcher under --perpetual --period 1h --detach tmux --no-wake --repo "$S/work/repo" --workdir "$S/work" >/dev/null 2>&1 \
  || "$OTTO" run --goal g --name r --launcher under --perpetual --period 1h --detach tmux --repo "$S/work/repo" --workdir "$S/work" >/dev/null
ID=$("$OTTO" state list --json | python3 -c "import json,sys;print(next(r['id'] for r in json.load(sys.stdin) if r['name']=='r'))")
RUN=$OTTO_HOME/runs/$ID/run.json
REC=$OTTO_HOME/sandbox/runs/$ID.json
echo "{\"launcher\":\"under\",\"workdir\":\"$S/work\",\"write\":[\"$S/work/repo\"]}" >"$REC"
ORIGINAL=$(python3 -c "import json;print(json.dumps(json.load(open('$RUN'))['launcher']))")
set_launcher() { python3 - "$RUN" "$1" <<'PY'
import json,sys
r=json.load(open(sys.argv[1])); r['launcher'].update(json.loads(sys.argv[2])); json.dump(r,open(sys.argv[1],'w'))
PY
}
field() { python3 -c "import json;r=json.load(open('$RUN'));print($1)"; }
started() { [ -f "$S/started" ] && wc -l <"$S/started" | tr -d ' ' || echo 0; }
quiet() { sed -E "s#$S#<scratch>#g" | cut -c1-240; }

for edit in '{"kind":"claude"}' "{\"repos\":[\"$S/work/repo\",\"$HOME/.ssh\"]}" "{\"workdir\":\"$S/work/other\"}"; do
  echo "== run.json's launcher edited: $(echo "$edit" | quiet)"
  set_launcher "$edit"
  "$OTTO" wake "$ID" 2>&1 | quiet; echo "otto wake exit ${PIPESTATUS[0]}"
  "$OTTO" chat "$ID" hello </dev/null 2>&1 | quiet; echo "otto chat exit ${PIPESTATUS[0]}"
  echo "commands started: $(started)"
  echo "journal: $(grep '"event":"sandbox-refused"' "$OTTO_HOME/runs/$ID/journal.jsonl" | tail -1 | quiet)"
  "$OTTO" poke --verbose --grace 0 2>&1 | grep -F "$ID" | quiet
  echo "after poke: status $(field "r['status']"), gate $(field "r['gate']['slug']"), spawnAttempts $(field "r['spawnAttempts']"), started $(started)"
  "$OTTO" ls 2>&1 | grep -A1 -E "^$ID " | quiet
  "$OTTO" show "$ID" 2>&1 | grep -m1 -i "blocked" | quiet
  set_launcher "$ORIGINAL"
  echo "-- launcher put back, otto wake --detach none"
  "$OTTO" wake "$ID" --detach none 2>&1 | quiet | tail -2
  echo "status $(field "r['status']"), gate $(field "r['gate'] and r['gate']['slug']"), started $(started)"
  echo
  python3 - "$RUN" <<'PY'
import json,sys
r=json.load(open(sys.argv[1])); r['status']='sleeping'; r['nextWakeAt']='2036-01-01T00:00:00Z'; json.dump(r,open(sys.argv[1],'w'))
PY
done

echo "== an older binary, $("$OLD" --version), reads a refused run and a home with records"
set_launcher '{"kind":"claude"}'
"$OTTO" wake "$ID" >/dev/null 2>&1
"$OLD" ls 2>&1 | grep -A1 -E "^$ID " | quiet; echo "otto ls exit ${PIPESTATUS[0]}"
"$OLD" show "$ID" 2>&1 | grep -m1 -i "blocked" | quiet
"$OLD" logs "$ID" 2>&1 | grep -c sandbox | sed 's/^/otto logs lines naming the sandbox: /'
"$OLD" why "$ID" 2>&1 | quiet | head -4; echo "otto why exit ${PIPESTATUS[0]}"
"$OLD" state get "$ID" --field blocked 2>&1 | quiet; echo "state get exit ${PIPESTATUS[0]}"
"$OLD" poke --verbose --grace 0 2>&1 | grep -F "$ID" | quiet

echo; echo "== the way out: answer the question while they still differ, then for real"
"$OTTO" answer "$ID" --choice Fixed 2>&1 | quiet | tail -3
sleep 1
for _ in 1 2 3 4 5 6 7 8 9 10; do [ "$(field "r['gate'] is not None")" = True ] && break; sleep 2; done
echo "still different: status $(field "r['status']"), gate $(field "r['gate'] and r['gate']['slug']"), started $(started)"
set_launcher "$ORIGINAL"; rm "$REC"; set_launcher '{"kind":"under"}'
echo "record deleted, launcher restored"
"$OTTO" wake "$ID" --detach none 2>&1 | quiet | tail -2
echo "status $(field "r['status']"), gate $(field "r['gate'] and r['gate']['slug']"), started $(started)"
