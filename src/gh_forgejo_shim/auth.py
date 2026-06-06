from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

from .config import normalize_host

TOKEN_ENV_VARS = ("FJ_SHIM_TOKEN", "FORGEJO_TOKEN", "GITEA_TOKEN", "FJ_TOKEN")
KEYCHAIN_SERVICE = "gh-forgejo-shim"
AUTH_FILE_NAME = "auth.json"


class AuthStorageError(RuntimeError):
    pass


@dataclass(frozen=True)
class TokenDiscovery:
    token: str
    source: str


@dataclass(frozen=True)
class StoredToken:
    token: str
    storage: str


@dataclass(frozen=True)
class AuthStatus:
    host: str
    exists: bool
    storage: str | None = None


def token_from_env(env: Mapping[str, str] | None = None) -> str | None:
    discovery = token_from_env_with_source(env)
    return discovery.token if discovery else None


def token_from_env_with_source(env: Mapping[str, str] | None = None) -> TokenDiscovery | None:
    values = env if env is not None else os.environ
    for name in TOKEN_ENV_VARS:
        value = values.get(name, "").strip()
        if value:
            return TokenDiscovery(value, name)
    return None


def discover_fj_token(
    host: str | None = None,
    *,
    env: Mapping[str, str] | None = None,
    home: Path | None = None,
) -> str | None:
    discovery = discover_token(host, env=env, home=home)
    return discovery.token if discovery else None


def discover_token(
    host: str | None = None,
    *,
    env: Mapping[str, str] | None = None,
    home: Path | None = None,
    platform: str | None = None,
    include_stored: bool = True,
) -> TokenDiscovery | None:
    env_token = token_from_env_with_source(env)
    if env_token:
        return env_token

    normalized_host = normalize_host(host) if host else None
    if normalized_host and include_stored:
        stored = read_stored_token(normalized_host, home=home, platform=platform)
        if stored:
            return TokenDiscovery(stored.token, stored.storage)

    base = home or Path.home()
    candidates = [
        base / "Library" / "Application Support" / "Cyborus.forgejo-cli" / "keys.json",
        base / ".config" / "fj" / "config.json",
        base / ".config" / "fj" / "config.yml",
        base / ".config" / "fj" / "config.yaml",
        base / ".config" / "tea" / "config.yml",
        base / ".config" / "tea" / "config.yaml",
        base / ".config" / "gitea" / "tea.yml",
    ]

    for path in candidates:
        token = _read_token_file(path, normalized_host)
        if token:
            return TokenDiscovery(token, str(path))
    return None


def discover_import_token(
    host: str,
    *,
    env: Mapping[str, str] | None = None,
    home: Path | None = None,
) -> TokenDiscovery | None:
    return discover_token(host, env=env, home=home, include_stored=False)


def read_stored_token(
    host: str,
    *,
    home: Path | None = None,
    platform: str | None = None,
) -> StoredToken | None:
    normalized = normalize_host(host)
    if _use_keychain(platform):
        token = _read_keychain_token(normalized)
        if token:
            return StoredToken(token, "macOS Keychain")

    token = _read_file_token(normalized, home=home)
    if token:
        return StoredToken(token, str(auth_file_path(home)))
    return None


def write_stored_token(
    host: str,
    token: str,
    *,
    home: Path | None = None,
    platform: str | None = None,
) -> str:
    normalized = normalize_host(host)
    if not normalized:
        raise AuthStorageError("Forgejo host is required")
    if not token.strip():
        raise AuthStorageError("Forgejo token is required")

    if _use_keychain(platform):
        try:
            _write_keychain_token(normalized, token)
        except AuthStorageError:
            pass
        else:
            _delete_file_token(normalized, home=home)
            return "macOS Keychain"

    _write_file_token(normalized, token, home=home)
    return str(auth_file_path(home))


def delete_stored_token(
    host: str,
    *,
    home: Path | None = None,
    platform: str | None = None,
) -> bool:
    normalized = normalize_host(host)
    deleted = False
    if _use_keychain(platform):
        deleted = _delete_keychain_token(normalized) or deleted
    deleted = _delete_file_token(normalized, home=home) or deleted
    return deleted


def stored_auth_status(
    host: str,
    *,
    home: Path | None = None,
    platform: str | None = None,
) -> AuthStatus:
    normalized = normalize_host(host)
    stored = read_stored_token(normalized, home=home, platform=platform)
    if stored:
        return AuthStatus(normalized, True, stored.storage)
    return AuthStatus(normalized, False)


