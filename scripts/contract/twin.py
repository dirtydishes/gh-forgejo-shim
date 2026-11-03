"""`twin` cases: writes run by real gh through the shim against the local Forgejo twin.

GitHub-side oracles for writes would need writes to GitHub, which captures refuse. gh is the real
binary, so its output is GitHub's by construction. A twin case checks what the gateway decides:
that its answers drive gh to the expected exit and output, and that the forge ends up in the state
asked for, read back through Forgejo's own API. Each case works on a branch, and optionally a pull
request, of its own, so cases don't depend on their order.
"""

import base64
import json
import re
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from common import REPO

# What a twin case may run: the launched write commands, and only these flags.
TWIN_COMMANDS = {
    ("pr", "create"): {"--head", "--base", "--title", "--body", "--draft"},
    ("pr", "comment"): {"--body"},
    ("pr", "edit"): {"--title", "--body"},
    ("pr", "merge"): {"--merge", "--squash", "--rebase", "--auto", "--match-head-commit", "--delete-branch"},
    ("pr", "ready"): {"--undo"},
    ("pr", "close"): {"--delete-branch"},
    ("pr", "reopen"): set(),
    # Reads of the case's own queued run, and a comment on its own issue (C4).
    ("run", "list"): {"--branch", "--json", "--jq", "--limit", "--status", "--event"},
    ("run", "view"): {"--json", "--jq"},
    ("issue", "comment"): {"--body"},
    # The case's own issue: read back, listed, or made with a title naming the case (C4).
    ("issue", "view"): {"--json", "--jq"},
    ("issue", "list"): {"--json", "--jq", "--state", "--author", "--assignee", "--label", "--limit"},
    ("issue", "create"): {"--title", "--body"},
}
BOOL_FLAGS = {"--draft", "--merge", "--squash", "--rebase", "--auto", "--undo", "--delete-branch"}
PLACEHOLDERS = {"{pr}", "{branch}", "{run}", "{issue}"}
ISSUE_SELECT = "select(.number == {issue})"
# The selector each command takes; `run list` takes `--branch {branch}` instead.
SELECTORS = {("run", "view"): {"{run}"}, ("issue", "comment"): {"{issue}"}, ("issue", "view"): {"{issue}"}}
STATE_PATH = re.compile(r"(pulls|issues)/\{pr\}(/comments|/timeline)?|issues/\{issue\}(/comments)?|branches/\{branch\}")
SETUP = {"branch", "pr", "base", "draft", "closed", "protect", "status", "workflow", "issue", "label",
         "protect_head", "fork"}
# The organization a `fork` case forks into, then deletes its fork from.
FORK_OWNER = "gfj-forks"
# The workflow a `workflow` case needs on main: the gateway names a run's workflow from the
# default branch. It runs only for `twin/run-*` branches, so creating such a case's branch
# queues that case's one run, which waits (the twin has no runner), and no other push queues any.
WORKFLOW_PATH = ".forgejo/workflows/twin.yml"
WORKFLOW = ("name: twin\non:\n  push:\n    branches: ['twin/run-*']\njobs:\n  check:\n"
            "    runs-on: docker\n    steps:\n      - run: \"true\"\n")
RUN_BRANCH = re.compile(r"twin/run-[a-z0-9-]+")
# `workflow: "branch"` instead puts this one on the case's own branch only, outside twin/run-*, so
# it queues the only run: a run whose workflow isn't on the default branch (the gateway's #128).
BRANCH_WORKFLOW_PATH = ".forgejo/workflows/branch.yml"
BRANCH_WORKFLOW = "name: branch\non: push\njobs:\n  check:\n    runs-on: docker\n    steps:\n      - run: \"true\"\n"
# The twin's own draft prefix (seed.py), which Forgejo reads to mark a pull request a draft.
DRAFT_PREFIX = "scenario: draft"
BRANCH = re.compile(r"twin/[a-z0-9-]+")


