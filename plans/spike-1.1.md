# Spike 1.1: what a sandbox can and can't do on this machine

macOS 26.6.2, `sandbox-exec` and one real Claude Code sign-in, 2026-11-07. Item 35; plan: `plans/v3.md`,
"Spike first (1.1)". Nothing under `src/` changed and no real run moved. Transcripts are on the change as
evidence, named in brackets below.

## The decision rule's outcome

**The rule is met: `gh` and git credentials can be refused while `claude` still signs in, so the rule says
Seatbelt.** It is met by a route the plan didn't assume, and that route has a cost (below). D1 and D2 are the
person's. My recommendation is Seatbelt, with D1 decided knowing the three points here:

1. **The Keychain does not split.** Seatbelt has no per-item filter: the services are open or closed. Closing
   them signs `claude` out too. Refusing only `/usr/bin/security` also signs it out, because that is how it
   reads its own item, and a model's shell is a child of the same `claude`, so a profile can't allow one and
   refuse the other.
2. **What works: close the Keychain and hand the token in.** A wrapper *outside* the sandbox reads claude's
   access token and passes it on a file descriptor, `CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR`. A shell inside
   then finds it in no variable, no ancestor's environment, no open descriptor, and no recently changed file.
3. **It breaks two things the plan relies on.**
   - **A wake can't start another signed-in `claude`.** Anything it starts gets "Not logged in" [q3-nested-claude].
     So `otto-build` couldn't run `test/live.sh` from inside a sandbox, and its goal requires that. The ways out
     are a token the model can read (a variable or file, which a model can send), or live tests run by something
     outside the sandbox. This needs a decision before 1.2's profile reaches `otto-build`.
   - **Nothing refreshes the token inside.** The access token had 4.4 hours left when I looked; the refresh token
     stays outside, in the Keychain. A wrapper that reads it at each wake depends on something unsandboxed having
     refreshed it. I did not test a wake starting on an expired token, nor a token expiring mid-wake.
     `claude setup-token` is claude's own suggestion for unattended use; it needs a person and I did not run it.

## 1. Keychain

Every probe prints an exit code or a byte count [q1-baseline, q1-closed, q1-files-only, q1-routes, q1-descriptor].

| Probe | No sandbox | Files closed only | `plans/spike-1.1.sb` |
|---|---|---|---|
| `gh auth token` | 41 bytes | 0 | 0 |
| `gh auth status` | exit 0 | exit 1 | exit 1 |
| `git credential fill` | 100 | 0 | 0 |
| `security … -s gh:github.com -w` | 75 | **75** | 0 |
| `security … -s "Claude Code-credentials" -w` | 559 | **559** | 0 |
| read or copy `login.keychain-db` | 274060 | **274060** | 0 |
| `~/.config/gh/hosts.yml` | 86 | 0 | 0 |
| `~/.git-credentials` | 0 (no such file) | 0 | 0 |
| `~/.ssh`: list; read the key | 150; 464 | 0; 0 | 0; 0 |
| `ssh-add -l` | 94 | 0 | 0 |
| `ssh -T git@github.com`, authenticated | 0 | 0 | 0 |
| `git push --dry-run` at a repo I can't write | credential sent, repo refused | no credential | no credential |

`GH_TOKEN` and `GITHUB_TOKEN` are unset on this machine, so that row isn't tested against a real value. **The
profile doesn't clear variables**: a value in the launcher's environment would reach the wake, and otto's env
guard doesn't list them. The plan's "clearing variables" layer is still needed.

Claude's sign-in is the generic password `Claude Code-credentials` in the login Keychain [q1-claude-signin].

**Routes with the Keychain closed, and what a model inside could read of each:**

