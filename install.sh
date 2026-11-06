#!/usr/bin/env bash
# Installer for otto: builds the Rust binary, places it on PATH, installs otto's Claude
# Code skills, and registers the launchd reviver agent.
#
#   ./install.sh
#
# Each step is also available on its own once otto is installed:
#
#   otto install   # (re-)symlink the skills otto ships into ~/.claude/skills
#   otto agent start   # (re-)register and start the launchd reviver — installs it if missing
#   otto agent stop    # unregister the launchd reviver
#   otto service start # (re-)register and start the web UI as a launchd service
#   otto service restart # serve the newly installed binary, on the port it already uses
#
# Pass --service to also install the web UI as a launchd service. While it is running, this script
# and --rollback restart it, so it serves the binary they leave in place; one stopped with
# `otto service stop` stays stopped.
#
# otto compiles its harness in, so an install changes what every agent's next wake is told. The
# binary it replaces is kept beside the new one, and `./install.sh --rollback` puts it back.
set -euo pipefail

with_service=0
rollback=0
for arg in "$@"; do
	case "$arg" in
	--service) with_service=1 ;;
	--rollback) rollback=1 ;;
	*) echo "usage: $0 [--service] [--rollback]" >&2; exit 2 ;;
	esac
done

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin_dir="${OTTO_BIN_DIR:-$HOME/.local/bin}"

cd "$repo_dir"

if [ "$rollback" = 1 ]; then
	if [ ! -x "$bin_dir/otto.previous" ]; then
		echo "nothing to roll back to: no $bin_dir/otto.previous" >&2
		exit 1
	fi
	install -m 755 "$bin_dir/otto.previous" "$bin_dir/otto"
	rm "$bin_dir/otto.previous"
	echo "==> rolled back $bin_dir/otto to the one before the last install"
else
	echo "==> building otto (cargo build --release)"
	cargo build --release

	# A build that can't read this machine's runs must not replace one that can: the reviver and
	# every agent run the installed binary, and none of them could say what went wrong.
	echo "==> checking the new build reads your runs"
	target/release/otto ls >/dev/null

	mkdir -p "$bin_dir"
	# cp, not mv: its modified time is when it stopped being current, which scripts/check.sh reads.
	if [ -x "$bin_dir/otto" ]; then
		cp "$bin_dir/otto" "$bin_dir/otto.previous"
	fi
	install -m 755 target/release/otto "$bin_dir/otto"
	echo "==> installed $bin_dir/otto"
fi

if ! command -v otto >/dev/null 2>&1; then
	echo "note: $bin_dir is not on your PATH — add it to your shell profile to run otto directly." >&2
fi

echo "==> otto install (skills)"
"$bin_dir/otto" install --repo "$repo_dir"

echo "==> otto agent start (launchd reviver)"
"$bin_dir/otto" agent start

if [ "$with_service" = 1 ]; then
	echo "==> otto service start (web UI)"
	"$bin_dir/otto" service start
elif [ -f "$HOME/Library/LaunchAgents/com.joshuahill.otto-serve.plist" ]; then
	# `otto service stop` leaves the plist, so only launchd can say whether it should be running.
	if launchctl print "gui/$(id -u)/com.joshuahill.otto-serve" >/dev/null 2>&1; then
		# It keeps running the binary it started from, so a landed web fix wouldn't be what it serves.
		echo "==> otto service restart (web UI)"
		"$bin_dir/otto" service restart
	else
		echo "==> the web UI service isn't running, so it stays that way (otto service start brings it back)"
	fi
fi

echo "==> done"
