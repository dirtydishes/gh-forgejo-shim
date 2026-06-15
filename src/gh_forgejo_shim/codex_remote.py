from __future__ import annotations

import shlex
from dataclasses import dataclass

from .external import git_output, git_run
from .repo import parse_repo_spec


@dataclass(frozen=True)
class CodexRemoteCheck:
    ok: bool
    detail: str
    repair_commands: tuple[str, ...] = ()


@dataclass(frozen=True)
class CodexRemoteRepair:
    ok: bool
    changed: bool
    detail: str


def check_codex_upstream_remote(*, cwd: str | None = None) -> CodexRemoteCheck:
    upstream_url = git_output(["config", "--get", "remote.upstream.url"], cwd=cwd)
    if upstream_url:
        return CodexRemoteCheck(True, f"remote.upstream.url points at {upstream_url}")

    origin_url = git_output(["remote", "get-url", "origin"], cwd=cwd)
    if not origin_url:
        return CodexRemoteCheck(
            False,
            "remote.upstream.url is missing and origin remote is unavailable",
        )
    if not parse_repo_spec(origin_url):
        return CodexRemoteCheck(
            False,
            f"remote.upstream.url is missing and origin does not look like a supported repo URL: {origin_url}",
        )

    return CodexRemoteCheck(
        False,
        "remote.upstream.url is missing; Codex.app PR status probes expect it",
        (f"git remote add upstream {shlex.quote(origin_url)}",),
    )


def repair_codex_upstream_remote(*, cwd: str | None = None) -> CodexRemoteRepair:
    check = check_codex_upstream_remote(cwd=cwd)
    if check.ok:
        return CodexRemoteRepair(True, False, check.detail)

    origin_url = git_output(["remote", "get-url", "origin"], cwd=cwd)
    if not origin_url:
        return CodexRemoteRepair(False, False, "origin remote is missing")
    if not parse_repo_spec(origin_url):
        return CodexRemoteRepair(False, False, f"origin does not look like a supported repo URL: {origin_url}")

    code, message = git_run(["remote", "add", "upstream", origin_url], cwd=cwd)
    if code != 0:
        return CodexRemoteRepair(False, False, message or "git remote add upstream failed")

    origin_push_url = git_output(["remote", "get-url", "--push", "origin"], cwd=cwd)
    if origin_push_url and origin_push_url != origin_url:
        code, message = git_run(["remote", "set-url", "--push", "upstream", origin_push_url], cwd=cwd)
        if code != 0:
            return CodexRemoteRepair(False, True, message or "git remote set-url --push upstream failed")
        return CodexRemoteRepair(
            True,
            True,
            f"added upstream remote alias for Codex.app: {origin_url} with push URL {origin_push_url}",
        )
    return CodexRemoteRepair(True, True, f"added upstream remote alias for Codex.app: {origin_url}")