| Route | Signs in | What a shell inside can read |
|---|---|---|
| `CLAUDE_CODE_OAUTH_TOKEN` variable | yes | **The token.** `printenv` shows nothing under the Bash tool, and `ps` is refused, but `sysctl KERN_PROCARGS2` from any program reads `claude`'s environment: 108 bytes [q1-parent-env]. `(deny sysctl-read (sysctl-name "kern.procargs2"))` doesn't stop it [q3-claude] |
| A credentials file in a throwaway `CLAUDE_CONFIG_DIR` | yes | **The token**: it's a file claude must read |
| `CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR` | yes, subagents too | Nothing I could find: no variable, no descriptor; 0 of 9102 recently changed files under `~/.claude`, `$TMPDIR` and `/private/tmp` contain it [q1-descriptor] |

The token is still in `claude`'s memory. One `lldb` attach from inside failed [q1-memory], which is thin: one
process, and my `vmmap` probe gave no readable result. Treat memory as not ruled out. Neither is `claude` being
steered to send the token itself. The token's scopes are inference, profile, sessions, MCP servers, plugins and
file upload, so a leaked one spends the subscription and reads the account until it expires; the network is open.

**Launchd.** A one-shot job in `gui/<uid>`, the domain poke runs in, read both items outside the sandbox, was
refused under it, and ran the wrapper behind `caffeinate` and `env -u`, and again inside a tmux server launchd
started: `pong` each time, no dialog [q3-launchd]. With only the `security` binary refused, a Keychain read from another program through the Security API
returned `-128` inside its 15 second limit [q1-routes]. No probe hung on a dialog; I can't see the screen, so that is all I can say about dialogs.

**A sign-in warning worth knowing:** `claude` prints "this sign-in ends between 2026-11-09 and 2026-11-12" on
this machine and says to run `claude auth login`. Every run depends on that sign-in, sandboxed or not.

## 2. Report mode

**Yes.** `(allow … (with report))` lets it through and logs it, with no special rights to read the log
[q2-report-mode, q2-report-all]. It appeared by the first look, about 2 seconds after the write.

```sh
mkdir -p /private/tmp/q2/would-deny
cat > /private/tmp/q2/r.sb <<SB
(version 1)
(allow default)
(allow file-write* (with report) (with message "otto-run-778") (subpath "/private/tmp/q2/would-deny"))
SB
sandbox-exec -f /private/tmp/q2/r.sb /bin/sh -c 'echo hello > /private/tmp/q2/would-deny/f'; echo "exit $?"
sleep 3
log show --last 3m --style compact --predicate 'sender == "Sandbox" AND eventMessage CONTAINS "otto-run-778"'
```

The result names the operation and path: `Sandbox: bash(12846) allow file-write-create /private/tmp/q2-probe/would-deny/f`.

- **Attribution works through `with message`.** Two profiles running at once, each with its own message, did not
  mix, and a child process kept its parent's message. All 2000 records of a burst were kept. So 1.7 can put the
  run's id in each profile's message.
- **Keep `sender == "Sandbox"` in the predicate**: without it `log show` finds its own command line.
- **Report everything:** `(allow default (with report) (with message "…"))` also works and logs `mach-lookup`,
  but a two-command shell gave 417 lines. `otto sandbox observe` will have to filter hard.
- **A real denial** carrying a message is found the same way, as `deny(1) file-write-create`.
- A sandboxed process can also write to the log, so a hostile wake could add false lines. Not tested.

## 3. Other things a wake needs

All under the profile [q3-tools, q3-cargo, q3-claude]:

| Thing | Result | What it took |
|---|---|---|
| Subagents | work | nothing; in the same sandbox, `gh auth token` 0 bytes |
| MCP, stdio | works | nothing. Remote connectors not tried |
| `bun` run, install, `bun test` (legenda unit: 62 pass) | work | nothing. Playwright not tried |
| `uv run`, `uv venv`, `uv pip install` | work | nothing |
| `cargo test` | **653 passed, 0 failed**, 155s | unset `TMUX` |
| git fetch over https, commit | work | nothing |
| curl over https | works | closing `trustd.agent` doesn't break TLS |
| `gh api`, public repo | **fails** | closing `~/.config/gh` fails `gh` at startup; with an empty `GH_CONFIG_DIR` it asks for a sign-in. `gh` is unusable, even for public reads, without a token |
| nested `sandbox-exec` | **refused**, exit 71, `sandbox_apply: Operation not permitted` | can't be nested. No test uses it today; any test of 1.2 that starts one will fail inside a sandboxed wake |
| a stub launcher in tests | works | part of `cargo test` |
| `ps` | **refused**, exit 126 | otto's source doesn't call it; anything else that does will fail |
| a second `claude` | **"Not logged in"** | see the decision rule above |

