"""gh-capture against a fake gh: what it passes through, what it records, and what it never does."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

WRAPPER = Path(__file__).resolve().parent / "gh-capture"
# The fake gh: what it does is set by FAKE_GH, so each test asks for the behaviour it needs.
FAKE_GH = r'''#!/usr/bin/env python3
import os, signal, sys, time
mode = os.environ.get("FAKE_GH", "echo")
if mode == "echo":
    sys.stdout.write("out " + " ".join(sys.argv[1:]) + "\n")
    sys.stderr.write("err\n")
    sys.exit(int(os.environ.get("FAKE_EXIT", "0")))
if mode == "print":
    sys.stdout.write(os.environ["FAKE_OUT"])
'''


class Wrapper:
    """A temporary home for one test: the fake gh, a log path, and a way to run gh-capture."""

    def __init__(self, test):
        directory = tempfile.TemporaryDirectory()
        test.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.gh = self.root / "gh"
        self.gh.write_text(FAKE_GH)
        self.gh.chmod(0o755)
        self.log = self.root / "logs" / "calls.jsonl"
        self.env = {"PATH": os.environ["PATH"], "HOME": str(self.root), "GH_CAPTURE_REAL": str(self.gh),
                    "GH_CAPTURE_LOG": str(self.log)}

    def run(self, *argv, **env):
        return subprocess.run([sys.executable, str(WRAPPER), *argv], env=self.env | env, cwd=self.root,
                              stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)

    def records(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]


class PassThroughTest(unittest.TestCase):
    def test_gh_answers_unchanged_and_the_call_is_recorded(self):
        wrapper = Wrapper(self)
        result = wrapper.run("pr", "view", "1", FAKE_EXIT="3")
        self.assertEqual((result.returncode, result.stdout, result.stderr), (3, b"out pr view 1\n", b"err\n"))
        [record] = wrapper.records()
        self.assertEqual((record["argv"], record["exit"], record["stdout"], record["stderr"]),
                         (["pr", "view", "1"], 3, "out pr view 1\n", "err\n"))
        self.assertEqual(wrapper.log.stat().st_mode & 0o777, 0o600)


class LogFailureTest(unittest.TestCase):
    """#197 defect 6: a log that can't be written never breaks the gh call."""

    def test_a_log_path_without_a_directory(self):
        wrapper = Wrapper(self)
        result = wrapper.run("pr", "list", GH_CAPTURE_LOG="calls.jsonl")
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, b"out pr list\n", b"err\n"))
        self.assertEqual(json.loads((wrapper.root / "calls.jsonl").read_text())["argv"], ["pr", "list"])

    def test_a_log_that_cant_be_written(self):
        wrapper = Wrapper(self)
        blocker = wrapper.root / "file"
        blocker.write_text("")
        for log in (blocker / "calls.jsonl", wrapper.root):
            result = wrapper.run("pr", "list", GH_CAPTURE_LOG=str(log), FAKE_EXIT="2")
            self.assertEqual((result.returncode, result.stdout, result.stderr), (2, b"out pr list\n", b"err\n"), log)


if __name__ == "__main__":
    unittest.main()
