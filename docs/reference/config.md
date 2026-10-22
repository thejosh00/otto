# `config.json`

`~/.otto/config.json` (`$OTTO_HOME/config.json`) holds settings that belong to the machine rather
than to any one run. Unknown keys are refused, so a typo is an error rather than a setting that
silently does nothing.

```json
{
  "workdir": "~/work",
  "launchers": [
    { "name": "nono (sandbox)", "command": "nono run --profile nolabs-ai/claude -- claude", "grantFlag": "--allow" }
  ],
  "lander": "9",
  "processes": [
    { "name": "reviewed", "mode": "pr", "mergeWhenReady": true, "land": "Squash-merged; CI must pass first" }
  ],
  "projects": { "/Users/me/work/svc": "reviewed" }
}
```

## `workdir`

The directory every wake runs in, for runs created without `--workdir`. `~` is expanded, and it
must exist. The first `otto run` asks for it on a terminal and saves the answer here; change it
with:

```bash
otto config workdir ~/work     # set it
otto config workdir            # print it
```

A run records its working directory, resolved to an absolute path, when it is created — so
changing the default affects new runs only. Every wake of a run starts there, whoever starts it:
the reviver, the web UI, or a terminal. With neither a default nor `--workdir`, `otto run` refuses
off a terminal, and the web form asks for one.

## `launchers`

A list of launchers a run can use besides `claude` — optional; with none, `claude` is the only
one. See [sandboxing](../guides/sandboxing.md) for how they're used.

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | What `otto run --launcher` takes (a unique prefix will do, ignoring case), what the web UI offers, and what a run records |
| `command` | yes | The command line, split on whitespace, that ends in running claude. otto appends the wake's prompt and claude's flags to it |
| `grantFlag` | no | The flag that opens a directory in the sandbox (`--allow` for nono). otto passes it once each for `$OTTO_HOME`, the run's working directory, and every `--repo`, before the command's first `--`, or at the end of the command if it has none |

A launcher named `claude` replaces the built-in one — for example, to run claude from a
different path.

A run stores the launcher's **name**. Editing an entry changes the next wake of every run using
it; removing one makes those runs refuse to wake until it is back, or until they're recreated
with another launcher.

## `lander`

The run that lands approved [changes](../concepts.md#changes-code-handed-back) — the only one that
pushes them. It is kept by run id, so renaming the run doesn't lose it. Without one, approving a
change is refused, since nothing would land it.

```bash
otto config lander lander      # make the run named lander the lander
otto config lander             # print which run is
otto config lander --off       # none
```

otto makes one and names it here (a [system agent](../concepts.md#system-agents)); this is for
naming another.

## `processes`

How a change goes from started to landed, for the repositories that follow it — optional; with
none, the built-in `standard` and `direct` are the only ones. A process named like a built-in one
replaces it.

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | What `otto config project --process` takes, ignoring case |
| `mode` | yes | What approving a change does: `push` (lands it on its base) or `pr` (opens a pull request the babysitter watches) |
| `mergeWhenReady` | no | With `pr`: GitHub merges it once it is approved and passing, without asking again |
| `start` | no | How a change begins, in words, for the run that makes it: a worktree or a branch, a ticket first |
| `land` | no | How an approved change lands, in words, for the lander: a ship script, a merge method |
| `allowChanges` | no | With `pr`: what the babysitter may change on the pull request without you, in words it judges by — `trivial`, `small`, `anything but security`. Absent, or `none`: nothing |

`mode` and `mergeWhenReady` are otto's to act on; `start`, `land` and `allowChanges` are read by the
agents. How the babysitter reads `allowChanges` is its judgement, and the risk of it is yours: a
bigger change than you meant can reach the pull request without you. Approving a change copies it
onto the change, where `otto changes allow` (or the change's page) changes it for that one. The
built-in `standard` opens a pull request, and `direct` pushes to the base; both start each change in
a worktree of its own. A pull request needs a remote to open it on: in a repository without one,
approving is refused until it follows `direct`.

```bash
otto config process                                       # every process, and who follows it
otto config process reviewed --mode pr --merge-when-ready # make one, or change one
otto config process reviewed --land "Squash-merged"       # change only what it says about landing
otto config process reviewed --allow-changes "trivial"    # what the babysitter fixes without you
otto config process reviewed --delete                     # one a repository follows is refused
```

## `projects`

The process each repository follows, keyed by its main checkout's absolute path. A repository not
listed follows `standard`. Any checkout of a repository, or a directory inside one, names it.

```bash
otto config project ~/work/svc --process reviewed   # give it a process
otto config project ~/work/svc                      # print the one it follows (--json for agents)
otto config project ~/work/svc --process standard   # back to standard
otto config project                                 # every repository given one
```

Approving a change records the process it was approved under, with its `land` words, so changing a
repository's process changes only what is approved after.

## Environment variables

| Variable | Meaning |
|---|---|
| `OTTO_HOME` | Where otto keeps its data, instead of `~/.otto`. `otto agent start` and `otto service start` pass it on to the launchd jobs |
| `OTTO_BIN_DIR` | Where `./install.sh` puts the binary, instead of `~/.local/bin` |
| `OTTO_REPO` | The otto checkout `otto install` symlinks skills from, when not run inside it |
