from __future__ import annotations

import json
import stat
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.auth import auth_file_path, delete_stored_token, discover_fj_token, token_from_env, write_stored_token


class AuthTests(unittest.TestCase):
    def test_env_token_precedence(self) -> None:
        self.assertEqual(
            token_from_env(
                {
                    "FJ_SHIM_TOKEN": "shim",
                    "FORGEJO_TOKEN": "forgejo",
                    "GITEA_TOKEN": "gitea",
                    "FJ_TOKEN": "fj",
                }
            ),
            "shim",
        )

    def test_discovers_macos_fj_keys_json_token_for_host(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            path = home / "Library" / "Application Support" / "Cyborus.forgejo-cli"
            path.mkdir(parents=True)
            (path / "keys.json").write_text(
                json.dumps(
                    {
                        "hosts": {
                            "git.deltaisland.io": {
                                "type": "token",
                                "name": "dirtydishes",
                                "token": "secret-token",
                            },
                            "git.example.com": {
                                "type": "token",
                                "name": "example",
                                "token": "other-token",
                            },
                        }
                    }
                ),
                encoding="utf-8",
            )

            token = discover_fj_token("git.deltaisland.io", env={}, home=home)

        self.assertEqual(token, "secret-token")

    def test_env_token_wins_over_macos_fj_config(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            path = home / "Library" / "Application Support" / "Cyborus.forgejo-cli"
            path.mkdir(parents=True)
            (path / "keys.json").write_text(
                json.dumps({"hosts": {"git.deltaisland.io": {"token": "file-token"}}}),
                encoding="utf-8",
            )

            token = discover_fj_token(
                "git.deltaisland.io",
                env={"FJ_SHIM_TOKEN": "env-token"},
                home=home,
            )

        self.assertEqual(token, "env-token")

    def test_host_specific_discovery_ignores_unrelated_file_tokens(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            path = home / "Library" / "Application Support" / "Cyborus.forgejo-cli"
            path.mkdir(parents=True)
            (path / "keys.json").write_text(
                json.dumps({"hosts": {"git.other.test": {"token": "other-token"}}}),
                encoding="utf-8",
            )

            token = discover_fj_token("git.example.com", env={}, home=home)

        self.assertIsNone(token)

    def test_stores_and_discovers_shim_auth_file_token(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            storage = write_stored_token("https://Git.Example.com/path", "stored-token", home=home, platform="linux")
            token = discover_fj_token("git.example.com", env={}, home=home)

            mode = stat.S_IMODE(auth_file_path(home).stat().st_mode)

        self.assertEqual(storage, str(auth_file_path(home)))
        self.assertEqual(token, "stored-token")
        self.assertEqual(mode, 0o600)

    def test_env_token_wins_over_stored_shim_auth(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            write_stored_token("git.example.com", "stored-token", home=home, platform="linux")
            token = discover_fj_token("git.example.com", env={"FJ_SHIM_TOKEN": "env-token"}, home=home)

        self.assertEqual(token, "env-token")

    def test_delete_stored_token_removes_auth_file_entry(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            write_stored_token("git.example.com", "stored-token", home=home, platform="linux")

            deleted = delete_stored_token("git.example.com", home=home, platform="linux")
            token = discover_fj_token("git.example.com", env={}, home=home)

        self.assertTrue(deleted)
        self.assertIsNone(token)


if __name__ == "__main__":
    unittest.main()
