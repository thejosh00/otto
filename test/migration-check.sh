#!/bin/bash
# test/migration-check.sh — scripts/migration-check.sh against a small fake otto home in a temp dir.
# Records twice and compares (passes), then makes one change at a time on a fresh copy and checks
# the compare fails naming its category, or passes for what is allowed (a journal line appended, a
# launcher added). Never touches the real ~/.otto. Exits non-zero if any expectation fails.
set -u
here=$(cd "$(dirname "$0")/.." && pwd)
check="$here/scripts/migration-check.sh"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
failures=0

# The fake home: two runs, one numbered with everything, one from before numbering.
base="$scratch/base"
make_home() {
	local h=$1
	mkdir -p "$h/runs/1"/{gates,notes,artifacts/sub,outputs/report,changes/1-fix,messages} \
		"$h/runs/2026-09-13-once-an-hour-check"
	echo 1 >"$h/runs/.last-id"
	cat >"$h/runs/1/run.json" <<-'EOF'
	{"schemaVersion": 2, "id": "1", "name": "builder", "goal": "Build things", "doneCondition": null,
	 "status": "sleeping", "nextWakeAt": "2026-11-26T20:00:00Z", "gate": null,
	 "policy": {"periodMinutes": 15},
	 "check": {"script": "check.sh", "wakeAfter": 96, "consecutiveNoChange": 3, "noChangeTotal": 40},
	 "launcher": {"kind": "claude", "repos": []},
	 "notes": [{"id": "001", "file": "notes/001.md", "standing": true, "addedAt": "2026-11-01T00:00:00Z"},
	           {"id": "002", "file": "notes/002.md", "standing": false, "addedAt": "2026-11-02T00:00:00Z"}]}
	EOF
	printf '{"n":1,"kind":"wake-start"}\n{"n":2,"kind":"wake-end"}\n' >"$h/runs/1/journal.jsonl"
	echo "# Gate 001" >"$h/runs/1/gates/001-ask.md"
	echo "always test" >"$h/runs/1/notes/001.md"
	echo "look at item 3" >"$h/runs/1/notes/002.md"
	echo "notes" >"$h/runs/1/artifacts/sub/a.txt"
	echo "handing off" >"$h/runs/1/handoff.md"
	printf '#!/bin/sh\nexit 0\n' >"$h/runs/1/check.sh"
	echo "first" >"$h/runs/1/outputs/report/v1.md"
	echo "second" >"$h/runs/1/outputs/report/v2.md"
	cat >"$h/runs/1/outputs/index.json" <<-'EOF'
	{"outputs": [{"key": "report", "versions": [{"n": 1, "file": "outputs/report/v1.md"}, {"n": 2, "file": "outputs/report/v2.md"}]}]}
	EOF
	echo "diff --git a/x b/x" >"$h/runs/1/changes/1-fix/v1.patch"
	cat >"$h/runs/1/changes/index.json" <<-'EOF'
	{"changes": [{"n": 1, "key": "fix", "repo": "/r", "branch": "b", "base": "main", "state": "waiting",
	  "versions": [{"k": 1, "sha": "abc", "baseSha": "def", "patchId": "p1", "patch": "changes/1-fix/v1.patch"}],
	  "replies": [{"comment": "c1", "text": "thanks", "at": "2026-11-01T00:00:00Z", "state": "draft"}]}]}
	EOF
	echo "hello" >"$h/runs/1/messages/001-hi.md"
	echo '{"id":"001","kind":"update","title":"hi","sentAt":"2026-11-01T00:00:00Z","body":"messages/001-hi.md"}' \
		>"$h/runs/1/messages/001-hi.json"
	cat >"$h/runs/2026-09-13-once-an-hour-check/run.json" <<-'EOF'
	{"schemaVersion": 2, "id": "2026-09-13-once-an-hour-check", "name": "gtd-task-runner", "goal": "Tasks",
	 "doneCondition": "all done", "status": "sleeping", "nextWakeAt": null, "gate": null,
	 "paused": {"at": "2026-11-01T00:00:00Z", "reason": "not started yet — start it"},
	 "policy": {"periodMinutes": 10080, "periodAt": "07:00"}}
	EOF
	echo '{"kind":"start"}' >"$h/runs/2026-09-13-once-an-hour-check/journal.jsonl"
	printf '{"id":1,"text":"one"}\n{"id":2,"text":"two"}\n' >"$h/feedback.jsonl"
	cat >"$h/config.json" <<-'EOF'
	{"workdir": "/w", "lander": "1", "launchers": [{"name": "legenda", "command": "claude"}], "projects": {"/w/otto": "direct"}}
	EOF
}
make_home "$base"
"$check" record "$base" "$scratch/old.txt" || { echo "FAIL: record exited non-zero"; exit 1; }