def validate(case):
    """Reject a twin case that could act outside its own branch on the twin, or crash the run."""
    if not case["id"].startswith("twin-"):
        raise ValueError("twin case ids start with twin-")
    argv = case["argv"]
    command = tuple(argv[:2])
    flags = TWIN_COMMANDS.get(command)
    if flags is None:
        raise ValueError("twin command is outside the twin allowlist")
    selectors = []
    heads = []
    branches = []
    values = {}
    index = 2
    while index < len(argv):
        arg = argv[index]
        if arg.startswith("-"):
            if arg not in flags:
                raise ValueError(f"twin flag is not allowed: {arg}")
            if arg not in BOOL_FLAGS:
                index += 1
                if index == len(argv):
                    raise ValueError(f"missing value for {arg}")
                if arg == "--head":
                    heads.append(argv[index])
                if arg == "--base" and argv[index] != "main":
                    raise ValueError("--base must be main")
                if arg == "--branch":
                    branches.append(argv[index])
                values.setdefault(arg, []).append(argv[index])
                if arg == "--match-head-commit" and not re.fullmatch(r"\{head\}|[0-9a-f]{40}", argv[index]):
                    raise ValueError("--match-head-commit takes {head} or a full sha")
        elif arg in SELECTORS.get(command, {"{pr}", "{branch}"}):
            selectors.append(arg)
        else:
            raise ValueError("a twin selector must be the case's own {pr}, {branch}, {run} or {issue}")
        index += 1
    # Without a selector gh picks the checkout's branch, s/pass: scenario PR 1.
    if command == ("pr", "create"):
        if heads != ["{branch}"] or selectors:
            raise ValueError("pr create needs one --head {branch}, its own branch")
    elif command == ("run", "list"):
        # Without --branch it would list every case's runs.
        if branches != ["{branch}"] or selectors:
            raise ValueError("run list needs one --branch {branch}, the case's own, and no selector")
    elif command == ("issue", "list"):
        # Other cases make issues too: the case's --jq picks out its own, so nothing it prints
        # depends on theirs (a count of them would).
        if not any(ISSUE_SELECT in jq for jq in values.get("--jq", [])) or selectors:
            raise ValueError(f"issue list needs a --jq with {ISSUE_SELECT}, and no selector")
    elif command == ("issue", "create"):
        # The issue it makes names the case, and stdout gives its number as (?P<issue>...).
        titles = values.get("--title", [])
        if not titles or "{branch}" not in titles[-1] or "--body" not in values or selectors:
            raise ValueError("issue create needs --title naming {branch}, a --body, and no selector")
    elif len(selectors) != 1:
        raise ValueError("a twin pr command needs one selector, {pr} or {branch}")
    setup = case.get("setup")
    if not isinstance(setup, dict) or not BRANCH.fullmatch(setup.get("branch", "")):
        raise ValueError("twin setup needs a branch named twin/<name>")
    if set(setup) - SETUP:
        raise ValueError(f"twin setup takes only {sorted(SETUP)}")
    # prepare() names a case's own base <branch>-base; no case may claim such a name.
    if setup["branch"].endswith("-base"):
        raise ValueError("a twin branch can't end in -base, which names a case's own base")
    for flag in ("pr", "base", "draft", "closed", "protect_head", "fork", "workflow", "issue", "label"):
        if flag in setup and not isinstance(setup[flag], bool) and not (flag == "workflow" and setup[flag] == "branch"):
            raise ValueError(f"setup.{flag} is true or false")
    if (setup.get("draft") or setup.get("closed")) and not setup.get("pr"):
        raise ValueError("setup.draft and setup.closed need setup.pr")
    if setup.get("fork") and (not setup.get("pr") or setup.get("closed") or setup.get("protect_head")):
        raise ValueError("setup.fork needs setup.pr, and takes no closed or protect_head")
    if "{pr}" in argv and not setup.get("pr"):
        raise ValueError("{pr} needs setup.pr")
    if command[0] == "run" and not setup.get("workflow"):
        raise ValueError("a twin run command needs setup.workflow, its own run")
    if setup.get("workflow") is True and (not RUN_BRANCH.fullmatch(setup["branch"]) or setup.get("base")):
        raise ValueError("setup.workflow needs a twin/run-<name> branch made from main")
    if setup.get("workflow") == "branch" and (RUN_BRANCH.fullmatch(setup["branch"]) or setup.get("base")):
        raise ValueError('setup.workflow "branch" needs a branch outside twin/run-*, made from main')
    if command[0] == "issue" and (not setup.get("issue")) != (command == ("issue", "create")):
        raise ValueError("a twin issue command needs setup.issue, its own issue (issue create makes its own)")
    if setup.get("label") and not setup.get("issue"):
        raise ValueError("setup.label labels the case's own issue, so it needs setup.issue")
    # A merge goes into the case's own base branch, never main, which other cases read.
    if command == ("pr", "merge") and not setup.get("base"):
        raise ValueError("a twin merge needs setup.base, its own base branch")
    if setup.get("protect") is not None and (
        not setup.get("base") or not isinstance(setup["protect"], list)
        or not all(isinstance(context, str) for context in setup["protect"])
    ):
        raise ValueError("setup.protect lists required checks on the case's own base")
    status = setup.get("status")
    if status is not None and (
        not isinstance(status, dict) or set(status) != {"context", "state"}
        or status["state"] not in {"success", "failure", "pending"}
    ):
        raise ValueError("setup.status is a context and a state")
    if set(case.get("shim_config", {})) - {"wip_prefixes"}:
        raise ValueError("twin shim_config may only name wip_prefixes")
    expect = case.get("expect")
    if not isinstance(expect, dict) or set(expect) != {"exit", "stdout", "stderr"}:
        raise ValueError("twin expect needs exit, stdout and stderr")
    if not isinstance(expect["exit"], int):
        raise ValueError("twin expect.exit must be an integer")
    for stream in ("stdout", "stderr"):
        try:
            re.compile(expect[stream])
        except (TypeError, re.error) as error:
            raise ValueError(f"twin expect.{stream} isn't a pattern: {error}") from error
    # Neither stream may name what setup bound: check() binds groups from both, and a rebound
    # {issue} would point the state checks at another case's issue (o2#3).
    bound = {"branch", "head"} | {name for name, made in (
        ("pr", setup.get("pr")), ("issue", setup.get("issue")), ("run", setup.get("workflow")),
        ("label", setup.get("label"))) if made}
    if any(set(re.compile(expect[stream]).groupindex) & bound for stream in ("stdout", "stderr")):
        raise ValueError("a stdout pattern can't rebind a placeholder the case's setup made")
    for check in case.get("state", []):
        path = check.get("path", "")
        if not STATE_PATH.fullmatch(path) or not (set(check) - {"path"}) <= {"fields", "any", "none", "gone"}:
            raise ValueError("twin state checks read the case's own pull request or branch")
        # A branch check says only whether the case's own branch is gone.
        if path.startswith("branches/") != ("gone" in check) or ("gone" in check and (
            set(check) != {"path", "gone"} or not isinstance(check["gone"], bool)
        )):
            raise ValueError("a branches/{branch} check is exactly {path, gone: true or false}")
        if "fields" in check and not isinstance(check["fields"], dict):
            raise ValueError("twin state fields must be an object")
        for key in ("any", "none"):
            if key in check and (not path.endswith(("/comments", "/timeline")) or not isinstance(check[key], dict)):
                raise ValueError(f"twin state {key} is an object, on a comments or timeline list")
    if set(case) & {"oracle", "expected", "xfail"}:
        raise ValueError("twin cases expect only their own expect and state")


