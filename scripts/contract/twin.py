"""`twin` cases: writes run by real gh through the shim against the local Forgejo twin.

GitHub-side oracles for writes would need writes to GitHub, which captures refuse. gh is the real
binary, so its output is GitHub's by construction. A twin case checks what the gateway decides:
that its answers drive gh to the expected exit and output, and that the forge ends up in the state
asked for, read back through Forgejo's own API. Each case works on a branch, and optionally a pull
request, of its own, so cases don't depend on their order.
"""

import json
import re
import urllib.request
from pathlib import Path

from common import REPO

# What a twin case may run: the launched write commands, and only these flags.
TWIN_COMMANDS = {
    ("pr", "create"): {"--head", "--base", "--title", "--body", "--draft"},
    ("pr", "comment"): {"--body"},
    ("pr", "edit"): {"--title", "--body"},
}
PLACEHOLDERS = {"{pr}", "{branch}"}
STATE_PATH = re.compile(r"(pulls|issues)/\{pr\}(/comments)?")
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
    index = 2
    while index < len(argv):
        arg = argv[index]
        if arg.startswith("-"):
            if arg not in flags:
                raise ValueError(f"twin flag is not allowed: {arg}")
            if arg != "--draft":
                index += 1
                if index == len(argv):
                    raise ValueError(f"missing value for {arg}")
                if arg == "--head":
                    heads.append(argv[index])
                if arg == "--base" and argv[index] != "main":
                    raise ValueError("--base must be main")
        elif arg in PLACEHOLDERS:
            selectors.append(arg)
        else:
            raise ValueError("a twin selector must be {pr} or {branch}")
        index += 1
    # Without a selector gh picks the checkout's branch, s/pass: scenario PR 1.
    if command == ("pr", "create"):
        if heads != ["{branch}"] or selectors:
            raise ValueError("pr create needs one --head {branch}, its own branch")
    elif len(selectors) != 1:
        raise ValueError("a twin pr command needs one selector, {pr} or {branch}")
    setup = case.get("setup")
    if not isinstance(setup, dict) or not BRANCH.fullmatch(setup.get("branch", "")):
        raise ValueError("twin setup needs a branch named twin/<name>")
    if set(setup) - {"branch", "pr"}:
        raise ValueError("twin setup takes only branch and pr")
    if "{pr}" in argv and not setup.get("pr"):
        raise ValueError("{pr} needs setup.pr")
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
    for check in case.get("state", []):
        path = check.get("path", "")
        if not STATE_PATH.fullmatch(path) or not (set(check) - {"path"}) <= {"fields", "any"}:
            raise ValueError("twin state checks read the case's own pull request")
        if "fields" in check and not isinstance(check["fields"], dict):
            raise ValueError("twin state fields must be an object")
        if "any" in check and (not path.endswith("/comments") or not isinstance(check["any"], dict)):
            raise ValueError("twin state any is an object, on a comments list")
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
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f"{base}/api/v1/repos/{REPO}/{path}", data=data, method=method,
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
    branch = case["setup"]["branch"]
    request(base, token, "POST", f"contents/{branch}.txt",
            {"content": "dHdpbgo=", "branch": "main", "new_branch": branch, "message": branch})
    subs = {"branch": branch}
    if case["setup"].get("pr"):
        pr = request(base, token, "POST", "pulls", {"head": branch, "base": "main", "title": f"twin: {branch}"})
        subs["pr"] = str(pr["number"])
    return subs


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
    return diffs
