from __future__ import annotations

import json
import os
import sys
import webbrowser
from dataclasses import dataclass
from typing import Callable, Mapping, TextIO

from .auth import discover_fj_token
from .config import Config, load_config
from .create import CreateParseError, parse_create_args
from .external import find_program, run_program
from .forgejo import ForgejoClient, ForgejoError, RepoRef
from .normalize import (
    filter_fields,
    filter_repo_fields,
    normalize_pull,
    normalize_repo,
    status_for_current_branch,
)
from .repo import (
    current_branch,
    default_base_branch,
    detect_repo,
    fill_body,
    fill_title,
    parse_repo_spec,
)

SUPPORTED_PR_COMMANDS = {"checks", "create", "list", "new", "status", "view"}
SUPPORTED_REPO_COMMANDS = {"view"}


@dataclass(frozen=True)
class RouteDecision:
    kind: str
    reason: str
    repo: RepoRef | None = None


ClientFactory = Callable[[str | None], ForgejoClient]


def _is_supported_command(argv: list[str]) -> bool:
    if len(argv) < 2:
        return False
    if argv[0] == "pr":
        return argv[1] in SUPPORTED_PR_COMMANDS
    if argv[0] == "repo":
        return argv[1] in SUPPORTED_REPO_COMMANDS
    return False


def decide_route(
    argv: list[str],
    *,
    config: Config,
    env: Mapping[str, str] | None = None,
    cwd: str | None = None,
) -> RouteDecision:
    if len(argv) < 2 or not _is_supported_command(argv):
        return RouteDecision("delegate", "unsupported command")

    detection = detect_repo(argv, env=env, cwd=cwd)
    if detection.repo is None:
        return RouteDecision("delegate", "no repository detected")
    if not config.is_forgejo_host(detection.repo.host):
        return RouteDecision("delegate", f"host {detection.repo.host} is not allowlisted", detection.repo)
    return RouteDecision("forgejo", detection.source, detection.repo)


def run_gh(
    argv: list[str],
    *,
    env: Mapping[str, str] | None = None,
    cwd: str | None = None,
    stdout: TextIO | None = None,
    stderr: TextIO | None = None,
    stdin: TextIO | None = None,
    client_factory: ClientFactory | None = None,
) -> int:
    out = stdout or sys.stdout
    err = stderr or sys.stderr
    values = env if env is not None else os.environ
    config = load_config(env=values)
    decision = decide_route(argv, config=config, env=values, cwd=cwd)

    if decision.kind == "delegate":
        real_gh = find_program("gh", configured=config.paths.gh, env=values)
        if not real_gh:
            print(
                "gh-forgejo-shim: could not find the real gh executable; set FJ_SHIM_REAL_GH",
                file=err,
            )
            return 127
        return run_program(real_gh, argv)

    assert decision.repo is not None
    token = discover_fj_token(decision.repo.host, env=values)
    client = client_factory(token) if client_factory else ForgejoClient(token)
    try:
        return run_forgejo(
            argv,
            decision.repo,
            client,
            cwd=cwd,
            stdout=out,
            stderr=err,
            stdin=stdin,
        )
    except (CreateParseError, ForgejoError, ValueError) as exc:
        print(f"gh-forgejo-shim: {exc}", file=err)
        return 1


def run_forgejo(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    cwd: str | None = None,
    stdout: TextIO,
    stderr: TextIO,
    stdin: TextIO | None = None,
) -> int:
    if argv[0] == "repo":
        return run_forgejo_repo(argv, repo, client, stdout=stdout, stderr=stderr)
    return run_forgejo_pr(argv, repo, client, cwd=cwd, stdout=stdout, stderr=stderr, stdin=stdin)


