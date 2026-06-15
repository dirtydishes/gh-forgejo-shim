from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.bootstrap import format_bootstrap, run_bootstrap
from gh_forgejo_shim.config import load_config
from gh_forgejo_shim.shim import is_managed_shim


class BootstrapTests(unittest.TestCase):
    def test_bootstrap_allowlists_repo_and_installs_shim(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self._git_repo(root / "repo")
            bin_dir = root / "bin"
            config_path = root / "config.toml"
            env = {
                "PATH": str(bin_dir),
                "FJ_SHIM_TOKEN": "secret",
                "HOME": str(root),
            }

            result = run_bootstrap(
                cwd=str(repo),
                env=env,
                bin_dir=bin_dir,
                config_path=config_path,
                home=root,
            )

            self.assertTrue(is_managed_shim(bin_dir / "gh"))
            self.assertEqual(load_config(config_path, env={}).hosts, ("git.example.com",))
            self.assertTrue(self._check(result, "repository").ok)
            self.assertTrue(self._check(result, "shim").ok)
            self.assertTrue(self._check(result, "PATH").ok)
            self.assertTrue(self._check(result, "auth").ok)

    def test_bootstrap_prints_exact_repairs_for_repo_shape_gaps(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self._git_repo(
                root / "repo",
                set_origin_head=False,
                set_upstream=False,
                set_codex_upstream=False,
            )
            bin_dir = root / "bin"

            result = run_bootstrap(
                cwd=str(repo),
                env={"PATH": "", "HOME": str(root)},
                bin_dir=bin_dir,
                config_path=root / "config.toml",
                home=root,
            )

            output = format_bootstrap(result)

        self.assertIn('export PATH="', output)
        self.assertIn("gh-forgejo-shim auth login git.example.com", output)
        self.assertIn("gh-forgejo-shim auth import git.example.com", output)
        self.assertIn("git fetch origin", output)
        self.assertIn("git remote set-head origin -a", output)
        self.assertIn("git branch --set-upstream-to=origin/feature feature", output)
        self.assertIn("git remote add upstream https://git.example.com/owner/repo.git", output)

    def test_bootstrap_does_not_allowlist_github_repo(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self._git_repo(root / "repo", origin_url="https://github.com/owner/repo.git")
            config_path = root / "config.toml"

            result = run_bootstrap(
                cwd=str(repo),
                env={"PATH": "", "HOME": str(root)},
                bin_dir=root / "bin",
                config_path=config_path,
                home=root,
            )

            output = format_bootstrap(result)

        self.assertEqual(load_config(config_path, env={}).hosts, ())
        self.assertIn("leaving github.com out of the Forgejo allowlist", output)
        self.assertIn("Forgejo auth is not needed", output)

    def test_bootstrap_suggests_origin_when_remote_is_missing(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self._git_repo(root / "repo", add_origin=False, add_forgejo_remote=True)
            result = run_bootstrap(
                cwd=str(repo),
                env={"PATH": "", "FJ_SHIM_TOKEN": "secret", "HOME": str(root)},
                bin_dir=root / "bin",
                config_path=root / "config.toml",
                home=root,
            )

            output = format_bootstrap(result)

        self.assertIn("git remote add origin https://git.example.com/owner/repo.git", output)

    def _git_repo(
        self,
        path: Path,
        *,
        add_origin: bool = True,
        add_forgejo_remote: bool = False,
        set_origin_head: bool = True,
        set_upstream: bool = True,
        set_codex_upstream: bool = True,
        origin_url: str = "https://git.example.com/owner/repo.git",
    ) -> Path:
        path.mkdir(parents=True)
        self._git(path, "init", "-b", "feature")
        self._git(path, "config", "user.email", "test@example.com")
        self._git(path, "config", "user.name", "Test User")
        (path / "README.md").write_text("test\n", encoding="utf-8")
        self._git(path, "add", "README.md")
        self._git(path, "commit", "-m", "initial")

        if add_origin:
            self._git(path, "remote", "add", "origin", origin_url)
            self._git(path, "update-ref", "refs/remotes/origin/feature", "HEAD")
            if set_origin_head:
                self._git(path, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main")
            if set_upstream:
                self._git(path, "branch", "--set-upstream-to=origin/feature", "feature")
            if set_codex_upstream:
                self._git(path, "remote", "add", "upstream", origin_url)
        if add_forgejo_remote:
            self._git(path, "remote", "add", "forgejo", "https://git.example.com/owner/repo.git")
        return path

    def _git(self, cwd: Path, *args: str) -> None:
        subprocess.run(["git", *args], cwd=cwd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def _check(self, result, name):  # type: ignore[no-untyped-def]
        for check in result.checks:
            if check.name == name:
                return check
        raise AssertionError(name)


if __name__ == "__main__":
    unittest.main()