expect() { # <name> <pass|fail> <category> <mutation...>
	local name=$1 want=$2 category=$3
	shift 3
	local h="$scratch/h-$RANDOM$RANDOM"
	cp -R "$base" "$h"
	(cd "$h" && eval "$*")
	"$check" record "$h" "$scratch/new.txt"
	local out status
	out=$("$check" compare "$scratch/old.txt" "$scratch/new.txt")
	status=$?
	if [ "$want" = pass ] && [ $status -ne 0 ]; then
		echo "FAIL: $name: compare should pass, exited $status"; echo "$out"; failures=$((failures + 1)); return
	fi
	if [ "$want" = fail ]; then
		if [ $status -ne 1 ]; then
			echo "FAIL: $name: compare should fail, exited $status"; echo "$out"; failures=$((failures + 1)); return
		fi
		# The failure is named under "Must not change", by its category.
		if ! echo "$out" | sed -n '/^Must not change/,/^$/p' | grep -q " $category: "; then
			echo "FAIL: $name: no '$category' failure named"; echo "$out"; failures=$((failures + 1)); return
		fi
	fi
	echo "ok: $name ($want)"
}

# The record doesn't depend on when it was taken, and changes nothing it reads.
before=$(cd "$base" && find . -type f -exec shasum -a 256 {} + | sort)
"$check" record "$base" "$scratch/again.txt"
after=$(cd "$base" && find . -type f -exec shasum -a 256 {} + | sort)
[ "$before" = "$after" ] || { echo "FAIL: record wrote to the home"; failures=$((failures + 1)); }
if "$check" compare "$scratch/old.txt" "$scratch/again.txt" >/dev/null; then echo "ok: two records compare clean"; else
	echo "FAIL: two records of the same home differ"; failures=$((failures + 1))
fi
grep -q $'^2026-09-13-once-an-hour-check\tidentity\talias:once\t' "$scratch/old.txt" || { echo "FAIL: no 'once' alias recorded"; failures=$((failures + 1)); }
grep -q $'^2026-09-13-once-an-hour-check\tschedule\tnotStarted\ttrue' "$scratch/old.txt" || { echo "FAIL: not started not recorded"; failures=$((failures + 1)); }

expect "journal line appended" pass journal 'echo "{\"n\":3}" >>runs/1/journal.jsonl'
expect "launcher added" pass config "jq '.launchers += [{\"name\":\"sandbox-x\",\"command\":\"c\"}]' config.json >c && mv c config.json"
expect "nextWakeAt moved, noChangeTotal up" pass schedule "jq '.nextWakeAt = \"2026-11-27T00:00:00Z\" | .check.noChangeTotal = 41' runs/1/run.json >r && mv r runs/1/run.json"
expect "goal edited" fail goal "jq '.goal = \"Build other things\"' runs/1/run.json >r && mv r runs/1/run.json"
expect "journal line rewritten" fail journal 'printf "{\"n\":1,\"kind\":\"wake-start\"}\n{\"n\":2,\"kind\":\"other\"}\n" >runs/1/journal.jsonl'
expect "gate removed" fail gates 'rm runs/1/gates/001-ask.md'
expect "noChangeTotal reset" fail check "jq '.check.noChangeTotal = 0' runs/1/run.json >r && mv r runs/1/run.json"
expect "pending note removed" fail notes "jq '.notes |= map(select(.id != \"002\"))' runs/1/run.json >r && mv r runs/1/run.json"
expect "output version removed" fail outputs "rm runs/1/outputs/report/v2.md && jq '.outputs[0].versions |= .[:1]' runs/1/outputs/index.json >o && mv o runs/1/outputs/index.json"
expect "change patch edited" fail changes 'echo "+more" >>runs/1/changes/1-fix/v1.patch'
expect "feedback line dropped" fail feedback 'tail -n 1 feedback.jsonl >f && mv f feedback.jsonl'
expect "non-launcher config key changed" fail config "jq '.lander = \"2\"' config.json >c && mv c config.json"
expect "launcher changed" fail config "jq '.launchers[0].command = \"other\"' config.json >c && mv c config.json"
expect "alias removed" fail identity "jq '.name = \"renamed\"' runs/1/run.json >r && mv r runs/1/run.json"
expect "unread message marked read" fail messages "jq '.doneAt = \"2026-11-02T00:00:00Z\"' runs/1/messages/001-hi.json >m && mv m runs/1/messages/001-hi.json"
expect "draft reply dropped" fail drafts "jq '.changes[0].replies = []' runs/1/changes/index.json >c && mv c runs/1/changes/index.json"
expect "artifact edited" fail artifacts 'echo x >>runs/1/artifacts/sub/a.txt'
expect "handoff edited" fail handoff 'echo x >>runs/1/handoff.md'
expect "check script edited" fail check 'echo "# x" >>runs/1/check.sh'
expect "not started cleared" fail schedule "jq '.paused = null' runs/2026-09-13-once-an-hour-check/run.json >r && mv r runs/2026-09-13-once-an-hour-check/run.json"

if [ $failures -ne 0 ]; then echo "$failures expectation(s) failed"; exit 1; fi
echo "all migration-check expectations hold"
