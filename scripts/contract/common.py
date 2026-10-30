"""Case validation and explicit output normalization, shared by capture and tests."""

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REPO = "dirtydishes/gfj-oracle"
NORMALIZERS = {
    "host", "pr_urls", "timestamps", "durations", "shas", "node_ids",
    "check_details_urls", "json",
}
VALUE_FLAGS = {
    "--json", "--jq", "-q", "--head", "-H", "--base", "-B", "--author",
    "-A", "--state", "-s", "--limit", "-L", "--label",
    "-l", "--assignee", "-a", "--repo", "-R",
}
BOOL_FLAGS = {"--name-only", "--patch", "--required", "--verbose"}
SAFE_ENV = {"NO_COLOR", "CLICOLOR", "TERM", "TZ", "LANG", "LC_ALL", "COLUMNS"}

# Exact client-error probes keep unknown flags and missing values out of the general allowlist.
ERROR_PROBES = [
    ["pr", "view", "1", "--gfjs-unknown"],
    ["pr", "view", "1", "--template"],
    ["pr", "view", "1", "--json"],
    ["pr", "view", "1", "--json", "x"],
    ["pr", "view", "1", "--jq", ".number"],
    ["pr", "gfjs-unknown"],
    ["pr", "view", "1", "-Z"],
    ["pr", "view", "1", "--comments=no"],
]
LOCAL_PROBES = [
    ["pr", "merge", "1"],
    ["pr", "edit", "1", "--title", "x"],
    ["api", f"repos/{REPO}/pulls"],
]


def validate_case(case):
    """Reject unsafe commands before reading credentials or starting any command."""
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]*", case["id"]):
        raise ValueError("invalid case id")
    if case["repo"] != REPO:
        raise ValueError("only dirtydishes/gfj-oracle is allowed")
    if not set(case.get("env", {})) <= SAFE_ENV:
        raise ValueError("case env may only change output formatting")
    if not set(case["normalize"]) <= NORMALIZERS:
        raise ValueError("unknown normalizer")
    if "json" in case["normalize"] and not case.get("normalize_reason", {}).get("json"):
        raise ValueError("json normalization needs a one-line justification")
    if "xfail" in case and not case["xfail"].strip():
        raise ValueError("xfail needs a concrete reason")
    argv = case["argv"]
    if not isinstance(argv, list) or not argv or any(not isinstance(a, str) for a in argv):
        raise ValueError("argv must be a nonempty string array")
    kind = case.get("kind", "oracle")
    if kind == "shim-local":
        if argv not in LOCAL_PROBES:
            raise ValueError("shim-local command is outside the exact local allowlist")
        if not isinstance(case.get("expected"), dict) or "oracle" in case:
            raise ValueError("shim-local cases require expected output and no oracle")
        return
    if kind != "oracle":
        raise ValueError("unknown case kind")
    if argv in ERROR_PROBES:
        return
    command = tuple(argv[:2])
    if argv == ["--version"]:
        return
    allowed = {("pr", verb) for verb in ("view", "list", "checks", "diff", "status")}
    allowed |= {("repo", "view"), ("api", "user"), ("auth", "status")}
    if command not in allowed:
        raise ValueError("command is outside the read-only allowlist")
    # --show-token is deliberately excluded even though auth status is read-only.
    values = VALUE_FLAGS if command[0] == "pr" else {"--json", "--jq", "-q"}
    bools = BOOL_FLAGS if command[0] == "pr" else set()
    index = 2
    positional = 0
    while index < len(argv):
        arg = argv[index]
        flag, equal, value = arg.partition("=")
        if flag in values:
            if not equal:
                index += 1
                if index == len(argv):
                    raise ValueError(f"missing value for {flag}")
                value = argv[index]
            if flag in {"--repo", "-R"} and value != f"forge.test/{REPO}":
                raise ValueError("repository override must name the twin")
        elif arg in bools:
            pass
        elif command[0] == "pr" and arg.startswith("-R") and len(arg) > 2:
            if arg[2:].removeprefix("=") != f"forge.test/{REPO}":
                raise ValueError("repository override must name the twin")
        elif not arg.startswith("-") and command in {
            ("pr", "view"), ("pr", "checks"), ("pr", "diff"), ("repo", "view"),
        }:
            positional += 1
            if positional > 1:
                raise ValueError("too many positional arguments")
            if command == ("repo", "view"):
                if arg != f"forge.test/{REPO}":
                    raise ValueError("repository must name the twin")
            elif not re.fullmatch(r"[0-9]+|s/[a-z0-9-]+", arg):
                raise ValueError("PR selector must be a number or scenario branch")
        else:
            raise ValueError(f"flag or argument is not allowed: {arg}")
        index += 1


def load_cases(directory):
    cases = []
    for path in sorted(Path(directory).glob("*.json")):
        case = json.loads(path.read_text())
        validate_case(case)
        if path.stem != case["id"]:
            raise ValueError(f"case id does not match filename: {path.name}")
        cases.append((path, case))
    if not cases or len({case["id"] for _, case in cases}) != len(cases):
        raise ValueError("case directory is empty or has duplicate ids")
    return cases


def oracle_argv(case):
    if case.get("kind") == "shim-local":
        raise ValueError("shim-local cases must never reach real gh")
    return [arg.replace(f"forge.test/{REPO}", f"github.com/{REPO}") for arg in case["argv"]]


def normalize(text, names):
    for name in names:
        if name == "host":
            text = re.sub(r"(?<![\w.-])(?:github\.com|forge\.test)(?![\w.-])", "<host>", text)
        elif name == "pr_urls":
            text = re.sub(r"(/[^/\s\"]+/[^/\s\"]+)/pulls?/(\d+)", r"\1/pull/\2", text)
        elif name == "timestamps":
            text = re.sub(r"\b\d{4}-\d\d-\d\d[T ]\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)\b", "<timestamp>", text)
        elif name == "durations":
            text = re.sub(r"\b(?:\d+h)?(?:\d+m)?\d+(?:\.\d+)?s\b|\b\d+h(?:\d+m)?\b|\b\d+m\b", "<duration>", text)
        elif name == "shas":
            text = re.sub(r"\b[0-9a-fA-F]{7,40}\b", "<sha>", text)
        elif name == "node_ids":
            text = re.sub(r'("(?:id|node_id|nodeId)"\s*:\s*")[^"\n]+(")', r"\1<node-id>\2", text)
        elif name == "check_details_urls":
            text = re.sub(r'(?:https?://[^/\s\"]+)?/[^/\s\"]+/[^/\s\"]+/actions/runs/\d+/(?:jobs|job)/\d+', "<check-url>", text)
            text = re.sub(r'("detailsUrl"\s*:\s*")[^"\n]+(")', r"\1<check-url>\2", text)
        elif name == "json":
            try:
                text = json.dumps(json.loads(text), sort_keys=True, separators=(",", ":")) + "\n"
            except json.JSONDecodeError:
                # Invalid/missing JSON remains a real output mismatch, never an xfail bypass.
                pass
        else:
            raise ValueError(f"unknown normalizer: {name}")
    return text
