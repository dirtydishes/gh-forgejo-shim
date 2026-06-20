from __future__ import annotations

import json
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping


SLOW_CALL_MS = 1000.0


@dataclass(frozen=True)
class TraceSummary:
    total: int
    failures: tuple[dict[str, Any], ...]
    slow_calls: tuple[dict[str, Any], ...]
    unsupported: tuple[dict[str, Any], ...]
    probe_counts: Counter[str]
    route_counts: Counter[str]
    parse_errors: tuple[str, ...] = ()


def load_trace(path: Path) -> tuple[list[dict[str, Any]], tuple[str, ...]]:
    records: list[dict[str, Any]] = []
    errors: list[str] = []
    with path.open("r", encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, start=1):
            stripped = line.strip()
            if not stripped:
                continue
            try:
                item = json.loads(stripped)
            except json.JSONDecodeError as exc:
                errors.append(f"line {line_number}: {exc.msg}")
                continue
            if isinstance(item, dict):
                records.append(item)
            else:
                errors.append(f"line {line_number}: expected object")
    return records, tuple(errors)


def summarize_trace(records: Iterable[Mapping[str, Any]], *, slow_ms: float = SLOW_CALL_MS) -> TraceSummary:
    materialized = [dict(record) for record in records]
    failures = tuple(record for record in materialized if _exit_code(record) not in {None, 0})
    slow_calls = tuple(record for record in materialized if _duration_ms(record) >= slow_ms)
    unsupported = tuple(record for record in materialized if _is_unsupported(record))
    probe_counts: Counter[str] = Counter()
    route_counts: Counter[str] = Counter()
    for record in materialized:
        probe = classify_probe(record)
        if probe:
            probe_counts[probe] += 1
        route = _route_kind(record)
        if route:
            route_counts[route] += 1
    return TraceSummary(
        total=len(materialized),
        failures=failures,
        slow_calls=slow_calls,
        unsupported=unsupported,
        probe_counts=probe_counts,
        route_counts=route_counts,
    )


def summarize_trace_file(path: Path, *, slow_ms: float = SLOW_CALL_MS) -> TraceSummary:
    records, errors = load_trace(path)
    summary = summarize_trace(records, slow_ms=slow_ms)
    return TraceSummary(
        total=summary.total,
        failures=summary.failures,
        slow_calls=summary.slow_calls,
        unsupported=summary.unsupported,
        probe_counts=summary.probe_counts,
        route_counts=summary.route_counts,
        parse_errors=errors,
    )


def format_trace_summary(summary: TraceSummary) -> str:
    lines = [f"Trace records: {summary.total}"]
    if summary.parse_errors:
        lines.append(f"Parse errors: {len(summary.parse_errors)}")
        lines.extend(f"  - {error}" for error in summary.parse_errors[:5])
    lines.append(_format_counter("Routes", summary.route_counts))
    lines.append(_format_counter("Known Codex probes", summary.probe_counts))
    lines.append(_format_records("Failures", summary.failures))
    lines.append(_format_records("Slow calls", summary.slow_calls))
    lines.append(_format_records("Unsupported/delegated commands", summary.unsupported))
    return "\n".join(line for line in lines if line)


def classify_probe(record: Mapping[str, Any]) -> str | None:
    kind = record.get("kind")
    argv = _argv(record)
    if kind == "git":
        return _classify_git_probe(argv)
    if kind != "gh":
        return None
    if argv in (["--version"], ["version"]):
        return "gh --version"
    if len(argv) >= 2 and argv[:2] == ["auth", "status"]:
        return "gh auth status"
    if len(argv) >= 2 and argv[0] == "api" and _api_endpoint(argv[1:]) in {"user", "/user"}:
        return "gh api user"
    if len(argv) >= 2 and argv[:2] == ["pr", "status"]:
        return "gh pr status"
    if len(argv) >= 2 and argv[:2] == ["pr", "list"]:
        return "gh pr list"
    if len(argv) >= 2 and argv[:2] == ["repo", "view"]:
        return "gh repo view"
    return None


def _classify_git_probe(argv: list[str]) -> str | None:
    if argv == ["rev-parse", "--show-toplevel"]:
        return "git repo root"
    if argv[:2] == ["remote", "-v"] or argv[:1] == ["remote"]:
        return "git remotes"
    if argv[:3] == ["symbolic-ref", "--quiet", "--short"] and "refs/remotes/origin/HEAD" in argv:
        return "git default branch"
    if argv == ["branch", "--show-current"]:
        return "git current branch"
    if argv[:3] == ["rev-parse", "--abbrev-ref", "--symbolic-full-name"]:
        return "git upstream"
    if argv[:1] == ["for-each-ref"]:
        return "git refs"
    return None


def _format_counter(title: str, counter: Counter[str]) -> str:
    if not counter:
        return f"{title}: none"
    items = ", ".join(f"{name}={count}" for name, count in counter.most_common())
    return f"{title}: {items}"


def _format_records(title: str, records: tuple[dict[str, Any], ...]) -> str:
    if not records:
        return f"{title}: none"
    lines = [f"{title}: {len(records)}"]
    for record in records[:8]:
        lines.append(
            "  - "
            f"{record.get('kind', '?')} {_command_text(record)} "
            f"exit={_exit_code(record)} duration={_duration_ms(record):.0f}ms "
            f"route={_route_kind(record) or '-'}"
        )
    if len(records) > 8:
        lines.append(f"  - ... {len(records) - 8} more")
    return "\n".join(lines)


def _argv(record: Mapping[str, Any]) -> list[str]:
    value = record.get("argv")
    if isinstance(value, list):
        return [str(item) for item in value]
    command = record.get("command")
    if isinstance(command, list):
        return [str(item) for item in command]
    return []


def _command_text(record: Mapping[str, Any]) -> str:
    return " ".join(_argv(record))


def _exit_code(record: Mapping[str, Any]) -> int | None:
    value = record.get("exit_code")
    return value if isinstance(value, int) else None


def _duration_ms(record: Mapping[str, Any]) -> float:
    value = record.get("duration_ms")
    if isinstance(value, (int, float)):
        return float(value)
    return 0.0


def _route_kind(record: Mapping[str, Any]) -> str | None:
    route = record.get("route")
    if isinstance(route, dict):
        value = route.get("kind")
        return str(value) if value is not None else None
    value = record.get("route_kind")
    return str(value) if value is not None else None


def _route_reason(record: Mapping[str, Any]) -> str:
    route = record.get("route")
    if isinstance(route, dict):
        value = route.get("reason")
        return str(value) if value is not None else ""
    value = record.get("route_reason")
    return str(value) if value is not None else ""


def _is_unsupported(record: Mapping[str, Any]) -> bool:
    reason = _route_reason(record).lower()
    if "unsupported" in reason:
        return True
    return _route_kind(record) == "delegate" and "unsupported" in reason


_API_FLAGS_WITH_VALUES = {
    "--cache",
    "-F",
    "--field",
    "-H",
    "--header",
    "--hostname",
    "--input",
    "--method",
    "-X",
    "--preview",
    "-p",
    "-f",
    "--raw-field",
    "--jq",
    "-q",
    "--template",
    "-t",
}


def _api_endpoint(argv: list[str]) -> str | None:
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg == "--":
            return argv[index + 1] if index + 1 < len(argv) else None
        if arg in _API_FLAGS_WITH_VALUES:
            index += 2
        elif arg.startswith("-"):
            index += 1
        else:
            return arg
    return None
