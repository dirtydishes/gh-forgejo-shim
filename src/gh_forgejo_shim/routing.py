from __future__ import annotations

import fnmatch
import json
import os
import re
import sys
import urllib.parse
import webbrowser
from dataclasses import dataclass
from typing import Callable, Mapping, TextIO

from .auth import discover_fj_token
from .config import Config, load_config
from .create import CreateParseError, body_from_file, parse_create_args
from .external import find_program, git_run, run_program
from .forgejo import ForgejoClient, ForgejoError, RepoRef
from .normalize import (
    filter_check_fields,
    filter_issue_fields,
    filter_fields,
    filter_repo_fields,
    normalize_pr_checks,
    normalize_issue,
    normalize_pull,
    normalize_repo,
    status_for_current_branch,
    with_status_check_rollup,
)
from .repo import (
    current_branch,
    default_base_branch,
    detect_repo,
    fill_body,
    fill_title,
    parse_repo_spec,
)

SUPPORTED_PR_COMMANDS = {"checks", "checkout", "co", "comment", "create", "diff", "list", "new", "status", "view"}
SUPPORTED_ISSUE_COMMANDS = {"create", "list", "ls", "new", "view"}
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
    if argv[0] == "issue":
        return argv[1] in SUPPORTED_ISSUE_COMMANDS
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
    if argv[0] == "issue":
        return run_forgejo_issue(argv, repo, client, stdout=stdout, stderr=stderr, stdin=stdin)
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
        return _run_checks(rest, repo, client, cwd=cwd, stdout=stdout)
    if command in {"checkout", "co"}:
        return _run_checkout(rest, repo, client, cwd=cwd, stdout=stdout, stderr=stderr)
    if command == "comment":
        return _run_pr_comment(rest, repo, client, cwd=cwd, stdout=stdout, stdin=stdin)
    if command == "diff":
        return _run_diff(rest, repo, client, cwd=cwd, stdout=stdout)
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


def run_forgejo_issue(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    stdout: TextIO,
    stderr: TextIO,
    stdin: TextIO | None = None,
) -> int:
    command = argv[1]
    rest = argv[2:]
    if command in {"create", "new"}:
        return _run_issue_create(rest, repo, client, stdout=stdout, stdin=stdin)
    if command in {"list", "ls"}:
        return _run_issue_list(rest, repo, client, stdout=stdout)
    if command == "view":
        return _run_issue_view(rest, repo, client, stdout=stdout)
    print(f"gh-forgejo-shim: unsupported Forgejo issue command: {command}", file=stderr)
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

    normalized = [
        filter_fields(normalize_pull(_enrich_pull_statuses(target_repo, client, pull, parsed.json_fields)), parsed.json_fields)
        for pull in pulls
    ]
    if parsed.json_fields:
        _print_json_or_jq_list(normalized, parsed.jq, stdout, template=parsed.template)
    else:
        for pull in normalized:
            print(_format_pull_list_item(pull), file=stdout)
    return 0


def _run_checks(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    cwd: str | None,
    stdout: TextIO,
) -> int:
    parsed = _parse_checks_args(argv)
    target_repo = parse_repo_spec(parsed.repo, default_host=repo.host) if parsed.repo else repo
    if target_repo is None:
        raise ValueError("could not parse --repo value")

    pull = _resolve_pull(target_repo, client, number=parsed.number, branch=None, cwd=cwd)
    statuses = [] if pull is None else _pull_statuses(target_repo, client, pull)
    checks = [filter_check_fields(check, parsed.json_fields) for check in normalize_pr_checks(statuses)]
    if parsed.json_fields:
        _print_json_or_jq_list(checks, parsed.jq, stdout)
    else:
        if not checks:
            print("no checks reported", file=stdout)
        for check in checks:
            print(_format_check_item(check), file=stdout)
    return 0