def api_base(home):
    """The twin's Forgejo, from the seeded shim config; only a local twin is ever written to."""
    config = (Path(home) / ".config/gh-forgejo-shim/config.toml").read_text()
    match = re.search(r'^api_url = "(http://127\.0\.0\.1:[0-9]+)"$', config, re.M)
    if not match:
        raise RuntimeError("twin cases need the local Forgejo twin")
    return match.group(1)


def request(base, token, method, path, body=None):
    return api(base, token, method, f"repos/{REPO}/{path}", body)


def api(base, token, method, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f"{base}/api/v1/{path}", data=data, method=method,
                                 headers={"Authorization": f"token {token}", "Content-Type": "application/json"})
    # No proxy, as seed.py: the token must go to the local twin, not to a proxy that resolves it.
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req, timeout=30) as response:
        return json.loads(response.read() or b"null")


def prepare(case, home, token):
    """Apply the case's shim config and make its branch and pull request. Returns the substitutions."""
    if case.get("shim_config"):
        lines = "".join(f"{key} = {json.dumps(value)}\n" for key, value in case["shim_config"].items())
        with open(Path(home) / ".config/gh-forgejo-shim/config.toml", "a") as config:
            config.write(lines)
    base = api_base(home)
    setup = case["setup"]
    branch = setup["branch"]
    # A merge case's own base branch, so nothing it merges reaches main.
    target = f"{branch}-base" if setup.get("base") else "main"
    if setup.get("workflow") is True:
        try:
            request(base, token, "GET", f"contents/{WORKFLOW_PATH}")
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
            request(base, token, "POST", f"contents/{WORKFLOW_PATH}",
                    {"content": base64.b64encode(WORKFLOW.encode()).decode(), "branch": "main", "message": "twin"})
    if target != "main":
        request(base, token, "POST", f"contents/{target}.txt",
                {"content": "dHdpbgo=", "branch": "main", "new_branch": target, "message": target})
    request(base, token, "POST", f"contents/{branch}.txt",
            {"content": "dHdpbgo=", "branch": target, "new_branch": branch, "message": branch})
    if setup.get("workflow") == "branch":
        request(base, token, "POST", f"contents/{BRANCH_WORKFLOW_PATH}",
                {"content": base64.b64encode(BRANCH_WORKFLOW.encode()).decode(), "branch": branch, "message": "branch"})
    head = request(base, token, "GET", f"branches/{urllib.parse.quote(branch, safe='')}")["commit"]["id"]
    subs = {"branch": branch, "head": head}
    if setup.get("status"):
        request(base, token, "POST", f"statuses/{head}", setup["status"] | {"description": "twin"})
    # Protecting the head branch makes Forgejo refuse to delete it.
    if setup.get("protect_head"):
        request(base, token, "POST", "branch_protections", {"rule_name": branch})
    if setup.get("protect"):
        request(base, token, "POST", "branch_protections",
                {"rule_name": target, "enable_status_check": True, "status_check_contexts": setup["protect"]})
    if setup.get("pr"):
        title = f"{DRAFT_PREFIX} twin: {branch}" if setup.get("draft") else f"twin: {branch}"
        head_ref = fork_head(base, token, branch) if setup.get("fork") else branch
        pr = request(base, token, "POST", "pulls", {"head": head_ref, "base": target, "title": title})
        subs["pr"] = str(pr["number"])
        if setup.get("fork"):
            # Forgejo closes a fork's pull requests when the fork is deleted, and reopens one on
            # request: an open pull request whose head repository is gone (`head.repo` null).
            api(base, token, "DELETE", f"repos/{FORK_OWNER}/{fork_name(branch)}")
            request(base, token, "PATCH", f"pulls/{pr['number']}", {"state": "open"})
        if setup.get("closed"):
            request(base, token, "PATCH", f"pulls/{pr['number']}", {"state": "closed"})
    if setup.get("issue"):
        # Assigned to the twin's user, and labelled with the case's own label when asked.
        body = {"title": f"twin: {branch}", "body": "twin", "assignees": [REPO.split("/")[0]]}
        if setup.get("label"):
            subs["label"] = "twin-" + branch.removeprefix("twin/")
            label = request(base, token, "POST", "labels", {"name": subs["label"], "color": "#ededed"})
            body["labels"] = [label["id"]]
        issue = request(base, token, "POST", "issues", body)
        subs["issue"] = str(issue["number"])
    if setup.get("workflow"):
        subs["run"] = own_run(base, token, branch)
    return subs


