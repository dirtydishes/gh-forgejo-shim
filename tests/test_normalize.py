from __future__ import annotations

import unittest

from gh_forgejo_shim.normalize import filter_fields, normalize_pull


class NormalizeTests(unittest.TestCase):
    def test_normalizes_forgejo_pull_to_github_shape(self) -> None:
        pull = {
            "number": 12,
            "title": "Add shim",
            "state": "open",
            "draft": True,
            "html_url": "https://git.example.com/o/r/pulls/12",
            "head": {"ref": "feature"},
            "base": {"ref": "main"},
            "user": {"login": "alice", "full_name": "Alice"},
            "created_at": "2026-05-31T00:00:00Z",
            "updated_at": "2026-05-31T01:00:00Z",
            "mergeable": True,
            "merge_state_status": "CLEAN",
        }
        normalized = normalize_pull(pull)
        self.assertEqual(normalized["number"], 12)
        self.assertEqual(normalized["state"], "OPEN")
        self.assertTrue(normalized["isDraft"])
        self.assertEqual(normalized["headRefName"], "feature")
        self.assertEqual(normalized["baseRefName"], "main")
        self.assertEqual(normalized["author"]["login"], "alice")
        self.assertEqual(normalized["mergeStateStatus"], "CLEAN")
        self.assertEqual(normalized["statusCheckRollup"], [])

    def test_filters_supported_json_fields(self) -> None:
        data = {"number": 1, "title": "T", "unknown": "x"}
        self.assertEqual(filter_fields(data, ("number", "unknown")), {"number": 1})


if __name__ == "__main__":
    unittest.main()
