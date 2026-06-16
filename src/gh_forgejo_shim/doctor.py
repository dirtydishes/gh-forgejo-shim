from __future__ import annotations

import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

from .auth import discover_fj_token
from .config import Config, is_known_github_host, load_config
from .external import find_program
from .gui_path import current_launchd_path, path_contains_dir
from .repo import detect_from_git
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
    cwd: str | None = None,
    fallback_dirs: Sequence[str] | None = None,
    launchd_path: str | None = None,
    check_gui_path: bool | None = None,
    check_current_repo: bool | None = None,
) -> list[Check]:
    values = env if env is not None else os.environ
    should_check_current_repo = check_current_repo if check_current_repo is not None else config is None
    cfg = config or load_config(env=values)
    target_bin_dir = bin_dir or default_bin_dir()
    wrapper = shim_path(target_bin_dir)

    if fallback_dirs is None:
        real_gh = find_program("gh", configured=cfg.paths.gh, env=values)
        real_fj = find_program("fj", configured=cfg.paths.fj, env=values)
    else:
        real_gh = find_program("gh", configured=cfg.paths.gh, env=values, fallback_dirs=fallback_dirs)
        real_fj = find_program("fj", configured=cfg.paths.fj, env=values, fallback_dirs=fallback_dirs)
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
        _auth_check(cfg.hosts, values, home=home),
    ]

    if should_check_current_repo:
        repo = detect_from_git(cwd=cwd)
        if repo and cfg.is_forgejo_host(repo.host):
            checks.append(
                Check(
                    "current repo host",
                    True,
                    f"{repo.host} is allowlisted for {repo.owner}/{repo.repo}",
                )
            )
        elif repo and is_known_github_host(repo.host):
            checks.append(
                Check(
                    "current repo host",
                    True,
                    f"{repo.host} will delegate to the real GitHub CLI for {repo.owner}/{repo.repo}",
                )
            )
        elif repo:
            checks.append(
                Check(
                    "current repo host",
                    False,
                    f"{repo.host} is not allowlisted; run gh-forgejo-shim config add-host {repo.host}",
                )
            )

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

    if check_gui_path if check_gui_path is not None else sys.platform == "darwin":
        gui_path = launchd_path if launchd_path is not None else current_launchd_path()
        if gui_path and path_contains_dir(gui_path, target_bin_dir):
            checks.append(Check("macOS gui PATH", True, f"{target_bin_dir} is visible to new GUI apps"))
        elif gui_path:
            checks.append(
                Check(
                    "macOS gui PATH",
                    False,
                    f"{target_bin_dir} is not in launchd PATH; run gh-forgejo-shim install-gui-path",
                )
            )
        else:
            checks.append(
                Check(
                    "macOS gui PATH",
                    False,
                    "launchd PATH is unset; run gh-forgejo-shim install-gui-path if GUI apps cannot find gh",
                )
            )

    return checks


def _auth_check(hosts: tuple[str, ...], env: Mapping[str, str], *, home: Path | None) -> Check:
    if not hosts:
        return Check("auth token", False, "add a Forgejo host before checking auth")

    found: list[str] = []
    missing: list[str] = []
    for host in hosts:
        token = discover_fj_token(host, env=env, home=home)
        if token:
            found.append(host)
        else:
            missing.append(host)

    if not missing:
        return Check("auth token", True, f"found for {', '.join(found)}")

    repair = "; ".join(f"gh-forgejo-shim auth login {host}" for host in missing)
    if found:
        detail = f"found for {', '.join(found)}; missing for {', '.join(missing)}; run {repair}"
    else:
        detail = f"missing for {', '.join(missing)}; run {repair} or set FJ_SHIM_TOKEN"
    return Check("auth token", False, detail)


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
