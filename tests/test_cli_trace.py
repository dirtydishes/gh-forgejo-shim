from __future__ import annotations

import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from gh_forgejo_shim import cli
from gh_forgejo_shim.codex_smoke import SmokeProbe, SmokeResult
from gh_forgejo_shim.git_recorder import GitRecorder


class TraceCliTests(unittest.TestCase):
    def test_trace_summarize_prints_summary(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            trace_path = Path(tmp) / "trace.jsonl"
            trace_path.write_text(
                '{"kind":"gh","argv":["auth","status"],"exit_code":0,"duration_ms":3,"route":{"kind":"forgejo"}}\n',
                encoding="utf-8",
            )
            out = io.StringIO()

            with contextlib.redirect_stdout(out):
                code = cli.main(["trace", "summarize", str(trace_path)])

        self.assertEqual(code, 0)
        self.assertIn("Trace records: 1", out.getvalue())
        self.assertIn("gh auth status=1", out.getvalue())

    def test_trace_smoke_returns_failure_for_required_probe_failure(self) -> None:
        required = SmokeProbe("gh version", ("gh", "--version"))
        optional = SmokeProbe("gh pr status", ("gh", "pr", "status"), optional=True)
        results = [
            SmokeResult(required, 127, 1, 0, 10, "missing gh"),
            SmokeResult(optional, 1, 1, 0, 10, "no pull request"),
        ]
        out = io.StringIO()

        with mock.patch.object(cli, "run_codex_smoke", return_value=results):
            with contextlib.redirect_stdout(out):
                code = cli.main(["trace", "smoke"])

        self.assertEqual(code, 1)
        self.assertIn("[fail] gh version", out.getvalue())
        self.assertIn("[optional-fail] gh pr status", out.getvalue())

    def test_git_recorder_create_prints_launch_environment(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            recorder = GitRecorder(
                wrapper_dir=root / "wrapper",
                wrapper_path=root / "wrapper" / "git",
                trace_path=root / "trace.jsonl",
                real_git=root / "git-real",
                path=f"{root / 'wrapper'}:/usr/bin",
                env={"PATH": f"{root / 'wrapper'}:/usr/bin", "FJ_SHIM_TRACE": str(root / "trace.jsonl")},
            )
            out = io.StringIO()

            with mock.patch.object(cli, "create_git_recorder", return_value=recorder):
                with contextlib.redirect_stdout(out):
                    code = cli.main(
                        [
                            "trace",
                            "git-recorder",
                            "create",
                            str(root / "trace.jsonl"),
                            "--real-git",
                            str(root / "git-real"),
                        ]
                    )

        self.assertEqual(code, 0)
        self.assertIn("created temporary git recorder", out.getvalue())
        self.assertIn("export FJ_SHIM_TRACE=", out.getvalue())
        self.assertIn("gfj trace git-recorder remove", out.getvalue())


if __name__ == "__main__":
    unittest.main()