def run_forgejo_pr(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    cwd: str | None = None,
    stdout: TextIO,
    stderr: TextIO,
    stdin: TextIO | None = None,
) -> int:
    command = argv[1]
    rest = argv[2:]
    if command in {"create", "new"}:
        return _run_create(rest, repo, client, cwd=cwd, stdout=stdout, stdin=stdin)
    if command == "checks":
        return _run_checks(rest, stdout=stdout)
    if command == "list":
        return _run_list(rest, repo, client, stdout=stdout)
    if command == "view":
        return _run_view(rest, repo, client, cwd=cwd, stdout=stdout)
    if command == "status":
        return _run_status(rest, repo, client, cwd=cwd, stdout=stdout)
    print(f"gh-forgejo-shim: unsupported Forgejo PR command: {command}", file=stderr)
    return 1


def run_forgejo_repo(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    stdout: TextIO,
    stderr: TextIO,
) -> int:
    command = argv[1]
    rest = argv[2:]
    if command == "view":
        return _run_repo_view(rest, repo, client, stdout=stdout)
    print(f"gh-forgejo-shim: unsupported Forgejo repo command: {command}", file=stderr)
    return 1


def _run_create(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    cwd: str | None,
    stdout: TextIO,
    stdin: TextIO | None,
) -> int:
    options = parse_create_args(argv, stdin=stdin)
    target_repo = parse_repo_spec(options.repo) if options.repo else repo
    if target_repo is None:
        raise ValueError("could not parse --repo value")

    base = options.base or default_base_branch(cwd=cwd)
    head = options.head or current_branch(cwd=cwd)
    if not head:
        raise ValueError("could not determine head branch; pass --head")

    title = options.title
    if not title and (options.fill or options.fill_first or options.fill_verbose):
        title = fill_title(cwd=cwd)
    if not title:
        raise ValueError("Forgejo PR create requires --title or one of --fill/--fill-first/--fill-verbose")

    body = options.body
    if body is None and (options.fill or options.fill_first or options.fill_verbose):
        body = fill_body(verbose=options.fill_verbose, cwd=cwd)
    if body is None:
        body = ""

    if options.web:
        url = f"{target_repo.web_base_url}/compare/{base}...{head}"
        webbrowser.open(url)
        print(url, file=stdout)
        return 0

    pull = client.create_pull(
        target_repo,
        title=title,
        body=body,
        base=base,
        head=head,
        draft=options.draft,
    )
    normalized = normalize_pull(pull)
    if options.json_fields:
        print(json.dumps(filter_fields(normalized, options.json_fields), sort_keys=True), file=stdout)
    else:
        print(normalized.get("url") or pull.get("html_url") or pull.get("url") or "", file=stdout)
    return 0


def _run_list(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    stdout: TextIO,
) -> int:
    parsed = _parse_list_args(argv)
    target_repo = parse_repo_spec(parsed.repo, default_host=repo.host) if parsed.repo else repo
    if target_repo is None:
        raise ValueError("could not parse --repo value")
    api_state = "closed" if parsed.state == "merged" else parsed.state
    pulls = client.list_pulls(target_repo, state=api_state, head=parsed.head)
    if parsed.state == "merged":
        pulls = [pull for pull in pulls if normalize_pull(pull).get("state") == "MERGED"]
    if parsed.base:
        pulls = [
            pull
            for pull in pulls
            if isinstance(pull.get("base"), dict) and pull["base"].get("ref") == parsed.base
        ]
    if parsed.limit is not None:
        pulls = pulls[: parsed.limit]

    normalized = [filter_fields(normalize_pull(pull), parsed.json_fields) for pull in pulls]
    if parsed.json_fields:
        _print_json_or_jq_list(normalized, parsed.jq, stdout)
    else:
        for pull in normalized:
            print(_format_pull_list_item(pull), file=stdout)
    return 0


def _run_checks(argv: list[str], *, stdout: TextIO) -> int:
    parsed = _parse_checks_args(argv)
    if parsed.json_fields:
        _print_json_or_jq_list([], parsed.jq, stdout)
    else:
        print("no checks reported", file=stdout)
    return 0


