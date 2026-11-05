#!/bin/sh
# The otto-build run's check (README, "otto improving itself"): a wake only when there's something
# for it to do, or otto failing. No model, no tokens.
otto="$HOME/.local/bin/otto"

# An agent stuck within a day of an install may be that install's doing, which is the run's to catch.
if [ -n "$(find "$HOME/.local/bin/otto.previous" -mmin -1440 2>/dev/null)" ] &&
	[ "$("$otto" state list --status blocked --json)" != "[]" ]; then
	echo "an agent is blocked since the last install"; exit 1
fi

due=$("$otto" feedback due) || { echo "otto failing"; exit 1; }
echo "$due"
[ "$due" = "nothing due" ]
