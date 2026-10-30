"""Tests for the boundaries that must not silently weaken the contract."""

import contextlib
import copy
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import capture
import runner
from common import REPO, ROOT, load_cases, normalize, oracle_argv, validate_case


class ContractTests(unittest.TestCase):
    def case(self, argv=None):
        return {"id": "probe", "repo": REPO, "argv": argv or ["api", "user", "--jq", ".login"],
                "normalize": [], "oracle": {"exit": 0, "stdout": "dirtydishes\n", "stderr": ""}}

    def test_all_committed_cases_validate(self):
        self.assertGreaterEqual(len(load_cases(ROOT / "contract/cases")), 9)

    def test_reject_writes_and_credential_disclosure(self):
        unsafe = [
            ["pr", "create"], ["pr", "close", "1"], ["auth", "token"], ["repo", "delete"],
            ["api", "user", "--method", "POST"], ["api", "user", "-XGET"],
            ["api", "user", "-f", "name=oops"], ["api", "user", "--input", "-"],
            ["api", "user", "--hostname", "evil.test"], ["api", "/user"],
            ["auth", "status", "--show-token"], ["pr", "checks", "1", "--watch"],
            ["pr", "view", "https://github.com/other/repo/pull/1"],
            ["pr", "view", "1", "--repo=github.com/other/repo"],
            ["repo", "view", "other/repo"], ["pr", "view", "1", "--web"],
            ["pr", "list", "--search", "repo:other/repo"],
        ]
        for argv in unsafe:
            with self.subTest(argv=argv), self.assertRaises(ValueError):
                validate_case(self.case(argv))

    def test_allowlisted_commands_and_twin_repo(self):
        safe = [["pr", verb] for verb in ("view", "list", "checks", "diff", "status")]
        safe += [["repo", "view", f"forge.test/{REPO}"], ["api", "user"], ["auth", "status"], ["--version"]]
        for argv in safe:
            validate_case(self.case(argv))

    def test_reject_env_injection_even_for_version(self):
        for argv in (["--version"], ["api", "user"]):
            for name in ("GH_TOKEN", "GH_REPO", "PATH", "HOME", "FJ_SHIM_HOSTS", "GFJS_TOKEN"):
                case = self.case(argv) | {"env": {name: "unsafe"}}
                with self.subTest(name=name, argv=argv), self.assertRaises(ValueError):
                    validate_case(case)

    def test_validate_every_case_before_auth(self):
        with patch.object(capture, "load_cases", side_effect=ValueError("unsafe case")), \
                patch.object(capture, "official_gh") as download, \
                patch.object(capture.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "unsafe case"):
                capture.main()
            download.assert_not_called()
            run.assert_not_called()

    def test_checksum_mismatch_refuses_executable(self):
        class Download:
            stdout = b"tampered archive"

        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(capture.platform, "system", return_value="Linux"), \
                patch.object(capture.platform, "machine", return_value="x86_64"), \
                patch.object(capture.subprocess, "run", return_value=Download()):
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                capture.official_gh(Path(temporary))
            self.assertFalse((Path(temporary) / "gh").exists())

    def test_oracle_repo_translation(self):
        case = self.case(["pr", "list", "--repo", f"forge.test/{REPO}"])
        self.assertEqual(oracle_argv(case)[-1], f"github.com/{REPO}")
        self.assertEqual(case["argv"][-1], f"forge.test/{REPO}")

    def test_host_and_pull_url_normalization_keeps_number(self):
        names = ["host", "pr_urls"]
        self.assertEqual(normalize("https://github.com/a/b/pull/1", names), normalize("https://forge.test/a/b/pulls/1", names))
        self.assertNotEqual(normalize("https://github.com/a/b/pull/1", names), normalize("https://forge.test/a/b/pulls/2", names))
        self.assertEqual(normalize("evilgithub.com forge.test.evil", ["host"]), "evilgithub.com forge.test.evil")

    def test_time_sha_and_node_id_normalizers(self):
        text = '{"id":"PR_kwDO123", "n":7, "time":"2026-10-30T11:15:00+00:00", "sha":"abcdef1234567890"} 1m4s'
        actual = normalize(text, ["timestamps", "durations", "shas", "node_ids"])
        self.assertEqual(actual, '{"id":"<node-id>", "n":7, "time":"<timestamp>", "sha":"<sha>"} <duration>')

    def test_check_url_normalization_both_forms(self):
        for value in ("https://github.com/a/b/actions/runs/123/job/456", "/a/b/actions/runs/7/jobs/8"):
            self.assertEqual(normalize(value, ["check_details_urls"]), "<check-url>")
        self.assertEqual(normalize('{"detailsUrl":"https://forge.test/custom/123"}', ["check_details_urls"]), '{"detailsUrl":"<check-url>"}')

    def test_json_normalization_keeps_values_and_array_order(self):
        self.assertEqual(normalize('{ "b":2,"a":1 }\n', ["json"]), '{"a":1,"b":2}\n')
        self.assertNotEqual(normalize('[1,2]', ["json"]), normalize('[2,1]', ["json"]))
        self.assertEqual(normalize('invalid\n', ["json"]), 'invalid\n')
        with self.assertRaises(ValueError):
            validate_case(self.case() | {"normalize": ["json"]})

    def test_expected_failure_must_keep_failing(self):
        case = self.case() | {"xfail": "known output mismatch"}
        actual = {"exit": 0, "stdout": "wrong\n", "stderr": ""}
        self.assertEqual(runner.classify(case, actual)[0], "xfail")
        del case["xfail"]
        self.assertEqual(runner.classify(case, actual)[0], "unexpected")

    def test_fixed_expected_failure_demands_removal(self):
        case = self.case() | {"xfail": "known output mismatch"}
        self.assertEqual(runner.classify(case, case["oracle"]), ("unexpected", "remove xfail from probe"))

    def test_exit_and_both_streams_matter(self):
        case = self.case()
        self.assertEqual(runner.classify(case, case["oracle"])[0], "pass")
        for key, value in (("exit", 1), ("stdout", "wrong\n"), ("stderr", "oops\n")):
            actual = copy.deepcopy(case["oracle"])
            actual[key] = value
            self.assertEqual(runner.classify(case, actual)[0], "unexpected")

    def test_real_gh_delegation_cannot_satisfy_xfail(self):
        cache = ROOT / "target/contract"
        # Linux's checkout is read-only during the Linux gate.
        parent = cache if os.access(cache, os.W_OK) else None
        with tempfile.TemporaryDirectory(dir=parent) as temporary:
            directory = Path(temporary)
            seed_home = directory / "seed-home"
            seed_home.mkdir()
            binary = directory / "bad-shim"
            binary.write_text('#!/bin/sh\nshift\nexec gh "$@"\n')
            binary.chmod(0o755)
            case = self.case() | {"xfail": "must not hide delegation"}
            cases_dir = directory / "cases"
            cases_dir.mkdir()
            (cases_dir / "probe.json").write_text(json.dumps(case))
            env = {"GFJS_TOKEN": "disposable-contract-test-token", "GFJS_SEED_HOME": str(seed_home),
                   "GFJS_WORK_DIR": str(directory), "GFJS_SHIM_BINARY": str(binary), "GFJS_CASES_DIR": str(cases_dir)}
            output = io.StringIO()
            with patch.dict(os.environ, env), contextlib.redirect_stdout(output):
                self.assertEqual(runner.main(), 1)
            self.assertIn("shim invoked the real-gh decoy", output.getvalue())
            self.assertIn("pass 0, xfail 0, unexpected 1", output.getvalue())


if __name__ == "__main__":
    unittest.main()