def _run_view(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    cwd: str | None,
    stdout: TextIO,
) -> int:
    parsed = _parse_view_status_args(argv)
    pull: dict[str, object] | None = None
    if parsed.number is not None:
        pull = client.get_pull(repo, parsed.number)
    else:
        branch = parsed.branch or current_branch(cwd=cwd)
        if branch:
            pulls = client.list_pulls(repo, state="open", head=branch)
            pull = pulls[0] if pulls else None

    if pull is None:
        if parsed.json_fields:
            _print_json_or_jq({}, parsed.jq, stdout)
        return 0

    normalized = normalize_pull(pull)
    if parsed.web:
        url = str(normalized.get("url") or "")
        if url:
            webbrowser.open(url)
            print(url, file=stdout)
        return 0
    if parsed.json_fields:
        _print_json_or_jq(filter_fields(normalized, parsed.json_fields), parsed.jq, stdout)
    else:
        print(_format_pull_text(normalized), file=stdout)
    return 0


def _run_repo_view(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    stdout: TextIO,
) -> int:
    parsed = _parse_repo_view_args(argv)
    target_repo = parse_repo_spec(parsed.repo, default_host=repo.host) if parsed.repo else repo
    if target_repo is None:
        raise ValueError("could not parse --repo value")
    data = normalize_repo(client.get_repo(target_repo), target_repo)
    if parsed.web:
        url = str(data.get("url") or "")
        if url:
            webbrowser.open(url)
            print(url, file=stdout)
        return 0
    if parsed.json_fields:
        _print_json_or_jq(filter_repo_fields(data, parsed.json_fields), parsed.jq, stdout)
    else:
        print(data.get("url") or target_repo.web_base_url, file=stdout)
    return 0


def _run_status(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    cwd: str | None,
    stdout: TextIO,
) -> int:
    parsed = _parse_view_status_args(argv)
    branch = parsed.branch or current_branch(cwd=cwd)
    pull = None
    if branch:
        pulls = client.list_pulls(repo, state="open", head=branch)
        pull = pulls[0] if pulls else None

    if pull is None:
        if parsed.json_fields:
            _print_json_or_jq(status_for_current_branch(None, parsed.json_fields), parsed.jq, stdout)
        return 0

    if parsed.json_fields:
        _print_json_or_jq(status_for_current_branch(pull, parsed.json_fields), parsed.jq, stdout)
    else:
        normalized = normalize_pull(pull)
        print(_format_pull_text(normalized), file=stdout)
    return 0


@dataclass(frozen=True)
class ViewStatusArgs:
    json_fields: tuple[str, ...] = ()
    repo: str | None = None
    web: bool = False
    jq: str | None = None
    template: str | None = None
    number: int | None = None
    branch: str | None = None


@dataclass(frozen=True)
class ListArgs:
    json_fields: tuple[str, ...] = ()
    repo: str | None = None
    jq: str | None = None
    template: str | None = None
    state: str = "open"
    limit: int | None = None
    head: str | None = None
    base: str | None = None


@dataclass(frozen=True)
class RepoViewArgs:
    json_fields: tuple[str, ...] = ()
    repo: str | None = None
    web: bool = False
    jq: str | None = None
    template: str | None = None


@dataclass(frozen=True)
class ChecksArgs:
    json_fields: tuple[str, ...] = ()
    repo: str | None = None
    jq: str | None = None
    number: int | None = None


