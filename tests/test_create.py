from __future__ import annotations

import io
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.create import CreateParseError, parse_create_args


class CreateFlagTests(unittest.TestCase):
    def test_supported_flags_are_translated(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            body_file = Path(tmp) / "body.md"
            body_file.write_text("hello\n", encoding="utf-8")
            options = parse_create_args(
                [
                    "--title",
                    "T",
                    "-F",
                    str(body_file),
                    "-B",
                    "main",
                    "-H",
                    "feature",
                    "-R",
                    "git.example.com/owner/repo",
                    "--fill",
                    "--fill-first",
                    "--fill-verbose",
                    "-w",
                    "-d",
                    "--json",
                    "number,title,url",
                ]
            )
        self.assertEqual(options.title, "T")
        self.assertEqual(options.body, "hello\n")
        self.assertEqual(options.base, "main")
        self.assertEqual(options.head, "feature")
        self.assertEqual(options.repo, "git.example.com/owner/repo")
        self.assertTrue(options.fill)
        self.assertTrue(options.fill_first)
        self.assertTrue(options.fill_verbose)
        self.assertTrue(options.web)
        self.assertTrue(options.draft)
        self.assertEqual(options.json_fields, ("number", "title", "url"))

    def test_body_file_stdin(self) -> None:
        options = parse_create_args(["--body-file", "-"], stdin=io.StringIO("from stdin"))
        self.assertEqual(options.body, "from stdin")

    def test_unsupported_metadata_flags_fail_clearly(self) -> None:
        for flag in ["--reviewer", "--label", "--assignee", "--project", "--milestone", "--template"]:
            with self.subTest(flag=flag):
                with self.assertRaisesRegex(CreateParseError, "unsupported Forgejo PR create flag"):
                    parse_create_args([flag, "value"])

    def test_unknown_flags_fail_clearly(self) -> None:
        with self.assertRaisesRegex(CreateParseError, "unsupported Forgejo PR create flag: --dry-run"):
            parse_create_args(["--dry-run"])


if __name__ == "__main__":
    unittest.main()
