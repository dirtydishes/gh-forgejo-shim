"""Replay the gh calls agent apps made (fixtures/clients) against the e2e Forgejo.

Each distinct call in the captures runs again as the app sent it, with the app's environment and
working directory: in `client` mode through the gh wrapper the shim's installer writes, in `server`
mode through plain gh and the server-mode gateway. Its answer must have the shape of GitHub's last
answer to the same call on the GitHub twin, whose PR states the e2e seed mirrors: the same exit
code, and for JSON the same keys and value types (GraphQL, `--json`) or no keys GitHub lacks (REST),
with a few fields holding the same value. DIFFERENCES lists where it may differ, and why.

The calls are read-only. ChatGPT's two writes, `pr merge --match-head-commit` and `pr create`, are
the contract's twin cases.

    E2E_TOKEN=<dirtydishes' token> python3 replay.py client|server

g2.sh runs it in the e2e client container, right after the twin is seeded.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

FIXTURES = Path(os.environ.get("REPLAY_FIXTURES", "/test/clients"))
HOST, OWNER, REPO = "forge.test", "dirtydishes", "gfj-oracle"
REAL_GH = "/usr/local/bin/gh"
# The environment a record may carry into the replay; anything else stays out.
SAFE_ENV = {"GH_PROMPT_DISABLED", "GIT_TERMINAL_PROMPT", "GH_PAGER", "NO_COLOR", "TERM", "CLICOLOR"}
# Fields whose values the twin's seed reproduces, so the shim's must equal GitHub's; a check's own
# fields too, which the PR card shows.
SAME_VALUE = {"number", "title", "state", "isDraft", "headRefName", "baseRefName", "body", "login"}
CHECK_TYPES = {"CheckRun", "StatusContext"}
CHECK_VALUE = {"__typename", "name", "context", "status", "conclusion", "state"}
# The order the captures were made in, so a later answer wins over an earlier one.
ORDER = ["chatgpt-22943-pr-card-s-pass.jsonl", "chatgpt-22943-states.jsonl", "claude-desktop-4.4212.jsonl",
         "claude-code-cli-2.1.309-314.jsonl", "chatgpt-22943-fj-probe.jsonl"]


def headless(path, actual, context):
    return path.endswith(".headRepository") and actual is None and context.get("state") in ("CLOSED", "MERGED")


def unchecked(path, actual, context):
    return path.endswith(".statusCheckRollup") and actual is None and context.get("number") == 6


def stale(path, actual, context):
    return context.get("number") == 3 and path.rsplit(".", 1)[-1] in ("status", "conclusion", "completedAt")


# (test, reason): a departure from GitHub's answer that the test accepts.
DIFFERENCES = [
    (headless, "the gateway reports a head repository only for an open PR whose branch is its head (MAPPING §10)"),
    (unchecked, "the e2e seed posts no statuses on PR 6 (scripts/e2e/seed.py); GitHub's PR 6 has two checks"),
    (stale, "GitHub's PR 3 check is stuck in progress (a stale job, contract/README.md); the e2e seed finishes it"),
]


def allowed(path, actual, context):
    return any(test(path, actual, context) for test, _ in DIFFERENCES)


def records():
    """Every captured call, with its file and its stdout resolved through `same_as`."""
    for path in sorted(FIXTURES.glob("*.jsonl"), key=lambda path: ORDER.index(path.name)):
        rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
        for row in rows:
            if isinstance(row.get("stdout"), dict) and "same_as" in row["stdout"]:
                row["stdout"] = rows[row["stdout"]["same_as"]]["stdout"]
            yield path.name, row


def forge_text(text):
    return text.replace("<user>", OWNER).replace("<repo>", REPO)


def forge_argv(argv):
    """The call as the app sends it in a Forgejo checkout: GitHub's host becomes the forge's, and
    `--repo OWNER/REPO` names the forge, as ChatGPT's own calls there do (the fj-probe capture)."""
    out = []
    for index, arg in enumerate(argv):
        before = argv[index - 1] if index else ""
        if before == "--hostname" and arg == "github.com":
            arg = HOST
        elif before in ("--repo", "-R") and arg.startswith("github.com/"):
            arg = HOST + arg[len("github.com"):]
        elif before in ("--repo", "-R") and arg.count("/") == 1:
            arg = f"{HOST}/{arg}"
        out.append(forge_text(arg))
    return out


