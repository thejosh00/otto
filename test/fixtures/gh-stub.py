#!/usr/bin/env python3
"""A stand-in for `gh`, for live tests: pull requests on a local bare remote, kept in a JSON file.

Enough of `gh pr` and `gh api` for a lander to open, read, merge, comment on and close pull requests
the way it would on GitHub. The test plays the reviewer by editing the state file (`$GH_STUB_STATE`)
and bumping `updatedAt`. Every call is logged there, so the test can say what the lander ran.
Anything it doesn't know exits 1 and says so, as `gh` does for a bad command.
"""
import json
import os
import subprocess
import sys
import tempfile
import time

STATE = os.environ["GH_STUB_STATE"]


def load():
    with open(STATE) as f:
        return json.load(f)


def save(state):
    tmp = STATE + ".tmp"
    with open(tmp, "w") as f:
        json.dump(state, f, indent=2)
    os.replace(tmp, STATE)


def git(*args, cwd=None):
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)


def now():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def flag(args, name, default=None):
    for i, a in enumerate(args):
        if a == name and i + 1 < len(args):
            return args[i + 1]
        if a.startswith(name + "="):
            return a.split("=", 1)[1]
    return default


def find(state, ref):
    ref = (ref or "").rstrip("/")
    for pr in state["prs"]:
        if ref in (pr["url"], str(pr["number"]), pr["head"]):
            return pr
    return None


def head_oid(state, pr):
    out = git("--git-dir", state["remote"], "rev-parse", "refs/heads/" + pr["head"])
    return out.stdout.strip() if out.returncode == 0 else pr.get("headRefOid", "")


def view(state, pr):
    ci = pr.get("ci", "PENDING")
    check = {"__typename": "CheckRun", "name": "test", "status": "COMPLETED" if ci != "PENDING" else "IN_PROGRESS",
             "conclusion": ci if ci != "PENDING" else "", "detailsUrl": pr["url"] + "/checks"}
    reviews = [{"id": r["id"], "author": {"login": r["login"]}, "state": r["state"], "body": r.get("body", ""),
                "submittedAt": r["at"]} for r in pr.get("reviews", [])]
    comments = [{"id": c["id"], "author": {"login": c["login"]}, "body": c["body"], "createdAt": c["at"],
                 "url": f'{pr["url"]}#issuecomment-{c["id"]}'} for c in pr.get("comments", [])]
    merged = pr["state"] == "MERGED"
    return {
        "number": pr["number"], "url": pr["url"], "title": pr["title"], "body": pr.get("body", ""),
        "state": pr["state"], "isDraft": False, "headRefName": pr["head"], "baseRefName": pr["base"],
        "headRefOid": head_oid(state, pr), "updatedAt": pr["updatedAt"],
        "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN" if pr.get("reviewDecision") == "APPROVED" and ci == "SUCCESS" else "BLOCKED",
        "reviewDecision": pr.get("reviewDecision", "REVIEW_REQUIRED"), "statusCheckRollup": [check],
        "reviews": reviews, "latestReviews": reviews, "comments": comments,
        "mergeCommit": {"oid": pr["mergeCommit"]} if merged else None, "mergedAt": pr.get("mergedAt"),
        "autoMergeRequest": {"enabledAt": pr["updatedAt"]} if pr.get("autoMerge") else None,
        "author": {"login": "otto-lander"},
    }


def emit(value, args):
    fields = flag(args, "--json")
    if fields and isinstance(value, dict):
        value = {k: value.get(k) for k in fields.split(",")}
    elif fields and isinstance(value, list):
        value = [{k: v.get(k) for k in fields.split(",")} for v in value]
    text = json.dumps(value)
    expr = flag(args, "--jq") or flag(args, "-q")
    if expr:
        out = subprocess.run(["jq", "-r", expr], input=text, capture_output=True, text=True)
        sys.stdout.write(out.stdout)
        sys.exit(out.returncode)
    print(text)


