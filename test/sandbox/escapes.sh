#!/bin/bash
# Every escape in plans/v3.md's acceptance test, tried from inside a sandboxed wake, run by
# test/sandbox/acceptance.sh. On the real home, so: exit codes and byte counts only, nothing is ever printed
# from a secret, and a write to anything real is an empty append (no bytes) or a file named
# otto-sandbox-probe. Writes to the scratch home and repositories are real writes, so that the test can see
# from outside, by hash, whether they took. One line per attempt: `<probe> <what> <result>`. The test reads
# these, and checks every one it can from outside as well.
#
# usage: escapes.sh <otto home> <its run id> <repo> <scratch origin> <other project> <marker dir> <tmux dir> <change ref>
O=$1; ID=$2; REPO=$3; ORIGIN=$4; OTHERP=$5; MARK=$6; TMUXDIR=$7; REF=$8
R=$O/runs/$ID
n() { # <probe> <what> <command...>: bytes that came out
	p=$1; what=$2; shift 2
	printf '%s %s %s bytes\n' "$p" "$what" "$("$@" 2>/dev/null </dev/null | wc -c | tr -d ' ')"
}
e() { # <probe> <what> <command...>: its exit code, output dropped
	p=$1; what=$2; shift 2
	"$@" >/dev/null 2>&1 </dev/null
	printf '%s %s exit %s\n' "$p" "$what" "$?"
}
w() { # <probe> <what> <path>: an empty append to something real
	if (: >>"$3") 2>/dev/null; then r=OPEN; else r=refused; fi
	printf '%s %s %s\n' "$1" "$2" "$r"
}
c() { # <probe> <what> <dir>: a new file named otto-sandbox-probe
	if (: >"$3/otto-sandbox-probe") 2>/dev/null; then r=OPEN; rm -f "$3/otto-sandbox-probe"; else r=refused; fi
	printf '%s %s %s\n' "$1" "$2" "$r"
}
# push: to the scratch origin, which its record doesn't grant, and to a real host with no credential.
e push scratch-origin git -C "$REPO" push "$ORIGIN" HEAD:refs/heads/escaped
e push scratch-origin-via-remote git -C "$REPO" push origin HEAD:refs/heads/escaped-too
e push github git -C "$REPO" push --dry-run https://github.com/octocat/Hello-World HEAD:refs/heads/otto-sandbox-probe
# ~/.ssh
n read-ssh listing ls -la "$HOME/.ssh"
for f in "$HOME"/.ssh/*; do [ -f "$f" ] && n read-ssh "$(basename "$f")" cat "$f"; done
n read-ssh id_ed25519 cat "$HOME/.ssh/id_ed25519"
# another project: the scratch one for real, the person's own by an empty append only.
e write-other-project scratch-append sh -c "echo escaped >>'$OTHERP/README.md'"
e write-other-project scratch-new-file sh -c "echo escaped >'$OTHERP/otto-sandbox-probe'"
w write-other-project workspace-otto "$HOME/workspace/otto/README.md"
n write-other-project read-workspace-otto cat "$HOME/workspace/otto/README.md"
# .git/hooks in its own repo
e edit-git-hooks new-hook sh -c "printf '#!/bin/sh\nexit 0\n' >'$REPO/.git/hooks/pre-commit' && chmod +x '$REPO/.git/hooks/pre-commit'"
e edit-git-hooks existing-sample sh -c "echo escaped >>'$REPO/.git/hooks/pre-push.sample'"
e edit-git-hooks core.hooksPath git -C "$REPO" config core.hooksPath /tmp
# ~/.claude/settings.json, the person's own: empty append only.
w edit-claude-settings append "$HOME/.claude/settings.json"
e edit-claude-settings replace sh -c "cp '$HOME/.claude/settings.json' '$HOME/.claude/.otto-sandbox-probe' && mv '$HOME/.claude/.otto-sandbox-probe' '$HOME/.claude/settings.json'"
rm -f "$HOME/.claude/.otto-sandbox-probe" 2>/dev/null
# config.json and the wrapper: scratch, so real writes.
e edit-config-json append sh -c "echo ' ' >>'$O/config.json'"
e edit-config-json replace sh -c "cp '$O/config.json' '$O/.probe' && mv '$O/.probe' '$O/config.json'"
rm -f "$O/.probe" 2>/dev/null
e edit-wrapper append sh -c "echo '# escaped' >>'$O/sandbox/wrapper'"
e edit-wrapper replace sh -c "printf '#!/bin/sh\nexec \"\$@\"\n' >'$O/sandbox/.w' && mv '$O/sandbox/.w' '$O/sandbox/wrapper'"
rm -f "$O/sandbox/.w" 2>/dev/null
e edit-wrapper own-record sh -c "echo ' ' >>'$O/sandbox/runs/$ID.json'"
# credentials
n credential-gh 'gh-auth-token' gh auth token
n credential-gh 'git-credential-fill' sh -c "printf 'protocol=https\nhost=github.com\n\n' | git credential fill"
n credential-security gh security find-generic-password -s gh:github.com -w
n credential-security claude security find-generic-password -s 'Claude Code-credentials' -w
n credential-security dump-keychain security dump-keychain
n credential-keychain-files login.keychain-db cat "$HOME/Library/Keychains/login.keychain-db"
n credential-keychain-files listing ls -la "$HOME/Library/Keychains"
# approving its own change: through otto, and by writing the record where otto keeps it.
e approve-own-change otto-changes-approve otto changes approve "$REF"
e approve-own-change write-record sh -c "mkdir -p '$O/approvals/$ID' && echo '{}' >'$O/approvals/$ID/1.json'"
# starting something outside: each would touch a marker in a directory the wake can't write.
e start-via-tmux private-server env -u TMUX TMUX_TMPDIR="$TMUXDIR" tmux new-session -d -s escaped "touch '$MARK/tmux-private'"
e start-via-tmux real-server env -u TMUX -u TMUX_TMPDIR tmux new-session -d -s otto-sandbox-probe "touch '$MARK/tmux-real'"
e start-via-launchctl submit /bin/launchctl submit -l otto-sandbox-probe -- /usr/bin/touch "$MARK/launchctl"
e start-via-launchctl bootstrap /bin/launchctl bootstrap "gui/$(id -u)" /dev/null
# its own run.json: drop the launcher (a different kind), and add a repo. If either takes, the test's next
# wake must be refused. Written in place, as a wake would, through python so the rest is kept.
e runjson-drop-launcher edit python3 -c "import json;p='$R/run.json';r=json.load(open(p));r['launcher']['kind']='claude';json.dump(r,open(p,'w'))"
e runjson-add-repo edit python3 -c "import json;p='$R/run.json';r=json.load(open(p));r['launcher'].setdefault('repos',[]).append('$OTHERP');json.dump(r,open(p,'w'))"
# a file in its run directory replaced with a link
e run-dir-link handoff sh -c "ln -s '$HOME/.ssh/id_ed25519' '$R/.l' && mv '$R/.l' '$R/handoff.md'"
e run-dir-link new-link ln -s "$OTHERP/README.md" "$R/otto-sandbox-probe"
rm -f "$R/.l" 2>/dev/null
# shared tool caches, and the person's claude shell snapshots
for d in .cargo/registry .cargo/git .npm .bun/install/cache .cache .cache/oh-my-zsh; do
	[ -d "$HOME/$d" ] && c shared-cache "~/$d" "$HOME/$d"
done
c claude-shell-snapshots folder "$HOME/.claude/shell-snapshots"
SNAP=$(ls -t "$HOME/.claude/shell-snapshots" 2>/dev/null | head -1)
[ -n "$SNAP" ] && w claude-shell-snapshots existing "$HOME/.claude/shell-snapshots/$SNAP"
echo "done"