def _run_diff(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    cwd: str | None,
    stdout: TextIO,
) -> int:
    parsed = _parse_diff_args(argv)
    target_repo = parse_repo_spec(parsed.repo, default_host=repo.host) if parsed.repo else repo
    if target_repo is None:
        raise ValueError("could not parse --repo value")

    pull = _resolve_pull(target_repo, client, number=parsed.number, branch=parsed.branch, cwd=cwd)
    if pull is None:
        return 0
    number = _pull_number(pull)
    if number is None:
        return 0

    if parsed.web:
        url = str(normalize_pull(pull).get("url") or f"{target_repo.web_base_url}/pulls/{number}") + "/files"
        webbrowser.open(url)
        print(url, file=stdout)
        return 0

    if parsed.name_only:
        for name in _filtered_file_names(client.list_pull_files(target_repo, number), parsed.excludes):
            print(name, file=stdout)
        return 0

    text = client.get_pull_diff(target_repo, number, patch=parsed.patch)
    print(text, end="" if text.endswith("\n") else "\n", file=stdout)
    return 0


def _run_pr_comment(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    cwd: str | None,
    stdout: TextIO,
    stdin: TextIO | None,
) -> int:
    parsed = _parse_comment_args(argv, stdin=stdin)
    target_repo = parse_repo_spec(parsed.repo, default_host=repo.host) if parsed.repo else repo
    if target_repo is None:
        raise ValueError("could not parse --repo value")

    pull = _resolve_pull(target_repo, client, number=parsed.number, branch=parsed.branch, cwd=cwd)
    if pull is None:
        raise ValueError("could not find pull request to comment on")
    number = _pull_number(pull)
    if number is None:
        raise ValueError("could not determine pull request number")

    if parsed.web:
        url = str(normalize_pull(pull).get("url") or f"{target_repo.web_base_url}/pulls/{number}") + "#new-comment"
        webbrowser.open(url)
        print(url, file=stdout)
        return 0

    if parsed.body is None:
        raise ValueError("Forgejo PR comment requires --body or --body-file")
    client.create_issue_comment(target_repo, number, body=parsed.body)
    return 0


def _run_checkout(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    cwd: str | None,
    stdout: TextIO,
    stderr: TextIO,
) -> int:
    parsed = _parse_checkout_args(argv)
    target_repo = parse_repo_spec(parsed.repo, default_host=repo.host) if parsed.repo else repo
    if target_repo is None:
        raise ValueError("could not parse --repo value")

    pull = _resolve_pull(target_repo, client, number=parsed.number, branch=parsed.branch, cwd=cwd)
    if pull is None:
        raise ValueError("could not find pull request to check out")
    number = _pull_number(pull)
    head_ref = _pull_head_ref(pull)

    fetch_refs = []
    if head_ref:
        fetch_refs.append(head_ref)
    if number is not None:
        fetch_refs.append(f"refs/pull/{number}/head")

    fetched = False
    fetch_error = ""
    for fetch_ref in dict.fromkeys(fetch_refs):
        code, message = git_run(["fetch", "origin", fetch_ref], cwd=cwd)
        if code == 0:
            fetched = True
            break
        fetch_error = message

    if not fetched:
        print(fetch_error or "git fetch failed", file=stderr)
        return 1

    if parsed.detach:
        code, message = git_run(["checkout", "--detach", "FETCH_HEAD"], cwd=cwd)
        if code != 0:
            print(message or "git checkout failed", file=stderr)
            return code
        print("checked out FETCH_HEAD", file=stdout)
        return 0

    branch = parsed.local_branch or head_ref or (f"pull-{number}" if number is not None else None)
    if branch is None:
        raise ValueError("could not determine local branch name")

    if parsed.force:
        checkout_args = ["checkout", "-B", branch, "FETCH_HEAD"]
    else:
        exists, _ = git_run(["show-ref", "--verify", "--quiet", f"refs/heads/{branch}"], cwd=cwd)
        checkout_args = ["checkout", branch] if exists == 0 else ["checkout", "-b", branch, "FETCH_HEAD"]

    code, message = git_run(checkout_args, cwd=cwd)
    if code != 0:
        print(message or "git checkout failed", file=stderr)
        return code

    if parsed.recurse_submodules:
        code, message = git_run(["submodule", "update", "--init", "--recursive"], cwd=cwd)
        if code != 0:
            print(message or "git submodule update failed", file=stderr)
            return code

    print(f"checked out {branch}", file=stdout)
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
            _print_json_or_jq({}, parsed.jq, stdout, template=parsed.template)
        return 0

    normalized = normalize_pull(_enrich_pull_statuses(repo, client, pull, parsed.json_fields))
    if parsed.web:
        url = str(normalized.get("url") or "")
        if url:
            webbrowser.open(url)
            print(url, file=stdout)
        return 0
    if parsed.json_fields:
        _print_json_or_jq(filter_fields(normalized, parsed.json_fields), parsed.jq, stdout, template=parsed.template)
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
        _print_json_or_jq(filter_repo_fields(data, parsed.json_fields), parsed.jq, stdout, template=parsed.template)
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
            _print_json_or_jq(status_for_current_branch(None, parsed.json_fields), parsed.jq, stdout, template=parsed.template)
        return 0

    if parsed.json_fields:
        enriched = _enrich_pull_statuses(repo, client, pull, parsed.json_fields)
        _print_json_or_jq(status_for_current_branch(enriched, parsed.json_fields), parsed.jq, stdout, template=parsed.template)
    else:
        normalized = normalize_pull(pull)
        print(_format_pull_text(normalized), file=stdout)
    return 0


