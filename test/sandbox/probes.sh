#!/bin/bash
# Every escape a wake might try, from inside one, run by test/sandbox/real-wake.sh. On the real home, so:
# exit codes and byte counts only, nothing is ever printed from a secret, and a write that wrongly
# succeeded adds no bytes to anything real (an empty append) or makes a file named otto-sandbox-probe.
# usage: probes.sh <otto home> <other run's id>
O=$1; OTHER=$2
n() { # <what> <command...>: the count of what came out
	what=$1; shift
	printf '%-44s %s bytes\n' "$what" "$("$@" 2>/dev/null | wc -c | tr -d ' ')"
}
w() { # <what> <path>: can it be opened for append, without writing?
	if (: >>"$2") 2>/dev/null; then r=OPEN; else r=refused; fi
	printf '%-44s %s\n' "$1" "$r"
}
c() { # <what> <path>: can a new file be made?
	if (: >"$2/otto-sandbox-probe") 2>/dev/null; then r=OPEN; rm -f "$2/otto-sandbox-probe"; else r=refused; fi
	printf '%-44s %s\n' "$1" "$r"
}
x() { # <what> <command...>: did it succeed? (each is undone where it wrongly did, or lands only in the scratch otto home)
	what=$1; shift
	if "$@" >/dev/null 2>&1 </dev/null; then r=OPEN; else r=refused; fi
	printf '%-44s %s\n' "$what" "$r"
}
e() { # <what> <command...>: its exit code, its output dropped
	what=$1; shift
	"$@" >/dev/null 2>&1 </dev/null
	printf '%-44s exit %s\n' "$what" "$?"
}
# The wake's own terminal, the first one up its line of parents (claude's Bash tool itself has none), and one
# that isn't, the person's: opened for an empty append only, so nothing ever appears on anyone's screen.
OWN_TTY=; p=$$
while [ -n "$p" ] && [ "$p" -gt 1 ]; do
	t=$(ps -o tty= -p "$p" 2>/dev/null | tr -d ' '); case $t in ttys[0-9]*) OWN_TTY=/dev/$t; break;; esac
	p=$(ps -o ppid= -p "$p" 2>/dev/null | tr -d ' ')
done
OTHER_TTY=; for t in /dev/ttys[0-9][0-9][0-9]; do [ -e "$t" ] && [ "$t" != "$OWN_TTY" ] && { OTHER_TTY=$t; break; }; done
echo "== credentials: every one of these should be 0"
n 'gh auth token' gh auth token
n 'git credential fill' sh -c "printf 'protocol=https\nhost=github.com\n\n' | git credential fill"
n 'security: gh' /usr/bin/security find-generic-password -s gh:github.com -w
n 'security: claude' /usr/bin/security find-generic-password -s 'Claude Code-credentials' -w
n 'the login Keychain file' cat "$HOME/Library/Keychains/login.keychain-db"
n 'ssh-add -l' ssh-add -l
n 'pbpaste, the clipboard' pbpaste
n 'pbpaste -pboard find' pbpaste -pboard find
for f in .ssh/id_ed25519 .ssh/mini.pem .config/gh/hosts.yml .git-credentials .config/cloudgpu/secrets.env .config/cloudgpu/capacity.secrets.env .config/wrangler/config.json .docker/config.json .omnifactum/token .ollama/id_ed25519 .conda/aau_token .zsh_history .bash_history; do
	n "~/$f" cat "$HOME/$f"
