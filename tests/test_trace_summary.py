from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.trace_summary import classify_probe, format_trace_summary, summarize_trace, summarize_trace_file


class TraceSummaryTests(unittest.TestCase):
    def test_classifies_known_codex_gh_and_git_probes(self) -> None:
        self.assertEqual(classify_probe({"kind": "gh", "argv": ["--version"]}), "gh --version")
        self.assertEqual(classify_probe({"kind": "gh", "argv": ["auth", "status"]}), "gh auth status")
        self.assertEqual(
            classify_probe({"kind": "gh", "argv": ["api", "--hostname", "git.example.com", "user"]}),
            "gh api user",
        )
        self.assertEqual(classify_probe({"kind": "gh", "argv": ["pr", "status"]}), "gh pr status")
        self.assertEqual(
            classify_probe(
                {
                    "kind": "git",
                    "argv": ["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"],
                }
            ),
            "git default branch",
        )

    def test_summarizes_failures_slow_calls_unsupported_and_routes(self) -> None:
        summary = summarize_trace(
            [
                {
                    "kind": "gh",
                    "argv": ["pr", "status"],
                    "exit_code": 1,
                    "duration_ms": 25,
                    "route": {"kind": "forgejo", "reason": "git remote"},
                },
                {
                    "kind": "gh",
                    "argv": ["workflow", "list"],
                    "exit_code": 0,
                    "duration_ms": 1200,
                    "route": {"kind": "delegate", "reason": "unsupported command"},
                },
                {
                    "kind": "git",
                    "argv": ["branch", "--show-current"],
                    "exit_code": 0,
                    "duration_ms": 5,
                },
            ]
        )

        self.assertEqual(summary.total, 3)
        self.assertEqual(len(summary.failures), 1)
        self.assertEqual(len(summary.slow_calls), 1)
        self.assertEqual(len(summary.unsupported), 1)
        self.assertEqual(summary.probe_counts["gh pr status"], 1)
        self.assertEqual(summary.probe_counts["git current branch"], 1)
        self.assertEqual(summary.route_counts["forgejo"], 1)
        self.assertEqual(summary.route_counts["delegate"], 1)

    def test_formats_summary_with_parse_errors(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "trace.jsonl"
            path.write_text(
                '{"kind":"gh","argv":["--version"],"exit_code":0,"duration_ms":2,"route":{"kind":"delegate"}}\n'
                'not json\n',
                encoding="utf-8",
            )

            output = format_trace_summary(summarize_trace_file(path))

        self.assertIn("Trace records: 1", output)
        self.assertIn("Parse errors: 1", output)
        self.assertIn("Known Codex probes: gh --version=1", output)


if __name__ == "__main__":
    unittest.main()
