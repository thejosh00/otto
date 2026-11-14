#!/bin/sh
# The otto-driver run's check (workflows/drive-flow.md): a wake only when there's something for it to
# do. No model, no tokens. Exit 0 = nothing new, exit 1 = wake it, anything else = a wake too.
#
# artifacts/waiting-on is the wake's cursor, and this only reads it:
#   a number   the report it assigned; wake when that report is verified or closed
#   hold       waiting on a person, about a change to the plan; a note about it wakes the run
#   missing    it isn't waiting on anything, so it has work to do
#
# artifacts/blocked-seen is its second cursor, read here the same way: a line "<id> <blocked at>" for
# each blocked agent a wake has already looked at. An agent blocked that isn't there is a wake: one
# newly blocked, or one that was let go and blocked again. One still blocked as the wake left it isn't.
otto="$HOME/.local/bin/otto"
dir="${OTTO_HOME:-$HOME/.otto}/runs/$OTTO_RUN_ID"

blocked=$("$otto" state list --status blocked --json) || { echo "otto failing"; exit 2; }
seen="$dir/artifacts/blocked-seen"
[ -f "$seen" ] || seen=/dev/null
unseen=$(printf '%s' "$blocked" | /usr/bin/jq -r --rawfile seen "$seen" '
	($seen | split("\n")) as $lines
	| [.[] | select(("\(.id) \(.blocked.at // "")") as $key | $lines | index($key) | not) | .name // .id]
	| if length == 0 then empty else "x" + (.[0] | tostring | .[0:80]) end') || {
	echo "cannot read which agents are blocked"; exit 2
}
if [ -n "$unseen" ]; then
	echo "an agent is blocked: ${unseen#x}"; exit 1
fi

# What happens to its own changes reaches it as a one-off note, and a check stands in front of the
# wake that would deliver one.
pending=$(/usr/bin/jq '[(.notes // [])[] | select(.standing != true)] | length' "$dir/run.json") || {
	echo "cannot read run.json"; exit 2
}
if [ "$pending" != "0" ]; then
	echo "a note is waiting"; exit 1
fi

if [ ! -f "$dir/artifacts/waiting-on" ]; then
	echo "not waiting on anything"; exit 1
fi
waiting=$(tr -d '[:space:]' < "$dir/artifacts/waiting-on")

case "$waiting" in
hold) echo "waiting on a person"; exit 0 ;;
'' | *[!0-9]*) echo "can't read what it is waiting on: $waiting"; exit 2 ;;
esac

status=$("$otto" feedback --all --json | /usr/bin/jq -r --argjson id "$waiting" '.[] | select(.id == $id) | .status') || {
	echo "otto failing"; exit 2
}
case "$status" in
verified | closed) echo "report $waiting is $status"; exit 1 ;;
'') echo "report $waiting isn't in the queue"; exit 2 ;;
*) echo "report $waiting is $status"; exit 0 ;;
esac
