from __future__ import annotations

import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.forgejo import RepoRef
from gh_forgejo_shim.routing import run_forgejo, run_forgejo_pr, run_gh
from tests.test_routing import FakeClient


CONTRACT_PATH = (
    Path(__file__).resolve().parents[1]
    / "docs"
    / "implementation"
    / "rust-rewrite"
    / "contracts"
    / "compatibility-contract.v1.json"
)
REPO = RepoRef("git.example.com", "owner", "repo")


class CompatibilityContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.contract = json.loads(CONTRACT_PATH.read_text(encoding="utf-8"))
        cls.probes = {probe["id"]: probe for probe in cls.contract["golden_probes"]}

    def test_contract_has_required_codex_golden_probes(self) -> None:
        self.assertEqual(self.contract["schema"], "gh-forgejo-shim.compatibility-contract.v1")
        self.assertEqual(self.contract["bead"], "gh-forgejo-shim-3sl")
        self.assertEqual(
            {
                "gh_version_delegated",
                "gh_auth_status_text",
                "gh_auth_status_json_hosts",
                "gh_api_user_jq_login",
                "gh_repo_view_json_codex",
                "gh_pr_status_current_branch_json",
                "gh_pr_list_codex_board_json",
                "gh_pr_view_json",
                "gh_pr_checks_json",
                "git_current_branch_trace",
                "git_default_branch_trace",
                "git_upstream_trace",
            },
            set(self.probes),
        )

    def test_command_matrix_covers_management_routed_and_delegated_surfaces(self) -> None:
        ids = {entry["id"] for entry in self.contract["command_matrix"]}
        for required in {
            "management.install_shim",
            "management.uninstall_shim",
            "management.doctor",
            "management.bootstrap",
            "management.config_add_host",
            "management.auth_login",
            "management.trace_smoke",
            "management.version",
            "managed_gh.version",
            "managed_gh.auth_status",
            "managed_gh.api_user",
            "managed_gh.repo_view",
            "managed_gh.pr_list",
            "managed_gh.pr_view",
            "managed_gh.pr_checks",
            "managed_gh.issue_list",
            "managed_gh.unsupported_or_github",
        }:
            self.assertIn(required, ids)

    def test_auth_status_text_fixture_matches_python_runtime(self) -> None:
        out = io.StringIO()
        err = io.StringIO()

        code = run_gh(
            ["auth", "status"],
            env={
                "FJ_SHIM_HOSTS": "git.example.com",
                "FJ_SHIM_REAL_GH": "/missing/gh",
                "FJ_SHIM_TOKEN": "secret-token",
                "GH_HOST": "git.example.com",
                "PATH": "",
            },
            stdout=out,
            stderr=err,
        )

        expected = self.probes["gh_auth_status_text"]["expected"]
        self.assertEqual(code, expected["exit_code"])
        self.assertEqual(out.getvalue(), expected["stdout"])
        self.assertEqual(err.getvalue(), expected["stderr"])

    def test_api_user_fixture_matches_python_runtime(self) -> None:
        out = io.StringIO()
        err = io.StringIO()

        code = run_gh(
            ["api", "user", "--jq", ".login"],
            env={
                "FJ_SHIM_HOSTS": "git.example.com",
                "FJ_SHIM_REAL_GH": "/missing/gh",
                "FJ_SHIM_TOKEN": "secret-token",
                "GH_HOST": "git.example.com",
                "PATH": "",
            },
            stdout=out,
            stderr=err,
            client_factory=lambda _token: FakeClient(),
        )

        expected = self.probes["gh_api_user_jq_login"]["expected"]
        self.assertEqual(code, expected["exit_code"])
        self.assertEqual(out.getvalue(), expected["stdout"])
        self.assertEqual(err.getvalue(), expected["stderr"])

    def test_repo_view_fixture_matches_python_runtime(self) -> None:
        out = io.StringIO()
        code = run_forgejo(
            [
                "repo",
                "view",
                "-R",
                "git.example.com/owner/repo",
                "--json",
                "nameWithOwner,url,defaultBranchRef,sshUrl",
            ],
            REPO,
            FakeClient(),
            stdout=out,
            stderr=io.StringIO(),
        )

        expected = self.probes["gh_repo_view_json_codex"]["expected"]
        self.assertEqual(code, expected["exit_code"])
        self.assertEqual(json.loads(out.getvalue()), expected["stdout_json"])

    def test_pr_status_fixture_matches_python_runtime(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                ["pr", "status", "--json", "number,title,url,headRefName"],
                REPO,
                FakeClient(
                    [
                        {
                            "number": 12,
                            "title": "Ship it",
                            "state": "open",
                            "html_url": "https://git.example.com/owner/repo/pulls/12",
                            "head": {"ref": "feature"},
                            "base": {"ref": "main"},
                            "user": {"login": "alice"},
                        }
                    ]
                ),
                cwd=str(cwd),
                stdout=out,
                stderr=io.StringIO(),
            )

        expected = self.probes["gh_pr_status_current_branch_json"]["expected"]
        self.assertEqual(code, expected["exit_code"])
        self.assertEqual(json.loads(out.getvalue()), expected["stdout_json"])

    def test_pr_list_fixture_matches_python_runtime(self) -> None:
        out = io.StringIO()
        code = run_forgejo_pr(
            [
                "pr",
                "list",
                "-R",
                "git.example.com/owner/repo",
                "--state",
                "open",
                "--limit",
                "10",
                "--json",
                "additions,baseRefName,createdAt,deletions,headRefName,isDraft,number,state,title,updatedAt,url,mergeStateStatus,mergeable,statusCheckRollup",
                "--author",
                "@me",
            ],
            REPO,
            FakeClient(
                [
                    {
                        "number": 8,
                        "title": "Make Codex happy",
                        "state": "open",
                        "html_url": "https://git.example.com/owner/repo/pulls/8",
                        "head": {"ref": "feature", "sha": "abc123"},
                        "base": {"ref": "main"},
                        "user": {"login": "alice"},
                        "created_at": "2026-05-31T00:00:00Z",
                        "updated_at": "2026-05-31T01:00:00Z",
                        "mergeable": False,
                    }
                ],
                statuses=[
                    {
                        "context": "ci/test",
                        "description": "tests passed",
                        "state": "success",
                        "target_url": "https://git.example.com/owner/repo/actions/runs/1",
                        "created_at": "2026-06-01T00:00:00Z",
                        "updated_at": "2026-06-01T00:02:00Z",
                    }
                ],
            ),
            stdout=out,
            stderr=io.StringIO(),
        )

        expected = self.probes["gh_pr_list_codex_board_json"]["expected"]
        self.assertEqual(code, expected["exit_code"])
        self.assertEqual(json.loads(out.getvalue()), expected["stdout_json"])

    def test_pr_view_fixture_matches_python_runtime(self) -> None:
        out = io.StringIO()
        code = run_forgejo_pr(
            ["pr", "view", "12", "--json", "number,title,url,headRefName"],
            REPO,
            FakeClient(
                [
                    {
                        "number": 12,
                        "title": "Ship it",
                        "state": "open",
                        "html_url": "https://git.example.com/owner/repo/pulls/12",
                        "head": {"ref": "feature"},
                    }
                ]
            ),
            stdout=out,
            stderr=io.StringIO(),
        )

        expected = self.probes["gh_pr_view_json"]["expected"]
        self.assertEqual(code, expected["exit_code"])
        self.assertEqual(json.loads(out.getvalue()), expected["stdout_json"])

    def test_pr_checks_fixture_matches_python_runtime(self) -> None:
        out = io.StringIO()
        code = run_forgejo_pr(
            ["pr", "checks", "8", "--json", "bucket,description,link,name,workflow"],
            REPO,
            FakeClient(
                pulls=[{"number": 8, "head": {"ref": "feature", "sha": "abc123"}}],
                statuses=[
                    {
                        "context": "ci/test",
                        "description": "tests passed",
                        "state": "success",
                        "target_url": "https://git.example.com/owner/repo/actions/runs/1",
                        "created_at": "2026-06-01T00:00:00Z",
                        "updated_at": "2026-06-01T00:02:00Z",
                    },
                    {
                        "context": "lint",
                        "description": "lint is running",
                        "state": "pending",
                        "created_at": "2026-06-01T00:01:00Z",
                    },
                ],
            ),
            stdout=out,
            stderr=io.StringIO(),
        )

        expected = self.probes["gh_pr_checks_json"]["expected"]
        self.assertEqual(code, expected["exit_code"])
        self.assertEqual(json.loads(out.getvalue()), expected["stdout_json"])

    def test_trace_redaction_contract_covers_gh_and_git_recorder_schemas(self) -> None:
        trace = self.contract["trace_contract"]
        self.assertEqual(trace["gh_record"]["redaction_replacement"], "<redacted>")
        self.assertEqual(
            trace["gh_record"]["auth_token_stdout_excerpt"],
            "[redacted: gh auth token output]",
        )
        self.assertEqual(
            trace["git_recorder_record"]["schema"],
            "gh-forgejo-shim.git-recorder.v1",
        )
        self.assertEqual(trace["git_recorder_record"]["redaction_replacement"], "[REDACTED]")

    def _git(self, cwd: Path, *args: str) -> None:
        subprocess.run(["git", *args], cwd=cwd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    unittest.main()
