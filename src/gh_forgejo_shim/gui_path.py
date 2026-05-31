from __future__ import annotations

import os
import subprocess
import xml.sax.saxutils
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

LABEL = "com.gh-forgejo-shim.user-gui-path"
PLIST_NAME = f"{LABEL}.plist"

SYSTEM_GUI_PATH_DIRS = (
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


@dataclass(frozen=True)
class GuiPathResult:
    plist_path: Path
    path_value: str
    applied: bool = False
    apply_error: str | None = None


def default_plist_path(*, home: Path | None = None) -> Path:
    root = home or Path.home()
    return root / "Library" / "LaunchAgents" / PLIST_NAME


def default_gui_path(
    *,
    home: Path | None = None,
    env: Mapping[str, str] | None = None,
) -> str:
    values = env if env is not None else os.environ
    root = home or Path(values.get("HOME", str(Path.home()))).expanduser()
    candidates = [str(root / ".local" / "bin"), *SYSTEM_GUI_PATH_DIRS]
    return os.pathsep.join(_dedupe(candidates))


def launch_agent_plist(path_value: str) -> str:
    escaped_path = xml.sax.saxutils.escape(path_value)
    return "\n".join(
        [
            '<?xml version="1.0" encoding="UTF-8"?>',
            '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"',
            '  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">',
            '<plist version="1.0">',
            "<dict>",
            "  <key>Label</key>",
            f"  <string>{LABEL}</string>",
            "",
            "  <key>ProgramArguments</key>",
            "  <array>",
            "    <string>/bin/launchctl</string>",
            "    <string>setenv</string>",
            "    <string>PATH</string>",
            f"    <string>{escaped_path}</string>",
            "  </array>",
            "",
            "  <key>RunAtLoad</key>",
            "  <true/>",
            "</dict>",
            "</plist>",
            "",
        ]
    )


def install_gui_path(
    *,
    path_value: str | None = None,
    plist_path: Path | None = None,
    apply_now: bool = True,
    env: Mapping[str, str] | None = None,
) -> GuiPathResult:
    value = path_value or default_gui_path(env=env)
    target = plist_path or default_plist_path()
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(launch_agent_plist(value), encoding="utf-8")

    if not apply_now:
        return GuiPathResult(target, value)

    applied, error = apply_gui_path(value)
    return GuiPathResult(target, value, applied=applied, apply_error=error)


def uninstall_gui_path(*, plist_path: Path | None = None) -> Path:
    target = plist_path or default_plist_path()
    if target.exists():
        target.unlink()
    return target


def apply_gui_path(path_value: str) -> tuple[bool, str | None]:
    try:
        completed = subprocess.run(
            ["/bin/launchctl", "setenv", "PATH", path_value],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
    except OSError as exc:
        return False, str(exc)
    if completed.returncode == 0:
        return True, None
    error = (completed.stderr or completed.stdout or f"launchctl exited {completed.returncode}").strip()
    return False, error


def current_launchd_path() -> str | None:
    try:
        completed = subprocess.run(
            ["/bin/launchctl", "getenv", "PATH"],
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


def path_contains_dir(path_value: str, directory: Path) -> bool:
    target = str(directory.expanduser())
    return target in path_value.split(os.pathsep)


def _dedupe(values: list[str]) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for value in values:
        if not value or value in seen:
            continue
        seen.add(value)
        result.append(value)
    return result
