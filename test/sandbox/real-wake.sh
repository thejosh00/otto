#!/bin/bash
# One real wake and one real chat under the sandbox wrapper, on a throwaway OTTO_HOME and a private tmux
# server, with the real HOME and sign-in. Spends a little of the subscription. Prints what happened, and
# keeps the wake's probes and journal in the directory you give it. No real run is touched.
#
#   test/sandbox/real-wake.sh <path to otto> <evidence dir>
set -u
OTTO=${1:?path to otto}; OUT=${2:?evidence dir}; HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd -P)
unset TMUX TMUX_PANE OTTO_RUN_ID OTTO_RUN_DIR OTTO_CHAT CLAUDECODE CLAUDE_CODE_ENTRYPOINT CLAUDE_CODE_SESSION_ID ANTHROPIC_API_KEY
# Where live tests' scratch goes. Not a temporary directory, which every wake can write, so that what the
# record grants is what's being tested; not the workspace, which is closed, as $OTTO_HOME never is on a real machine.
mkdir -p "$HOME/.work-trees"; S=$(mktemp -d "$HOME/.work-trees/otto-scratch-sandbox.XXXXXX"); S=$(cd "$S" && pwd -P)
trap 'env -u TMUX TMUX_TMPDIR="$S/t" tmux kill-server 2>/dev/null; rm -rf "$S"' EXIT
mkdir -p "$S/t" "$S/bin"
# A copy, as the one it was given may well be in a project the sandbox closes. On the PATH for the same reason.
cp "$OTTO" "$S/bin/otto"; OTTO=$S/bin/otto; export PATH="$S/bin:$PATH"
export OTTO_HOME=$S/home TMUX_TMPDIR=$S/t OTTO_NOTIFY=off OTTO_BIN=$OTTO GH_TOKEN=planted-value GITHUB_TOKEN=planted-value
REPO=$S/repo
git init -q -b main "$REPO"; git -C "$REPO" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
mkdir -p "$OTTO_HOME"
HOME_REAL=$HOME
# otto install, with its skills going to a scratch HOME: all that's wanted from it is the wrapper.
mkdir -p "$S/skills-home"; HOME=$S/skills-home "$OTTO" install --repo "$(cd "$HERE/../.." && pwd)" >/dev/null || { echo "otto install failed"; exit 1; }
HOME=$HOME_REAL
[ -x "$OTTO_HOME/sandbox/wrapper" ] || { echo "no wrapper"; exit 1; }
cat >"$OTTO_HOME/config.json" <<JSON
{"workdir":"$HOME/workspace","projects":{"$HOME/workspace/otto":"direct"},
 "launchers":[{"name":"sandbox","command":"env SANDBOX_KEPT=kept REPO=$REPO $OTTO_HOME/sandbox/wrapper -- claude","grantFlag":"--allow"},{"name":"stub","command":"/usr/bin/true"}]}
