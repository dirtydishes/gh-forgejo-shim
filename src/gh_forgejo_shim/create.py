from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import TextIO


class CreateParseError(ValueError):
    pass


@dataclass(frozen=True)
class CreateOptions:
    title: str | None = None
    body: str | None = None
    body_file: str | None = None
    base: str | None = None
    head: str | None = None
    repo: str | None = None
    fill: bool = False
    fill_first: bool = False
    fill_verbose: bool = False
    web: bool = False
    draft: bool = False
    json_fields: tuple[str, ...] = ()


FLAGS_WITH_VALUES = {
    "--title": "title",
    "-t": "title",
    "--body": "body",
    "-b": "body",
    "--body-file": "body_file",
    "-F": "body_file",
    "--base": "base",
    "-B": "base",
    "--head": "head",
    "-H": "head",
    "--repo": "repo",
    "-R": "repo",
    "--json": "json_fields",
}

BOOL_FLAGS = {
    "--fill": "fill",
    "--fill-first": "fill_first",
    "--fill-verbose": "fill_verbose",
    "--web": "web",
    "-w": "web",
    "--draft": "draft",
    "-d": "draft",
}

UNSUPPORTED_WITH_VALUES = {
    "--reviewer",
    "--reviewers",
    "-r",
    "--assignee",
    "--assignees",
    "-a",
    "--label",
    "--labels",
    "-l",
    "--project",
    "--projects",
    "-p",
    "--milestone",
    "-m",
    "--template",
    "-T",
    "--recover",
}

UNSUPPORTED_BOOL_FLAGS = {
    "--maintainer-can-modify",
    "--no-maintainer-edit",
    "--no-maintainer-can-modify",
}


def parse_create_args(argv: list[str], *, stdin: TextIO | None = None) -> CreateOptions:
    values: dict[str, object] = {
        "title": None,
        "body": None,
        "body_file": None,
        "base": None,
        "head": None,
        "repo": None,
        "fill": False,
        "fill_first": False,
        "fill_verbose": False,
        "web": False,
        "draft": False,
        "json_fields": (),
    }

    index = 0
    while index < len(argv):
        arg = argv[index]

        if arg in UNSUPPORTED_BOOL_FLAGS:
            raise CreateParseError(f"unsupported Forgejo PR create flag: {arg}")

        if arg in UNSUPPORTED_WITH_VALUES:
            raise CreateParseError(f"unsupported Forgejo PR create flag: {arg}")

        if "=" in arg:
            flag, raw_value = arg.split("=", 1)
            if flag in UNSUPPORTED_WITH_VALUES:
                raise CreateParseError(f"unsupported Forgejo PR create flag: {flag}")
            if flag in FLAGS_WITH_VALUES:
                _set_value(values, FLAGS_WITH_VALUES[flag], raw_value, stdin)
                index += 1
                continue

        if arg in FLAGS_WITH_VALUES:
            if index + 1 >= len(argv):
                raise CreateParseError(f"missing value for {arg}")
            _set_value(values, FLAGS_WITH_VALUES[arg], argv[index + 1], stdin)
            index += 2
            continue

        if arg in BOOL_FLAGS:
            values[BOOL_FLAGS[arg]] = True
            index += 1
            continue

        if arg.startswith("-"):
            raise CreateParseError(f"unsupported Forgejo PR create flag: {arg}")

        raise CreateParseError(f"unexpected positional argument for Forgejo PR create: {arg}")

    return CreateOptions(**values)


def body_from_file(path_value: str, *, stdin: TextIO | None = None) -> str:
    if path_value == "-":
        if stdin is None:
            raise CreateParseError("cannot read --body-file - without stdin")
        return stdin.read()
    path = Path(path_value)
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        raise CreateParseError(f"could not read body file {path}: {exc}") from exc


def _set_value(
    values: dict[str, object],
    name: str,
    raw_value: str,
    stdin: TextIO | None,
) -> None:
    if name == "json_fields":
        fields = tuple(item.strip() for item in raw_value.split(",") if item.strip())
        values[name] = fields
    elif name == "body_file":
        values[name] = raw_value
        values["body"] = body_from_file(raw_value, stdin=stdin)
    else:
        values[name] = raw_value
