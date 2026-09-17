---
name: manage-pr
description: "Prepare a branch for review in an isolated git worktree: pull the base branch to latest and rebase onto it if behind (resolving conflicts when possible), run the repo's pre-commit hooks (at their pinned versions) and the test suite, then commit the result and report what happened. Operates on a specific branch (asks if not given), works inside the workspace at .work-trees/manage-pr, and only ever uses git to commit to the branch — it never pushes and never touches PRs (you push and open/merge the PR manually). Removes the worktree once its branch is merged. Also has an opt-in 'babysit' mode that self-schedules ~hourly to rebase on upstream and apply confidently-actionable PR review comments, stopping as soon as it makes a green change (the only mode that reads PRs via gh; still never pushes or writes to PRs). Use when the user asks to get a branch ready for review, rebase-and-test before merging, 'manage'/'prep' a PR, or 'babysit'/'watch' a branch."
---

# Manage PR

Operates on a **branch** in an **isolated git worktree** so your current checkout is never disturbed. Pipeline: **set up worktree → pull base to latest → rebase onto base if behind (resolving conflicts when possible) → run the repo's pre-commit hooks (at their pinned versions) → run tests → commit any fixes → report readiness (with suggestions to improve this skill)**. It **only uses git to commit to the branch**: it **never pushes**, and it **never does PR handling** (no `gh`, no `gh pr create`) — you push and manage the PR manually. It **removes the worktree once the branch is merged**. The rebase → checks stages run via the **Workflow** tool for deterministic, gated control flow.

It also has an opt-in **Babysit mode** (see below): a self-scheduling ~hourly watch that rebases the branch when the base moves and applies review comments it can make confidently, then stops the moment it has made a green change. Babysit is the *only* mode that touches `gh`, and only to *read* PR comments — it still never pushes or writes to PRs.

## Where worktrees live

Under the **workspace root** (your primary working directory) at `.work-trees/manage-pr/` — e.g. `/Users/joshuahill/workspace/.work-trees/manage-pr/`. Historically this skill used `~/.work-trees`, but that path is **not writable in this environment** — worktrees must live inside the workspace. Throughout this doc, `<WT_BASE>` means `<workspace-root>/.work-trees/manage-pr`.

## Environment assumptions

This skill runs in a **write-restricted sandbox** with a **default-deny network allowlist** (yolo's `allowed-hosts.txt`, active copy at `~/.yolo/allowed-hosts.txt`). Assume up front (don't rediscover the hard way):

- **Writes are confined to the workspace**, so worktrees live in the workspace (`<WT_BASE>`), never under `~/.work-trees`. **But `~/.cache/pre-commit` IS writable** — pre-commit can build and reuse its hook environments there.
- **Network is allowlist-gated, not absent.** `git fetch` (`source.datanerd.us`), **PyPI** (`pypi.org`, `files.pythonhosted.org`), and **GitHub** (`github.com`) are on the allowlist, so `git fetch`, `uv`/`uvx`, and `pre-commit install-hooks` all work. Anything not listed is denied — treat a specific failing host as environmental (unverified/skipped-env), not a code failure, and keep remote ops best-effort. `gh` is out of scope regardless (PR handling is manual). If a needed host is missing, the fix is to add it to `allowed-hosts.txt` (takes effect on the next yolo launch), then re-run.
- **Prefer `pre-commit` itself** for the lint stage — it installs each hook's tool at the exact pinned `rev` and runs the correct file scope. Hand-invoking a `.venv` binary is a **fallback only**: the venv's tool versions can differ from the hooks' pinned `rev`s (e.g. venv `ruff` behind the pinned `ruff-pre-commit`), which silently produces false-green results. See Step 4.

## When to use

- "Get branch X ready for a PR" / "prep the PR for X" / "manage this PR"
- "Rebase and run the checks before I merge"
- "Is branch X ready to push?"

## Step 0 — Reap merged worktrees (git only)

