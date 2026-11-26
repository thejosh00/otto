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
# binary it replaces is kept beside the new one, unless the new one is identical, along with the
# otto home's config.json as it stood then (otto.previous.config.json), and `./install.sh --rollback`
# puts both back. The older binary refuses a config with keys it doesn't know, so the config goes back
# too — except that launchers added since (a run moved into a sandbox-* launcher, one you defined)
# are carried over, so no run loses the launcher it names. The config it replaces is kept as
# config.json.before-rollback in the otto home.
set -euo pipefail

with_service=0
rollback=0
# --skills-repo DIR: link the skills to that checkout, not this one. For an install from a
# temporary clone (otto's own, after a landing), whose directory is gone afterwards.
skills_repo=""
while [ $# -gt 0 ]; do
	case "$1" in
	--service) with_service=1 ;;
	--rollback) rollback=1 ;;
	--skills-repo)
		[ $# -ge 2 ] || { echo "usage: $0 [--service] [--rollback] [--skills-repo DIR]" >&2; exit 2; }
		skills_repo="$2"
		shift
		;;
	*) echo "usage: $0 [--service] [--rollback] [--skills-repo DIR]" >&2; exit 2 ;;
	esac
	shift
done

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin_dir="${OTTO_BIN_DIR:-$HOME/.local/bin}"
# As otto resolves it (src/paths.rs): $OTTO_HOME if set and not empty, a leading ~ expanded.
otto_home="${OTTO_HOME:-$HOME/.otto}"
case "$otto_home" in "~") otto_home="$HOME" ;; "~/"*) otto_home="$HOME/${otto_home#\~/}" ;; esac
config="$otto_home/config.json"
config_snapshot="$bin_dir/otto.previous.config.json"

cd "$repo_dir"

if [ "$rollback" = 1 ]; then
	if [ ! -x "$bin_dir/otto.previous" ]; then
		echo "nothing to roll back to: no $bin_dir/otto.previous" >&2
		exit 1
	fi
	install -m 755 "$bin_dir/otto.previous" "$bin_dir/otto"
	rm "$bin_dir/otto.previous"
	echo "==> rolled back $bin_dir/otto to the one before the last install"
	if [ ! -f "$config_snapshot" ]; then
		echo "==> no config snapshot; config.json left as it is"
	elif ! command -v jq >/dev/null 2>&1; then
		echo "error: jq not found, so $config was left untouched; the config it had at the last install is $config_snapshot" >&2
		rollback_failed=1
	elif [ ! -f "$config" ]; then
		mkdir -p "$otto_home"
		cp "$config_snapshot" "$config.rollback.$$" && mv "$config.rollback.$$" "$config"
		rm "$config_snapshot"
		echo "==> restored $config to the one from the last install"
	else
		tmp="$config.rollback.$$"
		# The snapshot, plus every launcher the current config has that the snapshot lacks, verbatim.
		# Launchers the snapshot has keep its definition; every other key takes the snapshot's value.
		added='(($snap[0].launchers // []) | map(.name)) as $old
			| [($now[0].launchers // [])[] | select(.name as $n | ($old | index($n)) == null)]'
		if jq -n --slurpfile now "$config" --slurpfile snap "$config_snapshot" "
			($added) as \$added
			| \$snap[0] + (if (\$added | length) > 0 then {launchers: ((\$snap[0].launchers // []) + \$added)} else {} end)
		" >"$tmp" 2>"$tmp.err" \
			&& carried="$(jq -nr --slurpfile now "$config" --slurpfile snap "$config_snapshot" "($added)[].name")"; then
			cp "$config" "$config.before-rollback"
			mv "$tmp" "$config"
			rm -f "$tmp.err" "$config_snapshot"
			echo "==> restored $config to the one from the last install"
			while IFS= read -r name; do
				[ -n "$name" ] && echo "    kept launcher $name: it was added since, so the snapshot didn't have it"
			done <<<"$carried"
			echo "    the config it replaced is saved as $config.before-rollback"
		else
			echo "error: could not merge $config_snapshot with $config ($(cat "$tmp.err" 2>/dev/null)); $config was left untouched" >&2
			rm -f "$tmp" "$tmp.err"
			rollback_failed=1
		fi
	fi
else
	echo "==> building otto (cargo build --release)"
	cargo build --release

	# A build that can't read this machine's runs must not replace one that can: the reviver and
	# every agent run the installed binary, and none of them could say what went wrong.
	echo "==> checking the new build reads your runs"
	target/release/otto ls >/dev/null

	mkdir -p "$bin_dir"
	# cp, not mv: its modified time is when it stopped being current, which scripts/check.sh reads.
	# The same build again replaces nothing, so what a rollback puts back stays the one before it.
	if [ -x "$bin_dir/otto" ] && ! cmp -s target/release/otto "$bin_dir/otto"; then
		cp "$bin_dir/otto" "$bin_dir/otto.previous"
		# The config that binary ran with: an older binary refuses keys a newer one added.
		if [ -f "$config" ]; then
			cp "$config" "$config_snapshot"
		else
			rm -f "$config_snapshot"
		fi
	fi
	install -m 755 target/release/otto "$bin_dir/otto"
	echo "==> installed $bin_dir/otto"
fi

if ! command -v otto >/dev/null 2>&1; then
	echo "note: $bin_dir is not on your PATH — add it to your shell profile to run otto directly." >&2
fi

echo "==> otto install (skills)"
"$bin_dir/otto" install --repo "${skills_repo:-$repo_dir}"

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

if [ "${rollback_failed:-0}" = 1 ]; then
	echo "==> the binary was rolled back, but config.json was not" >&2
	exit 1
fi
echo "==> done"