def skip_reason(row):
    words = [arg for arg in row["argv"] if not arg.startswith("-")][:2]
    if row.get("faked") or "stubbed" in row:
        return "the capture tool answered, not gh"
    if words in (["pr", "merge"], ["pr", "create"]):
        return "a write: the twin cases cover it (contract/cases/twin-pr-merge-match-head.json, twin-pr-create.json)"
    if "anthropics/claude-code" in row["argv"]:
        return "it names a GitHub repository outside the twin"
    return None


def cases():
    """The distinct calls to replay, each with GitHub's last answer to it, and the calls left out."""
    found = {}
    skipped = {}
    for index, (name, row) in enumerate(records()):
        forge = forge_argv(row["argv"])
        reason = skip_reason(row)
        if reason:
            skipped[reason] = skipped.get(reason, 0) + 1
            continue
        where = "checkout" if "<repo>" in row["cwd"] else "root" if row["cwd"] == "/" else "elsewhere"
        # GitHub's answer wins over one from the Forgejo host before the shim (a call there shows
        # only what the app sends), and a later answer (ORDER, then line) over an earlier one: the
        # captures changed the twin as they went (ChatGPT merged PR 8 and its agent opened PR 9), and
        # the seed has the final states.
        rank = (HOST not in row["argv"] and f"{HOST}/" not in " ".join(row["argv"]), index)
        case = {"forge": forge, "where": where, "source": name, "exit": row["exit"], "stdout": row["stdout"],
                "env": {key: value for key, value in row.get("env", {}).items() if key in SAFE_ENV}, "rank": rank}
        key = (tuple(forge), where)
        if key not in found or rank > found[key]["rank"]:
            found[key] = case
    return list(found.values()), skipped


def expectation(case, mode):
    """What this call must do here: (exit, stdout or None for GitHub's, delegates, reason or None)."""
    forge = case["forge"]
    if forge == ["--version"] and mode == "client":
        return case["exit"], None, True, "gh's own version: the shim hands it to real gh"
    if forge == ["auth", "status", "--active"] and case["where"] == "root" and mode == "client":
        return 1, "", True, "away from a forge checkout the shim leaves it to gh's own login, and the e2e has none"
    if forge[:2] == ["pr", "list"] and "s/nopr" in forge:
        nopr = [{"headRefName": "s/nopr", "number": 9, "state": "OPEN", "url": f"https://{HOST}/{OWNER}/{REPO}/pull/9"}]
        return 0, json.dumps(nopr), False, "GitHub's search hadn't found PR 9 yet, which ChatGPT's agent had just opened"
    return case["exit"], None, False, None


def shape(value):
    if isinstance(value, dict):
        return {key: shape(item) for key, item in value.items()}
    if isinstance(value, list):
        return [shape(item) for item in value]
    return "null" if value is None else type(value).__name__


def identity(item):
    """What names a list item across the two answers: a check, by its type and name."""
    if isinstance(item, dict) and item.get("__typename") in CHECK_TYPES:
        return item["__typename"], item.get("name") or item.get("context")
    return None


