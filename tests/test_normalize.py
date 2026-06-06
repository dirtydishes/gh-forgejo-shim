from __future__ import annotations

import unittest

from gh_forgejo_shim.normalize import (
    filter_fields,
    normalize_pr_checks,
    normalize_pull,
    normalize_status_check_rollup,
    status_for_current_branch,
)


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
        self.assertEqual(normalized["additions"], 0)
        self.assertEqual(normalized["deletions"], 0)
        self.assertEqual(normalized["mergeable"], "MERGEABLE")
        self.assertEqual(normalized["mergeStateStatus"], "CLEAN")
        self.assertEqual(normalized["statusCheckRollup"], [])

    def test_normalizes_merged_and_unknown_mergeable_values(self) -> None:
        normalized = normalize_pull(
            {
                "number": 12,
                "state": "closed",
                "merged_at": "2026-05-31T02:00:00Z",
                "mergeable": False,
            }
        )
        self.assertEqual(normalized["state"], "MERGED")
        self.assertEqual(normalized["mergeable"], "UNKNOWN")

    def test_filters_supported_json_fields(self) -> None:
        data = {"number": 1, "title": "T", "unknown": "x"}
        self.assertEqual(filter_fields(data, ("number", "unknown")), {"number": 1})

    def test_status_for_current_branch_wraps_pull_like_gh_status(self) -> None:
        status = status_for_current_branch(
            {
                "number": 12,
                "title": "Add shim",
                "state": "open",
                "html_url": "https://git.example.com/o/r/pulls/12",
                "head": {"ref": "feature"},
            },
            ("number", "title", "url"),
        )
        self.assertEqual(
            status,
            {
                "currentBranch": {
                    "number": 12,
                    "title": "Add shim",
                    "url": "https://git.example.com/o/r/pulls/12",
                },
                "createdBy": [],
                "needsReview": [],
            },
        )

    def test_normalizes_forgejo_commit_statuses_to_check_rows(self) -> None:
        statuses = [
            {
                "context": "ci/test",
                "description": "old result",
                "state": "failure",
                "created_at": "2026-06-01T00:00:00Z",
                "updated_at": "2026-06-01T00:01:00Z",
            },
            {
                "context": "ci/test",
                "description": "new result",
                "state": "success",
                "target_url": "https://git.example.com/owner/repo/actions/runs/2",
                "created_at": "2026-06-01T00:02:00Z",
                "updated_at": "2026-06-01T00:03:00Z",
            },
        ]
        checks = normalize_pr_checks(statuses)
        self.assertEqual(
            checks,
            [
                {
                    "bucket": "pass",
                    "completedAt": "2026-06-01T00:03:00Z",
                    "conclusion": "success",
                    "description": "new result",
                    "detailsUrl": "https://git.example.com/owner/repo/actions/runs/2",
                    "link": "https://git.example.com/owner/repo/actions/runs/2",
                    "name": "ci/test",
                    "startedAt": "2026-06-01T00:02:00Z",
                    "state": "completed",
                    "workflow": "ci/test",
                }
            ],
        )

    def test_normalizes_forgejo_commit_statuses_to_rollup_items(self) -> None:
        rollup = normalize_status_check_rollup(
            [
                {
                    "context": "deploy",
                    "description": "waiting",
                    "state": "pending",
                    "created_at": "2026-06-01T00:00:00Z",
                }
            ]
        )
        self.assertEqual(
            rollup,
            [
                {
                    "__typename": "StatusContext",
                    "completedAt": None,
                    "conclusion": None,
                    "context": "deploy",
                    "description": "waiting",
                    "detailsUrl": None,
                    "name": "deploy",
                    "startedAt": "2026-06-01T00:00:00Z",
                    "state": "PENDING",
                    "targetUrl": None,
                    "workflowName": "deploy",
                }
            ],
        )

if __name__ == "__main__":
    unittest.main()