def _run_issue_list(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    stdout: TextIO,
) -> int:
    parsed = _parse_issue_list_args(argv)
    target_repo = parse_repo_spec(parsed.repo, default_host=repo.host) if parsed.repo else repo
    if target_repo is None:
        raise ValueError("could not parse --repo value")
    if parsed.web:
        url = f"{target_repo.web_base_url}/issues"
        webbrowser.open(url)
        print(url, file=stdout)
        return 0
    issues = client.list_issues(
        target_repo,
        state=parsed.state,
        labels=parsed.labels,
        query=parsed.query,
        milestone=parsed.milestone,
        author=parsed.author,
        assignee=parsed.assignee,
        mentioned=parsed.mentioned,
        limit=parsed.limit,
    )
    if parsed.limit is not None:
        issues = issues[: parsed.limit]

    normalized = [filter_issue_fields(normalize_issue(issue), parsed.json_fields) for issue in issues]
    if parsed.json_fields:
        _print_json_or_jq_list(normalized, parsed.jq, stdout, template=parsed.template)
    else:
        for issue in normalized:
            print(_format_issue_list_item(issue), file=stdout)
    return 0


def _run_issue_view(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    stdout: TextIO,
) -> int:
    parsed = _parse_issue_view_args(argv)
    target_repo = parse_repo_spec(parsed.repo, default_host=repo.host) if parsed.repo else repo
    if target_repo is None:
        raise ValueError("could not parse --repo value")
    if parsed.number is None:
        raise ValueError("Forgejo issue view requires an issue number or URL")
    issue = client.get_issue(target_repo, parsed.number)
    normalized = normalize_issue(issue)
    if parsed.web:
        url = str(normalized.get("url") or f"{target_repo.web_base_url}/issues/{parsed.number}")
        webbrowser.open(url)
        print(url, file=stdout)
        return 0
    if parsed.json_fields:
        _print_json_or_jq(filter_issue_fields(normalized, parsed.json_fields), parsed.jq, stdout, template=parsed.template)
    else:
        print(_format_issue_text(normalized), file=stdout)
    return 0


