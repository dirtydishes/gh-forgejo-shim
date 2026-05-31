from __future__ import annotations

import os
import subprocess
from pathlib import Path
from typing import Mapping

from .shim import is_managed_shim


def find_program(
    name: str,
    *,
    configured: str | None = None,
    env: Mapping[str, str] | None = None,
) -> str | None:
    if configured:
        path = Path(configured).expanduser()
        if path.exists() and os.access(path, os.X_OK):
            return str(path)
        return None

    values = env if env is not None else os.environ
    for directory in values.get("PATH", "").split(os.pathsep):
        if not directory:
            continue
        candidate = Path(directory).expanduser() / name
        if not candidate.exists() or not os.access(candidate, os.X_OK):
            continue
        if name == "gh" and is_managed_shim(candidate):
            continue
        return str(candidate)
    return None


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
