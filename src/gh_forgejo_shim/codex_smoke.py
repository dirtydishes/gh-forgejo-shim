from __future__ import annotations

import os
import subprocess
import time
from dataclasses import dataclass
from typing import Callable, Mapping, Sequence


CODEX_PR_BOARD_FIELDS = (
    "additions,baseRefName,createdAt,deletions,headRefName,isDraft,"
    "number,state,title,updatedAt,url,mergeStateStatus,mergeable,statusCheckRollup"
)


@dataclass(frozen=True)
class SmokeProbe:
    name: str
    command: tuple[str, ...]
    optional: bool = False


@dataclass(frozen=True)
class SmokeResult:
    probe: SmokeProbe
    exit_code: int
    duration_ms: float
    stdout_bytes: int
    stderr_bytes: int
    stderr_excerpt: str = ""

    @property
    def ok(self) -> bool:
        return self.exit_code == 0 or self.probe.optional


Runner = Callable[..., subprocess.CompletedProcess[str]]


def codex_smoke_probes() -> tuple[SmokeProbe, ...]:
    return (
        SmokeProbe("gh version", ("gh", "--version")),
        SmokeProbe("gh auth status", ("gh", "auth", "status")),
        SmokeProbe("gh api user", ("gh", "api", "user")),
        SmokeProbe(
            "gh pr status",
            (
                "gh",
                "pr",
                "status",
                "--json",
                "number,title,url,headRefName,statusCheckRollup",
            ),
            optional=True,
        ),
        SmokeProbe(
            "gh pr list",
            (
                "gh",
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                "10",
                "--json",
                CODEX_PR_BOARD_FIELDS,
            ),
            optional=True,
        ),
        SmokeProbe("git repo root", ("git", "rev-parse", "--show-toplevel")),
        SmokeProbe("git remotes", ("git", "remote", "-v")),
        SmokeProbe(
            "git default branch",
            ("git", "symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"),
            optional=True,
        ),
        SmokeProbe("git current branch", ("git", "branch", "--show-current"), optional=True),
        SmokeProbe(
            "git upstream",
            ("git", "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"),
            optional=True,
        ),
        SmokeProbe(
            "git local and remote refs",
            (
                "git",
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads",
                "refs/remotes",
            ),
            optional=True,
        ),
    )


def run_codex_smoke(
    *,
    cwd: str | None = None,
    env: Mapping[str, str] | None = None,
    runner: Runner = subprocess.run,
) -> list[SmokeResult]:
    values = dict(os.environ if env is None else env)
    results: list[SmokeResult] = []
    for probe in codex_smoke_probes():
        started = time.perf_counter()
        try:
            completed = runner(
                list(probe.command),
                cwd=cwd,
                env=values,
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            exit_code = int(completed.returncode)
            stdout = completed.stdout or ""
            stderr = completed.stderr or ""
        except OSError as exc:
            exit_code = 127
            stdout = ""
            stderr = str(exc)
        results.append(
            SmokeResult(
                probe=probe,
                exit_code=exit_code,
                duration_ms=(time.perf_counter() - started) * 1000,
                stdout_bytes=len(stdout.encode("utf-8")),
                stderr_bytes=len(stderr.encode("utf-8")),
                stderr_excerpt=_excerpt(stderr),
            )
        )
    return results


def format_codex_smoke(results: Sequence[SmokeResult]) -> str:
    lines = ["Codex probe smoke results:"]
    for result in results:
        marker = "ok" if result.exit_code == 0 else ("optional-fail" if result.probe.optional else "fail")
        lines.append(
            f"[{marker}] {result.probe.name}: exit {result.exit_code}, "
            f"{result.duration_ms:.0f} ms, stdout {result.stdout_bytes} B, stderr {result.stderr_bytes} B"
        )
        if result.stderr_excerpt:
            lines.append(f"      stderr: {result.stderr_excerpt}")
    required_failures = [result for result in results if result.exit_code != 0 and not result.probe.optional]
    if required_failures:
        lines.append(f"Required probe failures: {len(required_failures)}")
    else:
        lines.append("Required probes passed.")
    return "\n".join(lines)


def _excerpt(value: str, *, limit: int = 240) -> str:
    compact = " ".join(value.split())
    if len(compact) <= limit:
        return compact
    return compact[: limit - 1] + "…"
