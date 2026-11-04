#!/usr/bin/env python3
"""Compare local argument errors with pinned gh, using dummy auth and a closed proxy."""

import json
import os
import subprocess
import tempfile
from pathlib import Path

from capture import official_gh
from common import ROOT


def main():
    commands = ["pr view", "pr list", "pr checks", "pr diff", "pr status",
                "repo view", "issue list", "issue view", "run list", "run view", "auth status", "api"]
    usage = json.loads((ROOT / "contract/usage-errors.json").read_text())
    probes = []
    for command in commands:
        prefix = command.split()
        probes.extend([prefix + ["--gfjs-unknown"], prefix + ["-Z"]])
        if "--json " in usage[command]["suffix"]:
            probes.extend([prefix + ["--json", "x"], prefix + ["--json"],
                           prefix + ["--jq", ".number"], prefix + ["--template"],
                           prefix + ["--json=number, title"]])
    probes.extend([
        ["pr", "view", "1", "2"], ["issue", "view"], ["api"],
        ["pr", "list", "arg"], ["pr", "list", "arg", "another"],
        ["pr", "view", "1", "--comments=bad"],
        ["pr", "view", "1", "-t"], ["pr", "view", "1", "-R"],
        ["pr", "list", "--limit", "nope"], ["run", "view", "--attempt", "nope"],
        ["pr", "view", "1", "--json", "x", "--gfjs-unknown"],
        ["pr", "viwe"], ["pr", "gfjs-unknown"],
    ])
    probes.extend([["pr", "list", "--limit", value, "--gfjs-unknown"] for value in ["0x20", "010", "0b10", "1_0", "+1", "-1", "08"]])
    probes.extend([
        ["pr", "view", "1", "--json", '\"number,title\"'],
        ["pr", "view", "1", "--json", '\"number'],
        ["pr", "view", "1", "--json", "number,"],
        ["pr", "view", "1", "--json", "number", "--json", "x"],
        ["pr", "view", "1", "-cwZ"],
        ["pr", "view", "1", "--comments", "false"],
        ["pr", "view", "1", "--template", "--gfjs-unknown"],
        ["pr", "cheks"],
        ["run", "-a2", "view", "--gfjs-unknown"],
        ["run", "-a", "2", "view", "--gfjs-unknown"],
        ["run", "-a", "list", "--gfjs-unknown"],
        ["pr", "--json", "x", "view", "1"],
        ["pr", "--gfjs-unknown", "gfjs-unknown"],
    ])
    cache = ROOT / "target/contract"
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="parser-", dir=cache) as temporary:
        binary = official_gh(Path(temporary))
        env = {"PATH": os.environ["PATH"], "HOME": temporary, "GH_CONFIG_DIR": temporary,
               "GH_TOKEN": "gfjs-dummy-client-errors", "NO_COLOR": "1", "TERM": "dumb",
               "GH_NO_UPDATE_NOTIFIER": "1", "GH_PROMPT_DISABLED": "1", "LC_ALL": "C",
               "HTTP_PROXY": "http://127.0.0.1:1", "HTTPS_PROXY": "http://127.0.0.1:1",
               "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}
        failed = 0
        for argv in probes:
            oracle = subprocess.run([str(binary), *argv], env=env | {
                "GH_HOST": "github.com", "GH_REPO": "github.com/dirtydishes/gfj-oracle"},
                cwd=temporary, capture_output=True, timeout=10)
            actual = subprocess.run([str(ROOT / "target/debug/gh-forgejo-shim"), "gh", *argv],
                env=env | {"GH_HOST": "forge.test", "GH_REPO": "forge.test/dirtydishes/gfj-oracle",
                           "FJ_SHIM_HOSTS": "forge.test"}, cwd=temporary, capture_output=True, timeout=10)
            expected = (oracle.returncode, oracle.stdout, oracle.stderr)
            observed = (actual.returncode, actual.stdout, actual.stderr)
            if expected != observed:
                failed += 1
                print(f"FAIL {argv!r}\n  gh:   {expected!r}\n  shim: {observed!r}")
        print(f"parser oracle: {len(probes) - failed} matched, {failed} mismatched")
        return int(failed != 0)


if __name__ == "__main__":
    raise SystemExit(main())
