#!/usr/bin/env bash
# rehearse.sh — try a new otto build on a copy of the otto home before it replaces the installed one
# (plans/v3.md, "Migrating existing runs" → "Rehearsal"). install.sh runs it before every install
# over an existing one.
#
#   scripts/rehearse.sh --new <bin> [--installed <bin>] [--home <otto-home>] [--log <file>] [--snapshot <phase>]
#
# The home is copied to a fresh temp dir; the real one is only read. On the copy, the installed
# binary and then the new one each run `otto ls`, `otto state list --json`, `otto changes --all
# --json` and `otto poke --dry-run --verbose`, and scripts/migration-check.sh records the copy
# between them and after, then compares the two records.
#
# It stops (exit 1) when:
#   - the new build's `otto ls` fails;
#   - another of the four fails with the new build where the installed one succeeded (both failing
#     is only shown, or a broken record would block installing its own fix);
#   - a run the installed build's `state list --json` has is missing from the new build's;
#   - migration-check compare fails: the new build changed or lost something that must survive.
# The diff of each command's output between the two builds is only shown, never a stop.
#
# Everything goes to the log (default <home>/logs/rehearsal.log, the previous one kept as
# rehearsal.log.1): the only write to the real home, in logs/, which migration-check doesn't record.
# stdout gets a short summary.
#
# --snapshot <phase>: first keep a tarball of the home as ~/.otto-snapshots/<date>-<phase>.tar.gz,
# the plan's "before each cut-over" snapshot. Refuses to overwrite one. install.sh never passes it.
#
# Every binary runs with OTTO_HOME set to the copy and without OTTO_RUN_ID, OTTO_RUN_DIR, TMUX and
# OTTO_NOTIFY. None of the four commands acts outside OTTO_HOME (poke --dry-run spawns, kills and
# notifies nothing); `otto ls` and poke create empty lock files, in the copy.
set -u

usage() {
	echo "usage: $0 --new <bin> [--installed <bin>] [--home <otto-home>] [--log <file>] [--snapshot <phase>]" >&2
	exit 2
}

new="" installed="" home="" log="" snapshot=""
while [ $# -gt 0 ]; do
	case "$1" in
	--new | --installed | --home | --log | --snapshot)
		[ $# -ge 2 ] || usage
		case "$1" in
		--new) new=$2 ;; --installed) installed=$2 ;; --home) home=$2 ;; --log) log=$2 ;; --snapshot) snapshot=$2 ;;
		esac
		shift
		;;
	*) usage ;;
	esac
	shift
done
[ -n "$new" ] || usage

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
check="$here/migration-check.sh"

if [ -z "$home" ]; then
	# As otto resolves it (src/paths.rs): $OTTO_HOME if set and not empty, a leading ~ expanded.
	home="${OTTO_HOME:-$HOME/.otto}"
fi
case "$home" in "~") home="$HOME" ;; "~/"*) home="$HOME/${home#\~/}" ;; esac
[ -n "$installed" ] || installed=$(command -v otto 2>/dev/null || true)

