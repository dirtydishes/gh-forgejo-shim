from __future__ import annotations

import argparse
import sys
from pathlib import Path

from . import __version__
from .bootstrap import format_bootstrap, run_bootstrap
from .config import add_host, load_config, remove_host
from .doctor import format_checks, run_checks
from .gui_path import install_gui_path, uninstall_gui_path
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

    if command == "bootstrap":
        try:
            result = run_bootstrap(
                bin_dir=Path(namespace.bin_dir).expanduser() if namespace.bin_dir else None,
                force=namespace.force,
            )
        except OSError as exc:
            print(f"gh-forgejo-shim: {exc}", file=sys.stderr)
            return 1
        print(format_bootstrap(result))
        return 0 if result.ok else 1

    if command == "install-gui-path":
        if sys.platform != "darwin":
            print("gh-forgejo-shim: install-gui-path is only supported on macOS", file=sys.stderr)
            return 1
        result = install_gui_path(path_value=namespace.path, apply_now=not namespace.no_apply)
        print(f"installed macOS GUI PATH LaunchAgent at {result.plist_path}")
        print(f"PATH={result.path_value}")
        if namespace.no_apply:
            print("restart your login session or load the LaunchAgent before reopening GUI apps")
        elif result.applied:
            print("applied PATH to the current launchd user session; restart GUI apps to inherit it")
        else:
            print(f"warning: could not apply PATH immediately: {result.apply_error}", file=sys.stderr)
            print("the LaunchAgent will apply PATH at the next login")
        return 0

    if command == "uninstall-gui-path":
        if sys.platform != "darwin":
            print("gh-forgejo-shim: uninstall-gui-path is only supported on macOS", file=sys.stderr)
            return 1
        path = uninstall_gui_path()
        print(f"removed macOS GUI PATH LaunchAgent at {path}")
        print("restart your login session to return GUI apps to the default launchd PATH")
        return 0

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

    gui_path = subparsers.add_parser(
        "install-gui-path",
        help="make macOS GUI apps inherit a PATH that can find the shim and Homebrew tools",
    )
    gui_path.add_argument("--path", help="explicit PATH value to persist for GUI apps")
    gui_path.add_argument(
        "--no-apply",
        action="store_true",
        help="write the LaunchAgent without applying PATH to the current launchd session",
    )

    subparsers.add_parser("uninstall-gui-path", help="remove the macOS GUI PATH LaunchAgent")

    subparsers.add_parser("doctor", help="check shim configuration")

    bootstrap = subparsers.add_parser(
        "bootstrap",
        help="detect the current repo, install the shim, and print exact setup repair commands",
    )
    bootstrap.add_argument("--bin-dir")
    bootstrap.add_argument("--force", action="store_true", help="overwrite an unrelated gh at the shim path")

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