def differences(expected, actual, exact, path="$", context=None):
    """Where actual departs from expected: `exact` (GraphQL, `--json`) wants the same keys,
    otherwise (REST) actual may lack keys GitHub has but not add its own. Types must match, lists
    the same length, and SAME_VALUE fields, and CHECK_VALUE fields of a check, the same values.
    `context` holds the enclosing objects' plain fields, for DIFFERENCES."""
    context = context or {}
    if isinstance(expected, dict) and isinstance(actual, dict):
        context = {**context, **{key: value for key, value in actual.items() if not isinstance(value, (dict, list))}}
        values = SAME_VALUE | (CHECK_VALUE if expected.get("__typename") in CHECK_TYPES else set())
        out = []
        missing = [key for key in expected if key not in actual]
        extra = [key for key in actual if key not in expected]
        if exact and missing:
            out.append(f"{path}: missing {', '.join(missing)}")
        if extra:
            out.append(f"{path}: unexpected {', '.join(extra)}")
        for key in expected:
            if key not in actual:
                continue
            where = f"{path}.{key}"
            out += differences(expected[key], actual[key], exact, where, context)
            mismatch = key in values and not isinstance(expected[key], (dict, list)) and expected[key] != actual[key]
            if mismatch and not allowed(where, actual[key], context):
                out.append(f"{where}: {actual[key]!r}, GitHub {expected[key]!r}")
        return out
    if isinstance(expected, list) and isinstance(actual, list):
        if len(expected) != len(actual):
            return [f"{path}: {len(actual)} items, GitHub {len(expected)}"]
        theirs = {identity(item): item for item in expected}
        out = []
        for index, item in enumerate(actual):
            key = identity(item)
            if key and key in theirs:
                out += differences(theirs[key], item, exact, f"{path}[{key[1]}]", context)
            elif key:
                out.append(f"{path}: {key[0]} {key[1]!r}, which GitHub's answer lacks")
            else:
                out += differences(expected[index], item, exact, f"{path}[{index}]", context)
        return out
    if shape(expected) != shape(actual) and not allowed(path, actual, context):
        return [f"{path}: {json.dumps(shape(actual))}, GitHub {json.dumps(shape(expected))}"]
    return []


def unversioned(text):
    """gh's version text without its version: the e2e's gh may be newer than the capture's (the
    weekly upstream workflow runs the newest)."""
    return re.sub(r"\d+\.\d+\.\d+(?: \(\d{4}-\d\d-\d\d\))?", "<version>", text)


def auth_lines(text):
    """`gh auth status` without what differs by design: the token's source, mask and scopes."""
    keep = []
    for line in text.splitlines():
        line = re.sub(r" \([^)]*\)$", "", line.strip())
        if line and not line.startswith(("- Token:", "- Token scopes:")):
            keep.append(line.replace("github.com", HOST))
    return keep


def compare(case, mode, code, stdout, delegated, token):
    """The ways this run departs from what it must do."""
    exit, wanted, delegates, _ = expectation(case, mode)
    problems = []
    if code != exit:
        problems.append(f"exit {code}, expected {exit}")
    if delegated and not delegates:
        problems.append(f"reached real gh outside a launch: {delegated[0][:100]}")
    if delegates and not delegated:
        problems.append("expected to reach real gh, and didn't")
    if case["forge"][:2] == ["auth", "token"]:
        # The capture never kept the token, but here it's the forge's own, and nothing else.
        if exit == 0 and stdout.strip() != token:
            problems.append("didn't print the forge's token alone")
        return problems
    if wanted is None and not isinstance(case["stdout"], str):
        return problems
    wanted = forge_text(case["stdout"] if wanted is None else wanted)
    try:
        want = json.loads(wanted)
    except ValueError:
        if case["forge"][:2] == ["auth", "status"]:
            if auth_lines(stdout) != auth_lines(wanted):
                problems.append(f"auth status {auth_lines(stdout)}, expected {auth_lines(wanted)}")
        elif case["forge"] == ["--version"]:
            if unversioned(stdout) != unversioned(wanted):
                problems.append(f"stdout {stdout[:200]!r}, expected the form of {wanted[:200]!r}")
        elif stdout != wanted:
            problems.append(f"stdout {stdout[:200]!r}, expected {wanted[:200]!r}")
        return problems
    try:
        got = json.loads(stdout)
    except ValueError:
        return problems + [f"stdout isn't JSON: {stdout[:200]!r}"]
    exact = case["forge"][:2] == ["api", "graphql"] or "--json" in case["forge"]
    return problems + differences(want, got, exact)