[ -x "$new" ] || { echo "rehearse: no executable new build at $new" >&2; exit 2; }
[ -n "$installed" ] && [ -x "$installed" ] || { echo "rehearse: no installed otto to compare with (--installed)" >&2; exit 2; }
[ -d "$home" ] || { echo "rehearse: no otto home at $home" >&2; exit 2; }
case "$new" in /*) ;; *) new="$PWD/$new" ;; esac
case "$installed" in /*) ;; *) installed="$PWD/$installed" ;; esac

if [ -n "$snapshot" ]; then
	snapdir="$HOME/.otto-snapshots"
	snapfile="$snapdir/$(date +%Y-%m-%d)-$snapshot.tar.gz"
	if [ -e "$snapfile" ]; then
		echo "rehearse: $snapfile already exists; not overwriting it (pick another phase name)" >&2
		exit 1
	fi
	mkdir -p "$snapdir"
	if ! tar -czf "$snapfile" -C "$(dirname "$home")" "$(basename "$home")" 2>"$snapfile.err"; then
		# A live home: a file vanishing mid-tar is reported but the archive is still made.
		[ -s "$snapfile" ] || { echo "rehearse: snapshot failed: $(cat "$snapfile.err")" >&2; rm -f "$snapfile" "$snapfile.err"; exit 1; }
		echo "rehearse: snapshot made with warnings: $(cat "$snapfile.err")" >&2
	fi
	rm -f "$snapfile.err"
	echo "rehearse: snapshot of $home kept as $snapfile"
fi

if [ -z "$log" ]; then
	log="$home/logs/rehearsal.log"
	mkdir -p "$home/logs"
	[ -f "$log" ] && mv -f "$log" "$log.1"
else
	mkdir -p "$(dirname "$log")"
fi
: >"$log"

scratch=$(mktemp -d "${TMPDIR:-/tmp}/otto-rehearsal.XXXXXX") || { echo "rehearse: mktemp failed" >&2; exit 2; }
trap 'rm -rf "$scratch"' EXIT
trap 'exit 130' INT TERM
copy="$scratch/home"

say() { echo "$*"; echo "$*" >>"$log"; }
logonly() { echo "$*" >>"$log"; }

logonly "rehearsal $(date -u +%Y-%m-%dT%H:%M:%SZ)"
logonly "  home:      $home"
logonly "  installed: $installed"
logonly "  new:       $new"

# The home is live: a file vanishing mid-copy (rsync exit 24) is fine.
if command -v rsync >/dev/null 2>&1; then
	rsync -a "$home/" "$copy/" >>"$log" 2>&1
	rc=$?
	[ $rc -eq 0 ] || [ $rc -eq 24 ] || { say "rehearse: copying $home failed (rsync exit $rc)"; exit 1; }
else
	cp -Rp "$home" "$copy" >>"$log" 2>&1 || logonly "cp reported errors (a live home); carrying on"
	[ -d "$copy" ] || { say "rehearse: copying $home failed"; exit 1; }
fi
# migration-check needs runs/ to call it a home; a home without one still has its config to keep.
[ -d "$copy/runs" ] || { mkdir "$copy/runs"; logonly "  (no runs/ in the home; an empty one made in the copy)"; }

commands=("ls" "state list --json" "changes --all --json" "poke --dry-run --verbose")
names=("ls" "state-list" "changes" "poke")

run_all() { # <which> <bin>
	local which=$1 bin=$2 i
	for i in "${!commands[@]}"; do
		# shellcheck disable=SC2086
		env -u OTTO_RUN_ID -u OTTO_RUN_DIR -u TMUX -u OTTO_NOTIFY OTTO_HOME="$copy" \
			"$bin" ${commands[$i]} >"$scratch/$which.${names[$i]}.out" 2>&1 </dev/null
		echo $? >"$scratch/$which.${names[$i]}.code"
		logonly ""
		logonly "\$ otto ${commands[$i]}   ($which build, exit $(cat "$scratch/$which.${names[$i]}.code"))"
		sed 's/^/    /' "$scratch/$which.${names[$i]}.out" >>"$log"
	done
}

# Order: installed build, record, new build, record, compare. The first record is taken after the
# installed build has had its turn on the copy, so whatever it does on its own (lock files, what a
# normal pass of today's otto does) is already in "before" and the comparison isolates what the new
# build did.
run_all installed "$installed"
check_out=$("$check" record "$copy" "$scratch/before.txt" 2>&1)
before_rc=$?
run_all new "$new"
check_out="$check_out$("$check" record "$copy" "$scratch/after.txt" 2>&1)"
after_rc=$?

stops=()
summary=()
for i in "${!commands[@]}"; do
	n=${names[$i]} c=${commands[$i]}
	old_code=$(cat "$scratch/installed.$n.code") new_code=$(cat "$scratch/new.$n.code")
	if [ "$new_code" != 0 ]; then
		if [ "$n" = ls ]; then
			state="failed (new build exit $new_code)"
			stops+=("the new build's \`otto ls\` fails (exit $new_code): it can't read these runs")
		elif [ "$old_code" = 0 ]; then
			state="failed (new build exit $new_code; the installed one succeeded)"
			stops+=("\`otto $c\` fails with the new build (exit $new_code) where the installed one succeeded")
		else
			state="failed with both builds (installed exit $old_code, new exit $new_code; shown only)"
		fi
	elif cmp -s "$scratch/installed.$n.out" "$scratch/new.$n.out"; then
		state="same"
	else
		state="differs (shown in the log)"
	fi
	summary+=("  otto $c: $state")
	logonly ""
	logonly "== diff of \`otto $c\`: installed -> new (shown only, never a stop)"
	diff -u --label "installed: otto $c" --label "new: otto $c" \
		"$scratch/installed.$n.out" "$scratch/new.$n.out" >>"$log" || true
done

# A run the installed build lists that the new one doesn't: the new build can't read it.
logonly ""
if jq -e 'type == "array"' "$scratch/installed.state-list.out" >/dev/null 2>&1; then
	jq -r '.[].id // empty' "$scratch/installed.state-list.out" | sort -u >"$scratch/ids.installed"
	jq -r '.[].id // empty' "$scratch/new.state-list.out" 2>/dev/null | sort -u >"$scratch/ids.new"
	missing=$(comm -23 "$scratch/ids.installed" "$scratch/ids.new")
	for id in $missing; do
		stops+=("the new build can't read run $id: \`otto state list --json\` from the installed build has it, the new build's doesn't")
	done
	logonly "runs listed: installed $(wc -l <"$scratch/ids.installed" | tr -d ' '), new $(wc -l <"$scratch/ids.new" | tr -d ' ')"
else
	summary+=("  runs check skipped: the installed build's \`state list --json\` printed no JSON list")
	logonly "runs check skipped: the installed build's \`state list --json\` printed no JSON list"
fi

logonly ""
logonly "== migration-check"
[ -n "$check_out" ] && logonly "$check_out"
if [ $before_rc -ne 0 ] || [ $after_rc -ne 0 ]; then
	stops+=("migration-check could not record the copy (exit $before_rc / $after_rc)")
	summary+=("  migration-check: error, could not record")
else
	report=$("$check" compare "$scratch/before.txt" "$scratch/after.txt" 2>&1)
	rc=$?
	logonly "$report"
	if [ $rc -eq 0 ]; then
		summary+=("  migration-check: pass")
	else
		summary+=("  migration-check: FAIL (exit $rc)")
		fails=$(printf '%s\n' "$report" | awk '/^Must not change/ { on = 1; next } /^$/ { on = 0 } on')
		[ -n "$fails" ] && summary+=("$fails")
		stops+=("migration-check: the new build changed or lost something that must survive")
	fi
fi

say ""
say "rehearsal of $new on a copy of $home:"
for line in "${summary[@]}"; do say "$line"; done
if [ ${#stops[@]} -gt 0 ]; then
	say "verdict: STOP — the new build must not replace the installed one:"
	for s in "${stops[@]}"; do say "  - $s"; done
	echo "log: $log"
	exit 1
fi
say "verdict: ok"
echo "log: $log"
exit 0