def auth_file_path(home: Path | None = None) -> Path:
    base = home or Path.home()
    return base / ".config" / "gh-forgejo-shim" / AUTH_FILE_NAME


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
                if isinstance(key, str) and normalize_host(key) == host:
                    found = _find_token(value, None)
                    if found:
                        return found
            if _dict_matches_host(data, host):
                found = _token_from_dict(data)
                if found:
                    return found
            for value in data.values():
                found = _find_token(value, host)
                if found:
                    return found
            return None

        found = _token_from_dict(data)
        if found:
            return found
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


def _token_from_dict(data: dict[object, object]) -> str | None:
    for key in ("token", "access_token"):
        value = data.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


def _dict_matches_host(data: dict[object, object], host: str) -> bool:
    for key in ("host", "hostname", "url", "server", "server_url", "base_url"):
        value = data.get(key)
        if isinstance(value, str) and normalize_host(value) == host:
            return True
    return False


def _find_token_in_text(text: str, host: str | None) -> str | None:
    if host:
        host_index = text.lower().find(host.lower())
        if host_index >= 0:
            nearby = text[host_index : host_index + 2000]
            token = _token_line(nearby)
            if token:
                return token
        return None
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


def _use_keychain(platform: str | None) -> bool:
    current = platform if platform is not None else sys.platform
    return current == "darwin" and shutil.which("security") is not None


def _read_keychain_token(host: str) -> str | None:
    result = _run_security(
        [
            "find-generic-password",
            "-a",
            host,
            "-s",
            KEYCHAIN_SERVICE,
            "-w",
        ]
    )
    if result.returncode != 0:
        return None
    token = result.stdout.strip()
    return token or None


def _write_keychain_token(host: str, token: str) -> None:
    result = _run_security(
        [
            "add-generic-password",
            "-a",
            host,
            "-s",
            KEYCHAIN_SERVICE,
            "-w",
            token,
            "-U",
        ]
    )
    if result.returncode != 0:
        message = result.stderr.strip() or "security add-generic-password failed"
        raise AuthStorageError(message)


def _delete_keychain_token(host: str) -> bool:
    result = _run_security(
        [
            "delete-generic-password",
            "-a",
            host,
            "-s",
            KEYCHAIN_SERVICE,
        ]
    )
    return result.returncode == 0


def _run_security(args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["security", *args],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def _read_file_token(host: str, *, home: Path | None = None) -> str | None:
    data = _read_auth_file(home=home)
    hosts = data.get("hosts")
    if not isinstance(hosts, dict):
        return None
    item = hosts.get(host)
    if isinstance(item, dict):
        token = item.get("token")
        if isinstance(token, str) and token.strip():
            return token.strip()
    if isinstance(item, str) and item.strip():
        return item.strip()
    return None


def _write_file_token(host: str, token: str, *, home: Path | None = None) -> None:
    data = _read_auth_file(home=home)
    hosts = data.get("hosts")
    if not isinstance(hosts, dict):
        hosts = {}
    hosts[host] = {"token": token}
    data["hosts"] = hosts
    _write_auth_file(data, home=home)


def _delete_file_token(host: str, *, home: Path | None = None) -> bool:
    path = auth_file_path(home)
    data = _read_auth_file(home=home)
    hosts = data.get("hosts")
    if not isinstance(hosts, dict) or host not in hosts:
        return False
    del hosts[host]
    if hosts:
        data["hosts"] = hosts
        _write_auth_file(data, home=home)
    else:
        try:
            path.unlink()
        except FileNotFoundError:
            pass
    return True


def _read_auth_file(*, home: Path | None = None) -> dict[str, object]:
    path = auth_file_path(home)
    if not path.exists():
        return {"hosts": {}}
    try:
        text = path.read_text(encoding="utf-8")
        data = json.loads(text)
    except (OSError, json.JSONDecodeError):
        return {"hosts": {}}
    if isinstance(data, dict):
        return data
    return {"hosts": {}}


def _write_auth_file(data: dict[str, object], *, home: Path | None = None) -> None:
    path = auth_file_path(home)
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        path.parent.chmod(0o700)
    except OSError:
        pass

    text = json.dumps(data, indent=2, sort_keys=True) + "\n"
    fd, temp_name = tempfile.mkstemp(prefix=f".{AUTH_FILE_NAME}.", dir=path.parent)
    temp_path = Path(temp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(text)
        temp_path.chmod(0o600)
        temp_path.replace(path)
        path.chmod(0o600)
    except OSError as exc:
        try:
            temp_path.unlink()
        except OSError:
            pass
        raise AuthStorageError(f"could not write {path}: {exc}") from exc
