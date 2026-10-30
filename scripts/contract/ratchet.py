"""Prove both expected-failure rules against live Forgejo using scratch case copies."""

import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

from common import ROOT, load_cases


def main():
    source_cases = load_cases(ROOT / "contract/cases")
    passes = sum("xfail" not in case for _, case in source_cases)
    xfails = len(source_cases) - passes
    with tempfile.TemporaryDirectory(prefix="ratchet-", dir=os.environ["GFJS_WORK_DIR"]) as temporary:
        cases = Path(temporary) / "cases"
        shutil.copytree(ROOT / "contract/cases", cases)
        missing = cases / "pr-checks-none-5.json"
        original = json.loads(missing.read_text())
        edited = dict(original)
        del edited["xfail"]
        missing.write_text(json.dumps(edited))

        def verify(label, required):
            result = subprocess.run(
                ["cargo", "test", "-p", "gh-forgejo-shim", "--locked", "--test", "contract", "--", "--ignored", "--nocapture"],
                cwd=ROOT, env=os.environ | {"GFJS_CASES_DIR": str(cases)},
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=120,
            )
            print(result.stdout, end="")
            summary = f"pass {passes}, xfail {xfails - 1}, unexpected 1"
            if result.returncode == 0 or required not in result.stdout or summary not in result.stdout:
                raise RuntimeError(f"{label}: did not observe the required contract failure")
            print(f"PASS {label}: cargo exited {result.returncode}, expected contract failure observed", flush=True)

        verify("removed xfail in scratch copy", "unexpected pr-checks-none-5")
        missing.write_text(json.dumps(original))
        # Mark a known passing case as expected failure to simulate a fixed implementation.
        passing = cases / "pr-view-closed-7.json"
        case = json.loads(passing.read_text())
        case["xfail"] = "self-test: a passing case must reject an obsolete xfail"
        passing.write_text(json.dumps(case))
        result = subprocess.run(
            ["cargo", "test", "-p", "gh-forgejo-shim", "--locked", "--test", "contract", "--", "--ignored", "--nocapture"],
            cwd=ROOT, env=os.environ | {"GFJS_CASES_DIR": str(cases)},
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=120,
        )
        print(result.stdout, end="")
        summary = f"pass {passes - 1}, xfail {xfails}, unexpected 1"
        if result.returncode == 0 or "remove xfail from pr-view-closed-7" not in result.stdout or summary not in result.stdout:
            raise RuntimeError("passing xfail did not fail the runner")
        print(f"PASS passing xfail in scratch copy: cargo exited {result.returncode}, remove-xfail instruction observed")


if __name__ == "__main__":
    main()