def own_run(base, token, branch):
    """The run the case's workflow push queued; Forgejo files it in the background."""
    ref = urllib.parse.quote(f"refs/heads/{branch}", safe="")
    for _ in range(30):
        runs = request(base, token, "GET", f"actions/runs?ref={ref}").get("workflow_runs") or []
        if len(runs) == 1:
            return str(runs[0]["id"])
        if runs:
            raise RuntimeError(f"{branch} has {len(runs)} runs, not its own one")
        time.sleep(1)
    raise RuntimeError(f"{branch}'s workflow push queued no run")


def fork_name(branch):
    return "fork-" + branch.removeprefix("twin/")


def fork_head(base, token, branch):
    """A fork of the twin repo with a commit of its own on `branch`, as a pull request's
    `owner:branch` head. The base repo keeps its own `branch`, which a delete aimed at the base
    would remove. Forgejo copies every branch into a fork, so the fork starts with that one."""
    try:
        api(base, token, "POST", "orgs", {"username": FORK_OWNER})
    except urllib.error.HTTPError as error:
        if error.code != 422:  # the organization already exists
            raise
    name = fork_name(branch)
    request(base, token, "POST", "forks", {"organization": FORK_OWNER, "name": name})
    # Forgejo makes the fork in the background.
    for _ in range(30):
        try:
            api(base, token, "POST", f"repos/{FORK_OWNER}/{name}/contents/fork.txt",
                {"content": "Zm9yawo=", "branch": branch, "message": "fork"})
            return f"{FORK_OWNER}:{branch}"
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
            time.sleep(1)
    raise RuntimeError(f"the fork {FORK_OWNER}/{name} never took a branch")


