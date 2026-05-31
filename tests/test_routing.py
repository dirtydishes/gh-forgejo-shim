from __future__ import annotations

import io
import json
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.config import Config
from gh_forgejo_shim.forgejo import ForgejoClient, RepoRef
from gh_forgejo_shim.routing import decide_route, run_forgejo, run_forgejo_pr


class FakeClient(ForgejoClient):
    def __init__(self, pulls: list[dict] | None = None) -> None:
        super().__init__(None)
        self.pulls = pulls or []
        self.created: dict | None = None
        self.repo_view = {
            "name": "repo",
            "full_name": "owner/repo",
            "html_url": "https://git.example.com/owner/repo",
            "ssh_url": "git@git.example.com:owner/repo.git",
            "default_branch": "main",
            "private": False,
            "owner": {"login": "owner"},
        }

    def create_pull(self, repo: RepoRef, **kwargs):  # type: ignore[no-untyped-def]
        self.created = {"repo": repo, **kwargs}
        return {
            "number": 7,
            "title": kwargs["title"],
            "state": "open",
            "html_url": "https://git.example.com/owner/repo/pulls/7",
            "head": {"ref": kwargs["head"]},
            "base": {"ref": kwargs["base"]},
            "user": {"login": "alice"},
        }

    def list_pulls(self, repo: RepoRef, *, state: str = "open", head: str | None = None):  # type: ignore[no-untyped-def]
        if head is None:
            return self.pulls
        return [
            pull
            for pull in self.pulls
            if pull.get("head", {}).get("ref") == head
        ]

    def get_pull(self, repo: RepoRef, number: int):  # type: ignore[no-untyped-def]
        for pull in self.pulls:
            if pull.get("number") == number:
                return pull
        raise ValueError("not found")

    def get_repo(self, repo: RepoRef):  # type: ignore[no-untyped-def]
        return self.repo_view


class RoutingTests(unittest.TestCase):
    def test_delegates_non_pr_command(self) -> None:
        decision = decide_route(
            ["issue", "list"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "delegate")

    def test_routes_pr_list_for_allowlisted_host(self) -> None:
        decision = decide_route(
            ["pr", "list", "-R", "git.example.com/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "forgejo")

    def test_routes_repo_view_for_allowlisted_host(self) -> None:
        decision = decide_route(
            ["repo", "view", "-R", "git.example.com/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "forgejo")

    def test_delegates_non_allowlisted_host(self) -> None:
        decision = decide_route(
            ["pr", "view", "-R", "github.com/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "delegate")

    def test_routes_supported_pr_command_for_allowlisted_host(self) -> None:
        decision = decide_route(
            ["pr", "view", "-R", "git.example.com/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "forgejo")

    def test_create_calls_forgejo_client(self) -> None:
        client = FakeClient()
        out = io.StringIO()
        code = run_forgejo_pr(
            [
                "pr",
                "create",
                "-R",
                "git.example.com/owner/repo",
                "--title",
                "Ship it",
                "--body",
                "body",
                "--base",
                "main",
                "--head",
                "feature",
                "--draft",
                "--json",
                "number,title,url",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            client,
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertIsNotNone(client.created)
        assert client.created is not None
        self.assertEqual(client.created["title"], "Ship it")
        self.assertTrue(client.created["draft"])
        self.assertEqual(json.loads(out.getvalue()), {"number": 7, "title": "Ship it", "url": "https://git.example.com/owner/repo/pulls/7"})

    def test_pr_list_outputs_json_array(self) -> None:
        out = io.StringIO()
        code = run_forgejo_pr(
            [
                "pr",
                "list",
                "-R",
                "git.example.com/owner/repo",
                "--json",
                "number,title,url,headRefName",
                "--head",
                "feature",
                "--limit",
                "1",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(
                [
                    {
                        "number": 8,
                        "title": "Make Codex happy",
                        "state": "open",
                        "html_url": "https://git.example.com/owner/repo/pulls/8",
                        "head": {"ref": "feature"},
                        "base": {"ref": "main"},
                        "user": {"login": "alice"},
                    },
                    {
                        "number": 9,
                        "title": "Other branch",
                        "state": "open",
                        "html_url": "https://git.example.com/owner/repo/pulls/9",
                        "head": {"ref": "other"},
                        "base": {"ref": "main"},
                        "user": {"login": "alice"},
                    },
                ]
            ),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            [
                {
                    "headRefName": "feature",
                    "number": 8,
                    "title": "Make Codex happy",
                    "url": "https://git.example.com/owner/repo/pulls/8",
                }
            ],
        )

    def test_repo_view_outputs_github_shaped_json(self) -> None:
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
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            {
                "defaultBranchRef": {"name": "main"},
                "nameWithOwner": "owner/repo",
                "sshUrl": "git@git.example.com:owner/repo.git",
                "url": "https://git.example.com/owner/repo",
            },
        )

    def test_no_current_branch_view_returns_empty_json_success(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                ["pr", "view", "--json", "number,title"],
                RepoRef("git.example.com", "owner", "repo"),
                FakeClient([]),
                cwd=str(cwd),
                stdout=out,
                stderr=io.StringIO(),
            )
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().strip(), "{}")

    def test_no_current_branch_status_returns_empty_status_success(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                ["pr", "status", "--json", "number,title"],
                RepoRef("git.example.com", "owner", "repo"),
                FakeClient([]),
                cwd=str(cwd),
                stdout=out,
                stderr=io.StringIO(),
            )
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out.getvalue()), {"createdBy": [], "currentBranch": None, "needsReview": []})

    def test_current_branch_status_returns_github_status_envelope(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                ["pr", "status", "--json", "number,title,url,headRefName"],
                RepoRef("git.example.com", "owner", "repo"),
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
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            {
                "createdBy": [],
                "currentBranch": {
                    "headRefName": "feature",
                    "number": 12,
                    "title": "Ship it",
                    "url": "https://git.example.com/owner/repo/pulls/12",
                },
                "needsReview": [],
            },
        )

    def test_status_supports_simple_jq_field_selection(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                [
                    "pr",
                    "status",
                    "--json",
                    "number,title,url",
                    "--jq",
                    ".currentBranch.url",
                ],
                RepoRef("git.example.com", "owner", "repo"),
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
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().strip(), "https://git.example.com/owner/repo/pulls/12")

    def _git(self, cwd: Path, *args: str) -> None:
        import subprocess

        subprocess.run(["git", *args], cwd=cwd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    unittest.main()
