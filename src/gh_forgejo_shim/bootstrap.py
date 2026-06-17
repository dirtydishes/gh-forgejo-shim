from __future__ import annotations

import os
import shlex
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

from .auth import discover_fj_token
from .config import add_host, load_config
from .external import git_output
from .gui_path import current_launchd_path, path_contains_dir
from .repo import RepoRef, current_branch, detect_from_git, parse_repo_spec
from .shim import install_shim, is_managed_shim, shim_path


@dataclass(frozen=True)
class BootstrapCheck:
    name: str
    ok: bool
    detail: str
    repair_commands: tuple[str, ...] = ()


@dataclass(frozen=True)
class BootstrapResult:
    repo: RepoRef | None
    config_path: Path
    shim: Path | None
    checks: tuple[BootstrapCheck, ...]

    @property
    def ok(self) -> bool:
        return all(check.ok for check in self.checks)


def run_bootstrap(
    *,
    cwd: str | None = None,
    env: Mapping[str, str] | None = None,
    bin_dir: Path | None = None,
    config_path: Path | None = None,
    force: bool = False,
    home: Path | None = None,
    launchd_path: str | None = None,
    check_gui_path: bool | None = None,
) -> BootstrapResult:
    values = env if env is not None else os.environ
    target_bin_dir = bin_dir or Path.home() / ".local" / "bin"
    target_shim = shim_path(target_bin_dir)
    repo = detect_from_git(cwd=cwd)

    checks: list[BootstrapCheck] = []
    config = load_config(config_path, env=values)
    if repo:
        config = add_host(repo.host, config_path)
        checks.append(
            BootstrapCheck(
                "repository",
                True,
                f"detected {repo.host}/{repo.owner}/{repo.repo} and allowlisted {repo.host}",
            )
        )
    else:
        checks.append(
            BootstrapCheck(
                "repository",
                False,
                "could not detect a Forgejo repository from git remotes",
                (
                    "git remote add origin https://git.example.com/owner/repo.git",
                    "git fetch origin",
                    "git remote set-head origin -a",
                    "gh-forgejo-shim config add-host git.example.com",
                ),
            )
        )

    try:
        installed = install_shim(bin_dir=target_bin_dir, force=force)
    except FileExistsError as exc:
        installed = None
        checks.append(
            BootstrapCheck(
                "shim",
                False,
                str(exc),
                (f"gh-forgejo-shim install-shim --bin-dir {shlex.quote(str(target_bin_dir))} --force",),
            )
        )
    else:
        checks.append(BootstrapCheck("shim", True, f"installed {installed}"))

    checks.append(_path_check(target_bin_dir, target_shim, values))
    if check_gui_path if check_gui_path is not None else sys.platform == "darwin":
        checks.append(_gui_path_check(target_bin_dir, launchd_path=launchd_path))
    checks.append(_auth_check(repo, values, home=home))
    checks.extend(_origin_checks(repo, cwd=cwd))

    return BootstrapResult(repo=repo, config_path=config.path, shim=installed, checks=tuple(checks))


def format_bootstrap(result: BootstrapResult) -> str:
    lines = ["gh-forgejo-shim bootstrap"]
    if result.repo:
        lines.append(f"repo: {result.repo.host}/{result.repo.owner}/{result.repo.repo}")
    else:
        lines.append("repo: unknown")
    lines.append(f"config: {result.config_path}")

    repair_commands: list[str] = []
    for check in result.checks:
        mark = "ok" if check.ok else "fix"
        lines.append(f"[{mark}] {check.name}: {check.detail}")
        repair_commands.extend(check.repair_commands)

    if repair_commands:
        lines.append("")
        lines.append("repair commands:")
        for command in _dedupe(repair_commands):
            lines.append(f"  {command}")
    else:
        lines.append("")
        lines.append("setup looks ready")
    return "\n".join(lines)


def _path_check(bin_dir: Path, target_shim: Path, env: Mapping[str, str]) -> BootstrapCheck:
    first_gh = _first_program("gh", env.get("PATH", ""))
    if first_gh and target_shim.exists() and _same_file(first_gh, target_shim) and is_managed_shim(target_shim):
        return BootstrapCheck("PATH", True, f"{target_shim} is the first gh in PATH")

    export = f'export PATH="{bin_dir}:$PATH"'
    if first_gh:
        detail = f"gh currently resolves to {first_gh} before {target_shim}"
    elif str(bin_dir) not in env.get("PATH", "").split(os.pathsep):
        detail = f"{bin_dir} is not in PATH"
    else:
        detail = f"{target_shim} exists but gh was not found in PATH"
    return BootstrapCheck("PATH", False, detail, (export, "gh-forgejo-shim doctor"))


