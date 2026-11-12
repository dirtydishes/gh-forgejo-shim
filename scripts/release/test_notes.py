import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import notes  # noqa: E402

REPO = "owner/repo"
ROOT = HERE.parent.parent
NOTES = ROOT / "docs" / "release-notes.md"

SAMPLE = """\
# Unreleased

- Something not shipped yet.

The v0.2.0 notes follow.

# gh-forgejo-shim v0.2.0

- **New thing.** See [the mapping](gateway/MAPPING.md#limits) and [the site](https://example.com).

  ```bash
  # a comment, not a heading
  gfj doctor
  ```

## Upgrading

Read [the install guide](installation.md) and [above](#upgrading).

The v0.1.0 notes follow.

# gh-forgejo-shim v0.1.0

- First release.
"""


class SectionTest(unittest.TestCase):
    def test_takes_only_the_tags_section(self):
        body = notes.section(SAMPLE, "v0.2.0", REPO)
        self.assertIn("New thing", body)
        self.assertNotIn("First release", body)
        self.assertNotIn("not shipped", body)

    def test_drops_the_heading_and_the_trailer(self):
        body = notes.section(SAMPLE, "v0.2.0", REPO)
        self.assertNotIn("# gh-forgejo-shim", body)
        self.assertNotIn("notes follow", body)
        self.assertTrue(body.startswith("- **New thing.**"))
        self.assertTrue(body.endswith("[above](#upgrading).\n"))

    def test_keeps_comments_in_code_blocks(self):
        body = notes.section(SAMPLE, "v0.2.0", REPO)
        self.assertIn("  # a comment, not a heading\n  gfj doctor", body)
        self.assertIn("## Upgrading", body)

    def test_a_fence_closes_only_on_its_own_kind(self):
        text = (
            "# gh-forgejo-shim v1.0.0\n\n"
            "~~~\n```\n# inside tildes\n~~~\n\n"
            "````\n```\n# inside four backticks\n```\n````\n\n"
            "- after\n\n# gh-forgejo-shim v0.9.0\n"
        )
        body = notes.section(text, "v1.0.0", REPO)
        self.assertIn("# inside tildes", body)
        self.assertIn("# inside four backticks", body)
        self.assertTrue(body.endswith("- after\n"))

    def test_leaves_links_in_code_blocks(self):
        text = "# gh-forgejo-shim v1.0.0\n\n```\n[a](x.md)\n```\n"
        self.assertIn("\n[a](x.md)\n", notes.section(text, "v1.0.0", REPO))

    def test_points_relative_links_at_the_tag(self):
        body = notes.section(SAMPLE, "v0.2.0", REPO)
        self.assertIn(
            "[the mapping](https://github.com/owner/repo/blob/v0.2.0/docs/gateway/MAPPING.md#limits)",
            body,
        )
        self.assertIn(
            "[the install guide](https://github.com/owner/repo/blob/v0.2.0/docs/installation.md)",
            body,
        )
        self.assertIn("[the site](https://example.com)", body)
        self.assertIn("[above](#upgrading)", body)

    def test_last_section_runs_to_the_end(self):
        self.assertEqual(notes.section(SAMPLE, "v0.1.0", REPO), "- First release.\n")

    def test_missing_tag_fails(self):
        with self.assertRaises(notes.NotesError):
            notes.section(SAMPLE, "v9.9.9", REPO)

    def test_empty_section_fails(self):
        text = "# gh-forgejo-shim v1.0.0\n\nThe v0.9.0 notes follow.\n\n# gh-forgejo-shim v0.9.0\n\n- x\n"
        with self.assertRaises(notes.NotesError):
            notes.section(text, "v1.0.0", REPO)

    def test_duplicate_heading_fails(self):
        with self.assertRaises(notes.NotesError):
            notes.section(SAMPLE + "\n# gh-forgejo-shim v0.2.0\n\n- again\n", "v0.2.0", REPO)


class RepoNotesTest(unittest.TestCase):
    """The committed notes file: every release's notes hold that release alone."""

    def test_each_release_gets_only_its_own_notes(self):
        text = NOTES.read_text()
        tags = re.findall(r"^# gh-forgejo-shim (v\S+)$", text, re.M)
        self.assertGreater(len(tags), 1)
        for tag in tags:
            with self.subTest(tag=tag):
                body = notes.section(text, tag, REPO)
                self.assertNotIn("# gh-forgejo-shim v", body)
                self.assertNotIn("notes follow", body)

    def test_cli_writes_one_release(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = pathlib.Path(tmp) / "body.md"
            subprocess.run(
                [sys.executable, str(HERE / "notes.py"), str(NOTES), "v0.2.1", REPO, str(out)],
                check=True,
            )
            body = out.read_text()
        self.assertNotIn("# gh-forgejo-shim v0.2.0", body)
        self.assertGreater(len(body), 0)


if __name__ == "__main__":
    unittest.main()