def merge(state, pr, args):
    method = "squash" if "--squash" in args else "rebase" if "--rebase" in args else "merge"
    with tempfile.TemporaryDirectory() as tmp:
        work = os.path.join(tmp, "w")
        git("clone", "--quiet", state["remote"], work)
        ident = ["-c", "user.email=gh@stub", "-c", "user.name=gh"]
        git("checkout", "--quiet", pr["base"], cwd=work)
        if method == "squash":
            r = git("merge", "--squash", "origin/" + pr["head"], cwd=work)
            ok = r.returncode == 0 and git(*ident, "commit", "--quiet", "-m", f'{pr["title"]} (#{pr["number"]})', cwd=work).returncode == 0
        elif method == "rebase":
            ok = git(*ident, "rebase", "--quiet", pr["base"], "origin/" + pr["head"], cwd=work).returncode == 0
            ok = ok and git("checkout", "--quiet", "-B", pr["base"], cwd=work).returncode == 0
        else:
            ok = git(*ident, "merge", "--quiet", "--no-ff", "-m", f'Merge pull request #{pr["number"]}', "origin/" + pr["head"], cwd=work).returncode == 0
        if not ok or git("push", "--quiet", "origin", pr["base"], cwd=work).returncode != 0:
            print("Pull request is not mergeable: the merge commit cannot be cleanly created.", file=sys.stderr)
            sys.exit(1)
        pr["mergeCommit"] = git("rev-parse", "HEAD", cwd=work).stdout.strip()
    pr["state"] = "MERGED"
    pr["mergedAt"] = pr["updatedAt"] = now()
    if "--delete-branch" in args or "-d" in args:
        git("--git-dir", state["remote"], "branch", "-D", pr["head"])
    print(f'✓ Merged pull request #{pr["number"]} ({pr["title"]})')