**tmux: yes, a profile can refuse the real server and allow a private one.** Both `tmux ls` and `-S` by path get
`Operation not permitted` on `/private/tmp/tmux-<uid>/default`. A private server under `TMUX_TMPDIR` starts and
runs, and a pane in it is still confined: 0 bytes from `hosts.yml` and from the Keychain. This relies on the
real server being at its default path; a server on any other socket isn't refused.

## 4. Private repos

**None in play are private** [q45]. `projects` in `~/.otto/config.json`, the repos of each active run and
`architect-build`'s six: cloudgpu, comfy-edit, lectio, legenda, omnifactum and otto are all public, and an
anonymous `git ls-remote` exits 0 on each. `~/workspace` itself and `cloudgpu-queue` have no `origin`. So fetching
needs no credentials.

## 5. Ship scripts

**None** [q45]. In all six I listed the top level, `scripts`, `bin`, `tools`, `.github/workflows` and
`package.json` scripts, and searched tracked `*.sh`, Makefiles, justfiles, workflows and `scripts/` for `git push`,
`gh pr create|merge` and `gh release`, and `CLAUDE.md` and `AGENTS.md` for how to ship: no hits. So 1.5 has no
script to replace. Otto's own landing text still says "or the repository's own ship script", which 1.5 will have
to decide about.

## Does the launcher mechanism fit `sandbox-exec`?

**Not as it stands; a small wrapper does, and needs no change to otto for this much.** `sandbox-exec` has
`-D name=value`, and no flag that takes a directory, so `grantFlag` has nothing to name. The token forces a wrapper
regardless, since something outside the sandbox has to read it. Tested chain:

```
caffeinate -i -s env -u … <wrapper> '<prompt>' --add-dir … --session-id … [under tmux, under launchd]
  wrapper: read the token → descriptor 3 → exec sandbox-exec -D HOME=… -f <profile> claude "$@" 3<&3
```

For 1.2 this profile grants nothing per run, since it allows everything by default. A profile that denies by
default needs the directories, and I'd have the wrapper take them: `grantFlag: "--allow"` gives it
`--allow <dir>` before the first `--`, and it generates a profile per run. That wrapper's grant handling isn't
built or tested here. It's worth noting that the wrapper lives where a wake must not write.

## One real wake [wake-transcript]

A throwaway `OTTO_HOME` and private tmux server, with a launcher in that home's config running the wrapper, started
by `otto run` in tmux. It read `run.json`, made a worktree, committed, submitted a change, wrote its handoff and
exited as a complete wake (`wake-complete`, 18 turns, no incomplete wake). Its own probes: `gh auth token` 0
bytes, the `security` read 0 bytes, `git push --dry-run` exit 128. Not started by poke; the launchd half was
tested separately above.

## The profile: what `plans/spike-1.1.sb` closes and leaves

Closes: the Keychain, whole; `~/.ssh`, `~/.config/gh`, `~/.git-credentials`; the ssh agent; the real tmux server.
It allows everything else by default, so it isn't a design for 1.2.

