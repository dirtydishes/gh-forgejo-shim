from __future__ import annotations

import argparse
import sys
from pathlib import Path

from . import __version__
from .config import add_host, load_config, remove_host
from .doctor import format_checks, run_checks
from .routing import run_gh
from .shim import install_shim, uninstall_shim


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if args and args[0] == "gh":
        return run_gh(args[1:])

    parser = build_parser()
    namespace = parser.parse_args(args)
    command = namespace.command

    if command == "install-shim":
        try:
            path = install_shim(
                bin_dir=Path(namespace.bin_dir).expanduser() if namespace.bin_dir else None,
                force=namespace.force,
            )
        except FileExistsError as exc:
            print(f"gh-forgejo-shim: {exc}", file=sys.stderr)
            return 1
        print(f"installed gh shim at {path}")
        return 0

    if command == "uninstall-shim":
        try:
            path = uninstall_shim(
                bin_dir=Path(namespace.bin_dir).expanduser() if namespace.bin_dir else None,
            )
        except FileExistsError as exc:
            print(f"gh-forgejo-shim: {exc}", file=sys.stderr)
            return 1
        print(f"removed gh shim at {path}")
        return 0

    if command == "doctor":
        checks = run_checks()
        print(format_checks(checks))
        return 0 if all(check.ok for check in checks) else 1

    if command == "config":
        return run_config(namespace)

    if command == "version":
        print(__version__)
        return 0

    parser.print_help()
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="gh-forgejo-shim")
    subparsers = parser.add_subparsers(dest="command")

    install = subparsers.add_parser("install-shim", help="install the user-local gh wrapper")
    install.add_argument("--bin-dir")
    install.add_argument("--force", action="store_true")

    uninstall = subparsers.add_parser("uninstall-shim", help="remove the user-local gh wrapper")
    uninstall.add_argument("--bin-dir")

    subparsers.add_parser("doctor", help="check shim configuration")
    subparsers.add_parser("version", help="print version")

    config = subparsers.add_parser("config", help="manage persistent configuration")
    config_subparsers = config.add_subparsers(dest="config_command", required=True)
    add = config_subparsers.add_parser("add-host")
    add.add_argument("host")
    remove = config_subparsers.add_parser("remove-host")
    remove.add_argument("host")
    config_subparsers.add_parser("list")

    return parser


def run_config(namespace: argparse.Namespace) -> int:
    if namespace.config_command == "add-host":
        config = add_host(namespace.host)
        print(f"added Forgejo host: {namespace.host}")
        print_hosts(config.hosts)
        return 0

    if namespace.config_command == "remove-host":
        config = remove_host(namespace.host)
        print(f"removed Forgejo host: {namespace.host}")
        print_hosts(config.hosts)
        return 0

    if namespace.config_command == "list":
        config = load_config()
        print_hosts(config.hosts)
        return 0

    return 1


def print_hosts(hosts: tuple[str, ...]) -> None:
    if not hosts:
        print("no Forgejo hosts configured")
        return
    for host in hosts:
        print(host)
