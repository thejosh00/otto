#!/bin/bash
# migration-check.sh — record what must survive a release in an otto home, and compare two records
# (plans/v3.md, "Migrating existing runs" → "What must survive").
#
#   scripts/migration-check.sh record <otto-home> <out-file>
#   scripts/migration-check.sh compare <old-record> <new-record>
#
# `record` only reads the home. It writes one fact per line, sorted, as
#   <run>\t<category>\t<key>\t<value>
# where <run> is the run's directory name, or `-` for the home itself. Big things (goal,
# doneCondition, handoff, notes, files under artifacts/ outputs/ changes/, the check script, each
# journal and feedback.jsonl line) are recorded by hash: the first 16 hex digits of their sha256.
#
# `compare` prints every difference as `<run> <category>: <what>` and exits 1 if any must not happen,
# 0 if none. What moves on a live home on its own is listed in a section of its own and never fails.
# Needs bash, jq, shasum and perl (Digest::SHA, part of macOS's perl) for the per-line hashes.
set -u
export LC_ALL=C

usage() {
	echo "usage: $0 record <otto-home> <out-file>" >&2
	echo "       $0 compare <old-record> <new-record>" >&2
	exit 2
}

# The short hash of a file's bytes, or `missing`.
hash_file() {
	if [ -f "$1" ]; then shasum -a 256 "$1" | cut -c1-16; else echo missing; fi
}

# The short hash of stdin.
hash_stdin() { shasum -a 256 | cut -c1-16; }

fact() { printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4"; }

# Each file under a directory, by hash: `<run> <category> file:<path> <hash>`. `index.json` at
# the top is left to the caller, which records what it says rather than its bytes.
hash_tree() {
	local run=$1 category=$2 dir=$3
	[ -d "$dir" ] || return 0
	(cd "$dir" && find . -type f ! -path ./index.json -exec shasum -a 256 {} +) |
		while IFS= read -r line; do
			fact "$run" "$category" "file:${line#*  ./}" "$(printf %s "$line" | cut -c1-16)"
		done
}

# One line per line of a file, numbered, by hash. A record of `old` is an exact prefix of `new`
# exactly when every one of old's numbered lines is in new with the same hash.
hash_lines() {
	local run=$1 category=$2 file=$3
	[ -f "$file" ] || return 0
	perl -MDigest::SHA=sha256_hex -e '
		my ($run, $category) = splice(@ARGV, 0, 2);
		while (<>) { printf "%s\t%s\tline:%07d\t%s\n", $run, $category, $., substr(sha256_hex($_), 0, 16) }
	' "$run" "$category" "$file"
}

# The short id `otto` prints for a run from before numbering (src/paths.rs short_id_among): the
# shortest unique prefix of its slug, extended to the end of its word. `once` for gtd-task-runner.
slug_of() {
	case $1 in
	[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]-*) echo "${1#??????????-}" ;;
	*) echo "$1" ;;
	esac
}
is_number() { case $1 in '' | *[!0-9]*) return 1 ;; *) return 0 ;; esac; }
short_id() {
	local id=$1 all=$2 slug end candidate count only other rest word
	if is_number "$id"; then echo "$id"; return; fi
	slug=$(slug_of "$id")
	for ((end = 1; end <= ${#slug}; end++)); do
		candidate=${slug:0:end}
		count=0 only=
		for other in $all; do
			is_number "$other" && continue
			case $other in "$candidate"*) ;; *) case $(slug_of "$other") in "$candidate"*) ;; *) continue ;; esac ;; esac
			count=$((count + 1)) only=$other
		done
		if [ "$count" = 1 ] && [ "$only" = "$id" ]; then
			rest=${slug:end}
			word=${rest%%-*}
			echo "${candidate}${word}"
			return
		fi
	done
	echo "$id"
}

