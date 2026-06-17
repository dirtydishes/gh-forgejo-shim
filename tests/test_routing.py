from __future__ import annotations

import io
import json
import re
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.config import Config
from gh_forgejo_shim.forgejo import ForgejoClient, RepoRef
from gh_forgejo_shim.routing import decide_route, run_forgejo, run_forgejo_pr, run_gh


def codex_create_pr_url(output: str) -> str | None:
    for match in re.finditer(r"https?://\S+", output):
        url = match.group(0).rstrip("),.")
        if re.search(r"/pull/\d+(?:\b|$)", url):
            return url
    return None


class FakeClient(ForgejoClient):
    def __init__(
        self,
        pulls: list[dict] | None = None,
        issues: list[dict] | None = None,
        statuses: list[dict] | None = None,
    ) -> None:
        super().__init__(None)
        self.pulls = pulls or []
        self.issues = issues or []
        self.statuses = statuses or []
        self.created: dict | None = None
        self.created_issue: dict | None = None
        self.comments: list[dict] = []
        self.diff_text = "diff --git a/README.md b/README.md\n"
        self.pull_files = [{"filename": "README.md"}, {"filename": "generated/output.txt"}]
        self.current_user = {"login": "alice", "full_name": "Alice Example"}
        self.repo_view = {
            "id": 99,
            "name": "repo",
            "full_name": "owner/repo",
            "html_url": "https://git.example.com/owner/repo",
            "ssh_url": "git@git.example.com:owner/repo.git",
            "default_branch": "main",
            "private": False,
            "has_issues": True,
            "has_projects": True,
            "has_wiki": False,
            "allow_merge_commits": True,
            "allow_rebase": True,
            "allow_squash_merge": True,
            "forks_count": 2,
            "stars_count": 3,
            "watchers_count": 4,
            "open_issues_count": 5,
            "open_pr_counter": 6,
            "language": "Python",
            "permissions": {"pull": True, "push": True, "admin": False},
            "owner": {"login": "owner"},
        }

    def create_pull(self, repo: RepoRef, **kwargs):  # type: ignore[no-untyped-def]
        self.created = {"repo": repo, **kwargs}
        return {
            "number": 7,
            "title": kwargs["title"],
            "state": "open",
            "html_url": "https://git.example.com/owner/repo/pulls/7",
            "head": {"ref": kwargs["head"]},
            "base": {"ref": kwargs["base"]},
            "user": {"login": "alice"},
        }

    def list_pulls(self, repo: RepoRef, *, state: str = "open", head: str | None = None):  # type: ignore[no-untyped-def]
        if head is None:
            return self.pulls
        return [
            pull
            for pull in self.pulls
            if pull.get("head", {}).get("ref") == head
        ]

    def get_pull(self, repo: RepoRef, number: int):  # type: ignore[no-untyped-def]
        for pull in self.pulls:
            if pull.get("number") == number:
                return pull
        raise ValueError("not found")

    def get_pull_diff(self, repo: RepoRef, number: int, *, patch: bool = False):  # type: ignore[no-untyped-def]
        return self.diff_text

    def list_pull_files(self, repo: RepoRef, number: int):  # type: ignore[no-untyped-def]
        return self.pull_files

    def list_commit_statuses(self, repo: RepoRef, sha: str):  # type: ignore[no-untyped-def]
        return self.statuses

    def get_repo(self, repo: RepoRef):  # type: ignore[no-untyped-def]
        return self.repo_view

    def list_issues(self, repo: RepoRef, **kwargs):  # type: ignore[no-untyped-def]
        return self.issues

    def get_issue(self, repo: RepoRef, number: int):  # type: ignore[no-untyped-def]
        for issue in self.issues:
            if issue.get("number") == number:
                return issue
        raise ValueError("not found")

    def list_labels(self, repo: RepoRef):  # type: ignore[no-untyped-def]
        return [{"id": 2, "name": "bug"}, {"id": 3, "name": "help wanted"}]

    def create_issue(self, repo: RepoRef, **kwargs):  # type: ignore[no-untyped-def]
        self.created_issue = {"repo": repo, **kwargs}
        return {
            "number": 14,
            "title": kwargs["title"],
            "body": kwargs.get("body", ""),
            "state": "open",
            "html_url": "https://git.example.com/owner/repo/issues/14",
            "user": {"login": "alice"},
        }

    def create_issue_comment(self, repo: RepoRef, number: int, *, body: str):  # type: ignore[no-untyped-def]
        comment = {"repo": repo, "number": number, "body": body}
        self.comments.append(comment)
        return comment

    def get_current_user(self, host: str):  # type: ignore[no-untyped-def]
        return self.current_user


