from __future__ import annotations

import os
import re
import urllib.parse
from dataclasses import dataclass
from typing import Mapping

from .config import normalize_host
from .external import git_output
from .forgejo import RepoRef


@dataclass(frozen=True)
class Detection:
    repo: RepoRef | None
    source: str


def parse_repo_spec(spec: str, *, default_host: str | None = None) -> RepoRef | None:
    value = spec.strip()
    if not value:
        return None

    parsed = urllib.parse.urlparse(value)
    if parsed.scheme in {"http", "https", "ssh", "git"} and parsed.netloc:
        host = parsed.hostname or parsed.netloc.split("@")[-1]
        parts = _path_parts(parsed.path)
        if len(parts) >= 2:
            return RepoRef(normalize_host(host), parts[-2], _strip_git(parts[-1]))
        return None

    scp_match = re.match(r"^(?:[^@/\s]+@)?([^:/\s]+):/?(.+)$", value)
    if scp_match and "/" in scp_match.group(2):
        host = scp_match.group(1)
        parts = _path_parts(scp_match.group(2))
        if len(parts) >= 2:
            return RepoRef(normalize_host(host), parts[-2], _strip_git(parts[-1]))

    parts = _path_parts(value)
    if len(parts) >= 3 and _looks_like_host(parts[0]):
        return RepoRef(normalize_host(parts[0]), parts[-2], _strip_git(parts[-1]))
    if len(parts) == 2 and default_host:
        return RepoRef(normalize_host(default_host), parts[0], _strip_git(parts[1]))
    return None


def detect_repo(
    argv: list[str],
    *,
    env: Mapping[str, str] | None = None,
    cwd: str | None = None,
) -> Detection:
    values = env if env is not None else os.environ

    repo_arg = extract_repo_arg(argv)
    if repo_arg:
        repo = parse_repo_spec(repo_arg, default_host=values.get("GH_HOST"))
        if repo:
            return Detection(repo, "-R/--repo")

    gh_repo = values.get("GH_REPO")
    if gh_repo:
        repo = parse_repo_spec(gh_repo, default_host=values.get("GH_HOST"))
        if repo:
            return Detection(repo, "GH_REPO")

    remote_repo = detect_from_git(cwd=cwd)
    if values.get("GH_HOST") and remote_repo:
        repo = RepoRef(normalize_host(values["GH_HOST"]), remote_repo.owner, remote_repo.repo)
        return Detection(repo, "GH_HOST")

    if remote_repo:
        return Detection(remote_repo, "git remote")

    return Detection(None, "unknown")


def detect_from_git(*, cwd: str | None = None) -> RepoRef | None:
    remote = git_output(["remote", "get-url", "origin"], cwd=cwd)
    if remote:
        repo = parse_repo_spec(remote)
        if repo:
            return repo

    remotes = git_output(["remote"], cwd=cwd)
    if not remotes:
        return None
    for name in remotes.splitlines():
        remote = git_output(["remote", "get-url", name], cwd=cwd)
        if not remote:
            continue
        repo = parse_repo_spec(remote)
        if repo:
            return repo
    return None


def extract_repo_arg(argv: list[str]) -> str | None:
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in {"-R", "--repo"} and index + 1 < len(argv):
            return argv[index + 1]
        if arg.startswith("--repo="):
            return arg.split("=", 1)[1]
        index += 1
    return None


def current_branch(*, cwd: str | None = None) -> str | None:
    return git_output(["branch", "--show-current"], cwd=cwd)


def default_base_branch(*, cwd: str | None = None) -> str:
    origin_head = git_output(["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], cwd=cwd)
    if origin_head and "/" in origin_head:
        return origin_head.split("/", 1)[1]
    return "main"


def fill_title(*, cwd: str | None = None) -> str | None:
    return git_output(["log", "-1", "--pretty=%s"], cwd=cwd) or current_branch(cwd=cwd)


def fill_body(*, verbose: bool = False, cwd: str | None = None) -> str:
    if not verbose:
        return ""
    return git_output(["log", "-1", "--pretty=%b"], cwd=cwd) or ""


def _path_parts(path: str) -> list[str]:
    stripped = path.strip("/")
    if not stripped:
        return []
    return [part for part in stripped.split("/") if part]


def _strip_git(value: str) -> str:
    return value[:-4] if value.endswith(".git") else value


def _looks_like_host(value: str) -> bool:
    return "." in value or ":" in value or value in {"localhost"}
