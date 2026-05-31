from __future__ import annotations

import unittest

from gh_forgejo_shim.repo import parse_repo_spec


class RepoParsingTests(unittest.TestCase):
    def test_https_url(self) -> None:
        repo = parse_repo_spec("https://git.example.com/owner/repo.git")
        self.assertIsNotNone(repo)
        assert repo is not None
        self.assertEqual((repo.host, repo.owner, repo.repo), ("git.example.com", "owner", "repo"))

    def test_ssh_url(self) -> None:
        repo = parse_repo_spec("ssh://git@git.example.com/owner/repo.git")
        self.assertIsNotNone(repo)
        assert repo is not None
        self.assertEqual((repo.host, repo.owner, repo.repo), ("git.example.com", "owner", "repo"))

    def test_scp_style_ssh(self) -> None:
        repo = parse_repo_spec("git@git.example.com:owner/repo.git")
        self.assertIsNotNone(repo)
        assert repo is not None
        self.assertEqual((repo.host, repo.owner, repo.repo), ("git.example.com", "owner", "repo"))

    def test_host_prefixed_owner_repo(self) -> None:
        repo = parse_repo_spec("git.example.com/owner/repo")
        self.assertIsNotNone(repo)
        assert repo is not None
        self.assertEqual((repo.host, repo.owner, repo.repo), ("git.example.com", "owner", "repo"))

    def test_plain_owner_repo_uses_default_host(self) -> None:
        repo = parse_repo_spec("owner/repo", default_host="git.example.com")
        self.assertIsNotNone(repo)
        assert repo is not None
        self.assertEqual((repo.host, repo.owner, repo.repo), ("git.example.com", "owner", "repo"))

    def test_plain_owner_repo_without_default_host_is_unknown(self) -> None:
        self.assertIsNone(parse_repo_spec("owner/repo"))

    def test_web_pull_url_uses_repository_path_not_trailing_segments(self) -> None:
        repo = parse_repo_spec("https://git.example.com/owner/repo/pulls/7")
        self.assertIsNotNone(repo)
        assert repo is not None
        self.assertEqual((repo.host, repo.owner, repo.repo), ("git.example.com", "owner", "repo"))

    def test_web_issue_url_uses_repository_path_not_trailing_segments(self) -> None:
        repo = parse_repo_spec("https://git.example.com/owner/repo/issues/13")
        self.assertIsNotNone(repo)
        assert repo is not None
        self.assertEqual((repo.host, repo.owner, repo.repo), ("git.example.com", "owner", "repo"))


if __name__ == "__main__":
    unittest.main()
