from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

from .auth import discover_fj_token
from .config import Config, load_config
from .external import find_program
from .shim import default_bin_dir, shim_path


@dataclass(frozen=True)
class Check:
    name: str
    ok: bool
    detail: str


def run_checks(
    *,
    config: Config | None = None,
    env: Mapping[str, str] | None = None,
    bin_dir: Path | None = None,
    home: Path | None = None,
) -> list[Check]:
    values = env if env is not None else os.environ
    cfg = config or load_config(env=values)
    target_bin_dir = bin_dir or default_bin_dir()
    wrapper = shim_path(target_bin_dir)

    real_gh = find_program("gh", configured=cfg.paths.gh, env=values)
    real_fj = find_program("fj", configured=cfg.paths.fj, env=values)
    token = discover_fj_token(next(iter(cfg.hosts), None), env=values, home=home)

    checks = [
        Check(
            "real gh",
            real_gh is not None,
            real_gh or "set FJ_SHIM_REAL_GH or install GitHub CLI",
        ),
        Check(
            "fj",
            real_fj is not None,
            real_fj or "set FJ_SHIM_REAL_FJ or install fj",
        ),
        Check(
            "forgejo hosts",
            bool(cfg.hosts),
            ", ".join(cfg.hosts) if cfg.hosts else "add one with gh-forgejo-shim config add-host HOST",
        ),
        Check(
            "auth token",
            token is not None,
            "found" if token else "set FJ_SHIM_TOKEN, FORGEJO_TOKEN, GITEA_TOKEN, or FJ_TOKEN",
        ),
    ]

    path_entries = values.get("PATH", "").split(os.pathsep)
    bin_dir_string = str(target_bin_dir)
    if wrapper.exists():
        if path_entries and path_entries[0] == bin_dir_string:
            detail = f"{bin_dir_string} is first in PATH"
            ok = True
        elif bin_dir_string in path_entries:
            detail = f"{bin_dir_string} is in PATH but not first"
            ok = False
        else:
            detail = f"{bin_dir_string} is not in PATH"
            ok = False
    else:
        detail = f"{wrapper} is not installed"
        ok = False
    checks.append(Check("shim path", ok, detail))

    return checks


def format_checks(checks: list[Check]) -> str:
    lines = ["gh-forgejo-shim doctor"]
    for check in checks:
        mark = "ok" if check.ok else "warn"
        lines.append(f"[{mark}] {check.name}: {check.detail}")
    return "\n".join(lines)