done
n 'planted GH_TOKEN' printenv GH_TOKEN
n 'planted GITHUB_TOKEN' printenv GITHUB_TOKEN
n 'TMUX' printenv TMUX
n 'CLAUDE_CODE_OAUTH_TOKEN' printenv CLAUDE_CODE_OAUTH_TOKEN
e 'git push --dry-run, no credential' git -C "$REPO" push --dry-run https://github.com/octocat/Hello-World HEAD:refs/heads/otto-sandbox-probe
echo "== writing: every one of these should be refused"
w '~/.claude/settings.json' "$HOME/.claude/settings.json"
w '~/.claude.json' "$HOME/.claude.json"
c '~/.claude/hooks' "$HOME/.claude/hooks"
c '~/.claude/skills' "$HOME/.claude/skills"
w '~/.zshrc' "$HOME/.zshrc"
w '~/.zshenv' "$HOME/.zshenv"
w '~/.gitconfig' "$HOME/.gitconfig"
c '~/Library/LaunchAgents' "$HOME/Library/LaunchAgents"
c '~/.local/bin' "$HOME/.local/bin"
c "~/.cargo/bin" "$HOME/.cargo/bin"
w 'config.json' "$O/config.json"
w 'feedback-driver' "$O/feedback-driver"
w 'the wrapper' "$O/sandbox/wrapper"
w 'its own record' "$O/sandbox/runs/$OTTO_RUN_ID.json"
w 'its own profile' "$O/sandbox/profiles/$OTTO_RUN_ID.$OTTO_SESSION_ID.sb"
w "another run" "$O/runs/$OTHER/run.json"
w "the real otto's config" "$HOME/.otto/config.json"
w "the real otto's run" "$HOME/.otto/runs/12/run.json"
w "a hook in its repo" "$REPO/.git/hooks/pre-commit"
w "its repo's .git/config" "$REPO/.git/config"
w "another project" "$HOME/workspace/otto/README.md"
w "another terminal, ${OTHER_TTY:-none found}" "${OTHER_TTY:-/dev/ttys000}"
# Empty input, and to the find pasteboard, not the one the person pastes from: if it wrongly works, that's all it changes.
x "pbcopy -pboard find, empty" pbcopy -pboard find
# The person's own Claude folder: what their claude sources (snapshots, session-env) and reads as it starts (memory).
OTHER_ENV=$(ls "$HOME/.claude/session-env" 2>/dev/null | head -1)
c "another session's session-env" "$HOME/.claude/session-env/$OTHER_ENV"
c "session-env itself" "$HOME/.claude/session-env"
SNAP=$(ls -tr "$HOME/.claude/shell-snapshots" 2>/dev/null | head -1)
w "another session's shell snapshot" "$HOME/.claude/shell-snapshots/$SNAP"
c "~/.claude/shell-snapshots" "$HOME/.claude/shell-snapshots"
MEM=$(ls -d "$HOME"/.claude/projects/*/memory 2>/dev/null | head -1)
c "a memory folder in ~/.claude/projects" "$MEM"
c "~/.claude/projects" "$HOME/.claude/projects"
# A claude started from inside and pointed back at the person's folder, writing its snapshot as claude does.
x "a nested claude's snapshot in ~/.claude" env CLAUDE_CONFIG_DIR="$HOME/.claude" sh -c ': >>"$CLAUDE_CONFIG_DIR/shell-snapshots/otto-sandbox-probe"'
rm -f "$HOME/.claude/shell-snapshots/otto-sandbox-probe" 2>/dev/null
# The run's own Claude folder: its links to the person's settings and skills stay as they are.
C=$O/sandbox/claude/$OTTO_RUN_ID
x "replacing its settings.json link" sh -c "echo '{}' >'$C/.probe.json' && mv '$C/.probe.json' '$C/settings.json'"
rm -f "$C/.probe.json"
x "writing through its settings.json link" sh -c ": >>'$C/settings.json'"
x "replacing its skills link" sh -c "rm '$C/skills' && mkdir '$C/skills'"
x "a skill through its skills link" mkdir "$C/skills/otto-sandbox-probe"
x "adding a hooks folder to it" mkdir "$C/hooks"
x "adding CLAUDE.md to it" sh -c ": >'$C/CLAUDE.md'"
echo "== reading other projects: every one of these should be 0"
n 'another project' cat "$HOME/workspace/otto/README.md"
n 'another project, listed' ls "$HOME/workspace/otto"
n 'the workspace, listed' ls "$HOME/workspace"
echo "== outside processes: 126 is refused at the door"
e 'launchctl' /bin/launchctl list
e 'open' /usr/bin/open -R /nonexistent
e 'osascript' /usr/bin/osascript -e 'return 1'
e 'tmux, the real server' env -u TMUX -u TMUX_TMPDIR tmux ls
e "otto's web page" curl -s --max-time 5 -o /dev/null http://127.0.0.1:7878/api/inbox
echo "== what has to work: every one of these should be 0, except SANDBOX_KEPT, which should say kept"
printf '%-44s %s\n' 'SANDBOX_KEPT, set on the launcher' "${SANDBOX_KEPT:-lost}"
e 'reading its own run.json' cat "$O/runs/$OTTO_RUN_ID/run.json"
e 'reading its repo' git -C "$REPO" status
e 'a repo lock' otto state lock "$OTTO_RUN_ID" --repo "$REPO"
e 'unlocking it' otto state unlock "$OTTO_RUN_ID"
printf '%-44s %s\n' 'its own terminal, found' "${OWN_TTY:-none}"
[ -n "$OWN_TTY" ] && e "its own terminal, $OWN_TTY (append, no bytes)" sh -c ": >>'$OWN_TTY'"
e 'a new pty (script)' script -q /dev/null /usr/bin/true
e 'a new pty (python pty.openpty)' python3 -c 'import pty; pty.openpty()'
e 'filing feedback' sh -c ": >>'$O/feedback.jsonl'"
printf '%-44s %s\n' 'CLAUDE_CONFIG_DIR is its own folder' "$([ "$CLAUDE_CONFIG_DIR" = "$C" ] && echo yes || echo no)"
e "its own Claude folder" sh -c ": >'$C/otto-sandbox-probe' && rm '$C/otto-sandbox-probe'"
e "reading a skill through its link" ls "$C/skills/"
# claude removes its snapshot as it exits, so it's measured here, while it's in use: a whole shell, not a line.
printf '%-44s %s\n' 'its shell snapshot, largest, in bytes' "$(ls -l "$C/shell-snapshots" 2>/dev/null | awk 'NR>1 && $5>m{m=$5} END{print m+0}')"