class RoutingTests(unittest.TestCase):
    def test_delegates_non_pr_command(self) -> None:
        decision = decide_route(
            ["api", "repos/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "delegate")

    def test_routes_auth_status_for_allowlisted_gh_host(self) -> None:
        decision = decide_route(
            ["auth", "status"],
            config=Config(hosts=("git.example.com",)),
            env={"GH_HOST": "git.example.com"},
        )
        self.assertEqual(decision.kind, "forgejo")

    def test_forgejo_auth_status_uses_shim_auth_instead_of_real_gh(self) -> None:
        out = io.StringIO()
        code = run_gh(
            ["auth", "status"],
            env={
                "FJ_SHIM_HOSTS": "git.example.com",
                "FJ_SHIM_REAL_GH": "/missing/gh",
                "FJ_SHIM_TOKEN": "secret-token",
                "GH_HOST": "git.example.com",
                "PATH": "",
            },
            stdout=out,
            stderr=io.StringIO(),
        )

        self.assertEqual(code, 0)
        self.assertIn("git.example.com", out.getvalue())
        self.assertIn("Logged in", out.getvalue())

    def test_forgejo_auth_status_supports_json_hosts_field(self) -> None:
        out = io.StringIO()
        code = run_gh(
            ["auth", "status", "--json", "hosts"],
            env={
                "FJ_SHIM_HOSTS": "git.example.com",
                "FJ_SHIM_REAL_GH": "/missing/gh",
                "FJ_SHIM_TOKEN": "secret-token",
                "GH_HOST": "git.example.com",
                "PATH": "",
            },
            stdout=out,
            stderr=io.StringIO(),
        )

        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            {
                "hosts": {
                    "git.example.com": [
                        {
                            "active": True,
                            "gitProtocol": "https",
                            "host": "git.example.com",
                            "login": "",
                            "scopes": "",
                            "state": "success",
                            "tokenSource": "gh-forgejo-shim",
                        }
                    ]
                }
            },
        )

    def test_forgejo_auth_token_uses_shim_auth_instead_of_real_gh(self) -> None:
        out = io.StringIO()
        code = run_gh(
            ["auth", "token", "--hostname", "git.example.com"],
            env={
                "FJ_SHIM_HOSTS": "git.example.com",
                "FJ_SHIM_REAL_GH": "/missing/gh",
                "FJ_SHIM_TOKEN": "secret-token",
                "PATH": "",
            },
            stdout=out,
            stderr=io.StringIO(),
        )

        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue(), "secret-token\n")

    def test_forgejo_api_user_probe_uses_forgejo_client(self) -> None:
        out = io.StringIO()
        code = run_gh(
            ["api", "user", "--jq", ".login"],
            env={
                "FJ_SHIM_HOSTS": "git.example.com",
                "FJ_SHIM_REAL_GH": "/missing/gh",
                "FJ_SHIM_TOKEN": "secret-token",
                "GH_HOST": "git.example.com",
                "PATH": "",
            },
            stdout=out,
            stderr=io.StringIO(),
            client_factory=lambda token: FakeClient(),
        )

        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue(), "alice\n")

    def test_routes_issue_list_for_allowlisted_host(self) -> None:
        decision = decide_route(
            ["issue", "list", "-R", "git.example.com/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "forgejo")

    def test_routes_issue_view_url_for_allowlisted_host(self) -> None:
        decision = decide_route(
            ["issue", "view", "https://git.example.com/owner/repo/issues/13"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "forgejo")
        self.assertIsNotNone(decision.repo)
        assert decision.repo is not None
        self.assertEqual((decision.repo.owner, decision.repo.repo), ("owner", "repo"))

    def test_routes_pr_list_for_allowlisted_host(self) -> None:
        decision = decide_route(
            ["pr", "list", "-R", "git.example.com/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "forgejo")

    def test_routes_repo_view_for_allowlisted_host(self) -> None:
        decision = decide_route(
            ["repo", "view", "-R", "git.example.com/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "forgejo")

    def test_delegates_non_allowlisted_host(self) -> None:
        decision = decide_route(
            ["pr", "view", "-R", "github.com/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "delegate")

    def test_delegates_github_even_when_accidentally_allowlisted(self) -> None:
        decision = decide_route(
            ["pr", "view", "-R", "github.com/owner/repo"],
            config=Config(hosts=("github.com", "git.example.com")),
            env={},
        )
        self.assertEqual(decision.kind, "delegate")

    def test_routes_supported_pr_command_for_allowlisted_host(self) -> None:
        decision = decide_route(
            ["pr", "view", "-R", "git.example.com/owner/repo"],
            config=Config(hosts=("git.example.com",)),
            env={},
        )
        self.assertEqual(decision.kind, "forgejo")

    def test_create_calls_forgejo_client(self) -> None:
        client = FakeClient()
        out = io.StringIO()
        code = run_forgejo_pr(
            [
                "pr",
                "create",
                "-R",
                "git.example.com/owner/repo",
                "--title",
                "Ship it",
                "--body",
                "body",
                "--base",
                "main",
                "--head",
                "feature",
                "--draft",
                "--json",
                "number,title,url",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            client,
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertIsNotNone(client.created)
        assert client.created is not None
        self.assertEqual(client.created["title"], "Ship it")
        self.assertTrue(client.created["draft"])
        self.assertEqual(json.loads(out.getvalue()), {"number": 7, "title": "Ship it", "url": "https://git.example.com/owner/repo/pulls/7"})

    def test_create_default_output_surfaces_url_for_codex_app(self) -> None:
        client = FakeClient()
        out = io.StringIO()
        code = run_forgejo_pr(
            [
                "pr",
                "create",
                "-R",
                "git.example.com/owner/repo",
                "--title",
                "Ship it",
                "--body",
                "body",
                "--base",
                "main",
                "--head",
                "feature",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            client,
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            codex_create_pr_url(out.getvalue()),
            "https://git.example.com/owner/repo/pulls/7#codex-pr=/pull/7",
        )

    def test_pr_list_outputs_json_array(self) -> None:
        out = io.StringIO()
        code = run_forgejo_pr(
            [
                "pr",
                "list",
                "-R",
                "git.example.com/owner/repo",
                "--json",
                "number,title,url,headRefName",
                "--head",
                "feature",
                "--limit",
                "1",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(
                [
                    {
                        "number": 8,
                        "title": "Make Codex happy",
                        "state": "open",
                        "html_url": "https://git.example.com/owner/repo/pulls/8",
                        "head": {"ref": "feature"},
                        "base": {"ref": "main"},
                        "user": {"login": "alice"},
                    },
                    {
                        "number": 9,
                        "title": "Other branch",
                        "state": "open",
                        "html_url": "https://git.example.com/owner/repo/pulls/9",
                        "head": {"ref": "other"},
                        "base": {"ref": "main"},
                        "user": {"login": "alice"},
                    },
                ]
            ),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            [
                {
                    "headRefName": "feature",
                    "number": 8,
                    "title": "Make Codex happy",
                    "url": "https://git.example.com/owner/repo/pulls/8",
                }
            ],
        )

    def test_pr_list_outputs_codex_board_fields(self) -> None:
        out = io.StringIO()
        code = run_forgejo_pr(
            [
                "pr",
                "list",
                "-R",
                "git.example.com/owner/repo",
                "--state",
                "open",
                "--limit",
                "10",
                "--json",
                "additions,baseRefName,createdAt,deletions,headRefName,isDraft,number,state,title,updatedAt,url,mergeStateStatus,mergeable,statusCheckRollup",
                "--author",
                "@me",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(
                [
                    {
                        "number": 8,
                        "title": "Make Codex happy",
                        "state": "open",
                        "html_url": "https://git.example.com/owner/repo/pulls/8",
                        "head": {"ref": "feature", "sha": "abc123"},
                        "base": {"ref": "main"},
                        "user": {"login": "alice"},
                        "created_at": "2026-05-31T00:00:00Z",
                        "updated_at": "2026-05-31T01:00:00Z",
                        "mergeable": False,
                    }
                ],
                statuses=[
                    {
                        "context": "ci/test",
                        "description": "tests passed",
                        "state": "success",
                        "target_url": "https://git.example.com/owner/repo/actions/runs/1",
                        "created_at": "2026-06-01T00:00:00Z",
                        "updated_at": "2026-06-01T00:02:00Z",
                    }
                ],
            ),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            [
                {
                    "additions": 0,
                    "baseRefName": "main",
                    "createdAt": "2026-05-31T00:00:00Z",
                    "deletions": 0,
                    "headRefName": "feature",
                    "isDraft": False,
                    "mergeStateStatus": "UNKNOWN",
                    "mergeable": "UNKNOWN",
                    "number": 8,
                    "state": "OPEN",
                    "statusCheckRollup": [
                        {
                            "__typename": "StatusContext",
                            "completedAt": "2026-06-01T00:02:00Z",
                            "conclusion": "success",
                            "context": "ci/test",
                            "description": "tests passed",
                            "detailsUrl": "https://git.example.com/owner/repo/actions/runs/1",
                            "name": "ci/test",
                            "startedAt": "2026-06-01T00:00:00Z",
                            "state": "SUCCESS",
                            "targetUrl": "https://git.example.com/owner/repo/actions/runs/1",
                            "workflowName": "ci/test",
                        }
                    ],
                    "title": "Make Codex happy",
                    "updatedAt": "2026-05-31T01:00:00Z",
                    "url": "https://git.example.com/owner/repo/pulls/8",
                }
            ],
        )

    def test_pr_list_accepts_merged_state(self) -> None:
        out = io.StringIO()
        code = run_forgejo_pr(
            [
                "pr",
                "list",
                "-R",
                "git.example.com/owner/repo",
                "--state",
                "merged",
                "--json",
                "number,state",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(
                [
                    {"number": 8, "state": "closed", "merged_at": "2026-05-31T01:00:00Z"},
                    {"number": 9, "state": "closed"},
                ]
            ),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out.getvalue()), [{"number": 8, "state": "MERGED"}])

    def test_pr_checks_maps_forgejo_statuses_to_github_shaped_json(self) -> None:
        out = io.StringIO()
        code = run_forgejo_pr(
            [
                "pr",
                "checks",
                "8",
                "-R",
                "git.example.com/owner/repo",
                "--json",
                "bucket,description,link,name,workflow",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(
                pulls=[
                    {
                        "number": 8,
                        "head": {"ref": "feature", "sha": "abc123"},
                    }
                ],
                statuses=[
                    {
                        "context": "ci/test",
                        "description": "tests passed",
                        "state": "success",
                        "target_url": "https://git.example.com/owner/repo/actions/runs/1",
                        "created_at": "2026-06-01T00:00:00Z",
                        "updated_at": "2026-06-01T00:02:00Z",
                    },
                    {
                        "context": "lint",
                        "description": "lint is running",
                        "state": "pending",
                        "created_at": "2026-06-01T00:01:00Z",
                    },
                ],
            ),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            [
                {
                    "bucket": "pass",
                    "description": "tests passed",
                    "link": "https://git.example.com/owner/repo/actions/runs/1",
                    "name": "ci/test",
                    "workflow": "ci/test",
                },
                {
                    "bucket": "pending",
                    "description": "lint is running",
                    "link": None,
                    "name": "lint",
                    "workflow": "lint",
                },
            ],
        )

    def test_pr_diff_outputs_forgejo_diff(self) -> None:
        out = io.StringIO()
        code = run_forgejo_pr(
            ["pr", "diff", "8"],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(
                [
                    {
                        "number": 8,
                        "title": "Make Codex happy",
                        "state": "open",
                        "html_url": "https://git.example.com/owner/repo/pulls/8",
                        "head": {"ref": "feature"},
                    }
                ]
            ),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertIn("diff --git", out.getvalue())

    def test_pr_diff_name_only_filters_excluded_files(self) -> None:
        out = io.StringIO()
        code = run_forgejo_pr(
            ["pr", "diff", "8", "--name-only", "--exclude", "generated/*"],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient([{"number": 8, "head": {"ref": "feature"}}]),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().splitlines(), ["README.md"])

    def test_pr_comment_posts_body_to_issue_comments(self) -> None:
        client = FakeClient([{"number": 8, "head": {"ref": "feature"}}])
        out = io.StringIO()
        code = run_forgejo_pr(
            ["pr", "comment", "8", "--body", "Looks good"],
            RepoRef("git.example.com", "owner", "repo"),
            client,
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue(), "")
        self.assertEqual(client.comments[0]["number"], 8)
        self.assertEqual(client.comments[0]["body"], "Looks good")

    def test_pr_checkout_fetches_and_checks_out_head_branch(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            remote = root / "remote.git"
            seed = root / "seed"
            work = root / "work"
            self._git(root, "init", "--bare", str(remote))

            seed.mkdir()
            self._git(seed, "init")
            self._git(seed, "config", "user.email", "test@example.com")
            self._git(seed, "config", "user.name", "Test User")
            (seed / "README.md").write_text("main\n", encoding="utf-8")
            self._git(seed, "add", "README.md")
            self._git(seed, "commit", "-m", "initial")
            self._git(seed, "branch", "-M", "main")
            self._git(seed, "remote", "add", "origin", str(remote))
            self._git(seed, "push", "-u", "origin", "main")
            self._git(seed, "checkout", "-b", "feature")
            (seed / "README.md").write_text("feature\n", encoding="utf-8")
            self._git(seed, "commit", "-am", "feature")
            self._git(seed, "push", "origin", "feature")
            self._git(root, "--git-dir", str(remote), "symbolic-ref", "HEAD", "refs/heads/main")
            self._git(root, "clone", str(remote), str(work))
            self._git(work, "checkout", "main")

            out = io.StringIO()
            code = run_forgejo_pr(
                ["pr", "checkout", "8"],
                RepoRef("git.example.com", "owner", "repo"),
                FakeClient([{"number": 8, "head": {"ref": "feature"}}]),
                cwd=str(work),
                stdout=out,
                stderr=io.StringIO(),
            )
            self.assertEqual(code, 0)
            self.assertEqual(self._git_output(work, "branch", "--show-current"), "feature")

    def test_repo_view_outputs_github_shaped_json(self) -> None:
        out = io.StringIO()
        code = run_forgejo(
            [
                "repo",
                "view",
                "-R",
                "git.example.com/owner/repo",
                "--json",
                "nameWithOwner,url,defaultBranchRef,sshUrl",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            {
                "defaultBranchRef": {"name": "main"},
                "nameWithOwner": "owner/repo",
                "sshUrl": "git@git.example.com:owner/repo.git",
                "url": "https://git.example.com/owner/repo",
            },
        )

    def test_repo_view_outputs_richer_github_json_fields(self) -> None:
        out = io.StringIO()
        code = run_forgejo(
            [
                "repo",
                "view",
                "-R",
                "git.example.com/owner/repo",
                "--json",
                "id,hasIssuesEnabled,hasProjectsEnabled,hasWikiEnabled,forkCount,stargazerCount,watchers,primaryLanguage,viewerPermission,visibility,pullRequests,issues,mergeCommitAllowed,rebaseMergeAllowed,squashMergeAllowed",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            {
                "forkCount": 2,
                "hasIssuesEnabled": True,
                "hasProjectsEnabled": True,
                "hasWikiEnabled": False,
                "id": 99,
                "issues": {"nodes": [], "totalCount": 5},
                "mergeCommitAllowed": True,
                "primaryLanguage": {"name": "Python"},
                "pullRequests": {"nodes": [], "totalCount": 6},
                "rebaseMergeAllowed": True,
                "squashMergeAllowed": True,
                "stargazerCount": 3,
                "viewerPermission": "WRITE",
                "visibility": "PUBLIC",
                "watchers": {"nodes": [], "totalCount": 4},
            },
        )

    def test_issue_list_outputs_github_shaped_json(self) -> None:
        out = io.StringIO()
        code = run_forgejo(
            [
                "issue",
                "list",
                "-R",
                "git.example.com/owner/repo",
                "--state",
                "all",
                "--json",
                "number,title,state,url,author,comments",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(
                issues=[
                    {
                        "number": 13,
                        "title": "Fix it",
                        "state": "open",
                        "html_url": "https://git.example.com/owner/repo/issues/13",
                        "user": {"login": "alice"},
                        "comments": 2,
                    }
                ]
            ),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            [
                {
                    "author": {"login": "alice", "name": None},
                    "comments": 2,
                    "number": 13,
                    "state": "OPEN",
                    "title": "Fix it",
                    "url": "https://git.example.com/owner/repo/issues/13",
                }
            ],
        )

    def test_issue_view_outputs_github_shaped_json(self) -> None:
        out = io.StringIO()
        code = run_forgejo(
            [
                "issue",
                "view",
                "13",
                "-R",
                "git.example.com/owner/repo",
                "--json",
                "number,title,body,state,closed",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            FakeClient(
                issues=[
                    {
                        "number": 13,
                        "title": "Fix it",
                        "body": "Details",
                        "state": "closed",
                        "html_url": "https://git.example.com/owner/repo/issues/13",
                    }
                ]
            ),
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            {
                "body": "Details",
                "closed": True,
                "number": 13,
                "state": "CLOSED",
                "title": "Fix it",
            },
        )

    def test_issue_create_calls_forgejo_client(self) -> None:
        client = FakeClient()
        out = io.StringIO()
        code = run_forgejo(
            [
                "issue",
                "create",
                "-R",
                "git.example.com/owner/repo",
                "--title",
                "Bug",
                "--body",
                "It broke",
                "--assignee",
                "alice,bob",
                "--label",
                "bug,3",
            ],
            RepoRef("git.example.com", "owner", "repo"),
            client,
            stdout=out,
            stderr=io.StringIO(),
        )
        self.assertEqual(code, 0)
        self.assertIsNotNone(client.created_issue)
        assert client.created_issue is not None
        self.assertEqual(client.created_issue["title"], "Bug")
        self.assertEqual(client.created_issue["body"], "It broke")
        self.assertEqual(client.created_issue["assignees"], ("alice", "bob"))
        self.assertEqual(client.created_issue["labels"], (2, 3))
        self.assertEqual(out.getvalue().strip(), "https://git.example.com/owner/repo/issues/14")

    def test_no_current_branch_view_returns_empty_json_success(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                ["pr", "view", "--json", "number,title"],
                RepoRef("git.example.com", "owner", "repo"),
                FakeClient([]),
                cwd=str(cwd),
                stdout=out,
                stderr=io.StringIO(),
            )
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().strip(), "{}")

    def test_no_current_branch_status_returns_empty_status_success(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                ["pr", "status", "--json", "number,title"],
                RepoRef("git.example.com", "owner", "repo"),
                FakeClient([]),
                cwd=str(cwd),
                stdout=out,
                stderr=io.StringIO(),
            )
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out.getvalue()), {"createdBy": [], "currentBranch": None, "needsReview": []})

    def test_current_branch_status_returns_github_status_envelope(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                ["pr", "status", "--json", "number,title,url,headRefName"],
                RepoRef("git.example.com", "owner", "repo"),
                FakeClient(
                    [
                        {
                            "number": 12,
                            "title": "Ship it",
                            "state": "open",
                            "html_url": "https://git.example.com/owner/repo/pulls/12",
                            "head": {"ref": "feature"},
                            "base": {"ref": "main"},
                            "user": {"login": "alice"},
                        }
                    ]
                ),
                cwd=str(cwd),
                stdout=out,
                stderr=io.StringIO(),
            )
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(out.getvalue()),
            {
                "createdBy": [],
                "currentBranch": {
                    "headRefName": "feature",
                    "number": 12,
                    "title": "Ship it",
                    "url": "https://git.example.com/owner/repo/pulls/12",
                },
                "needsReview": [],
            },
        )

    def test_status_supports_simple_jq_field_selection(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                [
                    "pr",
                    "status",
                    "--json",
                    "number,title,url",
                    "--jq",
                    ".currentBranch.url",
                ],
                RepoRef("git.example.com", "owner", "repo"),
                FakeClient(
                    [
                        {
                            "number": 12,
                            "title": "Ship it",
                            "state": "open",
                            "html_url": "https://git.example.com/owner/repo/pulls/12",
                            "head": {"ref": "feature"},
                            "base": {"ref": "main"},
                            "user": {"login": "alice"},
                        }
                    ]
                ),
                cwd=str(cwd),
                stdout=out,
                stderr=io.StringIO(),
            )
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().strip(), "https://git.example.com/owner/repo/pulls/12")

    def test_pr_view_supports_simple_template_url_selection(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                [
                    "pr",
                    "view",
                    "--json",
                    "number,title,url",
                    "--template",
                    "{{.url}}",
                ],
                RepoRef("git.example.com", "owner", "repo"),
                FakeClient(
                    [
                        {
                            "number": 12,
                            "title": "Ship it",
                            "state": "open",
                            "html_url": "https://git.example.com/owner/repo/pulls/12",
                            "head": {"ref": "feature"},
                        }
                    ]
                ),
                cwd=str(cwd),
                stdout=out,
                stderr=io.StringIO(),
            )
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue(), "https://git.example.com/owner/repo/pulls/12")

    def test_pr_status_supports_nested_template_url_selection(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                [
                    "pr",
                    "status",
                    "--json",
                    "number,title,url",
                    "--template",
                    "{{.currentBranch.url}}",
                ],
                RepoRef("git.example.com", "owner", "repo"),
                FakeClient(
                    [
                        {
                            "number": 12,
                            "title": "Ship it",
                            "state": "open",
                            "html_url": "https://git.example.com/owner/repo/pulls/12",
                            "head": {"ref": "feature"},
                        }
                    ]
                ),
                cwd=str(cwd),
                stdout=out,
                stderr=io.StringIO(),
            )
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue(), "https://git.example.com/owner/repo/pulls/12")

    def test_pr_status_supports_jq_value_template_selection(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cwd = Path(tmp)
            self._git(cwd, "init")
            self._git(cwd, "checkout", "-b", "feature")
            out = io.StringIO()
            code = run_forgejo_pr(
                [
                    "pr",
                    "status",
                    "--json",
                    "number,title,url",
                    "--jq",
                    ".currentBranch.url",
                    "--template",
                    "{{.}}",
                ],
                RepoRef("git.example.com", "owner", "repo"),
                FakeClient(
                    [
                        {
                            "number": 12,
                            "title": "Ship it",
                            "state": "open",
                            "html_url": "https://git.example.com/owner/repo/pulls/12",
                            "head": {"ref": "feature"},
                        }
                    ]
                ),
                cwd=str(cwd),
                stdout=out,
                stderr=io.StringIO(),
            )
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue(), "https://git.example.com/owner/repo/pulls/12")

    def _git(self, cwd: Path, *args: str) -> None:
        import subprocess

        subprocess.run(["git", *args], cwd=cwd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def _git_output(self, cwd: Path, *args: str) -> str:
        import subprocess

        return subprocess.check_output(["git", *args], cwd=cwd, text=True, stderr=subprocess.DEVNULL).strip()


if __name__ == "__main__":
    unittest.main()
