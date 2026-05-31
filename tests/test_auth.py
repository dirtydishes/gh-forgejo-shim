from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.auth import discover_fj_token, token_from_env


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


if __name__ == "__main__":
    unittest.main()