| Escape from the plan's table | State under this profile |
|---|---|
| Talking to outside processes: tmux | closed, by test |
| `launchctl` | can read (107718 bytes). `bootstrap`, `submit` from inside were refused and `asuser` stayed confined (0 bytes). Better than the plan assumed, but still 1.2's to deny |
| `osascript` | runs. Scripting another app wasn't tried |
| `open` | not tried, because it would act on the person's screen |
| Persistence | **open**: `~/Library/LaunchAgents` writable, so a job could load outside the sandbox at next login. `~/.zshenv` wasn't writable here only because it doesn't exist; home is writable |
| Claude settings | **open**: `~/.claude/settings.json` writable, and its hooks run code |
| Other otto state | **open**: `~/.otto/config.json` writable, **so a wake could remove its own sandbox from the next wake's launcher** |
| Self-approval, git hooks and config, check scripts, lander, `otto-build` installing | **open**; 1.3, 1.4, 1.5, D2 |
| Other credentials in files | **open**: only the three paths above are named. `.netrc`, `.npmrc`, cloud keys and apps' `.env` files are readable if present; I did not survey them |

The network is open, as the plan's escape table doesn't address it. A token a model can read can be sent.

## What makes the plan wrong, or incomplete

1. "Seatbelt profile, plus clearing credential variables and helpers" isn't enough: it also needs a wrapper that hands
   in the token, and a way to keep that token fresh.
2. The plan's goal text for `otto-build` assumes live tests run inside a wake. Inside a sandbox they can't sign in.
   Its table doesn't mention this.
3. Live tests of the sandbox itself can't run inside a sandbox, since `sandbox-exec` doesn't nest.
4. `gh` is unavailable to sandboxed wakes even for public reads, which matters to any run that reads PRs. 1.5's lander
   will have to fetch those another way.
5. Otto's `$OTTO_HOME/config.json` is the most direct escape I found, for what the plan already lists under 1.2.

## What I did not manage to test

An expired or mid-wake token; `claude setup-token`; a wake started by the real poke, as opposed to its launchd
domain; remote MCP connectors; Playwright; reading `claude`'s memory beyond one failed attach; `GH_TOKEN` set to a
real value; `open`, or AppleScript driving another app; forging log records.

## How this was run, including one failure

Probes print exit codes and byte counts. **One broke that:** a probe with its redirect covering half of a
`cat a || cat b` printed a passphrase-protected ssh private key into a session log, and so to the model's API. It
never reached this repo or any evidence; the person was told at once with the details and decides about the key.
The probes were rewritten so that output goes straight to `wc -c`.

Nothing real changed: no Keychain item, no real `~/.otto/config.json`, no `~/.otto/sandbox/`, no push. The temporary
launchd jobs were removed and none remain. A `git push --dry-run` went at `octocat/Hello-World`, which this account
can't write. No real run moved into a sandbox. The sessions run for this left logs under `~/.claude/projects/`.

## Reproducing it

```sh
# From the repo root. Each probe in question 1 under the profile; expect the third column above.
P="sandbox-exec -D HOME=$HOME -f plans/spike-1.1.sb"
$P gh auth token 2>/dev/null | wc -c
printf 'protocol=https\nhost=github.com\n\n' | $P git credential fill 2>/dev/null | wc -c
$P security find-generic-password -s gh:github.com -w 2>/dev/null | wc -c
$P cat ~/.config/gh/hosts.yml 2>/dev/null | wc -c
$P cat ~/.ssh/id_ed25519 2>/dev/null | wc -c
$P cat ~/Library/Keychains/login.keychain-db 2>/dev/null | wc -c
$P ssh-add -l 2>/dev/null | wc -c
$P claude -p 'Reply with the single word pong' </dev/null   # Not logged in: the Keychain is closed

# Signed in, with the token on a descriptor. Save as a launcher, or run it from a script:
exec 3< <(security find-generic-password -s "Claude Code-credentials" -w 2>/dev/null \
  | python3 -Ic 'import json,sys; print(json.load(sys.stdin)["claudeAiOauth"]["accessToken"],end="")')
CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR=3 $P claude -p 'Reply with the single word pong' </dev/null 3<&3
```

Run that with bash; the descriptor line is bash syntax, and zsh doesn't split `$P`. For the real wake, put the last
three lines in an executable script with `"$@"` in place of the prompt, name it as a launcher in a throwaway
`$OTTO_HOME/config.json`, and set `OTTO_HOME` and `TMUX_TMPDIR` to throwaway directories with `TMUX` unset.
