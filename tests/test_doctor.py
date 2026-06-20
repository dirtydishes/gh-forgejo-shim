from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.config import Config, PathsConfig
from gh_forgejo_shim.doctor import run_checks
from gh_forgejo_shim.shim import install_shim


class DoctorTests(unittest.TestCase):
    def test_reports_missing_gh(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": ""},
                home=Path(tmp),
                fallback_dirs=(),
            )
        self.assertFalse(self._check(checks, "real gh").ok)

    def test_reports_missing_fj(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            gh = self._fake_executable(Path(tmp), "gh")
            checks = run_checks(
                config=Config(hosts=("git.example.com",), paths=PathsConfig(gh=str(gh))),
                env={"PATH": ""},
                home=Path(tmp),
                fallback_dirs=(),
            )
        self.assertFalse(self._check(checks, "fj").ok)

    def test_reports_missing_hosts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            checks = run_checks(config=Config(hosts=()), env={"PATH": ""}, home=Path(tmp))
        self.assertFalse(self._check(checks, "forgejo hosts").ok)

    def test_reports_missing_auth(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": ""},
                home=Path(tmp),
            )
        self.assertFalse(self._check(checks, "auth token").ok)

    def test_accepts_token_env(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": "", "FJ_SHIM_TOKEN": "secret"},
                home=Path(tmp),
            )
        self.assertTrue(self._check(checks, "auth token").ok)

    def test_reports_current_repo_host_that_is_not_allowlisted(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self._git_repo(root / "repo", "https://new.example.com/owner/repo.git")

            checks = run_checks(
                config=Config(hosts=("old.example.com",)),
                env={"PATH": "", "FJ_SHIM_TOKEN": "secret"},
                home=root,
                cwd=str(repo),
                check_current_repo=True,
            )

        check = self._check(checks, "current repo host")
        self.assertFalse(check.ok)
        self.assertIn("new.example.com is not allowlisted", check.detail)
        self.assertIn("gh-forgejo-shim config add-host new.example.com", check.detail)

    def test_accepts_current_repo_host_when_allowlisted(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self._git_repo(root / "repo", "https://git.example.com/owner/repo.git")

            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": "", "FJ_SHIM_TOKEN": "secret"},
                home=root,
                cwd=str(repo),
                check_current_repo=True,
            )

        check = self._check(checks, "current repo host")
        self.assertTrue(check.ok)
        self.assertIn("git.example.com is allowlisted", check.detail)

    def test_current_github_repo_is_ok_for_delegation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self._git_repo(root / "repo", "https://github.com/owner/repo.git")

            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": "", "FJ_SHIM_TOKEN": "secret"},
                home=root,
                cwd=str(repo),
                check_current_repo=True,
            )

        check = self._check(checks, "current repo host")
        self.assertTrue(check.ok)
        self.assertIn("will delegate to the real GitHub CLI", check.detail)

    def test_current_repo_reports_missing_codex_upstream_remote(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self._git_repo(root / "repo", "https://git.example.com/owner/repo.git")

            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": "", "FJ_SHIM_TOKEN": "secret"},
                home=root,
                cwd=str(repo),
                check_current_repo=True,
            )

        check = self._check(checks, "Codex upstream remote")
        self.assertFalse(check.ok)
        self.assertIn("remote.upstream.url is missing", check.detail)

    def test_current_repo_accepts_codex_upstream_remote(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = self._git_repo(root / "repo", "https://git.example.com/owner/repo.git")
            self._git(repo, "remote", "add", "upstream", "https://git.example.com/owner/repo.git")

            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": "", "FJ_SHIM_TOKEN": "secret"},
                home=root,
                cwd=str(repo),
                check_current_repo=True,
            )

        check = self._check(checks, "Codex upstream remote")
        self.assertTrue(check.ok)
        self.assertIn("remote.upstream.url points at", check.detail)

    def test_path_ordering_allows_unrelated_dirs_before_shim(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bin_dir = root / "bin"
            other = root / "other"
            other.mkdir()
            install_shim(bin_dir=bin_dir)

            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": os.pathsep.join([str(other), str(bin_dir)])},
                bin_dir=bin_dir,
                home=root,
            )
            self.assertTrue(self._check(checks, "shim path").ok)

            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": os.pathsep.join([str(bin_dir), str(other)])},
                bin_dir=bin_dir,
                home=root,
            )
            self.assertTrue(self._check(checks, "shim path").ok)

    def test_path_ordering_warns_when_another_gh_wins(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bin_dir = root / "bin"
            other = root / "other"
            other.mkdir()
            self._fake_executable(other, "gh")
            install_shim(bin_dir=bin_dir)

            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": os.pathsep.join([str(other), str(bin_dir)])},
                bin_dir=bin_dir,
                home=root,
            )
            self.assertFalse(self._check(checks, "shim path").ok)

    def test_path_ordering_warns_for_unmanaged_gh_at_shim_path(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            self._fake_executable(bin_dir, "gh")

            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": str(bin_dir)},
                bin_dir=bin_dir,
                home=root,
            )
            self.assertFalse(self._check(checks, "shim path").ok)

    def test_macos_gui_path_check_accepts_shim_dir(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bin_dir = root / "bin"
            install_shim(bin_dir=bin_dir)

            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": str(bin_dir)},
                bin_dir=bin_dir,
                home=root,
                launchd_path=os.pathsep.join(["/usr/bin", str(bin_dir)]),
                check_gui_path=True,
            )

        self.assertTrue(self._check(checks, "macOS gui PATH").ok)

    def test_macos_gui_path_check_warns_when_shim_dir_missing(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bin_dir = root / "bin"
            install_shim(bin_dir=bin_dir)

            checks = run_checks(
                config=Config(hosts=("git.example.com",)),
                env={"PATH": str(bin_dir)},
                bin_dir=bin_dir,
                home=root,
                launchd_path="/usr/bin:/bin",
                check_gui_path=True,
            )

        self.assertFalse(self._check(checks, "macOS gui PATH").ok)

    def _fake_executable(self, root: Path, name: str) -> Path:
        path = root / name
        path.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        path.chmod(0o755)
        return path

    def _git_repo(self, path: Path, origin: str) -> Path:
        path.mkdir(parents=True)
        self._git(path, "init", "-b", "feature")
        self._git(path, "remote", "add", "origin", origin)
        return path

    def _git(self, cwd: Path, *args: str) -> None:
        subprocess.run(["git", *args], cwd=cwd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def _check(self, checks, name):  # type: ignore[no-untyped-def]
        for check in checks:
            if check.name == name:
                return check
        raise AssertionError(name)


if __name__ == "__main__":
    unittest.main()