def main(args):
    state = load()
    state["log"].append(" ".join(args))
    save(state)
    state = load()
    cmd = args[:2]
    rest = args[2:]
    pr = None

    if args[:2] == ["auth", "status"]:
        print("github.com\n  ✓ Logged in to github.com account otto-lander")
        return
    if args[:2] == ["repo", "view"]:
        emit({"nameWithOwner": state["repo"], "url": "https://github.com/" + state["repo"], "defaultBranchRef": {"name": "main"}}, rest)
        return
    if cmd == ["pr", "create"]:
        head, base, title = flag(rest, "--head") or flag(rest, "-H"), flag(rest, "--base", "main") or flag(rest, "-B"), flag(rest, "--title") or flag(rest, "-t")
        body = flag(rest, "--body") or flag(rest, "-b") or ""
        if flag(rest, "--body-file") or flag(rest, "-F"):
            with open(flag(rest, "--body-file") or flag(rest, "-F")) as f:
                body = f.read()
        if not head or git("--git-dir", state["remote"], "rev-parse", "--verify", "--quiet", "refs/heads/" + head).returncode != 0:
            print(f"pull request create failed: GraphQL: Head sha can't be blank, No commits between main and {head} (createPullRequest)", file=sys.stderr)
            sys.exit(1)
        # As gh does: one open pull request a branch.
        for existing in state["prs"]:
            if existing["head"] == head and existing["state"] == "OPEN":
                print(f'a pull request for branch "{head}" into branch "{base or "main"}" already exists:\n{existing["url"]}', file=sys.stderr)
                sys.exit(1)
        n = len(state["prs"]) + 1
        pr = {"number": n, "url": f'https://github.com/{state["repo"]}/pull/{n}', "head": head, "base": base or "main",
              "title": title or head, "body": body, "state": "OPEN", "updatedAt": now(), "ci": "PENDING",
              "reviewDecision": "REVIEW_REQUIRED", "reviews": [], "comments": [], "review_comments": []}
        state["prs"].append(pr)
        save(state)
        print(pr["url"])
        return
    if cmd[0] == "pr" and len(cmd) == 2 and cmd[1] in ("view", "merge", "close", "comment", "checks", "ready", "edit", "diff"):
        target = rest[0] if rest and not rest[0].startswith("-") else None
        pr = find(state, target) if target else (state["prs"][-1] if state["prs"] else None)
        if not pr:
            print(f"no pull requests found for {target}", file=sys.stderr)
            sys.exit(1)
        if cmd[1] == "view":
            emit(view(state, pr), rest)
        elif cmd[1] == "merge":
            if "--auto" in rest:
                pr["autoMerge"] = True
                pr["updatedAt"] = now()
                print(f'✓ Pull request #{pr["number"]} will be automatically merged when all requirements are met')
            else:
                merge(state, pr, rest)
            save(state)
        elif cmd[1] == "close":
            pr["state"] = "CLOSED"
            pr["updatedAt"] = now()
            if "--delete-branch" in rest or "-d" in rest:
                git("--git-dir", state["remote"], "branch", "-D", pr["head"])
            save(state)
            print(f'✓ Closed pull request #{pr["number"]}')
        elif cmd[1] == "comment":
            body = flag(rest, "--body") or flag(rest, "-b")
            if flag(rest, "--body-file") or flag(rest, "-F"):
                with open(flag(rest, "--body-file") or flag(rest, "-F")) as f:
                    body = f.read()
            pr["comments"].append({"id": 7000 + len(pr["comments"]), "login": "otto-lander", "body": body or "", "at": now()})
            save(state)
            print(pr["url"] + "#issuecomment-" + str(7000 + len(pr["comments"]) - 1))
        elif cmd[1] == "checks":
            ci = pr.get("ci", "PENDING")
            print(f'test\t{"pass" if ci == "SUCCESS" else "fail" if ci == "FAILURE" else "pending"}\t1m\t{pr["url"]}/checks')
            sys.exit(0 if ci == "SUCCESS" else 8 if ci == "PENDING" else 1)
        else:
            print(f'✓ {cmd[1]} #{pr["number"]}')
        return
    if cmd == ["pr", "list"]:
        head = flag(rest, "--head") or flag(rest, "-H")
        want = (flag(rest, "--state") or flag(rest, "-s") or "open").upper()
        prs = [view(state, p) for p in state["prs"] if (not head or p["head"] == head) and (want == "ALL" or p["state"] == want)]
        emit(prs, rest)
        return
    if args[:1] == ["api"]:
        path = next((a for a in args[1:] if not a.startswith("-") and "/" in a), "")
        parts = path.strip("/").split("/")
        body = next((a.split("=", 1)[1] for i, a in enumerate(args) if args[i - 1] in ("-f", "-F", "--field", "--raw-field") and a.startswith("body=")), None)
        if "pulls" in parts:
            pr = find(state, parts[parts.index("pulls") + 1])
        elif "issues" in parts:
            pr = find(state, parts[parts.index("issues") + 1])
        if pr and parts[-1] == "replies":
            pr["review_comments"].append({"id": 9500 + len(pr["review_comments"]), "in_reply_to_id": int(parts[-2]), "user": {"login": "otto-lander"}, "body": body or "", "created_at": now()})
            save(state)
            emit(pr["review_comments"][-1], args)
        elif pr and parts[-1] == "comments" and "pulls" in parts:
            if body is not None:
                pr["review_comments"].append({"id": 9500 + len(pr["review_comments"]), "user": {"login": "otto-lander"}, "body": body, "created_at": now()})
                save(state)
            emit(pr["review_comments"], args)
        elif pr and parts[-1] == "comments":
            if body is not None:
                pr["comments"].append({"id": 7000 + len(pr["comments"]), "login": "otto-lander", "body": body, "at": now()})
                save(state)
            emit([{"id": c["id"], "user": {"login": c["login"]}, "body": c["body"], "created_at": c["at"]} for c in pr["comments"]], args)
        elif pr and parts[-1] == "reviews":
            emit([{"id": r["id"], "user": {"login": r["login"]}, "state": r["state"], "body": r.get("body", ""), "submitted_at": r["at"]} for r in pr["reviews"]], args)
        elif pr:
            emit(view(state, pr), args)
        else:
            emit({}, args)
        return
    if args[:2] == ["run", "rerun"] or args[:2] == ["run", "list"] or args[:2] == ["run", "view"]:
        emit([], args) if args[1] == "list" else print("✓ Requested")
        return
    print(f"gh (live-test stand-in): unknown command {' '.join(args)!r}", file=sys.stderr)
    sys.exit(1)


if __name__ == "__main__":
    main(sys.argv[1:])
