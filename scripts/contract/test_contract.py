"""Tests for the boundaries that must not silently weaken the contract."""

import contextlib
import copy
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import capture
import runner
from common import (DECOY_PROBES, LOCAL_PROBES, REPO, ROOT, launch_list, launched, load_cases, normalize,
                    oracle_argv, validate_case)


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

    def test_local_cases_cannot_be_captured_or_disguised_as_oracle_cases(self):
        for argv in LOCAL_PROBES:
            case = self.case(argv)
            with self.subTest(argv=argv):
                with self.assertRaises(ValueError):
                    validate_case(case)
                case["kind"] = "shim-local"
                case["expected"] = case.pop("oracle")
                validate_case(case)
                with self.assertRaisesRegex(ValueError, "only oracle cases are captured"):
                    oracle_argv(case)
                with self.assertRaises(ValueError):
                    validate_case(case | {"argv": ["pr", "create"]})

    def test_selecting_only_local_cases_does_not_read_auth_or_download(self):
        case = self.case(LOCAL_PROBES[0]) | {"kind": "shim-local"}
        case["expected"] = case.pop("oracle")
        with patch.object(capture, "load_cases", return_value=[(Path("probe.json"), case)]), \
                patch("sys.argv", ["capture.py", "--case", "probe"]), \
                patch.object(capture, "official_gh") as download, \
                patch.object(capture.subprocess, "run") as run:
            capture.main()
            download.assert_not_called()
            run.assert_not_called()

    def test_error_probe_exceptions_are_exact(self):
        for argv in (["pr", "view", "1", "--gfjs-unknown", "--web"],
                     ["pr", "gfjs-unknown", "--repo", "evil.test/other/repo"],
                     ["pr", "view", "1", "--template", "--web"]):
            with self.subTest(argv=argv), self.assertRaises(ValueError):
                validate_case(self.case(argv))

    def test_local_expected_output_compares_both_streams_and_exit(self):
        case = self.case(LOCAL_PROBES[0]) | {"kind": "shim-local"}
        case["expected"] = case.pop("oracle")
        self.assertEqual(runner.classify(case, case["expected"]), ("pass", ""))
        for key, value in (("exit", 97), ("stdout", "wrong"), ("stderr", "wrong")):
            self.assertEqual(runner.classify(case, case["expected"] | {key: value})[0], "unexpected")

    def test_twin_cases_stay_on_their_own_branch(self):
        good = {"id": "twin-x", "kind": "twin", "repo": REPO, "setup": {"branch": "twin/x", "pr": True},
                "argv": ["pr", "edit", "{pr}", "--title", "t"], "expect": {"exit": 0, "stdout": "", "stderr": ""},
                "state": [{"path": "pulls/{pr}", "fields": {"title": "t"}}], "normalize": []}
        validate_case(good)
        self.assertTrue(launched(good, ["pr edit"]))
        with self.assertRaisesRegex(ValueError, "only oracle cases are captured"):
            oracle_argv(good)
        for change, message in [
            ({"argv": ["pr", "checkout", "{pr}"]}, "twin allowlist"),
            # A merge goes into its own base, never main.
            ({"argv": ["pr", "merge", "{pr}", "--merge"]}, "setup.base"),
            ({"argv": ["pr", "merge", "{pr}", "--merge", "--match-head-commit", "main"],
              "setup": {"branch": "twin/x", "pr": True, "base": True}}, "full sha"),
            ({"argv": ["pr", "merge", "{pr}", "--merge", "--delete-branch"],
              "setup": {"branch": "twin/x", "pr": True, "base": True}}, "flag is not allowed"),
            ({"setup": {"branch": "twin/x", "pr": True, "protect": ["c"]}}, "own base"),
            ({"setup": {"branch": "twin/x", "pr": True, "status": {"context": "c"}}}, "context and a state"),
            ({"argv": ["pr", "edit", "7", "--title", "t"]}, "selector"),
            ({"argv": ["pr", "edit", "{pr}", "-R", "github.com/o/r"]}, "flag is not allowed"),
            ({"argv": ["pr", "create", "--head", "main", "--title", "t"]}, "own branch"),
            ({"setup": {"branch": "main"}}, "twin/<name>"),
            ({"shim_config": {"hosts": ["github.com"]}}, "wip_prefixes"),
            ({"state": [{"path": "pulls/1", "fields": {}}]}, "own pull request"),
            ({"oracle": {}}, "own expect"),
            ({"xfail": "x"}, "own expect"),
            ({"id": "create-x"}, "start with twin-"),
            # No selector: gh would pick the checkout's branch, scenario PR 1.
            ({"argv": ["pr", "edit", "--title", "t"]}, "one selector"),
            ({"argv": ["pr", "comment", "--body", "b"]}, "one selector"),
            ({"argv": ["pr", "create", "--title", "t"]}, "one --head"),
            ({"argv": ["pr", "create", "--head", "{branch}", "--base", "s/pass"]}, "base must be main"),
            ({"setup": {"branch": "twin/x"}}, "needs setup.pr"),
            ({"expect": {"exit": 0, "stdout": "(", "stderr": ""}}, "isn't a pattern"),
            ({"expect": {"exit": "0", "stdout": "", "stderr": ""}}, "integer"),
            ({"state": [{"path": "pulls/{pr}", "any": {"title": "t"}}]}, "comments or timeline list"),
            ({"state": [{"path": "pulls/{pr}", "none": {"title": "t"}}]}, "comments or timeline list"),
            ({"setup": {"branch": "twin/x-base", "pr": True}}, "end in -base"),
            ({"setup": {"branch": "twin/x", "pr": True, "base": "no"}}, "true or false"),
            ({"setup": {"branch": "twin/x", "draft": True}, "argv": ["pr", "edit", "{branch}", "--title", "t"]}, "need setup.pr"),
            # C4: a run case reads only its own run; an issue case comments only on its own issue.
            ({"argv": ["run", "list", "--json", "status"],
              "setup": {"branch": "twin/run-x", "workflow": True}}, "--branch {branch}"),
            ({"argv": ["run", "list", "--branch", "main"],
              "setup": {"branch": "twin/run-x", "workflow": True}}, "the case's own"),
            # A value that only reads --branch isn't the flag (o1#6).
            ({"argv": ["run", "list", "--jq", "--branch"],
              "setup": {"branch": "twin/run-x", "workflow": True}}, "the case's own"),
            ({"argv": ["run", "view", "{run}"], "setup": {"branch": "twin/run-x"}}, "setup.workflow"),
            ({"argv": ["run", "view", "{pr}"], "setup": {"branch": "twin/run-x", "pr": True, "workflow": True}},
             "selector"),
            ({"argv": ["run", "view", "{run}"], "setup": {"branch": "twin/x", "workflow": True}}, "twin/run-<name>"),
            ({"argv": ["run", "view", "{run}"], "setup": {"branch": "twin/run-x", "workflow": "branch"}},
             "outside twin/run-"),
            ({"argv": ["run", "view", "{run}"], "setup": {"branch": "twin/x", "workflow": "main"}}, "true or false"),
            ({"argv": ["run", "view", "{run}", "--log"], "setup": {"branch": "twin/run-x", "workflow": True}},
             "flag is not allowed"),
            ({"argv": ["issue", "comment", "{issue}", "--body", "b"], "setup": {"branch": "twin/x"}}, "setup.issue"),
            ({"argv": ["issue", "comment", "{pr}", "--body", "b"], "setup": {"branch": "twin/x", "pr": True, "issue": True}},
             "selector"),
            ({"argv": ["issue", "comment", "{issue}", "--edit-last", "--body", "b"],
              "setup": {"branch": "twin/x", "issue": True}}, "flag is not allowed"),
            ({"argv": ["issue", "list", "--json", "number"], "setup": {"branch": "twin/x", "issue": True}},
             "select\\(.number"),
            ({"argv": ["issue", "list", "--jq", ".[0]"], "setup": {"branch": "twin/x", "issue": True}},
             "select\\(.number"),
            # Mentioning {issue} isn't selecting it: a count depends on other cases (o1#5).
            ({"argv": ["issue", "list", "--json", "number", "--jq", "[length, {issue}]"],
              "setup": {"branch": "twin/x", "issue": True}}, "select\\(.number"),
            ({"argv": ["issue", "view", "{issue}", "--json", "title"], "setup": {"branch": "twin/x", "issue": True},
              "expect": {"exit": 0, "stdout": "(?P<issue>[0-9]+)", "stderr": ""}}, "rebind"),
            ({"argv": ["issue", "view", "{issue}", "--json", "title"], "setup": {"branch": "twin/x", "issue": True},
              "expect": {"exit": 0, "stdout": "", "stderr": "(?P<issue>[0-9]+)"}}, "rebind"),
            ({"argv": ["issue", "view", "{issue}", "--json", "title"], "setup": {"branch": "twin/x", "label": True}},
             "needs setup.issue"),
            ({"argv": ["issue", "view", "{pr}"], "setup": {"branch": "twin/x", "pr": True, "issue": True}},
             "selector"),
            ({"argv": ["issue", "create", "--title", "t", "--body", "b"], "setup": {"branch": "twin/x"}},
             "naming {branch}"),
            ({"argv": ["issue", "create", "--title", "{branch}"], "setup": {"branch": "twin/x"}}, "a --body"),
            ({"argv": ["issue", "create", "--title", "{branch}", "--body", "b"],
              "setup": {"branch": "twin/x", "issue": True}}, "makes its own"),
            ({"argv": ["issue", "create", "--title", "{branch}", "--body", "b", "--label", "bug"],
              "setup": {"branch": "twin/x"}}, "flag is not allowed"),
        ]:
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, message):
                validate_case(good | change)

    def test_decoy_cases_are_exact_and_never_captured(self):
        case = {"id": "probe", "kind": "decoy", "repo": REPO, "argv": DECOY_PROBES[0], "normalize": []}
        validate_case(case)
        with self.assertRaisesRegex(ValueError, "decoy allowlist"):
            validate_case(case | {"argv": ["pr", "view", "1"]})
        with self.assertRaisesRegex(ValueError, "expect only the decoy"):
            validate_case(case | {"oracle": {"exit": 0, "stdout": "", "stderr": ""}})
        with self.assertRaisesRegex(ValueError, "only oracle cases are captured"):
            oracle_argv(case)
        self.assertEqual(runner.classify(case, {"exit": 97, "stdout": "", "stderr": ""}), ("pass", ""))

    def test_only_oracle_cases_on_the_launch_list_get_real_gh(self):
        commands = launch_list()
        self.assertEqual(commands[:2], ["pr view", "pr checks"])
        self.assertTrue(launched(self.case(["api", "user"]), ["api"]), "a one-word entry")
        self.assertFalse(launched(self.case(["api", "user"]), ["pr view"]))
        self.assertTrue(launched(self.case(["pr", "view", "1"]), commands))
        self.assertTrue(launched(self.case(["pr", "checks", "1"]), commands))
        self.assertFalse(launched(self.case(["repo", "list"]), commands))
        self.assertFalse(launched(self.case(["repo", "view"]), commands))
        local = self.case(["pr", "view", "1"]) | {"kind": "shim-local"}
        self.assertFalse(launched(local, commands))
        decoy = {"id": "probe", "kind": "decoy", "repo": REPO, "argv": DECOY_PROBES[0], "normalize": []}
        self.assertFalse(launched(decoy, commands))
        self.assertTrue(any(case.get("kind") == "decoy" for _, case in load_cases(ROOT / "contract/cases")),
                        "a committed case must prove an off-list command still reaches the decoy")

    def test_launch_list_gh_runs_real_gh_only_for_a_launch(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            real = directory / "real gh"
            real.write_text("#!/bin/sh\nprintf real\n")
            real.chmod(0o755)
            wrapper = directory / "gh"
            wrapper.write_text(runner.gh_script(real))
            wrapper.chmod(0o755)
            for config, launched_call in ((directory / "gfj-0123456789abcdef", True),
                                          (directory / "gh-config", False),
                                          (directory / "gfj-backup", False)):
                marker = directory / "marker"
                result = subprocess.run([str(wrapper), "pr", "view"], capture_output=True, text=True,
                                        env={"GH_CONFIG_DIR": str(config), "GFJS_DECOY_MARKER": str(marker)})
                self.assertEqual((result.returncode, result.stdout), (0, "real") if launched_call else (97, ""),
                                 config.name)
                self.assertEqual(marker.exists(), not launched_call, config.name)
                marker.unlink(missing_ok=True)

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

    def test_zero_duration_only_normalizes_the_checks_elapsed_column(self):
        for bucket in ("pass", "fail", "pending", "skipping"):
            row = f"0\t{bucket}\t0\thttps://forge.test/0\tdescription 0\n"
            self.assertEqual(normalize(row, ["durations"]),
                             f"0\t{bucket}\t<duration>\thttps://forge.test/0\tdescription 0\n")
        for text in ('{"number":0}\n', "0\n", "job\tunknown\t0\turl\t\n", "job\tpass\tbroken\turl\t\n"):
            self.assertEqual(normalize(text, ["durations"]), text)

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
