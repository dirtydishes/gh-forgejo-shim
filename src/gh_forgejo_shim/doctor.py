from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

from .auth import discover_fj_token
from .config import Config, load_config
from .external import find_program
from .shim import default_bin_dir, is_managed_shim, shim_path


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

    if wrapper.exists() and is_managed_shim(wrapper):
        first_gh = _first_program("gh", values.get("PATH", ""))
        if first_gh and _same_file(first_gh, wrapper):
            detail = f"{wrapper} is the first gh in PATH"
            ok = True
        elif first_gh:
            detail = f"gh resolves to {first_gh} before {wrapper}"
            ok = False
        else:
            detail = f"{wrapper} exists but no gh executable was found in PATH"
            ok = False
    elif wrapper.exists():
        detail = f"{wrapper} exists but is not managed by gh-forgejo-shim"
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


def _first_program(name: str, path_value: str) -> Path | None:
    for directory in path_value.split(os.pathsep):
        if not directory:
            continue
        candidate = Path(directory).expanduser() / name
        if candidate.exists() and os.access(candidate, os.X_OK):
            return candidate
    return None


def _same_file(left: Path, right: Path) -> bool:
    try:
        return left.samefile(right)
    except OSError:
        return left.resolve() == right.resolve()
