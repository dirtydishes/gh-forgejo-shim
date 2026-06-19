from __future__ import annotations

import json
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

from gh_forgejo_shim.trace import (
    AUTH_TOKEN_REDACTED,
    REDACTED,
    TraceRecorder,
    TraceRepo,
    TraceRoute,
    redact_text,
)


def fixed_now() -> datetime:
    return datetime(2026, 6, 19, 16, 45, 12, 345000, tzinfo=timezone.utc)


class TraceTests(unittest.TestCase):
    def test_redacts_auth_sensitive_text(self) -> None:
        token = "ghp_abcdefghijklmnopqrstuvwxyz123456"
        text = (
            f"Authorization: token {token}\n"
            f"FJ_SHIM_TOKEN={token}\n"
            f'{{"access_token": "{token}", "password": "hunter2"}}\n'
            f"https://alice:{token}@git.example.com/owner/repo"
        )

        redacted = redact_text(text)

        self.assertNotIn(token, redacted)
        self.assertNotIn("hunter2", redacted)
        self.assertIn(REDACTED, redacted)

    def test_noop_when_trace_env_is_unset(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            recorder = TraceRecorder.from_env({}, clock=fixed_now)
            emitted = recorder.emit(
                argv=["repo", "view"],
                route=TraceRoute("delegate", "disabled"),
                cwd=tmp,
                duration_ms=1,
                exit_code=0,
                stdout="ok",
                stderr="",
            )

            self.assertFalse(emitted)
            self.assertEqual(list(Path(tmp).iterdir()), [])

    def test_appends_jsonl_record(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "trace.jsonl"
            recorder = TraceRecorder.from_env({"FJ_SHIM_TRACE": str(path)}, clock=fixed_now)
            route = TraceRoute(
                "forgejo",
                "git remote",
                host="git.example.com",
                repo=TraceRepo("git.example.com", "owner", "repo"),
            )

            self.assertTrue(
                recorder.emit(
                    argv=["repo", "view"],
                    route=route,
                    cwd="/work/repo",
                    duration_seconds=0.123,
                    exit_code=0,
                    stdout="hello",
                    stderr=b"warn",
                )
            )
            self.assertTrue(
                recorder.emit(
                    argv=["pr", "status"],
                    route=route,
                    cwd="/work/repo",
                    duration_ms=7,
                    exit_code=1,
                    stdout_bytes=10,
                    stderr_bytes=2,
                )
            )

            lines = path.read_text(encoding="utf-8").splitlines()
            self.assertEqual(len(lines), 2)
            first = json.loads(lines[0])
            second = json.loads(lines[1])

        self.assertEqual(first["timestamp"], "2026-06-19T16:45:12.345Z")
        self.assertEqual(first["cwd"], "/work/repo")
        self.assertEqual(first["argv"], ["repo", "view"])
        self.assertEqual(first["route"], {"kind": "forgejo", "reason": "git remote"})
        self.assertEqual(first["host"], "git.example.com")
        self.assertEqual(
            first["repo"],
            {
                "host": "git.example.com",
                "owner": "owner",
                "name": "repo",
                "full_name": "owner/repo",
            },
        )
        self.assertEqual(first["duration_ms"], 123)
        self.assertEqual(first["exit_code"], 0)
        self.assertEqual(first["stdout"], {"bytes": 5})
        self.assertEqual(first["stderr"], {"bytes": 4})
        self.assertEqual(second["stdout"], {"bytes": 10})
        self.assertEqual(second["stderr"], {"bytes": 2})

    def test_body_excerpt_is_opt_in(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "trace.jsonl"
            recorder = TraceRecorder.from_env(
                {
                    "FJ_SHIM_TRACE": str(path),
                    "FJ_SHIM_TRACE_BODY": "1",
                },
                clock=fixed_now,
                max_excerpt_chars=5,
            )

            recorder.emit(
                argv=["repo", "view"],
                route=TraceRoute("forgejo"),
                cwd="/work/repo",
                duration_ms=5,
                exit_code=0,
                stdout="abcdef",
                stderr="err",
            )

            record = json.loads(path.read_text(encoding="utf-8"))

        self.assertEqual(record["stdout"], {"bytes": 6, "excerpt": "abcde", "truncated": True})
        self.assertEqual(record["stderr"], {"bytes": 3, "excerpt": "err", "truncated": False})

    def test_auth_token_stdout_is_suppressed_even_when_body_enabled(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "trace.jsonl"
            token = "plain-forgejo-token-without-a-known-prefix"
            recorder = TraceRecorder.from_env(
                {
                    "FJ_SHIM_TRACE": str(path),
                    "FJ_SHIM_TRACE_BODY": "true",
                },
                clock=fixed_now,
            )

            recorder.emit(
                argv=["auth", "token", "--hostname", "git.example.com"],
                route=TraceRoute("forgejo", "host", host="git.example.com"),
                cwd="/work/repo",
                duration_ms=3,
                exit_code=0,
                stdout=f"{token}\n",
                stderr="",
            )

            raw = path.read_text(encoding="utf-8")
            record = json.loads(raw)

        self.assertNotIn(token, raw)
        self.assertEqual(record["stdout"]["excerpt"], AUTH_TOKEN_REDACTED)
        self.assertEqual(record["stdout"]["truncated"], False)


if __name__ == "__main__":
    unittest.main()
