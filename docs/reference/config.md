# `config.json`

`~/.otto/config.json` (`$OTTO_HOME/config.json`) holds settings that belong to the machine rather
than to any one run. Unknown keys are refused, so a typo is an error rather than a setting that
silently does nothing.

```json
{
  "workdir": "~/work",
  "launchers": [
    { "name": "nono (sandbox)", "command": "nono run --profile nolabs-ai/claude -- claude", "grantFlag": "--allow" }
  ]
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

## Environment variables

| Variable | Meaning |
|---|---|
| `OTTO_HOME` | Where otto keeps its data, instead of `~/.otto`. `otto agent start` and `otto service start` pass it on to the launchd jobs |
| `OTTO_BIN_DIR` | Where `./install.sh` puts the binary, instead of `~/.local/bin` |
| `OTTO_REPO` | The otto checkout `otto install` symlinks skills from, when not run inside it |
