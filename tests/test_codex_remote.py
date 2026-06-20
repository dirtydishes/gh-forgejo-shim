from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.codex_remote import check_codex_upstream_remote, repair_codex_upstream_remote


class CodexRemoteTests(unittest.TestCase):
    def test_check_suggests_upstream_alias_when_origin_exists(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self._git_repo(Path(tmp) / "repo")

            check = check_codex_upstream_remote(cwd=str(repo))

        self.assertFalse(check.ok)
        self.assertIn("remote.upstream.url is missing", check.detail)
        self.assertEqual(
            check.repair_commands,
            ("git remote add upstream https://git.example.com/owner/repo.git",),
        )

    def test_repair_adds_upstream_alias_from_origin(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self._git_repo(Path(tmp) / "repo")

            result = repair_codex_upstream_remote(cwd=str(repo))
            upstream = self._git_output(repo, "config", "--get", "remote.upstream.url")

        self.assertTrue(result.ok)
        self.assertTrue(result.changed)
        self.assertEqual(upstream, "https://git.example.com/owner/repo.git")

    def test_repair_copies_origin_push_url_when_it_differs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self._git_repo(Path(tmp) / "repo")
            self._git(repo, "remote", "set-url", "--push", "origin", "ssh://git@git.example.com/owner/repo.git")

            result = repair_codex_upstream_remote(cwd=str(repo))
            upstream = self._git_output(repo, "config", "--get", "remote.upstream.url")
            upstream_push = self._git_output(repo, "remote", "get-url", "--push", "upstream")

        self.assertTrue(result.ok)
        self.assertTrue(result.changed)
        self.assertEqual(upstream, "https://git.example.com/owner/repo.git")
        self.assertEqual(upstream_push, "ssh://git@git.example.com/owner/repo.git")

    def test_repair_leaves_existing_upstream_alone(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = self._git_repo(Path(tmp) / "repo")
            self._git(repo, "remote", "add", "upstream", "https://github.com/original/repo.git")

            result = repair_codex_upstream_remote(cwd=str(repo))
            upstream = self._git_output(repo, "config", "--get", "remote.upstream.url")

        self.assertTrue(result.ok)
        self.assertFalse(result.changed)
        self.assertEqual(upstream, "https://github.com/original/repo.git")

    def test_repair_refuses_repo_without_origin(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp) / "repo"
            repo.mkdir()
            self._git(repo, "init", "-b", "main")

            result = repair_codex_upstream_remote(cwd=str(repo))

        self.assertFalse(result.ok)
        self.assertFalse(result.changed)
        self.assertIn("origin remote is missing", result.detail)

    def _git_repo(self, path: Path) -> Path:
        path.mkdir(parents=True)
        self._git(path, "init", "-b", "main")
        self._git(path, "remote", "add", "origin", "https://git.example.com/owner/repo.git")
        return path

    def _git(self, cwd: Path, *args: str) -> None:
        subprocess.run(["git", *args], cwd=cwd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def _git_output(self, cwd: Path, *args: str) -> str:
        completed = subprocess.run(
            ["git", *args],
            cwd=cwd,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        return completed.stdout.strip()


if __name__ == "__main__":
    unittest.main()
