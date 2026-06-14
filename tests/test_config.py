from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.config import add_host, load_config, remove_host


class ConfigTests(unittest.TestCase):
    def test_no_default_hosts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            config = load_config(Path(tmp) / "config.toml", env={})
        self.assertEqual(config.hosts, ())

    def test_env_hosts_replace_config_hosts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "config.toml"
            path.write_text('hosts = ["git.example.com"]\n', encoding="utf-8")
            config = load_config(path, env={"FJ_SHIM_HOSTS": "code.example.org, git.local"})
        self.assertEqual(config.hosts, ("code.example.org", "git.local"))

    def test_github_hosts_are_ignored_even_if_configured(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "config.toml"
            path.write_text('hosts = ["github.com", "git.example.com"]\n', encoding="utf-8")
            config = load_config(path, env={})
        self.assertEqual(config.hosts, ("git.example.com",))
        self.assertFalse(config.is_forgejo_host("github.com"))

    def test_add_and_remove_host(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "config.toml"
            add_host("https://Git.Example.com/path", path)
            config = load_config(path, env={})
            self.assertEqual(config.hosts, ("git.example.com",))

            remove_host("git.example.com", path)
            config = load_config(path, env={})
            self.assertEqual(config.hosts, ())

    def test_add_host_does_not_persist_github(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "config.toml"
            add_host("github.com", path)
            config = load_config(path, env={})
        self.assertEqual(config.hosts, ())

    def test_env_paths_override_config(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "config.toml"
            path.write_text('[paths]\ngh = "/bin/gh"\nfj = "/bin/fj"\n', encoding="utf-8")
            config = load_config(
                path,
                env={
                    "FJ_SHIM_REAL_GH": "/custom/gh",
                    "FJ_SHIM_REAL_FJ": "/custom/fj",
                },
            )
        self.assertEqual(config.paths.gh, "/custom/gh")
        self.assertEqual(config.paths.fj, "/custom/fj")


if __name__ == "__main__":
    unittest.main()
