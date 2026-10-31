"""Compare the isolated shim's exit/stdout/stderr with committed real-gh captures."""

import difflib
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from common import REPO, ROOT, load_cases, normalize


def classify(case, actual):
    expected = case["expected"] if case.get("kind") == "shim-local" else case["oracle"]
    diffs = []
    if actual["exit"] != expected["exit"]:
        diffs.append(f"exit: expected {expected['exit']}, got {actual['exit']}")
    for stream in ("stdout", "stderr"):
        left = normalize(expected[stream], case["normalize"])
        right = normalize(actual[stream], case["normalize"])
        if left != right:
            diffs.extend(difflib.unified_diff(left.splitlines(keepends=True), right.splitlines(keepends=True),
                                            fromfile=f"oracle/{stream}", tofile=f"shim/{stream}"))
    if diffs:
        return ("xfail" if case.get("xfail") else "unexpected"), "\n".join(line.rstrip("\n") for line in diffs)
    if case.get("xfail"):
        return "unexpected", f"remove xfail from {case['id']}"
    return "pass", ""


def execute(case, binary, seed_home, work_dir):
    with tempfile.TemporaryDirectory(prefix=case["id"] + "-", dir=work_dir) as temporary:
        directory = Path(temporary)
        home = directory / "home"
        shutil.copytree(seed_home, home)
        tools = directory / "tools"
        tools.mkdir()
        git = shutil.which("git")
        if not git:
            raise RuntimeError("git is required")
        (tools / "git").symlink_to(git)
        decoy = tools / "gh"
        marker = directory / "delegated"
        decoy.write_text('#!/bin/sh\n: > "$GFJS_DECOY_MARKER"\nprintf "%s\\n" "ERROR: contract delegated to real gh" >&2\nexit 97\n')
        decoy.chmod(0o755)
        entrypoint = directory / "gh"
        entrypoint.write_text(f'#!/bin/sh\nexec {shlex.quote(str(binary))} gh "$@"\n')
        entrypoint.chmod(0o755)
        checkout = directory / "checkout"
        checkout.mkdir()
        # A fresh environment prevents host tokens, config, proxies, and Git overrides from leaking in.
        # PATH has only git and the gh decoy, so macOS cannot consult the user's security/Keychain.
        env = {
            "HOME": str(home), "PATH": str(tools), "GH_HOST": "forge.test",
            "GH_PROMPT_DISABLED": "1", "GH_PAGER": "cat", "NO_COLOR": "1", "TERM": "dumb",
            "LC_ALL": "C", "TZ": "UTC", "GFJS_TOKEN": os.environ["GFJS_TOKEN"],
            "GFJS_DECOY_MARKER": str(marker), "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        } | case.get("env", {})
        for args in (("init", "-q", "-b", "s/pass"), ("remote", "add", "origin", f"https://forge.test/{REPO}.git")):
            subprocess.run([git, *args], cwd=checkout, env=env, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        result = subprocess.run([str(entrypoint), *case["argv"]], cwd=checkout, env=env,
                                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        if marker.exists():
            raise RuntimeError(f"{case['id']}: shim invoked the real-gh decoy")
        if result.returncode < 0:
            raise RuntimeError(f"{case['id']}: shim terminated by signal {-result.returncode}")
        actual = {"exit": result.returncode, "stdout": result.stdout.decode(), "stderr": result.stderr.decode()}
        if env["GFJS_TOKEN"] in actual["stdout"] or env["GFJS_TOKEN"] in actual["stderr"]:
            raise RuntimeError(f"{case['id']}: refused output containing the Forgejo token")
        return actual


def main():
    cases = load_cases(os.environ.get("GFJS_CASES_DIR", ROOT / "contract/cases"))
    binary = Path(os.environ["GFJS_SHIM_BINARY"]).resolve(strict=True)
    seed_home = Path(os.environ["GFJS_SEED_HOME"])
    work_dir = Path(os.environ["GFJS_WORK_DIR"])
    counts = {"pass": 0, "xfail": 0, "unexpected": 0}
    for _, case in cases:
        try:
            actual = execute(case, binary, seed_home, work_dir)
            outcome, detail = classify(case, actual)
        except (RuntimeError, OSError, subprocess.SubprocessError) as error:
            # Setup, timeout, crashes, and fallback delegation can never satisfy an xfail.
            outcome, detail = "unexpected", str(error)
        counts[outcome] += 1
        print(f"{outcome} {case['id']}" + (f": {case['xfail']}" if outcome == "xfail" else ""))
        if detail:
            print(detail)
    print(f"pass {counts['pass']}, xfail {counts['xfail']}, unexpected {counts['unexpected']}")
    return int(counts["unexpected"] != 0)


if __name__ == "__main__":
    sys.exit(main())