JSON
"$OTTO" config project "$REPO" --process direct >/dev/null
"$OTTO" run --goal other --name other --launcher stub --perpetual --period 1h --detach none --workdir "$REPO" >/dev/null 2>&1
ids() { "$OTTO" state list --json | python3 -c "import json,sys;print(next(r['id'] for r in json.load(sys.stdin) if r['name']=='$1'))"; }
OTHER=$(ids other)
cp "$HERE/probes.sh" "$REPO/.probes.sh"; git -C "$REPO" add .probes.sh; git -C "$REPO" -c user.name=t -c user.email=t@t commit -q -m probes
GOAL="Do these in order, then stop. (0) Write the names of up to three skills you have available, one per line and nothing else, into $REPO/.skills.txt. (1) Run: bash $REPO/.probes.sh $OTTO_HOME $OTHER > $REPO/.probes.txt 2>&1, where the output is meant to go, as this is a sandbox and little else is writable. Do not read anything it would have refused; don't try to get round a refusal. (2) In $REPO: take the repo lock, make a worktree at $REPO/.wt on a new branch sandbox-try, unlock, add a file hello.txt there saying hello, commit it (use -c user.name=t -c user.email=t@t). (3) Submit it as a change with --no-evidence 'a sandbox test'. (4) Write the handoff, and set the status to done with the reason that the steps ran."
# The record has to be there before the first wake, and an id is only given out at the start: the next is the other's + 1.
NEXT=$((OTHER + 1))
python3 - "$OTTO_HOME/sandbox/runs/$NEXT.json" "$HOME" "$REPO" <<'PY'
import json,sys
json.dump({"launcher":"sandbox","workdir":sys.argv[2]+"/workspace","write":[sys.argv[3]]},open(sys.argv[1],"w"))
PY
echo "== starting the wake, id $NEXT expected"
"$OTTO" run --goal "$GOAL" --name real --launcher sandbox --perpetual --period 1h --detach tmux --repo "$REPO" --workdir "$HOME/workspace" >/dev/null 2>"$OUT/run-stderr.txt"
ID=$(ids real); [ "$ID" = "$NEXT" ] || { echo "id was $ID"; exit 1; }
J=$OTTO_HOME/runs/$ID/journal.jsonl
for _ in $(seq 1 120); do grep -q '"wake-complete"\|"wake-incomplete"' "$J" 2>/dev/null && break; /bin/sleep 5; done
echo "== the wake's journal"; sed -E 's/"(message|note)":"[^"]{200,}"/"\1":"…"/' "$J" | cut -c1-260 | tee "$OUT/journal.txt"
echo "== the worktree and branch"; git -C "$REPO" log --oneline -1 sandbox-try; git -C "$REPO" show --stat --format=%s sandbox-try | tail -2
echo "== the change"; "$OTTO" changes --all 2>&1 | head -4 | cut -c1-120
echo "== handoff written?"; ls -la "$OTTO_HOME/runs/$ID/handoff.md" | cut -c30-
echo "== the profile's terminal devices: /dev/tty, new ptys, and the wake's own terminal by name if it has one"; grep -h 'dev/tty\|ptmx' "$OTTO_HOME"/sandbox/profiles/"$ID".*.sb | sort -u
echo "== the probes"; cp "$REPO/.probes.txt" "$OUT/probes.txt"; cat "$OUT/probes.txt"
echo "== a chat, under the same profile"
"$OTTO" chat "$ID" "Reply with the single word pong, and run nothing." </dev/null 2>&1 | tail -6 | cut -c1-200
echo "== times the wrapper refused, which should be 0: $(grep -c sandbox-refused "$J")"
# The run's own Claude folder (CLAUDE_CONFIG_DIR): names and sizes only.
C=$OTTO_HOME/sandbox/claude/$ID
echo "== the run's own Claude folder: $(ls -A "$C" 2>/dev/null | tr '\n' ' ')"
echo "links: $(for n in settings.json CLAUDE.md skills commands agents plugins hooks; do [ -L "$C/$n" ] && printf '%s ' "$n"; done)"
echo "shell snapshots left there after it exited, in bytes: $(ls -l "$C/shell-snapshots" 2>/dev/null | awk 'NR>1{printf "%s ", $5}')"
# claude removes its snapshot as it exits: the probes measured it while the wake was running.
BIGGEST=$(sed -n 's/^its shell snapshot, largest, in bytes *//p' "$OUT/probes.txt"); BIGGEST=${BIGGEST:-0}
SKILLS=$(cat "$REPO/.skills.txt" 2>/dev/null | tr -d '\r' | sed 's/^[-* ]*//; s/`//g' | grep -v '^$')
echo "skills the wake named: $(echo "$SKILLS" | tr '\n' ' ')"
KNOWN=0; for n in $SKILLS; do [ -e "$HOME/.claude/skills/$n" ] && KNOWN=$((KNOWN + 1)); done
SESSIONS=$(ls "$C"/projects/*/*.jsonl 2>/dev/null | wc -l | tr -d ' ')
echo "transcripts there: $SESSIONS"
ok() { if [ "$2" = yes ]; then echo "PASS $1"; else echo "FAIL $1"; FAILED=1; fi; }
FAILED=0
echo "== checks"
ok "the wake completed" "$(grep -q '"wake-complete"' "$J" && echo yes || echo no)"
ok "its shell snapshot was full size in its own folder while it ran (largest $BIGGEST bytes, more than 1000)" "$([ "$BIGGEST" -gt 1000 ] && echo yes || echo no)"
ok "it found a skill of the person's, through the link ($KNOWN named of those in ~/.claude/skills)" "$([ "$KNOWN" -gt 0 ] && echo yes || echo no)"
ok "its transcripts landed in its own folder" "$([ "$SESSIONS" -gt 0 ] && echo yes || echo no)"
ok "no credentials file in its own folder" "$([ -e "$C/.credentials.json" ] && echo no || echo yes)"
ok "every write probe of ~/.claude and the links was refused" "$(sed -n '/^== writing/,/^== reading/p' "$OUT/probes.txt" | grep -q 'OPEN$' && echo no || echo yes)"
# A wake has no terminal (stdin /dev/null, output on pipes), so its profile names none; a chat in a pane has one, and it's named.
NAMED=$(grep -ho '(literal "/dev/ttys[0-9]*")' "$OTTO_HOME"/sandbox/profiles/"$ID".*.sb | sort -u | wc -l | tr -d ' ')
FOUND=$(sed -n 's/^its own terminal, found *//p' "$OUT/probes.txt")
ok "its profile names at most one terminal ($NAMED), and its own, if found ($FOUND), opened" "$( { [ "$NAMED" -le 1 ] && { [ "$FOUND" = none ] || grep -q '^its own terminal, /dev/ttys[0-9]* *(append, no bytes) *exit 0$' "$OUT/probes.txt"; }; } && echo yes || echo no)"
ok "it could still make new ptys" "$([ "$(grep -c '^a new pty.*exit 0$' "$OUT/probes.txt")" = 2 ] && echo yes || echo no)"
ok "the clipboard read nothing" "$(grep '^pbpaste' "$OUT/probes.txt" | grep -qv ' 0 bytes$' && echo no || echo yes)"
ok "the wrapper refused nothing" "$([ "$(grep -c sandbox-refused "$J")" = 0 ] && echo yes || echo no)"
exit $FAILED