record_run() {
	local home=$1 run=$2 all=$3
	local dir="$home/runs/$run" json="$home/runs/$run/run.json"
	if ! jq -e . "$json" >/dev/null 2>&1; then
		fact "$run" identity run.json unreadable
		return
	fi

	# Identity, schedule, gates, notes, check: what run.json says.
	jq -r --arg run "$run" '
		def v: if type == "string" then . else tojson end;
		def line(c; k; x): [$run, c, k, (x | v)] | @tsv;
		line("identity"; "id"; .id),
		line("identity"; "name"; .name // ""),
		line("identity"; "formerId"; .formerId),
		(if .name then line("identity"; "alias:" + .name; "-") else empty end),
		(if .formerId then line("identity"; "alias:" + .formerId; "-") else empty end),
		line("schedule"; "nextWakeAt"; .nextWakeAt),
		(if has("armedWakeAt") then line("schedule"; "armedWakeAt"; .armedWakeAt) else empty end),
		line("schedule"; "periodMinutes"; .policy.periodMinutes),
		line("schedule"; "periodAt"; .policy.periodAt),
		line("schedule"; "pause"; .paused),
		line("schedule"; "notStarted"; (.paused != null and .paused.until == null
			and ((.paused.reason // "") | startswith("not started yet")))),
		line("gates"; "open"; .gate),
		(if .check then
			line("check"; "noChangeTotal"; .check.noChangeTotal),
			line("check"; "wakeAfter"; .check.wakeAfter)
		else empty end)
	' "$json"
	is_number "$run" && fact "$run" identity number "$run"
	fact "$run" identity "alias:$(short_id "$run" "$all")" -
	fact "$run" goal hash "$(jq -c .goal "$json" | hash_stdin)"
	fact "$run" doneCondition hash "$(jq -c .doneCondition "$json" | hash_stdin)"

	# The check's script: a path in run.json, relative to the run's directory unless absolute.
	local script
	script=$(jq -r '.check.script // empty' "$json")
	if [ -n "$script" ]; then
		case $script in /*) ;; *) script="$dir/$script" ;; esac
		fact "$run" check script "$(hash_file "$script")"
	fi

	# Notes: pending (one-off) and standing, by the hash of their text.
	jq -r '.notes[]? | [.id, (if .standing then "standing" else "pending" end), .file] | @tsv' "$json" |
		while IFS=$'\t' read -r id kind file; do
			fact "$run" notes "$kind:$id" "$(hash_file "$dir/$file")"
		done

	# Gates: every gate file, answered or not, and the open one above.
	hash_tree "$run" gates "$dir/gates"

	# Messages: each one's body, which are unread, and any draft to send on.
	local m id body
	for m in "$dir"/messages/*.json; do
		[ -f "$m" ] || continue
		id=$(jq -r .id "$m")
		body=$(jq -r .body "$m")
		fact "$run" messages "body:$id" "$(hash_file "$dir/$body")"
		[ "$(jq -r '.doneAt // empty' "$m")" = "" ] && fact "$run" messages "unread:$id" unread
		[ "$(jq -r '.draft // empty' "$m")" != "" ] && fact "$run" drafts "message:$id" "$(jq -c .draft "$m" | hash_stdin)"
	done

	# Outputs: every version the index lists, and every file.
	if [ -f "$dir/outputs/index.json" ]; then
		jq -r '.outputs[]? | .key as $k | .versions[]? | [$k, (.n | tostring), .file] | @tsv' "$dir/outputs/index.json" |
			while IFS=$'\t' read -r key n file; do
				fact "$run" outputs "version:$key/v$n" "$(hash_file "$dir/$file")"
			done
	fi
	hash_tree "$run" outputs "$dir/outputs"

	# Changes: each record and version as the index has it, the replies drafted, every patch file.
	if [ -f "$dir/changes/index.json" ]; then
		jq -r --arg run "$run" '
			.changes[]? as $c
			| ([$run, "changes", "record:" + $c.key, ({n: $c.n, repo: $c.repo, branch: $c.branch, base: $c.base} | tojson)] | @tsv),
			  ($c.versions[]? | [$run, "changes", "version:" + $c.key + "/v" + (.k | tostring),
				({sha, baseSha, patchId, patch} | tojson)] | @tsv),
			  ($c.replies[]? | select(.state == "draft") | [$run, "drafts", "reply:" + $c.key + ":" + .comment, (.text | @base64)] | @tsv)
		' "$dir/changes/index.json"
	fi
	hash_tree "$run" changes "$dir/changes"

	hash_tree "$run" artifacts "$dir/artifacts"
	fact "$run" handoff hash "$(hash_file "$dir/handoff.md")"
	hash_lines "$run" journal "$dir/journal.jsonl"
}

record() {
	local home=$1 out=$2
	[ -d "$home/runs" ] || { echo "no runs/ under $home: not an otto home" >&2; exit 2; }
	local tmp all run repo
	tmp=$(mktemp)
	all=$(cd "$home/runs" && for d in *; do [ -d "$d" ] && echo "$d"; done)
	{
		[ -f "$home/runs/.last-id" ] && fact - identity lastId "$(tr -d ' \n' <"$home/runs/.last-id")"
		for run in $all; do record_run "$home" "$run" "$all"; done

		hash_lines - feedback "$home/feedback.jsonl"

		# config.json: each launcher whole, by name; every other key flattened to its leaves.
		if [ -f "$home/config.json" ]; then
			jq -r '
				(.launchers // [] | .[] | ["-", "config", "launcher:" + .name, tojson] | @tsv),
				(del(.launchers) | paths(scalars or ((type == "array" or type == "object") and length == 0)) as $p
					| ["-", "config", ($p | map(tostring) | join(".")), (getpath($p) | tojson)] | @tsv)
			' "$home/config.json"
		fi

		# Worktrees of each repo a run has in play (its launcher.repos).
		for repo in $(cat "$home"/runs/*/run.json 2>/dev/null | jq -r '.launcher.repos[]? // empty' 2>/dev/null | sort -u); do
			[ -d "$repo" ] || { fact - worktrees "$repo" missing; continue; }
			git -C "$repo" worktree list --porcelain 2>/dev/null | awk -v repo="$repo" '
				/^worktree / { path = substr($0, 10); branch = "detached"; next }
				/^branch / { branch = substr($0, 8); next }
				/^$/ { if (path != "") printf "-\tworktrees\t%s:%s\t%s\n", repo, path, branch; path = "" }
				END { if (path != "") printf "-\tworktrees\t%s:%s\t%s\n", repo, path, branch }'
		done
	} | sort -u >"$tmp"
	mv "$tmp" "$out"
}

compare() {
	local old=$1 new=$2
	[ -f "$old" ] && [ -f "$new" ] || usage
	awk -F'\t' '
		# Fields that move on a live home by themselves between two records.
		function moves(cat, key) { return cat == "schedule" && (key == "nextWakeAt" || key == "armedWakeAt") }
		# Counters that only go up: up is a live home working, down or reset is lost.
		function counter(cat, key) { return (cat == "check" && key == "noChangeTotal") || (cat == "identity" && key == "lastId") }
		function fail(s) { fails[++nf] = s }
		function moved(s) { moves_[++nm] = s }
		NR == FNR { k = $1 FS $2 FS $3; old[k] = $4; oldrun[$1] = 1; next }
		{
			k = $1 FS $2 FS $3; seen[k] = 1
			if (!(k in old)) {
				if ($2 == "config" && $3 !~ /^launcher:/) fail($1 " config: " $3 " added (" $4 ")")
				else if (!($1 in oldrun)) added[$1 FS "new run"]++
				else added[$1 FS $2]++
			} else if (old[k] != $4) {
				what = $3 " changed: " old[k] " -> " $4
				if (moves($2, $3)) moved($1 " " $2 ": " what)
				else if (counter($2, $3) && $4 + 0 > old[k] + 0) moved($1 " " $2 ": " $3 " went up: " old[k] " -> " $4)
				else fail($1 " " $2 ": " what)
			}
		}
		END {
			for (k in old) if (!(k in seen)) {
				split(k, f, FS)
				if (moves(f[2], f[3])) moved(f[1] " " f[2] ": " f[3] " removed (was " old[k] ")")
				else fail(f[1] " " f[2] ": " f[3] " removed (was " old[k] ")")
			}
			for (a in added) {
				split(a, f, FS)
				if (f[2] == "new run") moved(f[1] " new run: " added[a] " facts")
				else moved(f[1] " " f[2] ": " added[a] " added (" (f[2] == "journal" || f[2] == "feedback" ? "lines appended" : "new entries") ")")
			}
			print "migration-check: comparing " ARGV[1] " (old) with " ARGV[2] " (new)."
			print "Anything lost or rewritten fails: a journal or feedback.jsonl line changed or gone (appending"
			print "is fine), a goal, note, gate, message, draft, output version, change, artifact, handoff, check"
			print "script or alias changed or gone, a launcher removed or changed, or any other config key changed."
			print "On a live home some facts move by themselves between two records. These never fail:"
			print "nextWakeAt and armedWakeAt, noChangeTotal and .last-id going up, and anything added (journal"
			print "lines appended, new notes, messages, outputs, changes, launchers, runs)."
			print "A person acting between two records (answering a gate, reading a message, a wake taking a"
			print "one-off note) shows as a failure: say so beside it if that is what happened."
			print ""
			print "Must not change (" nf + 0 "):"
			if (nf == 0) print "  none"
			for (i = 1; i <= nf; i++) print "  " fails[i]
			print ""
			print "Expected to move on a live home (" nm + 0 "):"
			if (nm == 0) print "  none"
			for (i = 1; i <= nm; i++) print "  " moves_[i]
			exit nf > 0 ? 1 : 0
		}
	' "$old" "$new"
}

case ${1:-} in
record) [ $# -eq 3 ] || usage; record "$2" "$3" ;;
compare) [ $# -eq 3 ] || usage; compare "$2" "$3" ;;
*) usage ;;
esac
