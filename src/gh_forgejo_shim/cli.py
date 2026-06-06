from __future__ import annotations

import argparse
import getpass
import sys
from pathlib import Path
from typing import Callable, Mapping, TextIO

from . import __version__
from .auth import (
    AuthStorageError,
    delete_stored_token,
    discover_import_token,
    discover_token,
    stored_auth_status,
    write_stored_token,
)
from .bootstrap import format_bootstrap, run_bootstrap
from .config import add_host, load_config, normalize_host, remove_host
from .doctor import format_checks, run_checks
from .forgejo import ForgejoClient, ForgejoError
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

    if command == "auth":
        return run_auth(namespace)

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

    auth = subparsers.add_parser("auth", help="manage native Forgejo auth")
    auth_subparsers = auth.add_subparsers(dest="auth_command", required=True)
    login = auth_subparsers.add_parser("login", help="validate and store a Forgejo access token")
    login.add_argument("host", nargs="?")
    import_parser = auth_subparsers.add_parser("import", help="import Forgejo auth from env or fj/tea/gitea config")
    import_parser.add_argument("host", nargs="?")
    status = auth_subparsers.add_parser("status", help="show whether shim auth exists without printing secrets")
    status.add_argument("host", nargs="?")
    logout = auth_subparsers.add_parser("logout", help="remove stored shim auth for a Forgejo host")
    logout.add_argument("host", nargs="?")

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


def run_auth(
    namespace: argparse.Namespace,
    *,
    env: Mapping[str, str] | None = None,
    home: Path | None = None,
    platform: str | None = None,
    config_path: Path | None = None,
    stdout: TextIO | None = None,
    stderr: TextIO | None = None,
    input_func: Callable[[str], str] = input,
    token_prompt: Callable[[str], str] = getpass.getpass,
    client_factory: Callable[[str | None], ForgejoClient] | None = None,
) -> int:
    out = stdout or sys.stdout
    err = stderr or sys.stderr

    try:
        if namespace.auth_command == "login":
            return _auth_login(
                namespace.host,
                env=env,
                home=home,
                platform=platform,
                config_path=config_path,
                stdout=out,
                input_func=input_func,
                token_prompt=token_prompt,
                client_factory=client_factory,
            )
        if namespace.auth_command == "import":
            return _auth_import(
                namespace.host,
                env=env,
                home=home,
                platform=platform,
                config_path=config_path,
                stdout=out,
                input_func=input_func,
                client_factory=client_factory,
            )
        if namespace.auth_command == "status":
            return _auth_status(
                namespace.host,
                env=env,
                home=home,
                platform=platform,
                config_path=config_path,
                stdout=out,
            )
        if namespace.auth_command == "logout":
            return _auth_logout(
                namespace.host,
                env=env,
                home=home,
                platform=platform,
                config_path=config_path,
                stdout=out,
                input_func=input_func,
            )
    except (AuthStorageError, ForgejoError, ValueError) as exc:
        print(f"gh-forgejo-shim: {exc}", file=err)
        return 1

    return 1


def _auth_login(
    host_arg: str | None,
    *,
    env: Mapping[str, str] | None,
    home: Path | None,
    platform: str | None,
    config_path: Path | None,
    stdout: TextIO,
    input_func: Callable[[str], str],
    token_prompt: Callable[[str], str],
    client_factory: Callable[[str | None], ForgejoClient] | None,
) -> int:
    host = _prompt_host(host_arg, env=env, config_path=config_path, input_func=input_func)
    token = token_prompt(f"Forgejo access token for {host}: ").strip()
    if not token:
        raise ValueError("Forgejo token is required")

    login = _validate_token(host, token, client_factory=client_factory)
    storage = write_stored_token(host, token, home=home, platform=platform)
    add_host(host, config_path)
    print(_validated_message("logged in", host, login), file=stdout)
    print(f"saved auth in {storage}", file=stdout)
    return 0


