from __future__ import annotations

import io
import json
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.config import Config
from gh_forgejo_shim.forgejo import ForgejoClient, RepoRef
from gh_forgejo_shim.routing import decide_route, run_forgejo_pr


class FakeClient(ForgejoClient):
    def __init__(self, pulls: list[dict] | None = None) -> None:
        super().__init__(None)
        self.pulls = pulls or []
        self.created: dict | None = None

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


class RoutingTests(unittest.TestCase):
    def test_delegates_non_pr_command(self) -> None:
        decision = decide_route(
            ["issue", "list"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "delegate")

    def test_delegates_unsupported_pr_command(self) -> None:
        decision = decide_route(
            ["pr", "list", "-R", "git.example.com/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "delegate")

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

    def _git(self, cwd: Path, *args: str) -> None:
        import subprocess

        subprocess.run(["git", *args], cwd=cwd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    unittest.main()
