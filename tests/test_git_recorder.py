from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.git_recorder import (
    TRACE_BODY_ENV,
    TRACE_PATH_ENV,
    create_git_recorder,
    remove_git_recorder,
)


class GitRecorderTests(unittest.TestCase):
    def test_create_returns_wrapper_paths_and_launch_environment(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake_git = self._fake_git(root / "real" / "git", stdout="ok\n", stderr="", exit_code=0)
            trace = root / "trace" / "git.jsonl"

            recorder = create_git_recorder(trace, real_git=fake_git, root_dir=root, env={"PATH": "/usr/bin:/bin"})

            self.assertTrue(recorder.wrapper_path.exists())
            self.assertTrue(os.access(recorder.wrapper_path, os.X_OK))
            self.assertEqual(recorder.wrapper_path.name, "git")
            self.assertEqual(recorder.trace_path, trace.resolve())
            self.assertEqual(recorder.real_git, fake_git.resolve())
            self.assertTrue(recorder.path.startswith(str(recorder.wrapper_dir) + os.pathsep))
            self.assertEqual(recorder.env["PATH"], recorder.path)
            self.assertEqual(recorder.env[TRACE_PATH_ENV], str(trace.resolve()))
            self.assertEqual(recorder.env["FJ_SHIM_REAL_GIT"], str(fake_git.resolve()))

            removed = remove_git_recorder(recorder)
            self.assertEqual(removed, recorder.wrapper_dir)
            self.assertFalse(recorder.wrapper_dir.exists())

    def test_wrapper_records_invocation_and_preserves_real_git_result(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake_git = self._fake_git(
                root / "real" / "git",
                stdout="fake stdout\n",
                stderr="fake stderr\n",
                exit_code=7,
            )
            trace = root / "trace.jsonl"
            recorder = create_git_recorder(trace, real_git=fake_git, root_dir=root, env={"PATH": ""})

            completed = subprocess.run(
                ["git", "status", "--short"],
                cwd=root,
                env=recorder.env,
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )

            self.assertEqual(completed.returncode, 7)
            self.assertEqual(completed.stdout, "fake stdout\n")
            self.assertEqual(completed.stderr, "fake stderr\n")

            records = self._read_records(trace)
            self.assertEqual(len(records), 1)
            record = records[0]
            self.assertEqual(record["kind"], "git")
            self.assertIsInstance(record["timestamp"], str)
            self.assertEqual(Path(str(record["cwd"])).resolve(), root.resolve())
            self.assertEqual(record["argv"], ["status", "--short"])
            self.assertEqual(record["command_argv"], ["git", "status", "--short"])
            self.assertEqual(record["exit_code"], 7)
            self.assertGreaterEqual(record["duration"], 0)
            self.assertGreaterEqual(record["duration_ms"], 0)
            self.assertEqual(record["stdout_bytes"], len(b"fake stdout\n"))
            self.assertEqual(record["stderr_bytes"], len(b"fake stderr\n"))
            self.assertEqual(record["stdout"]["bytes"], len(b"fake stdout\n"))
            self.assertEqual(record["stderr"]["bytes"], len(b"fake stderr\n"))
            self.assertNotIn("stdout_excerpt", record)
            self.assertNotIn("stderr_excerpt", record)
            self.assertNotIn("excerpt", record["stdout"])
            self.assertNotIn("excerpt", record["stderr"])

    def test_trace_body_excerpts_are_opt_in_and_redacted(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake_git = self._fake_git(
                root / "real" / "git",
                stdout="token=stdout-secret\nvisible\n",
                stderr="Authorization: Bearer stderr-secret\n",
                exit_code=0,
            )
            trace = root / "trace.jsonl"
            recorder = create_git_recorder(trace, real_git=fake_git, root_dir=root, env={"PATH": ""})
            env = dict(recorder.env)
            env[TRACE_BODY_ENV] = "1"

            completed = subprocess.run(
                [
                    "git",
                    "fetch",
                    "--password=argv-secret",
                    "--token",
                    "next-argv-secret",
                    "https://user:password@example.com/repo.git",
                ],
                cwd=root,
                env=env,
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )

            self.assertEqual(completed.returncode, 0)
            record = self._read_records(trace)[0]
            serialized = json.dumps(record)
            self.assertIn("[REDACTED]", serialized)
            self.assertNotIn("stdout-secret", serialized)
            self.assertNotIn("stderr-secret", serialized)
            self.assertNotIn("argv-secret", serialized)
            self.assertNotIn("next-argv-secret", serialized)
            self.assertNotIn("user:password", serialized)
            self.assertEqual(
                record["argv"],
                [
                    "fetch",
                    "--password=[REDACTED]",
                    "--token",
                    "[REDACTED]",
                    "https://[REDACTED]@example.com/repo.git",
                ],
            )
            self.assertIn("visible", record["stdout_excerpt"])
            self.assertEqual(record["stdout_excerpt"], record["stdout"]["excerpt"])
            self.assertEqual(record["stderr_excerpt"], record["stderr"]["excerpt"])

    def test_remove_refuses_unmanaged_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            unmanaged = Path(tmp) / "unmanaged"
            unmanaged.mkdir()
            (unmanaged / "git").write_text("#!/bin/sh\n", encoding="utf-8")

            with self.assertRaises(FileExistsError):
                remove_git_recorder(unmanaged)

    def _fake_git(self, path: Path, *, stdout: str, stderr: str, exit_code: int) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            "\n".join(
                [
                    f"#!{sys.executable}",
                    "from __future__ import annotations",
                    "import sys",
                    f"sys.stdout.write({stdout!r})",
                    f"sys.stderr.write({stderr!r})",
                    f"raise SystemExit({exit_code})",
                    "",
                ]
            ),
            encoding="utf-8",
        )
        path.chmod(0o755)
        return path

    def _read_records(self, path: Path) -> list[dict[str, object]]:
        return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]


if __name__ == "__main__":
    unittest.main()