def _gui_path_check(bin_dir: Path, *, launchd_path: str | None) -> BootstrapCheck:
    gui_path = launchd_path if launchd_path is not None else current_launchd_path()
    if gui_path and path_contains_dir(gui_path, bin_dir):
        return BootstrapCheck("macOS GUI PATH", True, f"{bin_dir} is visible to new GUI apps")
    command = "gh-forgejo-shim install-gui-path"
    if gui_path:
        return BootstrapCheck(
            "macOS GUI PATH",
            False,
            f"{bin_dir} is not in launchd PATH; Codex.app will not see the shim after restart",
            (command,),
        )
    return BootstrapCheck(
        "macOS GUI PATH",
        False,
        "launchd PATH is unset; GUI apps may not find gh or the shim after restart",
        (command,),
    )


def _auth_check(repo: RepoRef | None, env: Mapping[str, str], *, home: Path | None = None) -> BootstrapCheck:
    token = discover_fj_token(repo.host if repo else None, env=env, home=home)
    if token:
        return BootstrapCheck("auth", True, "found Forgejo token")
    login_command = f"gh-forgejo-shim auth login {repo.host}" if repo else "gh-forgejo-shim auth login HOST"
    import_command = f"gh-forgejo-shim auth import {repo.host}" if repo else "gh-forgejo-shim auth import HOST"
    return BootstrapCheck(
        "auth",
        False,
        "no Forgejo token found in shim storage, env, or common fj/tea/gitea config files",
        (login_command, import_command, "gh-forgejo-shim doctor"),
    )


def _origin_checks(repo: RepoRef | None, *, cwd: str | None) -> tuple[BootstrapCheck, ...]:
    origin_url = git_output(["remote", "get-url", "origin"], cwd=cwd)
    branch = current_branch(cwd=cwd)
    checks: list[BootstrapCheck] = []

    if origin_url:
        parsed = parse_repo_spec(origin_url)
        if parsed:
            checks.append(BootstrapCheck("origin", True, f"origin points at {origin_url}"))
        else:
            checks.append(
                BootstrapCheck(
                    "origin",
                    False,
                    f"origin exists but does not look like a Forgejo repository URL: {origin_url}",
                    _origin_repair_commands(repo),
                )
            )
    else:
        checks.append(
            BootstrapCheck(
                "origin",
                False,
                "origin remote is missing",
                _origin_repair_commands(repo),
            )
        )

    origin_head = git_output(["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], cwd=cwd)
    if origin_head:
        checks.append(BootstrapCheck("origin/HEAD", True, origin_head))
    else:
        checks.append(
            BootstrapCheck(
                "origin/HEAD",
                False,
                "origin/HEAD is not set",
                ("git fetch origin", "git remote set-head origin -a", "git remote set-head origin main"),
            )
        )

    upstream = git_output(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"], cwd=cwd)
    if upstream:
        checks.append(BootstrapCheck("branch upstream", True, upstream))
    elif branch:
        checks.append(
            BootstrapCheck(
                "branch upstream",
                False,
                f"{branch} does not track an origin branch",
                (f"git branch --set-upstream-to=origin/{branch} {branch}",),
            )
        )
    else:
        checks.append(BootstrapCheck("branch upstream", False, "not on a named branch"))

    return tuple(checks)


def _origin_repair_commands(repo: RepoRef | None) -> tuple[str, ...]:
    if not repo:
        return (
            "git remote add origin https://git.example.com/owner/repo.git",
            "git fetch origin",
            "git remote set-head origin -a",
        )
    url = f"https://{repo.host}/{repo.owner}/{repo.repo}.git"
    return (
        f"git remote add origin {shlex.quote(url)}",
        "git fetch origin",
        "git remote set-head origin -a",
    )


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


def _dedupe(values: list[str]) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for value in values:
        if value in seen:
            continue
        seen.add(value)
        result.append(value)
    return result