class Client:
    """The shim installed as gh by its own installer, configured for the forge, with no gh login.
    Real gh runs only when the shim launches it; any other call to it is logged as a delegation."""

    def __init__(self, root, token):
        home = root / "home"
        config = home / ".config/gh-forgejo-shim"
        config.mkdir(parents=True)
        (config / "config.toml").write_text(f'hosts = ["{HOST}"]\n[host."{HOST}"]\n'
                                            f'api_url = "http://forgejo:3000"\ntoken_env = "REPLAY_TOKEN"\n')
        self.log = root / "delegations"
        recorder = root / "real-gh"
        run_dir = "*/gfj-" + "[0-9a-f]" * 16
        recorder.write_text(f'#!/bin/sh\ncase "$GH_CONFIG_DIR" in {run_dir}) ;; *) echo "$*" >> {self.log} ;; esac\n'
                            f'exec {REAL_GH} "$@"\n')
        recorder.chmod(0o755)
        bin_dir = root / "bin"
        self.env = {"HOME": str(home), "FJ_SHIM_REAL_GH": str(recorder), "REPLAY_TOKEN": token,
                    "PATH": f"{bin_dir}:/usr/local/bin:/usr/bin:/bin"}
        subprocess.run(["gh-forgejo-shim", "install-shim", "--bin-dir", str(bin_dir)], env=self.env, check=True,
                       stdout=subprocess.DEVNULL)

    def delegated(self):
        """The calls that reached real gh outside a launch since the last look."""
        if not self.log.exists():
            return []
        calls = self.log.read_text().splitlines()
        self.log.unlink()
        return calls


class Server:
    """Plain gh against the server-mode gateway at https://forge.test, logged in to it the way the
    server-mode docs have users do."""

    def __init__(self, root, token):
        self.env = {"HOME": str(root / "home"), "GH_CONFIG_DIR": str(root / "gh"), "PATH": "/usr/local/bin:/usr/bin:/bin"}
        subprocess.run([REAL_GH, "auth", "login", "--hostname", HOST, "--with-token"], input=token.encode(),
                       env=self.env, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def delegated(self):
        # Without the shim, every call is real gh's own.
        return []


def checkout(root):
    """A checkout of the twin on `s/pass`, as the apps had one open."""
    path = root / "checkout"
    path.mkdir()
    for args in (["init", "-q", "-b", "s/pass"], ["remote", "add", "origin", f"https://{HOST}/{OWNER}/{REPO}.git"]):
        subprocess.run(["git", *args], cwd=path, check=True)
    return path


def main():
    mode = sys.argv[1]
    if mode not in ("client", "server"):
        raise SystemExit("usage: replay.py client|server")
    token = os.environ["E2E_TOKEN"]
    calls, skipped = cases()
    failed = 0
    with tempfile.TemporaryDirectory(prefix="replay-") as directory:
        root = Path(directory)
        target = Client(root, token) if mode == "client" else Server(root, token)
        # Where each call ran: the twin's checkout, `/`, or any other folder.
        places = {"checkout": checkout(root), "root": Path("/"), "elsewhere": root / "elsewhere"}
        places["elsewhere"].mkdir()
        for case in sorted(calls, key=lambda case: case["forge"]):
            result = subprocess.run(["gh", *case["forge"]], cwd=places[case["where"]],
                                    env=target.env | case["env"], stdin=subprocess.DEVNULL,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
            stdout = result.stdout.decode()
            problems = compare(case, mode, result.returncode, stdout, target.delegated(), token)
            if token in stdout or token in result.stderr.decode():
                # A token only ever reaches stdout for `auth token`, whose output the capture never kept.
                if case["forge"][:2] != ["auth", "token"]:
                    problems.append("printed the token")
            label = re.sub(r"\s+", " ", " ".join(case["forge"]))
            label = (label if len(label) < 110 else label[:107] + "...") + f" [{case['where']}]"
            print(("FAIL " if problems else "PASS ") + label)
            reason = expectation(case, mode)[3]
            if reason:
                print(f"  as expected: {reason}")
            for problem in problems:
                print(f"  {problem}")
            failed += bool(problems)
    for reason, count in sorted(skipped.items()):
        print(f"skipped {count} captured calls: {reason}")
    for _, reason in DIFFERENCES:
        print(f"allowed: {reason}")
    print(f"{'FAIL' if failed else 'PASS'} replay ({mode}): {len(calls) - failed} of {len(calls)} captured calls")
    return int(bool(failed))


if __name__ == "__main__":
    sys.exit(main())