def _run_issue_create(
    argv: list[str],
    repo: RepoRef,
    client: ForgejoClient,
    *,
    stdout: TextIO,
    stdin: TextIO | None,
) -> int:
    parsed = _parse_issue_create_args(argv, stdin=stdin)
    target_repo = parse_repo_spec(parsed.repo, default_host=repo.host) if parsed.repo else repo
    if target_repo is None:
        raise ValueError("could not parse --repo value")
    if parsed.web:
        url = f"{target_repo.web_base_url}/issues/new"
        webbrowser.open(url)
        print(url, file=stdout)
        return 0
    if not parsed.title:
        raise ValueError("Forgejo issue create requires --title")
    issue = client.create_issue(
        target_repo,
        title=parsed.title,
        body=parsed.body or "",
        assignees=parsed.assignees,
        labels=_resolve_label_ids(client, target_repo, parsed.labels),
        milestone=parsed.milestone,
    )
    normalized = normalize_issue(issue)
    print(normalized.get("url") or issue.get("html_url") or issue.get("url") or "", file=stdout)
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


@dataclass(frozen=True)
class DiffArgs:
    repo: str | None = None
    number: int | None = None
    branch: str | None = None
    web: bool = False
    name_only: bool = False
    patch: bool = False
    excludes: tuple[str, ...] = ()


@dataclass(frozen=True)
class CommentArgs:
    repo: str | None = None
    number: int | None = None
    branch: str | None = None
    body: str | None = None
    web: bool = False


@dataclass(frozen=True)
class CheckoutArgs:
    repo: str | None = None
    number: int | None = None
    branch: str | None = None
    local_branch: str | None = None
    detach: bool = False
    force: bool = False
    recurse_submodules: bool = False


@dataclass(frozen=True)
class IssueListArgs:
    json_fields: tuple[str, ...] = ()
    repo: str | None = None
    jq: str | None = None
    template: str | None = None
    state: str = "open"
    limit: int | None = 30
    labels: tuple[str, ...] = ()
    query: str | None = None
    author: str | None = None
    assignee: str | None = None
    mentioned: str | None = None
    milestone: str | None = None
    web: bool = False


@dataclass(frozen=True)
class IssueViewArgs:
    json_fields: tuple[str, ...] = ()
    repo: str | None = None
    jq: str | None = None
    template: str | None = None
    number: int | None = None
    web: bool = False
    comments: bool = False