Before anything else, clean up finished work. For each directory under `<WT_BASE>/` (skip if the dir doesn't exist):

1. Get its branch: `git -C <dir> rev-parse --abbrev-ref HEAD`.
2. Determine its base: prefer the value recorded at creation — `cat "$(git -C <dir> rev-parse --git-path manage-pr-base)" 2>/dev/null` — and fall back to `git -C <dir> symbolic-ref refs/remotes/origin/HEAD` (strip `refs/remotes/origin/`), commonly `main` / `master`, only if the marker is missing.
3. Best-effort refresh refs, ignoring failure (no network is normal here): `git -C <dir> fetch --prune origin 2>/dev/null` (prune so deleted remote branches disappear locally — see step 4's squash heuristic).
4. Decide if it's done and can be reaped:
   - **Contained-in-base (merge/rebase merges):** `git -C <dir> rev-list --count <branch> ^origin/<base>`. `0` → the branch is fully contained in the base → **merged**. Remove it: `git -C <dir> worktree remove <dir>` (run with your shell cwd OUTSIDE `<dir>`). If it refuses because the tree is dirty, tell the user and let them decide — don't `--force` away uncommitted work. Optionally offer to delete the now-merged local branch.
   - **Squash-merge heuristic:** a squash-merge leaves the branch NOT contained in base, so the count above stays non-zero even though the PR merged. As a secondary signal, if the remote branch has vanished after the prune — `git -C <dir> rev-parse --verify --quiet refs/remotes/origin/<branch>` returns nothing (and it previously tracked a remote) — the branch was very likely squash-merged and deleted. In that case **offer** to reap it (don't auto-remove; confirm with the user, since a vanished remote ref can also mean someone deleted an unmerged branch).
   - **Otherwise** (still contained-nowhere and remote branch still present) → **do not remove it**; list the worktree and let the user decide whether it's done.

Then prune stale registrations — if a worktree directory was deleted manually (e.g. `rm -rf`), git still lists it. Sweep those out by running `git -C <repo> worktree prune` for each repo whose worktrees live under `<WT_BASE>/`. (If you haven't resolved a repo yet, you can also `cd` into any surviving worktree and run `git worktree prune` there — it prunes for that worktree's repo.)

Mention which worktrees you reaped or pruned, which you left for the user to judge, or that there were none.

## Step 1 — Determine the branch (ask if not given)

- If the user named a branch (as an argument or in the request), use it.
- If not, **ask which branch** to manage. To help them choose, list recent local branches: `git -C <repo> branch --sort=-committerdate | head`. (Do **not** use `gh` to list PRs — PR handling is out of scope.)
- You also need the **repo** the branch lives in. If the current directory is inside a git repo, use it. Otherwise ask for the repo path (the workspace root itself is not a repo). Record its root: `git -C <repo> rev-parse --show-toplevel`. Keep this path — call it `<mainRepo>`; the pipeline needs it to locate a populated `.venv` (see Step 4).

## Step 1.5 — Load the repo's build-info (do this at the START — it can affect any later step)

As soon as you know the repo (`<repoName>`), read its per-repo build-info memory: **`<workspace-root>/memory/build-info/<repoName>.md`** (e.g. `/Users/joshuahill/workspace/memory/build-info/sre-orchestration-service.md`). This is a small store of repo-specific build/setup knowledge that this skill has learned — and it can bear on **any** stage, not just worktree file setup:

- untracked local files a fresh worktree needs (`.env`, credentials, fixtures — applied in Step 2.6);
- the right test command or how to run it (feeds Step 3.5 / Step 4's `testCmd`);
- known rebase-conflict hot spots, hooks that need special handling, push-scope quirks, etc.

Read it now and **keep it in mind through the whole pipeline** — reference it in each step it touches rather than rediscovering the same friction. If the file (or the `build-info/` dir) doesn't exist yet, that's fine: proceed with defaults, and whenever you hit and resolve repo-specific friction during this run (a missing local file, a test-command workaround, a recurring conflict), **record it in `build-info/<repoName>.md`** so the next run starts informed (see the store's `README.md` for the one-file-per-repo convention). Mention in your Step 3 plan summary whether build-info was found and what it told you.

## Step 2 — Create / reuse the worktree

1. Create the base dir under the workspace: `mkdir -p <WT_BASE>`.
2. Compute a path: `<WT_BASE>/<repoName>-<branchSlug>` where `<branchSlug>` is the branch with `/` replaced by `-`.
3. Best-effort make the branch present locally: `git -C <mainRepo> fetch origin <branch> 2>/dev/null` (ignore failure — a purely local branch, or no network, is fine; a cached remote-tracking ref is enough).
4. If a worktree already exists at that path (`git -C <mainRepo> worktree list` shows it), reuse it. Otherwise create it:
   - Existing local branch: `git -C <mainRepo> worktree add <path> <branch>`
   - Remote-only branch: `git -C <mainRepo> worktree add <path> --track origin/<branch>`
5. **Record the base for later reaping.** Determine the base now (Step 3.1) and stash it in the worktree's private git dir (outside the working tree, so it never dirties `git status`): `echo <base> > "$(git -C <path> rev-parse --git-path manage-pr-base)"`. Also stash the main repo path the same way (`manage-pr-mainrepo`) so Step 0 can remove the worktree from the right repo. Step 0 reads these back instead of re-deriving.
6. **Set up untracked local files the checks need (the fresh-worktree gotcha).** A worktree contains only *tracked* files — anything `.gitignore`d (`.env`, local config, credential/fixture files) is absent, so hooks and tests that read them fail in the worktree with errors that never happen in the developer's main checkout (e.g. `error: No environment file found at: .env`). Do this **before** running the pipeline, and (when reusing a worktree) only for files that are missing:
   - **Apply the build-info you loaded in Step 1.5:** if `build-info/<repoName>.md` listed local-file setup, run exactly what it says (which files, the copy commands, why).
   - **Default `.env` heuristic (when build-info is silent):** if the worktree has no `.env` but the repo ships a `.env.example` / `.env-example` (or the mainRepo has a real `.env`), create one — **prefer copying `<mainRepo>/.env`** (it has the developer's actual values the hooks/tests read), else copy the example placeholder: `cp <mainRepo>/.env <path>/.env` || `cp <path>/.env.example <path>/.env`. These files are gitignored, so copying them in does **not** dirty `git status`. If you discovered this need for the first time this run, add it to `build-info/<repoName>.md` per Step 1.5.
   - Note which local files you set up when you summarize the plan (Step 3), and if a check still fails on a *different* missing local file, that's the same class of problem — set it up and record it in build-info.
7. From here on, the worktree path IS the repo the pipeline operates on. Tell the user where it is.

## Step 3 — Gather pipeline context (inline, no mutations)

In the **worktree** (`<path>`):

1. Base branch (git only): `git -C <path> symbolic-ref refs/remotes/origin/HEAD` → strip `refs/remotes/origin/`. Common: `main` / `master`. Do **not** query `gh`.
2. Working tree state: `git -C <path> status --porcelain`.
3. Behind/ahead vs base: `git -C <path> rev-list --count <branch>..origin/<base>` (behind) and `git -C <path> rev-list --count origin/<base>..<branch>` (ahead). If the base ref is stale because you couldn't fetch, say so. **Also compute ahead-of-remote-branch:** `git -C <path> rev-list --count origin/<branch>..<branch> 2>/dev/null` (0, or empty, if there's no remote branch yet). If this is > 0, the remote branch is behind local, which **widens the pre-push hook scope** at push time — record it for Step 4's push-scope check.
4. Lint setup: read `.pre-commit-config.yaml` to see which hooks and pinned `rev`s apply. The checks stage runs **`pre-commit` itself** (after `pre-commit install-hooks`) so tools match their pinned versions — do **not** plan to hand-invoke `.venv` tools as the primary path (that's a fallback for when pre-commit genuinely can't run). Just confirm the config exists.
5. Test command: infer from the repo (`pyproject.toml`/`uv.lock` → `pytest`; `package.json` `test`; `go test ./...`; `cargo test`; `Makefile` `test`, etc.).

Summarize the plan (worktree path, base branch, any untracked local files you set up per Step 2.6 — e.g. copied `.env`, whether a rebase looks needed **and whether the rebase gate will act on it or defer** — see Step 4's gate: when behind, the pipeline rebases only if the branch is >`staleAfterHours` (default 24h) out of date OR the base's new commits overlap files the branch modifies, else it defers to avoid churn — that pre-commit hooks will run at their pinned versions scoped to the PR diff, whether the remote branch is behind local (push-scope check), the test command).

## Step 4 — Run the pipeline via Workflow

Invoke the **Workflow** tool with the script below, passing context as `args` (a real JSON object). `repoRoot` MUST be the worktree path; `mainRepo` is the original checkout from Step 1 (used to find a populated `.venv` / tool binaries when the worktree has none):

```json
{
  "repoRoot": "/Users/joshuahill/workspace/.work-trees/manage-pr/myrepo-feature-x",
  "mainRepo": "/Users/joshuahill/workspace/myrepo",
  "branch": "feature/x",
  "base": "main",
  "runLint": true,
  "testCmd": "uv run pytest",
  "aheadOfRemote": 0,
  "staleAfterHours": 24
}
```

If `runLint` is false, the lint stage is skipped and reported as skipped. If `testCmd` is empty/null, the test stage is skipped and reported as skipped — never silently treated as passing. `aheadOfRemote` is the Step 3.3 ahead-of-remote-branch count (commits the local branch has that its remote tracking branch lacks); pass it so the lint stage knows whether to run the push-scope check. Omit or pass `0` when there's no remote branch yet — the lint stage self-computes it if absent. `staleAfterHours` (default 24) is the **rebase-gate staleness threshold**: when the branch is behind, the Rebase stage rebases only if the branch has been out of date longer than this OR the base's new commits overlap files the branch modifies — otherwise it DEFERS the rebase (status `deferred`) to avoid churn on a fast-moving base, and the Checks stage still runs on the un-rebased branch. This gate applies to BOTH the one-shot prep pipeline and babysit ticks; lower it (e.g. to `0`) to force an always-rebase-when-behind run. The stages are **sequential on purpose**: they read/mutate one working tree. Do not parallelize and do not add `isolation: 'worktree'` (the pipeline already runs inside the worktree you created).

```javascript
export const meta = {
  name: 'manage-pr-pipeline',
  description: 'In an isolated worktree, rebase branch onto base if behind (resolving conflicts when possible), run the repo pre-commit hooks at their pinned versions and the test suite, commit any fixes, then report readiness. Only git-commits to the branch — never pushes, never touches PRs.',
  phases: [
    { title: 'Rebase', detail: 'pull base to latest, rebase if behind, attempt to resolve conflicts' },
    { title: 'Checks', detail: 'run pre-commit hooks (pinned versions), then the test suite' },
  ],
}

const cfg = args || {}
if (!cfg.repoRoot || !cfg.branch || !cfg.base) {
  return { ok: false, stage: 'config', error: 'repoRoot (worktree path), branch and base are required in args' }
}

const REBASE = {
  type: 'object', additionalProperties: false,
  properties: {
    status: { type: 'string', enum: ['clean', 'rebased', 'rebased-with-resolution', 'deferred', 'conflict', 'error'] },
    behindBy: { type: 'number' },
    deferredReason: { type: 'string' },
    summary: { type: 'string' },
    resolvedFiles: { type: 'array', items: { type: 'string' } },
    unresolvedFiles: { type: 'array', items: { type: 'string' } },
    suggestions: { type: 'array', items: { type: 'string' } },
  },
  required: ['status', 'summary'],
}
const LINT = {
  type: 'object', additionalProperties: false,
  properties: {
    ran: { type: 'boolean' },
    passed: { type: 'boolean' },
    summary: { type: 'string' },
    hooks: {
      type: 'array',
      items: {
        type: 'object', additionalProperties: false,
        properties: {
          id: { type: 'string' },
          command: { type: 'string' },
          status: { type: 'string', enum: ['pass', 'fail', 'fail-preexisting', 'skipped-env', 'skipped-na'] },
          details: { type: 'string' },
        },
        required: ['id', 'status'],
      },
    },
    suggestions: { type: 'array', items: { type: 'string' } },
  },
  required: ['ran', 'passed', 'summary'],
}
const CHECK = {
  type: 'object', additionalProperties: false,
  properties: {
    ran: { type: 'boolean' },
    passed: { type: 'boolean' },
    summary: { type: 'string' },
    details: { type: 'string' },
    suggestions: { type: 'array', items: { type: 'string' } },
  },
  required: ['ran', 'passed', 'summary'],
}

// ---- Stage 1: Rebase (attempt conflict resolution) --------------------------
phase('Rebase')
const rebase = await agent(
  `You are working in the git worktree at ${cfg.repoRoot}, checked out to branch "${cfg.branch}".
Do EXACTLY this:
1. PULL THE BASE TO LATEST FIRST so the rebase targets current ${cfg.base}. cd into the worktree and run: git fetch origin ${cfg.base}
   - On success, origin/${cfg.base} now points at the latest upstream base — rebase onto that.
   - If the fetch FAILS (e.g. no network access to the remote — common in this environment), that is NOT fatal: fall back to the cached remote-tracking ref origin/${cfg.base} that already exists locally, and NOTE in your summary (and add a suggestion) that the base could not be refreshed and may be stale.
2. Commits behind: git rev-list --count ${cfg.branch}..origin/${cfg.base}
3. If behind == 0: return status "clean", behindBy 0.
3a. REBASE GATE (avoid churn on fast-moving bases) — if behind > 0, do NOT rebase unconditionally. Rebase this run ONLY if AT LEAST ONE of these holds; otherwise DEFER:
    (i) STALE > threshold: the branch has been out of date for more than ${typeof cfg.staleAfterHours === 'number' ? cfg.staleAfterHours : 24} hours. Compute the age of the OLDEST base commit the branch is missing (that is when the branch first fell behind):
        oldest=$(git rev-list --reverse ${cfg.branch}..origin/${cfg.base} | head -1)
        age_h=$(( ( $(date +%s) - $(git show -s --format=%ct "$oldest") ) / 3600 ))
      STALE is true when age_h > ${typeof cfg.staleAfterHours === 'number' ? cfg.staleAfterHours : 24}. (Use the shell's own \`date +%s\` for "now" — do NOT rely on any other clock.)
    (ii) FILE OVERLAP: the base's new commits touch one or more files the branch also modifies (real conflict risk / semantic overlap = "changes that need to be merged"). Compute:
        mb=$(git merge-base ${cfg.branch} origin/${cfg.base})
        comm -12 <(git diff --name-only "$mb" ${cfg.branch} | sort -u) <(git diff --name-only "$mb" origin/${cfg.base} | sort -u)
      OVERLAP is true when that intersection is NON-EMPTY.
    - If NEITHER (i) nor (ii): DEFER — do NOT rebase. Return status "deferred", behindBy set, and deferredReason like "behind by N but only <${typeof cfg.staleAfterHours === 'number' ? cfg.staleAfterHours : 24}h out of date (age_h=<n>) and no overlap with branch files — deferring rebase to avoid churn". This is a SUCCESS, not an error: the Checks stage still runs on the un-rebased branch. Do NOT touch the tree.
    - If (i) OR (ii): proceed to step 4 and rebase. In your summary, state WHICH trigger fired (stale age_h=<n>, and/or the overlapping files).
4. REBASE (only when the gate in 3a says to): check git status --porcelain first. If the tree is dirty with UNCOMMITTED changes, DO NOT rebase — return status "error" saying there are uncommitted changes. If clean, run: git rebase origin/${cfg.base}
   - Clean rebase (no conflicts): return status "rebased", behindBy set, resolvedFiles [].
   - On conflict: ATTEMPT TO RESOLVE. For each conflicted file (git diff --name-only --diff-filter=U): read it, understand BOTH sides of every conflict hunk and the intent of the branch's change vs base, and resolve by integrating both intents — never blindly pick one side, never delete the other side's work. Remove all conflict markers, git add the file, then git rebase --continue. Repeat for each conflicted commit.
     - Only resolve hunks you can resolve CONFIDENTLY. If a hunk is genuinely ambiguous or you'd be guessing, do NOT guess: run "git rebase --abort" to restore the pre-rebase state and return status "conflict" with unresolvedFiles listing the files/areas that blocked you and why.
     - After a successful resolution, confirm no leftover conflict markers remain (grep -rn '^<<<<<<<\\|^=======\\|^>>>>>>>' should be empty) and return status "rebased-with-resolution" with resolvedFiles listing what you resolved. The Checks stage will validate your resolution via tests.
Return the structured result only. NEVER push or force-push anything. NEVER run gh or touch any PR.
If you hit any friction, ambiguity, or environmental workaround while doing this, add short, actionable notes to a "suggestions" array for how THIS skill/pipeline could be improved (omit if none).`,
  { label: 'rebase', phase: 'Rebase', schema: REBASE }
)
if (!rebase || rebase.status === 'conflict' || rebase.status === 'error') {
  return { ok: false, stage: 'rebase', rebase, suggestions: (rebase && rebase.suggestions) || [] }
}
log(`Rebase: ${rebase.status} (behind by ${rebase.behindBy ?? 0})`)

// A rebase REWRITES the branch, so origin/<branch> falls behind local HEAD and the
// pre-push scope widens even when Step 3's pre-rebase ahead-of-remote count was 0.
// The lint stage must run the push-scope check whenever this is true, not just when
// aheadOfRemote > 0.
const rebasedThisRun = rebase.status === 'rebased' || rebase.status === 'rebased-with-resolution'

// ---- Stage 2a: Lint (run pre-commit at PINNED versions, scoped to the PR diff) ----
phase('Checks')
let lint = { ran: false, passed: true, summary: 'skipped (runLint false)', hooks: [] }
if (cfg.runLint) {
  lint = await agent(
    `In the git worktree at ${cfg.repoRoot}, run the repository's pre-commit hooks (lint/format/type checks) at their PINNED versions. Read .pre-commit-config.yaml first for the hook ids and pinned revs.

PRIMARY PATH — use pre-commit itself. This guarantees each tool runs at its pinned rev and at the correct file scope; hand-invoking .venv tools caused version-mismatch false-greens before (e.g. venv ruff 0.13.3 passing while the pinned ruff 0.15.9 reformats a file).
1. Locate the pre-commit binary: try "pre-commit" on PATH, else ${cfg.repoRoot}/.venv/bin/pre-commit, else ${cfg.mainRepo ? cfg.mainRepo + '/.venv/bin/pre-commit' : '<mainRepo>/.venv/bin/pre-commit'}.
2. Install the hook environments (idempotent; uses ~/.cache/pre-commit which is writable, and clones hook repos from github.com + installs tools from pypi.org/files.pythonhosted.org, all allowlisted):
     cd ${cfg.repoRoot} && <pre-commit> install-hooks
   If this fails ONLY for environmental reasons (a host not on the allowlist, or the cache not writable), drop to the FALLBACK PATH below and clearly flag it. A hook-build failure is environmental (skipped-env), NOT a code failure.
3. Run the commit-stage hooks scoped to THIS PR's diff — the files that change between the base and the branch tip, i.e. what will merge into ${cfg.base}:
     <pre-commit> run --from-ref origin/${cfg.base} --to-ref HEAD
   Use --from-ref/--to-ref (NOT --all-files): pre-commit then checks exactly the files changed in that range, so pre-existing drift on untouched files is naturally out of scope.
4. AUTO-FIX handling: several hooks rewrite files (ruff --fix, ruff-format, isort, end-of-file-fixer, trailing-whitespace, uv-lock) and exit non-zero when they change something. That is a FIX, not a failure. After the run: git status --porcelain — if hooks modified files, git add -A && git commit -m "chore: pre-commit fixes", then RE-RUN step 3. Repeat until the run exits 0, or a NON-fixing hook (mypy, a local check) fails. Never hand-edit source to satisfy a linter. Never push. Never run gh.
5. A hook that still fails after fixes (a mypy type error, a failing local hook) is a REAL failure → passed=false; capture its output in that hook's details.
5a. PRE-EXISTING-DRIFT CHECK (before you call a hook failure a PR failure). Some hooks are scoped to fire on a broad file pattern but VALIDATE state that the PR does not own — a classic false-red is a local schema/contract/codegen check whose "files" pattern matches an unrelated file the PR touched, while the actual breakage lives in committed artifacts that are simply stale on ${cfg.base} (e.g. contract-schema-check firing on a models/ edit but reporting drift in a schema the PR never modified). Before recording status "fail" for ANY hook (most likely local hooks, not the pinned formatter/linter/type hooks), determine whether it is THIS PR's failure or pre-existing on ${cfg.base}:
   - Re-run that SAME hook against a clean ${cfg.base} tree and see if it fails identically. Cheapest reliable way: add a throwaway detached worktree at the base and run the hook's own command there —
       git -C ${cfg.repoRoot} worktree add --detach <WT_BASE>/_verify-base origin/${cfg.base}
       (copy any required local files — e.g. cp ${cfg.repoRoot}/.env <WT_BASE>/_verify-base/.env — since a fresh worktree lacks gitignored files; see the .env gotcha)
       cd <WT_BASE>/_verify-base && <run the exact hook command, e.g. uv run python -m ...>
       git -C ${cfg.repoRoot} worktree remove --force <WT_BASE>/_verify-base   (ALWAYS clean up)
     (<WT_BASE> is the manage-pr worktree base, the parent dir of ${cfg.repoRoot}.) A lighter check when the failure names specific files: confirm the PR's diff does NOT touch them — git diff --name-only origin/${cfg.base}...HEAD -- <those paths> is empty — which strongly implies pre-existing, but the base re-run is the definitive proof; prefer it for anything you'd otherwise block on.
   - If the SAME failure reproduces on clean ${cfg.base}: it is PRE-EXISTING drift, NOT introduced by this PR and not fixable within the PR's scope → record status "fail-preexisting" (NOT "fail"), explain in details that it reproduces on ${cfg.base} and which artifact is stale, and DO NOT let it flip passed=false. Do NOT attempt to fix it (regenerating committed schemas/contracts is often a declared BREAKING change needing human + downstream sign-off) — instead add a suggestion that a separate commit on ${cfg.base} should refresh it.
   - If it does NOT reproduce on clean ${cfg.base} (or you genuinely cannot run the base check): it IS this PR's failure → status "fail", passed=false.

PUSH-SCOPE CHECK (the stale-remote gotcha) — run this whenever the push range is WIDER than the PR diff, which is true if EITHER of these holds: (a) Step 3's ahead-of-remote-branch count is > 0 (${typeof cfg.aheadOfRemote === 'number' ? cfg.aheadOfRemote : 'unknown; compute git rev-list --count origin/' + cfg.branch + '..HEAD, treat missing remote branch as 0'}), OR (b) a rebase ran in THIS pipeline (rebaseHappenedThisRun=${rebasedThisRun}). A rebase REWRITES the branch, so origin/${cfg.branch} (still pointing at the OLD pre-rebase tip) ends up behind local HEAD even when the pre-rebase ahead-of-remote count was 0 — that Step 3 count is STALE after a rebase, so a rebase alone is sufficient reason to run this check. Only skip when BOTH are false (no rebase this run AND ahead-of-remote is 0) — and, as always, skip entirely if there is no remote branch yet. pre-commit's pre-push hook at "git push" time checks EVERY file in the commits being pushed (origin/${cfg.branch}..HEAD), which is WIDER than the PR diff when the remote branch is behind local. Those extra files can be unchanged vs ${cfg.base} yet still not clean under the pinned tools — pre-existing drift that will nonetheless BLOCK the user's push. To spare the user a failed push, also run the pre-push-stage hooks over the push range and commit any fixes:
     SKIP=pytest <pre-commit> run --hook-stage pre-push --from-ref origin/${cfg.branch} --to-ref HEAD
   Handle auto-fixes the same way (commit "chore: pre-commit fixes (pre-push scope)", re-run until clean). SKIP=pytest because tests run in the next stage; also skip any hook needing a non-allowlisted host (e.g. a terraform-validate hook that fetches from registry.terraform.io) — that is a skipped-env gap in this sandbox, NOT a code failure, and it passes on the user's real machine. Report anything touched here SEPARATELY as "drift outside this PR's diff, fixed so your push passes", and note the user could instead address that drift on ${cfg.base}. A non-fixing hook that fails here (and is NOT merely environmental) is a real pre-push blocker → surface it. If there is no remote branch yet, skip this check entirely.

FALLBACK PATH — ONLY if pre-commit genuinely cannot run. Invoke each hook's tool directly, but MATCH THE PINNED rev from .pre-commit-config.yaml — do NOT use the worktree/main .venv binary, whose version may differ from the hook's rev and give a false result. Prefer uvx with a pinned version (now that PyPI is allowlisted): e.g. uvx ruff@<rev> check <files>, uvx ruff@<rev> format --check <files>, uvx mypy@<rev> <files>. Scope to the branch's changed, still-present files: git diff --name-only --diff-filter=d origin/${cfg.base} -- '*.py' (--diff-filter=d drops deletions). For local hooks, use the hook's own entry/args and check its files: regex against changed files (skipped-na if none match); skip the test-suite hook (tests run next); for a hook whose entry is "uv run python -m <module>", invoke ${cfg.mainRepo ? cfg.mainRepo + '/.venv/bin/python' : '<mainRepo>/.venv/bin/python'} -m <module> with PYTHONPATH=${cfg.repoRoot}/src to bypass a blocked uv cache. Clearly flag that this ran as a fallback and note any version uncertainty.

For each hook, record { id, command, status: pass|fail|fail-preexisting|skipped-env|skipped-na, details }. Set passed=true ONLY if every hook that actually RAN passed (pass/skipped-na/skipped-env/fail-preexisting are acceptable — a "fail-preexisting" reproduces on clean ${cfg.base} so it is NOT this PR's failure; only a real "fail" ⇒ passed=false). In the summary, state which path you used (pre-commit vs fallback), separate real code failures from skipped-env gaps and from fail-preexisting drift (name the stale artifact and note it reproduces on ${cfg.base}), and surface any push-scope drift you fixed.
If you hit friction or an environmental workaround worth documenting, add short, actionable notes to a "suggestions" array for how THIS skill could be improved (omit if none).`,
    { label: 'lint', phase: 'Checks', schema: LINT }
  )
  if (!lint) lint = { ran: true, passed: false, summary: 'lint agent failed to report', hooks: [] }
}

// ---- Stage 2b: Tests --------------------------------------------------------
let tests = { ran: false, passed: true, summary: 'skipped (no test command)' }
if (cfg.testCmd) {
  tests = await agent(
    `In the git worktree at ${cfg.repoRoot}, run the test suite: ${cfg.testCmd}
ENVIRONMENT NOTE: uv's cache (~/.cache/uv) may be blocked, so "${cfg.testCmd}" can fail to start for cache/network reasons that are NOT code failures. If so, fall back to a populated venv from the main checkout:
  cd ${cfg.repoRoot}
  PYTHONPATH=${cfg.repoRoot}/src ${cfg.mainRepo ? cfg.mainRepo + '/.venv/bin/python' : '<mainRepo>/.venv/bin/python'} -m pytest -q
Before trusting that run, VERIFY the code under test resolves to the WORKTREE src (not the main checkout) — e.g. print the package __file__ and confirm it points under ${cfg.repoRoot}. Do not modify source to make tests pass.
DEPENDENCY-MISMATCH CAVEAT (important when you take the main-venv fallback): the PYTHONPATH trick makes the SOURCE resolve to the worktree, but the INSTALLED DEPENDENCIES still come from the main checkout's venv, which is provisioned for ${cfg.base} — NOT this branch. So check whether the branch's diff touches dependency files: git diff --name-only origin/${cfg.base} -- uv.lock pyproject.toml (also poetry.lock / requirements*.txt if present). If it does, a green main-venv run is NOT trustworthy — the branch's source ran against the base branch's installed packages (wrong/missing/differently-versioned deps). In that case PREFER installing the branch's own deps into the worktree first (e.g. "uv sync" / "uv run pytest", which honor the worktree's uv.lock — now that ~/.cache is typically writable this often works); only if that genuinely can't run, fall back to the main venv but set passed accordingly and CLEARLY FLAG in details + summary that tests ran against ${cfg.base}'s dependencies and are unverified for this branch's dependency changes (treat as a coverage gap, not a clean pass). If the branch did NOT touch dependency files, the main-venv fallback is fine.
Return ran=true and passed based on the exit status; put failing tests / relevant output in details (truncate to what matters). If the suite could only fail for environmental reasons and never actually executed, say so explicitly in details.
If you hit any friction or environmental workaround, add short, actionable notes to a "suggestions" array for how THIS skill could be improved (omit if none).`,
    { label: 'tests', phase: 'Checks', schema: CHECK }
  )
  if (!tests) tests = { ran: true, passed: false, summary: 'test agent failed to report' }
}
log(`Checks — lint: ${lint.passed ? 'pass' : 'FAIL'}, tests: ${tests.passed ? 'pass' : 'FAIL'}`)

// ---- Readiness verdict (git-commit only — no push, no PR) -------------------
const ready = lint.passed && tests.passed
const suggestions = [
  ...(rebase.suggestions || []),
  ...(lint.suggestions || []),
  ...(tests.suggestions || []),
]
return {
  ok: true,
  ready,
  worktree: cfg.repoRoot,
  rebase,
  lint,
  tests,
  suggestions,
  note: ready
    ? 'Rebased and green in the worktree; any lint fixes are committed to the branch. Ready to hand off. Nothing was pushed and no PR was touched — push and manage the PR manually.'
    : 'Checks failed — not ready. See failing details. Nothing was pushed and no PR was touched.',
}
```

## Step 5 — Report to the user

The workflow only git-commits to the branch. Read its returned object and report **what it did** and **whether the branch is ready to hand off**.

**If conflicts were resolved** (`rebase.status === 'rebased-with-resolution'`), always show the resolved diff before the verdict, so the user can review it — regardless of whether checks passed. Run it inline against the current (post-checks) worktree, scoped to the touched files:

```bash
git -C <worktree> diff origin/<base> -- <rebase.resolvedFiles...>
```

Present it as "Here's how the conflicts were resolved (final state vs `origin/<base>`)" and call out anything non-obvious. Summarize per file if large, and offer full hunks. Then:

- `stage: 'rebase'`, `status: 'conflict'` → rebase was aborted (tree restored); list `unresolvedFiles` and why they couldn't be safely resolved. Offer to resolve together.
- `stage: 'rebase'`, `status: 'error'` → surface the reason (usually uncommitted changes); nothing was touched.
- `rebase.status === 'deferred'` → the branch is behind but the rebase gate deferred it (fresh <`staleAfterHours` and no file overlap). Report it as a deliberate no-op with the `deferredReason` and `behindBy` (e.g. "behind by N, only Xh out of date, no overlapping files — rebase deferred to avoid churn"). Checks still ran on the un-rebased branch; it is NOT a failure and, in babysit, does NOT count as a change.
- `ready: false` → say which check failed and show the relevant failing output (`lint.hooks[].details` for the failing hook(s), or `tests.details`). Distinguish real `fail` from `skipped-env` (couldn't run) — a stage that only had environmental gaps is NOT a code failure; report it as unverified, not failed.
- Any hook with status `fail-preexisting` → report it explicitly but as a NON-blocker: it reproduces on clean `origin/<base>`, so it's stale-artifact drift the PR neither introduced nor can fix within scope. Name the stale artifact and suggest a separate refresh commit on `<base>`. It does not make the branch `ready: false`.
- `ready: true` → short summary: rebase (`clean` / `rebased` / `rebased-with-resolution` / `deferred`), lint (per-hook pass/skipped breakdown), tests (pass/skipped). **Verdict: ready to hand off.**

**Pushing and PR handling are the user's to do manually** — this skill never pushes and never runs `gh`. For convenience you may *show* (not run) the commands the user might use from the worktree:

```bash
# push the prepared branch yourself (a rebase means force-with-lease)
git -C <worktree> push --force-with-lease origin <branch>
```

…and mention opening/managing the PR is up to them. Do not run these.

Always report skipped stages/hooks as skipped, not passing. Always name the worktree path so the user knows where the prepared branch lives.

**Always end the report with a "Suggestions to improve this skill" section.** Combine the pipeline's aggregated `suggestions` (from the workflow return) with your own observations of any friction you hit running this skill this time — e.g. steps that needed manual workarounds, environmental assumptions that broke, unclear instructions, or hooks/tools the skill didn't anticipate. Keep them short and actionable. If there genuinely were none, say "No suggestions this run." Treat these as candidate edits to this SKILL.md, and offer to apply the worthwhile ones.

## Step 6 — Worktree cleanup after merge

The worktree persists so the user can push from it. It is removed:

- **Reaped on next run** — Step 0 removes any worktree whose branch is git-detectably merged into its base (contained-in-base), and *offers* to remove ones whose remote branch has vanished after a prune (the squash-merge heuristic). This is the reliable path.
- **Now, if the merge is imminent and the user wants to wait in-session** — offer to watch with a **Monitor** that git-checks merge status (no `gh`), e.g. a loop that best-effort `git -C <wt> fetch origin <base> 2>/dev/null` then emits when `git -C <wt> rev-list --count <branch> ^origin/<base>` reads `0`; when it fires, run `git -C <mainRepo> worktree remove <worktree>` and confirm.

Never delete a worktree with uncommitted changes without telling the user first.

## Babysit mode (opt-in, self-scheduling)

**What it is.** An opt-in watch mode: once armed, manage-pr wakes ~once an hour and does a light-touch pass on the branch — **rebase it if the base moved, and apply any PR review comments it can make confidently** — then **stops the moment it has actually changed the branch**, leaving the changes green and ready for you to review and push. Its whole purpose is to keep an in-review branch fresh and responsive without you babysitting it; the stop-on-change design means it hands back to you as soon as there's something to look at, rather than piling up unattended commits.

**Trigger.** The user says "babysit `<branch>`", "keep `<branch>` fresh", "watch the PR and apply review comments", or `/manage-pr babysit <repo> <branch>`. Resolve repo + branch (Steps 1–1.5) and ensure a worktree exists (Step 2) before arming.

**The one scoped exception to "no `gh`".** Normal manage-pr never runs `gh`. **Babysit is the sole exception, and only for READS** — it may use `gh` (authenticated against the branch's remote host; here New Relic GHE `source.datanerd.us`, so `gh` must be configured for that host) to find the PR and read its open review comments. Every write-side PR action stays manual and forbidden: **never push, never post/reply to a comment, never resolve a thread, never merge, never `gh pr create`/`merge`/`review`/`comment`.** Because `gh` needs auth and the sandbox is allowlist-gated, **babysit's literal first step (Step B0 below) is a `gh` connectivity/auth check** — if it fails, babysit degrades gracefully to **rebase-only** and says so.

### Step B0 — check `gh` connectivity/auth FIRST (before anything else)

`gh` requires authentication, and this sandbox is network-allowlisted, so **the very first thing babysit does — both when arming and at the start of every tick — is verify `gh` can actually read the PR host.** Never assume it works.

1. Derive the remote host from the branch's remote: `git -C <path> remote get-url origin` → parse the host (here `source.datanerd.us`, a GHE instance — not `github.com`).
2. **Sandbox gotcha — `gh` config path.** `gh` reads `~/.config/gh/config.yml` at startup and fails with `operation not permitted` if the sandbox can't read it. The yolo profile now grants **read-only `$HOME/.config`** (`yolo/src/yolo_cli/auth.py` `fs_read`) so this works — but only **after a yolo relaunch** regenerates the profile. If you still hit `operation not permitted`, the grant isn't active yet: relaunch yolo, or as a stopgap point `gh` at a writable dir with `export GH_CONFIG_DIR=<WT_BASE>/.ghconfig` (note a fresh config dir has no token — see below).
3. Probe auth: `gh auth status --hostname <host>` (exit 0 = a valid token for that host). A token can also come from a `GH_TOKEN`/`GH_ENTERPRISE_TOKEN`/`GITHUB_TOKEN` env var — check those too. **Auth must be established on the HOST, not in-sandbox:** the `$HOME/.config` grant is *read-only*, so `gh auth login` run inside yolo can't persist the token. The user logs in from a normal terminal outside yolo (`gh auth login --hostname <host>`); the sandbox then reads that token. Until a host-side token exists, auth fails → babysit is **rebase-only**.
4. Probe read access with a real, harmless read: `gh pr view <branch> --repo <owner/repo> --json number,title,url` (or `gh pr list --repo <owner/repo> --head <branch> --json number` if the PR number isn't known). This confirms both host reachability (allowlist) and that the token can read PRs.
5. **Interpret the result:**
   - **Both probes pass** → the comment-applying part of babysit is available; proceed.
   - **Auth fails** (no token / wrong host / expired) → `gh` is unusable. **When arming:** tell the user `gh` needs auth (e.g. `gh auth login --hostname <host>`) and ask whether to arm **rebase-only babysit** or hold off until they authenticate. Don't silently pretend comments will be handled. **In a tick:** run rebase-only for this pass and note it.
   - **Host unreachable** (allowlist blocks the host) → same as auth-fail: degrade to **rebase-only** and say so (the fix is adding the host to `allowed-hosts.txt`, effective next yolo launch).
6. Record the outcome so the tick prompt knows its mode (`gh-ok` → full babysit; `gh-unavailable` → rebase-only). A rebase-only babysit is still useful — it just won't apply review comments.

### Arming the schedule

1. **Run Step B0 (the `gh` connectivity/auth check) first** — decide full vs rebase-only mode and, on failure, confirm with the user before arming (see B0.4).
2. Prepare the branch once via the normal pipeline (Steps 0–5) so it starts **green**.
3. Schedule with **CronCreate**, recurring roughly hourly at an **off-minute** (per the tool's guidance — avoid `:00`/`:30`): e.g. cron `"17 * * * *"`, `recurring: true`. The prompt re-invokes a babysit **tick** for this exact repo + branch, and must carry the identifiers the tick needs plus the B0 mode, e.g.:
   > Run a manage-pr **babysit tick** for repo `<mainRepo>`, branch `<branch>` (worktree `<path>`, base `<base>`, gh-mode `<gh-ok|gh-unavailable>`): re-check `gh` connectivity (Step B0), reap-check, rebase if the base moved, apply any confidently-actionable open PR review comments (skip if `gh` unavailable), keep it green, and **if you made any committed change, stop the babysit schedule (CronDelete the job) and notify me**. If nothing changed, stay armed.
4. Tell the user the operational limits up front: it runs **only while this session is open and idle** (CronCreate jobs are **session-only**, in-memory, and fire only when the REPL is idle — they do not survive the session ending), and **recurring jobs auto-expire after 7 days**. To stop early, they cancel it (or ask you to — `CronDelete`).

### Each babysit tick

Operate in the **existing** worktree. Steps 1.5 (build-info) and 2.6 (untracked local files — e.g. re-copy `.env` if missing) still apply. Then, in order:

1. **Check `gh` connectivity/auth FIRST (Step B0).** Re-run the auth + read probes — a token can expire or the host can go unreachable between ticks. Sets this tick's mode: `gh-ok` (do the comment step) or `gh-unavailable` (skip it, rebase-only for this pass, note it). Do this before touching the branch.
2. **Merged?** Run the Step 0 contained-in-base check. If the branch merged → **stop babysitting** (CronDelete the job), reap the worktree, notify, done.
3. **Rebase on upstream — but only when the gate says to (Step 4's rebase gate).** `git fetch origin <base>`; if behind, apply the **rebase gate** before rebasing: rebase this tick ONLY if the branch is more than `staleAfterHours` (default **24h**) out of date OR the base's new commits **overlap files the branch modifies** (real conflict risk). Otherwise **DEFER** — leave the branch un-rebased this tick (status `deferred`), which is NOT a change and does NOT trip stop-on-change. This keeps babysit from rebasing on every master move of a fast-moving base. When the gate fires, run the pipeline's Rebase stage (resolve conflicts only when confident — otherwise abort cleanly, leave the branch untouched, and report). A successful rebase is **a change**; a `deferred` is not. (See Rules for how to compute staleness age and file overlap.)
4. **Apply review comments (only if Step 1 said `gh-ok`; `gh` read-only).** Read the PR's **open, unresolved** review comments. Apply one **only if** it's a concrete, unambiguous, safe code change you can make confidently (e.g. "rename X", "handle the None case here", "drop this dead branch"). **Skip** questions, opinions, discussion, anything needing a design decision or judgment, and anything you'd be guessing at — never guess, and never touch the comment thread. Each applied comment is its own commit (`fix: address review comment — <short paraphrase>`) and is **a change**.
5. **Keep it green.** After any change, run the Checks (pinned-version lint + tests) via the pipeline and commit auto-fixes. If a change can't be made green and you can't safely fix it, **revert that specific change**, restore the prior branch state, and report it — **babysit must never leave the branch broken or push-blocked.**
6. **Stop-on-change.** If the tick produced ANY green committed change (rebase and/or applied comments): **CronDelete the babysit job** and **PushNotification** the user — the changes sit in the worktree for review + push (babysit does **not** push). Summarize what it rebased/applied and what it skipped (with reasons).
7. **No change → keep watching.** If nothing needed doing (base unchanged, no actionable comments), leave the schedule running for the next hour. A quiet `log` line is fine; don't notify on a no-op.

### Stopping conditions (any ends babysitting)

- It made green changes — the designed stop, handing back to you.
- The branch merged (reap + stop).
- The user cancels, or the session ends (session-only jobs die with it).
- 7-day auto-expiry.

## Rules

- **Only git-commit to the branch.** The skill git-commits (rebase, lint fixes, babysit'd review-comment fixes) and stops. **Never push. Never create, comment on, resolve, or merge PRs** — the user does all push/PR handling manually. **`gh` is forbidden except in babysit mode, and there only for READS** (reading PR review comments); all write-side `gh`/PR actions remain forbidden everywhere.
- **Always operate in a worktree** under `<workspace-root>/.work-trees/manage-pr/` (NOT `~/.work-trees`, which isn't writable here), never in the user's live checkout.
- **Run the repo's pre-commit hooks at their pinned versions, scoped to the PR diff.** Prefer `pre-commit run --from-ref origin/<base> --to-ref HEAD` (after `pre-commit install-hooks`) so each tool matches its pinned `rev` — NOT a `.venv` binary, whose version can differ and give a false green (this bit us: venv `ruff` 0.13.3 passed while the pinned 0.15.9 reformatted a file). Let auto-fixing hooks fix + commit (`chore: pre-commit fixes`); re-run until clean. Only hand-invoke tools directly if pre-commit genuinely can't run, and then pin the version to match the hook's `rev` (e.g. `uvx ruff@<rev>`). Issues on files unchanged vs base are pre-existing drift, out of the PR's scope.
- **Watch the push scope when the remote branch is stale — AND after any rebase.** `git push` re-runs pre-push hooks over ALL unpushed commits (`origin/<branch>..HEAD`), not just the PR diff — so a behind remote branch can make the push fail on drift in files the PR doesn't touch. Run the pre-push-stage hooks over that range and commit the fixes (so the user's push isn't blocked; report them as drift outside the PR diff) whenever **either** ahead-of-remote > 0 **or** a rebase ran this pipeline. A rebase rewrites the branch, so `origin/<branch>` falls behind local HEAD and the push scope widens even when the pre-rebase ahead-of-remote count was 0 — that count is stale after a rebase, so don't gate the check on it alone. A hook that fails here only for environmental reasons (e.g. terraform-validate needing a non-allowlisted host) is a skipped-env gap, not a blocker — it passes on the user's real machine.
- **Record base + main-repo in the worktree at creation** (in its private git dir via `git rev-parse --git-path`), so Step 0 reaping reads them back instead of re-deriving/guessing.
- **Pull the base to latest before rebasing.** Fetch `origin/<base>` first so the rebase targets current upstream; if the fetch fails (no network), fall back to the cached ref and flag that the base may be stale.
- **Gate the rebase — don't rebase on every base move (avoids churn on fast-moving bases).** When the branch is behind, rebase this run ONLY if the branch is more than `staleAfterHours` (default **24h**) out of date **OR** the base's new commits touch files the branch also modifies; otherwise return status `deferred` and run the Checks on the un-rebased branch. Applies to BOTH the one-shot prep pipeline and babysit ticks. A `deferred` is a success, not a change (so babysit's stop-on-change does not fire on it). Compute in the worktree:
  ```bash
  # (i) staleness: age of the OLDEST base commit the branch is missing
  oldest=$(git rev-list --reverse <branch>..origin/<base> | head -1)
  age_h=$(( ( $(date +%s) - $(git show -s --format=%ct "$oldest") ) / 3600 ))   # STALE if age_h > 24
  # (ii) file overlap: base's changes ∩ branch's changes, non-empty ⇒ rebase now
  mb=$(git merge-base <branch> origin/<base>)
  comm -12 <(git diff --name-only "$mb" <branch> | sort -u) <(git diff --name-only "$mb" origin/<base> | sort -u)
  ```
  Lower `staleAfterHours` to 0 to force always-rebase-when-behind. This targets fast-moving repos like `sre-orchestration-service` where hourly rebasing produced needless force-push churn.
- **Attempt conflict resolution, but never guess.** Integrate both sides' intent; if a hunk is ambiguous, abort cleanly and report it. Tests validate any resolution made.
- **End every report with skill-improvement suggestions** (aggregated pipeline suggestions + your own friction observations), offered as candidate edits to this SKILL.md.
- **Always show the resolved diff** when conflicts were resolved, even if checks passed.
- **Never rebase over uncommitted changes**, and **never edit source to make tests or linters pass** — report failures honestly, and distinguish real failures from environmental (couldn't-run) gaps.
- **Before blocking on a hook failure, check whether it's PRE-EXISTING drift on the base (Step 4 lint step 5a).** Some hooks fire on a broad file pattern but validate committed artifacts the PR doesn't own (e.g. a schema/contract/codegen check firing on an unrelated `models/` edit while reporting drift in a stale committed schema). Re-run that same hook on a clean `origin/<base>` worktree (throwaway detached worktree, copy `.env`, always clean up): if it fails identically, it's not this PR's failure → record `fail-preexisting`, don't flip `passed=false`, don't try to fix it (regenerating committed schemas/contracts is often a declared BREAKING change needing human sign-off), and suggest a separate refresh commit on `<base>`. Only a failure that does NOT reproduce on clean `<base>` is a real `fail`. Record recurring false-reds per repo in build-info so future runs skip the rediscovery.
- **Load per-repo build-info at the START (Step 1.5), and apply it wherever it's relevant.** Read `<workspace-root>/memory/build-info/<repoName>.md` as soon as the repo is known — it can affect any stage (worktree local-file setup, the test command, rebase hot spots, push scope), so read it once and carry it through, not just at Step 2. Record repo-specific friction you resolve this run back into that store for next time.
- **Set up untracked local files a fresh worktree needs before running checks (Step 2.6).** A worktree has only tracked files, so gitignored `.env`/local config is missing and hooks or tests that read it fail with errors the developer never sees in their main checkout (e.g. `No environment file found at: .env`). Apply build-info's local-file setup, and by default copy an `.env` (prefer `<mainRepo>/.env`, else `.env.example`) into the worktree. Do **not** let a check "pass" by silently creating a missing local file inside a stage and not reporting it — that's a false green; set the file up up-front and report it.
- **The main-venv test fallback runs worktree source against the BASE branch's installed deps.** If the branch's diff touches dependency files (`uv.lock`/`pyproject.toml`/`poetry.lock`/`requirements*.txt`), prefer installing the branch's own deps into the worktree (`uv sync`/`uv run`) instead; if you must fall back to the main venv, flag the run as unverified for this branch's dependency changes — a green result there is a coverage gap, not a clean pass.
- **Remove the worktree once its branch is merged** (git-detected reap on next run, or watch now if asked); never `--force`-remove a dirty worktree without confirmation.
- Keep the pipeline stages sequential; don't parallelize or nest worktree isolation.
- **Babysit checks `gh` connectivity/auth FIRST, stops on change, and never leaves the branch broken.** Its literal first step — at arming and at the start of every tick — is a `gh` auth + read probe (Step B0); if `gh` is unavailable it degrades to **rebase-only** rather than assuming comments will be handled. The scheduled watch (CronCreate, ~hourly, off-minute, session-only, 7-day expiry) rebases on upstream and applies only *confidently-actionable* review comments; it **CronDeletes its own schedule the moment it makes any green change** and notifies you. It reverts any change it can't make green, never pushes, and only reads PRs via `gh` (never writes to them). Ambiguous comments are reported, not guessed.
