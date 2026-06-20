from __future__ import annotations

import subprocess
import unittest

from gh_forgejo_shim.codex_smoke import codex_smoke_probes, format_codex_smoke, run_codex_smoke


class CodexSmokeTests(unittest.TestCase):
    def test_probe_sequence_includes_codex_gh_and_git_checks(self) -> None:
        commands = [probe.command for probe in codex_smoke_probes()]

        self.assertIn(("gh", "--version"), commands)
        self.assertIn(("gh", "auth", "status"), commands)
        self.assertIn(("gh", "api", "user"), commands)
        self.assertIn(
            ("gh", "pr", "status", "--json", "number,title,url,headRefName,statusCheckRollup"),
            commands,
        )
        self.assertIn(("git", "branch", "--show-current"), commands)
        self.assertIn(
            ("git", "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"),
            commands,
        )

    def test_run_codex_smoke_records_sizes_and_optional_failures(self) -> None:
        seen: list[tuple[str, ...]] = []

        def runner(command, **_kwargs):  # type: ignore[no-untyped-def]
            seen.append(tuple(command))
            if command[:3] == ["gh", "pr", "status"]:
                return subprocess.CompletedProcess(command, 1, "", "no pull request found\n")
            return subprocess.CompletedProcess(command, 0, "ok\n", "")

        results = run_codex_smoke(env={"PATH": "/bin"}, runner=runner)

        self.assertEqual(len(results), len(codex_smoke_probes()))
        self.assertEqual(seen[0], ("gh", "--version"))
        status = next(result for result in results if result.probe.name == "gh pr status")
        self.assertEqual(status.exit_code, 1)
        self.assertTrue(status.ok)
        self.assertEqual(status.stderr_bytes, len("no pull request found\n".encode("utf-8")))
        self.assertEqual(status.stderr_excerpt, "no pull request found")

    def test_format_codex_smoke_marks_required_failures(self) -> None:
        def runner(command, **_kwargs):  # type: ignore[no-untyped-def]
            if command == ["gh", "--version"]:
                return subprocess.CompletedProcess(command, 127, "", "missing gh")
            return subprocess.CompletedProcess(command, 0, "ok", "")

        output = format_codex_smoke(run_codex_smoke(env={"PATH": ""}, runner=runner))

        self.assertIn("[fail] gh version: exit 127", output)
        self.assertIn("Required probe failures: 1", output)


if __name__ == "__main__":
    unittest.main()