def _parse_list_args(argv: list[str]) -> ListArgs:
    json_fields: tuple[str, ...] = ()
    repo = None
    jq = None
    template = None
    state = "open"
    limit = None
    head = None
    base = None
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in {"--json"}:
            if index + 1 >= len(argv):
                raise ValueError("missing value for --json")
            json_fields = _split_fields(argv[index + 1])
            index += 2
        elif arg.startswith("--json="):
            json_fields = _split_fields(arg.split("=", 1)[1])
            index += 1
        elif arg in {"-R", "--repo"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            repo = argv[index + 1]
            index += 2
        elif arg.startswith("--repo="):
            repo = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--jq", "-q"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            jq = argv[index + 1]
            index += 2
        elif arg.startswith("--jq=") or arg.startswith("-q="):
            jq = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--template", "-t"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            template = argv[index + 1]
            index += 2
        elif arg.startswith("--template=") or arg.startswith("-t="):
            template = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--state", "-s"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            state = _normalize_state(argv[index + 1])
            index += 2
        elif arg.startswith("--state=") or arg.startswith("-s="):
            state = _normalize_state(arg.split("=", 1)[1])
            index += 1
        elif arg in {"--limit", "-L"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            limit = _parse_limit(argv[index + 1])
            index += 2
        elif arg.startswith("--limit=") or arg.startswith("-L="):
            limit = _parse_limit(arg.split("=", 1)[1])
            index += 1
        elif arg in {"--head", "-H"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            head = argv[index + 1]
            index += 2
        elif arg.startswith("--head=") or arg.startswith("-H="):
            head = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--base", "-B"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            base = argv[index + 1]
            index += 2
        elif arg.startswith("--base=") or arg.startswith("-B="):
            base = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--author", "--app", "--assignee", "--label", "--search"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            index += 2
        elif arg.startswith(("--author=", "--app=", "--assignee=", "--label=", "--search=")):
            index += 1
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo PR list flag: {arg}")
        else:
            index += 1
    return ListArgs(
        json_fields=json_fields,
        repo=repo,
        jq=jq,
        template=template,
        state=state,
        limit=limit,
        head=head,
        base=base,
    )


def _parse_checks_args(argv: list[str]) -> ChecksArgs:
    json_fields: tuple[str, ...] = ()
    repo = None
    jq = None
    number = None
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in {"--json"}:
            if index + 1 >= len(argv):
                raise ValueError("missing value for --json")
            json_fields = _split_fields(argv[index + 1])
            index += 2
        elif arg.startswith("--json="):
            json_fields = _split_fields(arg.split("=", 1)[1])
            index += 1
        elif arg in {"-R", "--repo"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            repo = argv[index + 1]
            index += 2
        elif arg.startswith("--repo="):
            repo = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--jq", "-q"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            jq = argv[index + 1]
            index += 2
        elif arg.startswith("--jq=") or arg.startswith("-q="):
            jq = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--watch", "--fail-fast", "--required"}:
            index += 1
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo PR checks flag: {arg}")
        else:
            if arg.isdigit():
                number = int(arg)
            index += 1
    return ChecksArgs(json_fields=json_fields, repo=repo, jq=jq, number=number)


def _parse_view_status_args(argv: list[str]) -> ViewStatusArgs:
    json_fields: tuple[str, ...] = ()
    repo = None
    web = False
    jq = None
    template = None
    number = None
    branch = None
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in {"--json"}:
            if index + 1 >= len(argv):
                raise ValueError("missing value for --json")
            json_fields = _split_fields(argv[index + 1])
            index += 2
        elif arg.startswith("--json="):
            json_fields = _split_fields(arg.split("=", 1)[1])
            index += 1
        elif arg in {"-R", "--repo"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            repo = argv[index + 1]
            index += 2
        elif arg.startswith("--repo="):
            repo = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--web", "-w"}:
            web = True
            index += 1
        elif arg in {"--jq", "-q"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            jq = argv[index + 1]
            index += 2
        elif arg.startswith("--jq=") or arg.startswith("-q="):
            jq = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--template", "-t"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            template = argv[index + 1]
            index += 2
        elif arg.startswith("--template=") or arg.startswith("-t="):
            template = arg.split("=", 1)[1]
            index += 1
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo PR flag: {arg}")
        else:
            if arg.isdigit():
                number = int(arg)
            else:
                branch = arg
            index += 1
    return ViewStatusArgs(
        json_fields=json_fields,
        repo=repo,
        web=web,
        jq=jq,
        template=template,
        number=number,
        branch=branch,
    )


def _parse_repo_view_args(argv: list[str]) -> RepoViewArgs:
    json_fields: tuple[str, ...] = ()
    repo = None
    web = False
    jq = None
    template = None
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in {"--json"}:
            if index + 1 >= len(argv):
                raise ValueError("missing value for --json")
            json_fields = _split_fields(argv[index + 1])
            index += 2
        elif arg.startswith("--json="):
            json_fields = _split_fields(arg.split("=", 1)[1])
            index += 1
        elif arg in {"-R", "--repo"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            repo = argv[index + 1]
            index += 2
        elif arg.startswith("--repo="):
            repo = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--web", "-w"}:
            web = True
            index += 1
        elif arg in {"--jq", "-q"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            jq = argv[index + 1]
            index += 2
        elif arg.startswith("--jq=") or arg.startswith("-q="):
            jq = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--template", "-t"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            template = argv[index + 1]
            index += 2
        elif arg.startswith("--template=") or arg.startswith("-t="):
            template = arg.split("=", 1)[1]
            index += 1
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo repo view flag: {arg}")
        else:
            repo = arg
            index += 1
    return RepoViewArgs(json_fields=json_fields, repo=repo, web=web, jq=jq, template=template)


def _format_pull_text(data: dict[str, object]) -> str:
    number = data.get("number")
    title = data.get("title") or ""
    state = data.get("state") or "UNKNOWN"
    url = data.get("url") or ""
    return f"#{number} {title}\nstate: {state}\nurl: {url}".rstrip()


def _format_pull_list_item(data: dict[str, object]) -> str:
    number = data.get("number")
    title = data.get("title") or ""
    branch = data.get("headRefName") or ""
    return f"{number}\t{title}\t{branch}".rstrip()


def _print_json_or_jq(data: dict[str, object], jq: str | None, stdout: TextIO) -> None:
    if jq:
        value = _apply_simple_jq(data, jq)
        if value is None:
            return
        if isinstance(value, str):
            print(value, file=stdout)
        else:
            print(json.dumps(value, sort_keys=True), file=stdout)
        return
    print(json.dumps(data, sort_keys=True), file=stdout)


def _print_json_or_jq_list(data: list[dict[str, object]], jq: str | None, stdout: TextIO) -> None:
    if jq:
        value = _apply_simple_jq(data, jq)
        if value is None:
            return
        if isinstance(value, str):
            print(value, file=stdout)
        else:
            print(json.dumps(value, sort_keys=True), file=stdout)
        return
    print(json.dumps(data, sort_keys=True), file=stdout)


def _apply_simple_jq(data: object, expression: str) -> object:
    query = expression.strip()
    if query in {".", ""}:
        return data
    if query.endswith("// empty"):
        query = query[: -len("// empty")].strip()
    if not query.startswith("."):
        return data

    value = data
    for part in query[1:].split("."):
        if not part:
            continue
        if isinstance(value, list) and part.endswith("[]"):
            key = part[:-2]
            value = [item.get(key) for item in value if isinstance(item, dict)]
        elif isinstance(value, dict):
            value = value.get(part)
        else:
            return None
    return value


def _split_fields(value: str) -> tuple[str, ...]:
    return tuple(item.strip() for item in value.split(",") if item.strip())


def _parse_limit(value: str) -> int:
    try:
        limit = int(value)
    except ValueError as exc:
        raise ValueError(f"invalid --limit value: {value}") from exc
    if limit < 0:
        raise ValueError("--limit must be non-negative")
    return limit


def _normalize_state(value: str) -> str:
    normalized = value.lower()
    if normalized not in {"open", "closed", "merged", "all"}:
        raise ValueError("--state must be one of: open, closed, merged, all")
    return normalized
