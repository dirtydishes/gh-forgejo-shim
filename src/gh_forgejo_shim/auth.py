from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Mapping

TOKEN_ENV_VARS = ("FJ_SHIM_TOKEN", "FORGEJO_TOKEN", "GITEA_TOKEN", "FJ_TOKEN")


def token_from_env(env: Mapping[str, str] | None = None) -> str | None:
    values = env if env is not None else os.environ
    for name in TOKEN_ENV_VARS:
        value = values.get(name, "").strip()
        if value:
            return value
    return None


def discover_fj_token(
    host: str | None = None,
    *,
    env: Mapping[str, str] | None = None,
    home: Path | None = None,
) -> str | None:
    env_token = token_from_env(env)
    if env_token:
        return env_token

    base = home or Path.home()
    candidates = [
        base / ".config" / "fj" / "config.json",
        base / ".config" / "fj" / "config.yml",
        base / ".config" / "fj" / "config.yaml",
        base / ".config" / "tea" / "config.yml",
        base / ".config" / "tea" / "config.yaml",
        base / ".config" / "gitea" / "tea.yml",
    ]

    for path in candidates:
        token = _read_token_file(path, host)
        if token:
            return token
    return None


def _read_token_file(path: Path, host: str | None) -> str | None:
    if not path.exists() or not path.is_file():
        return None
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return None

    if path.suffix == ".json":
        try:
            data = json.loads(text)
        except json.JSONDecodeError:
            return None
        return _find_token(data, host)

    return _find_token_in_text(text, host)


def _find_token(data: object, host: str | None) -> str | None:
    if isinstance(data, dict):
        if host:
            for key, value in data.items():
                if isinstance(key, str) and key.lower() == host.lower():
                    found = _find_token(value, None)
                    if found:
                        return found
        for key in ("token", "access_token"):
            value = data.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
        for value in data.values():
            found = _find_token(value, host)
            if found:
                return found
    elif isinstance(data, list):
        for item in data:
            found = _find_token(item, host)
            if found:
                return found
    return None


def _find_token_in_text(text: str, host: str | None) -> str | None:
    if host:
        host_index = text.lower().find(host.lower())
        if host_index >= 0:
            nearby = text[host_index : host_index + 2000]
            token = _token_line(nearby)
            if token:
                return token
    return _token_line(text)


def _token_line(text: str) -> str | None:
    patterns = [
        r"(?im)^\s*(?:token|access_token)\s*:\s*['\"]?([^'\"\s#]+)",
        r"(?im)^\s*(?:token|access_token)\s*=\s*['\"]?([^'\"\s#]+)",
    ]
    for pattern in patterns:
        match = re.search(pattern, text)
        if match:
            return match.group(1).strip()
    return None
