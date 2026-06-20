from __future__ import annotations

import os
import shlex
import shutil
import sys
from pathlib import Path

MARKER = "managed by gh-forgejo-shim"


def default_bin_dir() -> Path:
    return Path.home() / ".local" / "bin"


def shim_path(bin_dir: Path | None = None) -> Path:
    return (bin_dir or default_bin_dir()) / "gh"


def shim_script(python_executable: str | None = None, *, management_command: str | None = None) -> str:
    command = management_command if python_executable is None else None
    command = command or (_management_command() if python_executable is None else None)
    if command:
        invocation = f'exec {shlex.quote(command)} gh "$@"'
    else:
        executable = python_executable or sys.executable
        invocation = f'exec {shlex.quote(executable)} -m gh_forgejo_shim gh "$@"'
    return "\n".join(
        [
            "#!/bin/sh",
            f"# {MARKER}",
            invocation,
            "",
        ]
    )


def is_managed_shim(path: Path) -> bool:
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False
    return MARKER in text and (
        "gh-forgejo-shim gh" in text or "-m gh_forgejo_shim gh" in text
    )


def install_shim(*, bin_dir: Path | None = None, force: bool = False) -> Path:
    target = shim_path(bin_dir)
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists() and not is_managed_shim(target) and not force:
        raise FileExistsError(f"{target} exists and is not managed by gh-forgejo-shim; pass --force to overwrite")
    target.write_text(shim_script(), encoding="utf-8")
    mode = target.stat().st_mode
    target.chmod(mode | 0o755)
    return target


def uninstall_shim(*, bin_dir: Path | None = None) -> Path:
    target = shim_path(bin_dir)
    if not target.exists():
        return target
    if not is_managed_shim(target):
        raise FileExistsError(f"{target} is not managed by gh-forgejo-shim; refusing to remove")
    target.unlink()
    return target


def path_contains_bin_dir(bin_dir: Path, *, path_value: str | None = None) -> bool:
    value = path_value if path_value is not None else os.environ.get("PATH", "")
    return str(bin_dir) in value.split(os.pathsep)


def _management_command() -> str | None:
    for name in ("gh-forgejo-shim", "gfj"):
        found = shutil.which(name)
        if found:
            return found
    return None
