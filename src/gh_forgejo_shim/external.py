from __future__ import annotations

import os
import subprocess
from pathlib import Path
from typing import Mapping, Sequence

from .shim import is_managed_shim

DEFAULT_FALLBACK_DIRS = (
    "~/.local/bin",
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/opt/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
)


def find_program(
    name: str,
    *,
    configured: str | None = None,
    env: Mapping[str, str] | None = None,
    fallback_dirs: Sequence[str] = DEFAULT_FALLBACK_DIRS,
) -> str | None:
    if configured:
        path = Path(configured).expanduser()
        if path.exists() and os.access(path, os.X_OK):
            if name == "gh" and is_managed_shim(path):
                configured = None
            else:
                return str(path)
        if configured:
            return None

    values = env if env is not None else os.environ
    for directory in _candidate_dirs(values, fallback_dirs):
        candidate = Path(directory).expanduser() / name
        if not candidate.exists() or not os.access(candidate, os.X_OK):
            continue
        if name == "gh" and is_managed_shim(candidate):
            continue
        return str(candidate)
    return None


def _candidate_dirs(values: Mapping[str, str], fallback_dirs: Sequence[str]) -> list[str]:
    home = values.get("HOME")
    seen: set[str] = set()
    result: list[str] = []
    for directory in [*values.get("PATH", "").split(os.pathsep), *fallback_dirs]:
        if not directory:
            continue
        if directory.startswith("~/") and home:
            directory = str(Path(home) / directory[2:])
        normalized = str(Path(directory).expanduser())
        if normalized in seen:
            continue
        seen.add(normalized)
        result.append(normalized)
    return result


def run_program(path: str, argv: list[str]) -> int:
    try:
        return subprocess.call([path, *argv])
    except FileNotFoundError:
        return 127


def git_output(args: list[str], *, cwd: str | None = None) -> str | None:
    try:
        completed = subprocess.run(
            ["git", *args],
            cwd=cwd,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
    except OSError:
        return None
    if completed.returncode != 0:
        return None
    value = completed.stdout.strip()
    return value or None


def git_run(args: list[str], *, cwd: str | None = None) -> tuple[int, str]:
    try:
        completed = subprocess.run(
            ["git", *args],
            cwd=cwd,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
    except OSError as exc:
        return 127, str(exc)
    message = completed.stderr.strip() or completed.stdout.strip()
    return completed.returncode, message
