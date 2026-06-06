from __future__ import annotations

import io
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.auth import discover_fj_token, write_stored_token
from gh_forgejo_shim.cli import build_parser, run_auth
from gh_forgejo_shim.config import add_host, load_config
from gh_forgejo_shim.forgejo import ForgejoClient


class FakeAuthClient(ForgejoClient):
    def __init__(self, token: str | None) -> None:
        super().__init__(token)
        self.seen_host: str | None = None

    def get_current_user(self, host: str):  # type: ignore[no-untyped-def]
        self.seen_host = host
        return {"login": "alice"}


class AuthCliTests(unittest.TestCase):
    def test_login_prompts_validates_stores_and_allowlists_host(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            config_path = root / "config.toml"
            out = io.StringIO()
            clients: list[FakeAuthClient] = []

            code = run_auth(
                self._parse("login"),
                env={},
                home=root,
                platform="linux",
                config_path=config_path,
                stdout=out,
                input_func=lambda _prompt: "https://Git.Example.com/path",
                token_prompt=lambda _prompt: "login-token",
                client_factory=lambda token: self._client(token, clients),
            )

            stored = discover_fj_token("git.example.com", env={}, home=root)
            hosts = load_config(config_path, env={}).hosts
            output = out.getvalue()

        self.assertEqual(code, 0)
        self.assertEqual(stored, "login-token")
        self.assertEqual(hosts, ("git.example.com",))
        self.assertEqual(clients[0].token, "login-token")
        self.assertEqual(clients[0].seen_host, "git.example.com")
        self.assertIn("logged in to git.example.com as alice", output)
        self.assertNotIn("login-token", output)

    def test_import_uses_env_token_without_printing_secret(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            config_path = root / "config.toml"
            out = io.StringIO()

            code = run_auth(
                self._parse("import", "git.example.com"),
                env={"FJ_SHIM_TOKEN": "import-token"},
                home=root,
                platform="linux",
                config_path=config_path,
                stdout=out,
                client_factory=lambda token: FakeAuthClient(token),
            )

            stored = discover_fj_token("git.example.com", env={}, home=root)
            hosts = load_config(config_path, env={}).hosts
            output = out.getvalue()

        self.assertEqual(code, 0)
        self.assertEqual(stored, "import-token")
        self.assertEqual(hosts, ("git.example.com",))
        self.assertIn("source: FJ_SHIM_TOKEN", output)
        self.assertNotIn("import-token", output)

    def test_status_reports_stored_auth_without_secret(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            config_path = root / "config.toml"
            add_host("git.example.com", config_path)
            write_stored_token("git.example.com", "stored-secret", home=root, platform="linux")
            out = io.StringIO()

            code = run_auth(
                self._parse("status"),
                env={},
                home=root,
                platform="linux",
                config_path=config_path,
                stdout=out,
            )

            output = out.getvalue()

        self.assertEqual(code, 0)
        self.assertIn("git.example.com: logged in", output)
        self.assertNotIn("stored-secret", output)

    def test_logout_removes_stored_shim_auth(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_stored_token("git.example.com", "stored-secret", home=root, platform="linux")
            out = io.StringIO()

            code = run_auth(
                self._parse("logout", "git.example.com"),
                env={},
                home=root,
                platform="linux",
                stdout=out,
            )

            token = discover_fj_token("git.example.com", env={}, home=root)

        self.assertEqual(code, 0)
        self.assertIsNone(token)
        self.assertIn("logged out of git.example.com", out.getvalue())

    def _parse(self, *argv: str):  # type: ignore[no-untyped-def]
        return build_parser().parse_args(["auth", *argv])

    def _client(self, token: str | None, clients: list[FakeAuthClient]) -> FakeAuthClient:
        client = FakeAuthClient(token)
        clients.append(client)
        return client


if __name__ == "__main__":
    unittest.main()