def _auth_import(
    host_arg: str | None,
    *,
    env: Mapping[str, str] | None,
    home: Path | None,
    platform: str | None,
    config_path: Path | None,
    stdout: TextIO,
    input_func: Callable[[str], str],
    client_factory: Callable[[str | None], ForgejoClient] | None,
) -> int:
    host = _prompt_host(host_arg, env=env, config_path=config_path, input_func=input_func)
    discovery = discover_import_token(host, env=env, home=home)
    if discovery is None:
        raise ValueError(
            "no Forgejo token found in FJ_SHIM_TOKEN, FORGEJO_TOKEN, GITEA_TOKEN, FJ_TOKEN, "
            "or common fj/tea/gitea config files"
        )

    login = _validate_token(host, discovery.token, client_factory=client_factory)
    storage = write_stored_token(host, discovery.token, home=home, platform=platform)
    add_host(host, config_path)
    print(_validated_message("imported auth", host, login), file=stdout)
    print(f"source: {discovery.source}", file=stdout)
    print(f"saved auth in {storage}", file=stdout)
    return 0


def _auth_status(
    host_arg: str | None,
    *,
    env: Mapping[str, str] | None,
    home: Path | None,
    platform: str | None,
    config_path: Path | None,
    stdout: TextIO,
) -> int:
    hosts = _status_hosts(host_arg, env=env, config_path=config_path)
    all_authenticated = True
    for host in hosts:
        status = stored_auth_status(host, home=home, platform=platform)
        if status.exists:
            print(f"{host}: logged in ({status.storage})", file=stdout)
            continue

        discovery = discover_token(host, env=env, home=home, platform=platform, include_stored=False)
        if discovery:
            print(f"{host}: token available from {discovery.source}; run gh-forgejo-shim auth import {host}", file=stdout)
        else:
            all_authenticated = False
            print(f"{host}: not logged in", file=stdout)
    return 0 if all_authenticated else 1


def _auth_logout(
    host_arg: str | None,
    *,
    env: Mapping[str, str] | None,
    home: Path | None,
    platform: str | None,
    config_path: Path | None,
    stdout: TextIO,
    input_func: Callable[[str], str],
) -> int:
    host = _prompt_host(host_arg, env=env, config_path=config_path, input_func=input_func)
    if delete_stored_token(host, home=home, platform=platform):
        print(f"logged out of {host}", file=stdout)
    else:
        print(f"no stored shim auth for {host}", file=stdout)
    return 0


def _prompt_host(
    host_arg: str | None,
    *,
    env: Mapping[str, str] | None,
    config_path: Path | None,
    input_func: Callable[[str], str],
) -> str:
    if host_arg:
        host = normalize_host(host_arg)
        if host:
            return host
        raise ValueError("Forgejo host is required")

    config = load_config(config_path, env=env)
    default = config.hosts[0] if len(config.hosts) == 1 else ""
    prompt = f"Forgejo host [{default}]: " if default else "Forgejo host: "
    host = normalize_host(input_func(prompt).strip() or default)
    if not host:
        raise ValueError("Forgejo host is required")
    return host


def _status_hosts(
    host_arg: str | None,
    *,
    env: Mapping[str, str] | None,
    config_path: Path | None,
) -> tuple[str, ...]:
    if host_arg:
        host = normalize_host(host_arg)
        if host:
            return (host,)
        raise ValueError("Forgejo host is required")

    config = load_config(config_path, env=env)
    if config.hosts:
        return config.hosts
    raise ValueError("pass a host or add one with gh-forgejo-shim config add-host HOST")


def _validate_token(
    host: str,
    token: str,
    *,
    client_factory: Callable[[str | None], ForgejoClient] | None,
) -> str | None:
    client = client_factory(token) if client_factory else ForgejoClient(token, timeout=10)
    user = client.get_current_user(host)
    if not isinstance(user, dict):
        raise ValueError("Forgejo API returned unexpected user data")
    login = user.get("login") or user.get("username") or user.get("name")
    return login if isinstance(login, str) and login.strip() else None


def _validated_message(action: str, host: str, login: str | None) -> str:
    if login:
        return f"{action} to {host} as {login}"
    return f"{action} to {host}"
