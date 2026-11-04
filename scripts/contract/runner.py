"""Compare the isolated shim's exit/stdout/stderr with committed real-gh captures."""

import difflib
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from capture import official_gh
from common import REPO, ROOT, launch_list, launched, load_cases, normalize
import twin


def classify(case, actual):
    if case.get("kind") == "decoy":
        # execute() already required the decoy to run; there is no output to compare.
        return "pass", ""
    if case.get("kind") == "twin":
        # execute() checked the exit, the output patterns and the forge's state.
        return ("unexpected", "\n".join(actual["diffs"])) if actual["diffs"] else ("pass", "")
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


def gh_script(real_gh=None):
    """The `gh` on a case's PATH. The decoy marks a delegation and fails. For a launch-list case,
    real gh runs only when the launcher started it, with a run directory as its config, so a
    delegation still hits the decoy."""
    decoy = ': > "$GFJS_DECOY_MARKER"\nprintf "%s\\n" "ERROR: contract delegated to real gh" >&2\nexit 97\n'
    if real_gh is None:
        return "#!/bin/sh\n" + decoy
    run_dir = "*/gfj-" + "[0-9a-f]" * 16
    return f'#!/bin/sh\ncase "$GH_CONFIG_DIR" in {run_dir}) exec {shlex.quote(str(real_gh))} "$@" ;; esac\n' + decoy


def execute(case, binary, seed_home, work_dir, real_gh=None):
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
        decoy.write_text(gh_script(real_gh))
        decoy.chmod(0o755)
        entrypoint = directory / "gh"
        entrypoint.write_text(f'#!/bin/sh\nexec {shlex.quote(str(binary))} gh "$@"\n')
        entrypoint.chmod(0o755)
        checkout = directory / "checkout"
        checkout.mkdir()
        # A fresh environment prevents host tokens, config, proxies, and Git overrides from leaking in.
        # PATH has only git and the gh decoy; FJ_SHIM_AUTH_STORE=file keeps the shim off the user's Keychain.
        env = {
            "HOME": str(home), "PATH": str(tools), "GH_HOST": "forge.test",
            "GH_PROMPT_DISABLED": "1", "GH_PAGER": "cat", "NO_COLOR": "1", "TERM": "dumb",
            "LC_ALL": "C", "TZ": "UTC", "GFJS_TOKEN": os.environ["GFJS_TOKEN"], "FJ_SHIM_AUTH_STORE": "file",
            "GFJS_DECOY_MARKER": str(marker), "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        } | case.get("env", {})
        for args in (("init", "-q", "-b", "s/pass"), ("remote", "add", "origin", f"https://forge.test/{REPO}.git")):
            subprocess.run([git, *args], cwd=checkout, env=env, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        argv = case["argv"]
        if case.get("kind") == "twin":
            subs = twin.prepare(case, home, env["GFJS_TOKEN"])
            argv = [twin.fill(arg, subs) for arg in argv]
        result = subprocess.run([str(entrypoint), *argv], cwd=checkout, env=env,
                                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        if case.get("kind") == "decoy":
            if not marker.exists() or result.returncode != 97:
                raise RuntimeError(f"{case['id']}: an off-list command did not reach the decoy")
            return {"exit": result.returncode, "stdout": "", "stderr": ""}
        if marker.exists():
            raise RuntimeError(f"{case['id']}: shim invoked the real-gh decoy")
        if result.returncode < 0:
            raise RuntimeError(f"{case['id']}: shim terminated by signal {-result.returncode}")
        actual = {"exit": result.returncode, "stdout": result.stdout.decode(), "stderr": result.stderr.decode()}
        if env["GFJS_TOKEN"] in actual["stdout"] or env["GFJS_TOKEN"] in actual["stderr"]:
            raise RuntimeError(f"{case['id']}: refused output containing the Forgejo token")
        if case.get("kind") == "twin":
            actual["diffs"] = twin.check(case, actual, home, env["GFJS_TOKEN"], subs)
        return actual


def main():
    cases = load_cases(os.environ.get("GFJS_CASES_DIR", ROOT / "contract/cases"))
    binary = Path(os.environ["GFJS_SHIM_BINARY"]).resolve(strict=True)
    seed_home = Path(os.environ["GFJS_SEED_HOME"])
    work_dir = Path(os.environ["GFJS_WORK_DIR"])
    counts = {"pass": 0, "xfail": 0, "unexpected": 0}
    commands = launch_list()
    real_gh = None
    if any(launched(case, commands) for _, case in cases):
        # Checksum-pinned official gh, as the oracle captures use; a mismatch raises.
        gh_dir = Path(tempfile.mkdtemp(prefix="real-gh-", dir=work_dir))
        real_gh = official_gh(gh_dir)
    # Twin cases write to the twin, so they run after every case that reads it. Fork cases run
    # last: reopening a PR whose fork is gone makes Forgejo 15's PR checker panic, and the panic
    # drops the checks queued with it, so the next case's PR could stay "checking" and refuse
    # its merge. Fork cases never wait for a check.
    for _, case in sorted(cases, key=lambda item: (item[1].get("kind") == "twin",
                                                   bool(item[1].get("setup", {}).get("fork")))):
        try:
            actual = execute(case, binary, seed_home, work_dir,
                             real_gh if launched(case, commands) else None)
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