def fill(text, subs):
    for key, value in subs.items():
        text = text.replace("{" + key + "}", value)
    return text


def check(case, actual, home, token, subs):
    """The differences between the case's expectations and what happened, as text lines."""
    diffs = []
    expect = case["expect"]
    if actual["exit"] != expect["exit"]:
        diffs.append(f"exit: expected {expect['exit']}, got {actual['exit']}")
    for stream in ("stdout", "stderr"):
        match = re.fullmatch(fill(expect[stream], subs), actual[stream], re.S)
        if not match:
            diffs.append(f"{stream} doesn't match {expect[stream]!r}: {actual[stream]!r}")
        else:
            # A stdout pattern can name the pull request gh made: (?P<pr>[0-9]+).
            subs.update({key: value for key, value in match.groupdict().items() if value})
    if diffs:
        return diffs
    base = api_base(home)
    for state in case.get("state", []):
        path = fill(state["path"], subs)
        if "gone" in state:
            path = "branches/" + urllib.parse.quote(subs["branch"], safe="")
            try:
                request(base, token, "GET", path)
                gone = False
            except urllib.error.HTTPError as error:
                # Only Forgejo's word that the branch doesn't exist counts: a 404 from a
                # permission check names no branch, as the gateway's delete also reads it.
                errors = json.loads(error.read() or b"{}").get("errors") or []
                if error.code != 404 or not any(
                    str(e).startswith("branch does not exist [") for e in errors
                ):
                    raise
                gone = True
            if gone != state["gone"]:
                diffs.append(f"branch {subs['branch']}: expected gone={state['gone']}, got gone={gone}")
            continue
        value = request(base, token, "GET", path)
        if "fields" in state:
            for key, want in state["fields"].items():
                got = value
                for part in key.split("."):
                    got = got.get(part) if isinstance(got, dict) else None
                if got != (fill(want, subs) if isinstance(want, str) else want):
                    diffs.append(f"{path} {key}: expected {want!r}, got {got!r}")
        if "any" in state:
            if not any(all(item.get(k) == v for k, v in state["any"].items()) for item in value or []):
                diffs.append(f"{path}: no item has {state['any']!r}")
        if "none" in state:
            if any(all(item.get(k) == v for k, v in state["none"].items()) for item in value or []):
                diffs.append(f"{path}: an item has {state['none']!r}")
    return diffs
