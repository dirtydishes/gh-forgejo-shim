from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

CONFIG_DIR = Path.home() / ".config" / "gh-forgejo-shim"
CONFIG_PATH = CONFIG_DIR / "config.toml"


@dataclass(frozen=True)
class PathsConfig:
    gh: str | None = None
    fj: str | None = None


@dataclass(frozen=True)
class Config:
    hosts: tuple[str, ...] = ()
    paths: PathsConfig = PathsConfig()
    path: Path = CONFIG_PATH

    def is_forgejo_host(self, host: str | None) -> bool:
        if not host:
            return False
        normalized = normalize_host(host)
        return normalized in {normalize_host(item) for item in self.hosts}


def normalize_host(host: str) -> str:
    value = host.strip()
    if "://" in value:
        value = value.split("://", 1)[1]
    value = value.split("/", 1)[0]
    return value.lower()


def load_config(
    path: Path | None = None,
    *,
    env: Mapping[str, str] | None = None,
) -> Config:
    config_path = path or CONFIG_PATH
    data: dict[str, object] = {}
    if config_path.exists():
        with config_path.open("rb") as handle:
            parsed = tomllib.load(handle)
        if isinstance(parsed, dict):
            data = parsed

    hosts = tuple(normalize_host(item) for item in _string_list(data.get("hosts")))

    paths_data = data.get("paths")
    gh = None
    fj = None
    if isinstance(paths_data, dict):
        gh = _optional_string(paths_data.get("gh"))
        fj = _optional_string(paths_data.get("fj"))

    values = env if env is not None else os.environ
    if "FJ_SHIM_HOSTS" in values:
        hosts = tuple(normalize_host(item) for item in split_hosts(values.get("FJ_SHIM_HOSTS", "")))
    gh = _optional_string(values.get("FJ_SHIM_REAL_GH")) or gh
    fj = _optional_string(values.get("FJ_SHIM_REAL_FJ")) or fj

    return Config(hosts=_dedupe(hosts), paths=PathsConfig(gh=gh, fj=fj), path=config_path)


def split_hosts(value: str) -> list[str]:
    normalized = value.replace(";", ",").replace(" ", ",")
    return [item.strip() for item in normalized.split(",") if item.strip()]


def add_host(host: str, path: Path | None = None) -> Config:
    config = load_config(path)
    hosts = sorted(set(config.hosts) | {normalize_host(host)})
    updated = Config(hosts=tuple(hosts), paths=config.paths, path=config.path)
    write_config(updated)
    return updated


def remove_host(host: str, path: Path | None = None) -> Config:
    config = load_config(path)
    normalized = normalize_host(host)
    hosts = tuple(item for item in config.hosts if normalize_host(item) != normalized)
    updated = Config(hosts=hosts, paths=config.paths, path=config.path)
    write_config(updated)
    return updated


def write_config(config: Config) -> None:
    config.path.parent.mkdir(parents=True, exist_ok=True)
    lines: list[str] = []
    hosts = ", ".join(_toml_string(host) for host in config.hosts)
    lines.append(f"hosts = [{hosts}]")
    if config.paths.gh or config.paths.fj:
        lines.append("")
        lines.append("[paths]")
        if config.paths.gh:
            lines.append(f"gh = {_toml_string(config.paths.gh)}")
        if config.paths.fj:
            lines.append(f"fj = {_toml_string(config.paths.fj)}")
    lines.append("")
    config.path.write_text("\n".join(lines), encoding="utf-8")


def _dedupe(values: tuple[str, ...]) -> tuple[str, ...]:
    seen: set[str] = set()
    result: list[str] = []
    for value in values:
        if value and value not in seen:
            seen.add(value)
            result.append(value)
    return tuple(result)


def _string_list(value: object) -> list[str]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, str) and item.strip()]


def _optional_string(value: object) -> str | None:
    if isinstance(value, str) and value.strip():
        return value.strip()
    return None


def _toml_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'