@dataclass(frozen=True)
class IssueCreateArgs:
    title: str | None = None
    body: str | None = None
    repo: str | None = None
    web: bool = False
    assignees: tuple[str, ...] = ()
    labels: tuple[str, ...] = ()
    milestone: int | None = None


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
        elif arg in {"--template", "-t", "--interval", "-i"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            index += 2
        elif arg.startswith(("--template=", "-t=", "--interval=", "-i=")):
            index += 1
        elif arg in {"--web", "-w"}:
            index += 1
        elif arg in {"--watch", "--fail-fast", "--required"}:
            index += 1
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo PR checks flag: {arg}")
        else:
            parsed_number, _ = _parse_number_or_branch(arg, kind="pull")
            if parsed_number is not None:
                number = parsed_number
            index += 1
    return ChecksArgs(json_fields=json_fields, repo=repo, jq=jq, number=number)


def _parse_diff_args(argv: list[str]) -> DiffArgs:
    repo = None
    number = None
    branch = None
    web = False
    name_only = False
    patch = False
    excludes: list[str] = []
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in {"-R", "--repo"}:
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
        elif arg == "--name-only":
            name_only = True
            index += 1
        elif arg == "--patch":
            patch = True
            index += 1
        elif arg == "--color":
            if index + 1 >= len(argv):
                raise ValueError("missing value for --color")
            index += 2
        elif arg.startswith("--color="):
            index += 1
        elif arg in {"--exclude", "-e"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            excludes.append(argv[index + 1])
            index += 2
        elif arg.startswith("--exclude=") or arg.startswith("-e="):
            excludes.append(arg.split("=", 1)[1])
            index += 1
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo PR diff flag: {arg}")
        else:
            number, branch = _parse_number_or_branch(arg, kind="pull")
            index += 1
    return DiffArgs(repo=repo, number=number, branch=branch, web=web, name_only=name_only, patch=patch, excludes=tuple(excludes))


def _parse_comment_args(argv: list[str], *, stdin: TextIO | None = None) -> CommentArgs:
    repo = None
    number = None
    branch = None
    body = None
    web = False
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in {"-R", "--repo"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            repo = argv[index + 1]
            index += 2
        elif arg.startswith("--repo="):
            repo = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--body", "-b"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            body = argv[index + 1]
            index += 2
        elif arg.startswith("--body=") or arg.startswith("-b="):
            body = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--body-file", "-F"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            body = body_from_file(argv[index + 1], stdin=stdin)
            index += 2
        elif arg.startswith("--body-file=") or arg.startswith("-F="):
            body = body_from_file(arg.split("=", 1)[1], stdin=stdin)
            index += 1
        elif arg in {"--web", "-w"}:
            web = True
            index += 1
        elif arg in {"--create-if-none", "--delete-last", "--edit-last", "--editor", "-e", "--yes"}:
            raise ValueError(f"unsupported Forgejo PR comment flag: {arg}")
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo PR comment flag: {arg}")
        else:
            number, branch = _parse_number_or_branch(arg, kind="pull")
            index += 1
    return CommentArgs(repo=repo, number=number, branch=branch, body=body, web=web)


def _parse_checkout_args(argv: list[str]) -> CheckoutArgs:
    repo = None
    number = None
    branch = None
    local_branch = None
    detach = False
    force = False
    recurse_submodules = False
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in {"-R", "--repo"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            repo = argv[index + 1]
            index += 2
        elif arg.startswith("--repo="):
            repo = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--branch", "-b"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            local_branch = argv[index + 1]
            index += 2
        elif arg.startswith("--branch=") or arg.startswith("-b="):
            local_branch = arg.split("=", 1)[1]
            index += 1
        elif arg == "--detach":
            detach = True
            index += 1
        elif arg in {"--force", "-f"}:
            force = True
            index += 1
        elif arg == "--recurse-submodules":
            recurse_submodules = True
            index += 1
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo PR checkout flag: {arg}")
        else:
            number, branch = _parse_number_or_branch(arg, kind="pull")
            index += 1
    return CheckoutArgs(
        repo=repo,
        number=number,
        branch=branch,
        local_branch=local_branch,
        detach=detach,
        force=force,
        recurse_submodules=recurse_submodules,
    )


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
            number, branch = _parse_number_or_branch(arg, kind="pull")
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
        elif arg in {"--branch", "-b"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            index += 2
        elif arg.startswith("--branch=") or arg.startswith("-b="):
            index += 1
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo repo view flag: {arg}")
        else:
            repo = arg
            index += 1
    return RepoViewArgs(json_fields=json_fields, repo=repo, web=web, jq=jq, template=template)


def _parse_issue_list_args(argv: list[str]) -> IssueListArgs:
    json_fields: tuple[str, ...] = ()
    repo = None
    jq = None
    template = None
    state = "open"
    limit: int | None = 30
    labels: list[str] = []
    query = None
    author = None
    assignee = None
    mentioned = None
    milestone = None
    web = False
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
            state = _normalize_issue_state(argv[index + 1])
            index += 2
        elif arg.startswith("--state=") or arg.startswith("-s="):
            state = _normalize_issue_state(arg.split("=", 1)[1])
            index += 1
        elif arg in {"--limit", "-L"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            limit = _parse_limit(argv[index + 1])
            index += 2
        elif arg.startswith("--limit=") or arg.startswith("-L="):
            limit = _parse_limit(arg.split("=", 1)[1])
            index += 1
        elif arg in {"--label", "-l"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            labels.extend(_split_label_values(argv[index + 1]))
            index += 2
        elif arg.startswith("--label=") or arg.startswith("-l="):
            labels.extend(_split_label_values(arg.split("=", 1)[1]))
            index += 1
        elif arg in {"--search", "-S"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            query = argv[index + 1]
            index += 2
        elif arg.startswith("--search=") or arg.startswith("-S="):
            query = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--author", "-A"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            author = argv[index + 1]
            index += 2
        elif arg.startswith("--author=") or arg.startswith("-A="):
            author = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--assignee", "-a"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            assignee = argv[index + 1]
            index += 2
        elif arg.startswith("--assignee=") or arg.startswith("-a="):
            assignee = arg.split("=", 1)[1]
            index += 1
        elif arg == "--mention":
            if index + 1 >= len(argv):
                raise ValueError("missing value for --mention")
            mentioned = argv[index + 1]
            index += 2
        elif arg.startswith("--mention="):
            mentioned = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--milestone", "-m"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            milestone = argv[index + 1]
            index += 2
        elif arg.startswith("--milestone=") or arg.startswith("-m="):
            milestone = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--web", "-w"}:
            web = True
            index += 1
        elif arg == "--app":
            if index + 1 >= len(argv):
                raise ValueError("missing value for --app")
            index += 2
        elif arg.startswith("--app="):
            index += 1
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo issue list flag: {arg}")
        else:
            index += 1
    return IssueListArgs(
        json_fields=json_fields,
        repo=repo,
        jq=jq,
        template=template,
        state=state,
        limit=limit,
        labels=tuple(labels),
        query=query,
        author=author,
        assignee=assignee,
        mentioned=mentioned,
        milestone=milestone,
        web=web,
    )


def _parse_issue_view_args(argv: list[str]) -> IssueViewArgs:
    json_fields: tuple[str, ...] = ()
    repo = None
    jq = None
    template = None
    number = None
    web = False
    comments = False
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
        elif arg in {"--web", "-w"}:
            web = True
            index += 1
        elif arg in {"--comments", "-c"}:
            comments = True
            index += 1
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo issue view flag: {arg}")
        else:
            parsed_number, _ = _parse_number_or_branch(arg, kind="issue")
            number = parsed_number
            index += 1
    return IssueViewArgs(json_fields=json_fields, repo=repo, jq=jq, template=template, number=number, web=web, comments=comments)


def _parse_issue_create_args(argv: list[str], *, stdin: TextIO | None = None) -> IssueCreateArgs:
    title = None
    body = None
    repo = None
    web = False
    assignees: list[str] = []
    labels: list[str] = []
    milestone = None
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in {"--title", "-t"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            title = argv[index + 1]
            index += 2
        elif arg.startswith("--title=") or arg.startswith("-t="):
            title = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--body", "-b"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            body = argv[index + 1]
            index += 2
        elif arg.startswith("--body=") or arg.startswith("-b="):
            body = arg.split("=", 1)[1]
            index += 1
        elif arg in {"--body-file", "-F"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            body = body_from_file(argv[index + 1], stdin=stdin)
            index += 2
        elif arg.startswith("--body-file=") or arg.startswith("-F="):
            body = body_from_file(arg.split("=", 1)[1], stdin=stdin)
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
        elif arg in {"--assignee", "-a"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            assignees.extend(_split_label_values(argv[index + 1]))
            index += 2
        elif arg.startswith("--assignee=") or arg.startswith("-a="):
            assignees.extend(_split_label_values(arg.split("=", 1)[1]))
            index += 1
        elif arg in {"--label", "-l"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            labels.extend(_split_label_values(argv[index + 1]))
            index += 2
        elif arg.startswith("--label=") or arg.startswith("-l="):
            labels.extend(_split_label_values(arg.split("=", 1)[1]))
            index += 1
        elif arg in {"--milestone", "-m"}:
            if index + 1 >= len(argv):
                raise ValueError(f"missing value for {arg}")
            milestone = _optional_int(argv[index + 1])
            index += 2
        elif arg.startswith("--milestone=") or arg.startswith("-m="):
            milestone = _optional_int(arg.split("=", 1)[1])
            index += 1
        elif arg in {"--editor", "-e", "--project", "-p", "--recover", "--template", "-T"}:
            raise ValueError(f"unsupported Forgejo issue create flag: {arg}")
        elif arg.startswith(("--project=", "-p=", "--recover=", "--template=", "-T=")):
            raise ValueError(f"unsupported Forgejo issue create flag: {arg.split('=', 1)[0]}")
        elif arg.startswith("-"):
            raise ValueError(f"unsupported Forgejo issue create flag: {arg}")
        else:
            raise ValueError(f"unexpected positional argument for Forgejo issue create: {arg}")
    return IssueCreateArgs(
        title=title,
        body=body,
        repo=repo,
        web=web,
        assignees=tuple(assignee for assignee in assignees if assignee != "@me"),
        labels=tuple(labels),
        milestone=milestone,
    )


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


def _format_issue_text(data: dict[str, object]) -> str:
    number = data.get("number")
    title = data.get("title") or ""
    state = data.get("state") or "UNKNOWN"
    url = data.get("url") or ""
    body = data.get("body") or ""
    return f"#{number} {title}\nstate: {state}\nurl: {url}\n\n{body}".rstrip()


def _format_issue_list_item(data: dict[str, object]) -> str:
    number = data.get("number")
    title = data.get("title") or ""
    state = data.get("state") or ""
    return f"{number}\t{title}\t{state}".rstrip()


def _format_check_item(data: dict[str, object]) -> str:
    bucket = data.get("bucket") or "pending"
    name = data.get("name") or "status"
    description = data.get("description") or ""
    return f"{bucket}\t{name}\t{description}".rstrip()


_TEMPLATE_FIELD_RE = re.compile(r"{{\s*\.([A-Za-z0-9_][A-Za-z0-9_.]*)?\s*}}")


def _print_json_or_jq(
    data: dict[str, object],
    jq: str | None,
    stdout: TextIO,
    *,
    template: str | None = None,
) -> None:
    value: object = data
    if jq:
        value = _apply_simple_jq(value, jq)
        if value is None:
            return
    if template:
        stdout.write(_render_simple_template(value, template))
        return
    if isinstance(value, str):
        print(value, file=stdout)
    else:
        print(json.dumps(value, sort_keys=True), file=stdout)


def _print_json_or_jq_list(
    data: list[dict[str, object]],
    jq: str | None,
    stdout: TextIO,
    *,
    template: str | None = None,
) -> None:
    value: object = data
    if jq:
        value = _apply_simple_jq(value, jq)
        if value is None:
            return
    if template:
        stdout.write(_render_simple_template(value, template))
        return
    if isinstance(value, str):
        print(value, file=stdout)
    else:
        print(json.dumps(value, sort_keys=True), file=stdout)


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


def _render_simple_template(data: object, template: str) -> str:
    def replace(match: re.Match[str]) -> str:
        query = match.group(1)
        value = data if not query else _lookup_dotted(data, query)
        if value is None:
            return ""
        if isinstance(value, str):
            return value
        if isinstance(value, bool):
            return "true" if value else "false"
        if isinstance(value, (int, float)):
            return str(value)
        return json.dumps(value, sort_keys=True)

    return _TEMPLATE_FIELD_RE.sub(replace, template)


def _lookup_dotted(data: object, query: str) -> object:
    value = data
    for part in query.split("."):
        if not part:
            continue
        if isinstance(value, dict):
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


def _normalize_issue_state(value: str) -> str:
    normalized = value.lower()
    if normalized not in {"open", "closed", "all"}:
        raise ValueError("--state must be one of: open, closed, all")
    return normalized


def _split_label_values(value: str) -> list[str]:
    return [item.strip() for item in value.split(",") if item.strip()]


def _optional_int(value: str) -> int | None:
    try:
        return int(value)
    except ValueError:
        return None


def _resolve_label_ids(client: ForgejoClient, repo: RepoRef, labels: tuple[str, ...]) -> tuple[int, ...]:
    if not labels:
        return ()
    result: list[int] = []
    unresolved = [label for label in labels if _optional_int(label) is None]
    label_map: dict[str, int] = {}
    if unresolved:
        for label in client.list_labels(repo):
            raw_name = label.get("name")
            raw_id = label.get("id")
            if isinstance(raw_name, str) and isinstance(raw_id, int):
                label_map[raw_name] = raw_id
    for label in labels:
        parsed = _optional_int(label)
        if parsed is not None:
            result.append(parsed)
        elif label in label_map:
            result.append(label_map[label])
        else:
            raise ValueError(f"could not resolve Forgejo issue label: {label}")
    return tuple(result)


def _parse_number_or_branch(value: str, *, kind: str) -> tuple[int | None, str | None]:
    number = _number_from_url(value, kind=kind)
    if number is not None:
        return number, None
    if value.isdigit():
        return int(value), None
    return None, value


def _number_from_url(value: str, *, kind: str) -> int | None:
    parsed = urllib.parse.urlparse(value)
    if parsed.scheme not in {"http", "https"}:
        return None
    parts = [part for part in parsed.path.strip("/").split("/") if part]
    markers = ("pulls", "pull") if kind == "pull" else ("issues", "issue")
    for index, part in enumerate(parts):
        if part in markers and index + 1 < len(parts) and parts[index + 1].isdigit():
            return int(parts[index + 1])
    return None


def _resolve_pull(
    repo: RepoRef,
    client: ForgejoClient,
    *,
    number: int | None,
    branch: str | None,
    cwd: str | None,
) -> dict[str, object] | None:
    if number is not None:
        return client.get_pull(repo, number)
    target_branch = branch or current_branch(cwd=cwd)
    if not target_branch:
        return None
    pulls = client.list_pulls(repo, state="open", head=target_branch)
    return pulls[0] if pulls else None


def _enrich_pull_statuses(
    repo: RepoRef,
    client: ForgejoClient,
    pull: dict[str, object],
    fields: tuple[str, ...],
) -> dict[str, object]:
    if "statusCheckRollup" not in fields:
        return pull
    statuses = _pull_statuses(repo, client, pull)
    return with_status_check_rollup(pull, statuses)


def _pull_statuses(repo: RepoRef, client: ForgejoClient, pull: dict[str, object]) -> list[dict[str, object]]:
    sha = _pull_head_sha(pull)
    if sha is None:
        return []
    return client.list_commit_statuses(repo, sha)


def _pull_number(pull: dict[str, object]) -> int | None:
    raw = pull.get("number") or pull.get("id")
    if isinstance(raw, bool):
        return None
    if isinstance(raw, int):
        return raw
    if isinstance(raw, str) and raw.isdigit():
        return int(raw)
    return None


def _pull_head_sha(pull: dict[str, object]) -> str | None:
    head = pull.get("head")
    if not isinstance(head, dict):
        return None
    sha = head.get("sha")
    return sha if isinstance(sha, str) and sha else None


def _pull_head_ref(pull: dict[str, object]) -> str | None:
    head = pull.get("head")
    if not isinstance(head, dict):
        return None
    ref = head.get("ref")
    return ref if isinstance(ref, str) and ref else None


def _filtered_file_names(files: list[dict[str, object]], excludes: tuple[str, ...]) -> list[str]:
    names = []
    for item in files:
        raw = item.get("filename") or item.get("name") or item.get("path")
        if not isinstance(raw, str) or _matches_any(raw, excludes):
            continue
        names.append(raw)
    return names


def _matches_any(value: str, patterns: tuple[str, ...]) -> bool:
    return any(fnmatch.fnmatch(value, pattern) for pattern in patterns)
