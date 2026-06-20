from __future__ import annotations

import json
import os
import re
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Mapping, Sequence, TextIO

TRACE_ENV = "FJ_SHIM_TRACE"
TRACE_BODY_ENV = "FJ_SHIM_TRACE_BODY"
DEFAULT_MAX_EXCERPT_CHARS = 4096
REDACTED = "<redacted>"
AUTH_TOKEN_REDACTED = "[redacted: gh auth token output]"

_FALSE_VALUES = {"", "0", "false", "no", "off"}
_TRUE_VALUES = {"1", "true", "yes", "on"}
_SENSITIVE_FLAG_NAMES = {
    "--authorization",
    "--password",
    "--secret",
    "--token",
}
_SENSITIVE_ENV_NAMES = (
    "FJ_SHIM_TOKEN",
    "FORGEJO_TOKEN",
    "GITEA_TOKEN",
    "FJ_TOKEN",
    "GH_TOKEN",
    "GITHUB_TOKEN",
)
_TOKEN_PREFIX_RE = re.compile(
    r"\b(?:gh[pousr]_[A-Za-z0-9_]{8,}|github_pat_[A-Za-z0-9_]{20,})\b"
)
_AUTH_HEADER_RE = re.compile(
    r"(?i)\b(authorization\s*:\s*)(bearer|token|basic)\s+([^\s,;]+)"
)
_SENSITIVE_KEY_RE = re.compile(
    r"(?i)(?<![A-Za-z0-9_.-])(['\"]?[A-Za-z0-9_.-]*"
    r"(?:authorization|credential|password|secret|token)[A-Za-z0-9_.-]*"
    r"['\"]?\s*[:=]\s*)(['\"]?)([^'\"\s,}]+)(['\"]?)"
)
_URL_CREDENTIAL_RE = re.compile(r"(?i)\b(https?://[^/\s:@]+:)([^@\s/]+)(@)")
_ENV_ASSIGNMENT_RE = re.compile(
    r"(?i)\b("
    + "|".join(re.escape(name) for name in _SENSITIVE_ENV_NAMES)
    + r")=([^\s]+)"
)


@dataclass(frozen=True)
class TraceRepo:
    host: str
    owner: str
    name: str

    @property
    def full_name(self) -> str:
        return f"{self.owner}/{self.name}"

    def to_dict(self) -> dict[str, str]:
        return {
            "host": self.host,
            "owner": self.owner,
            "name": self.name,
            "full_name": self.full_name,
        }


@dataclass(frozen=True)
class TraceRoute:
    kind: str
    reason: str = ""
    host: str | None = None
    repo: TraceRepo | None = None

    def to_dict(self) -> dict[str, str]:
        data = {"kind": self.kind}
        if self.reason:
            data["reason"] = self.reason
        return data


@dataclass(frozen=True)
class TraceRecorder:
    path: Path | None = None
    include_body: bool = False
    clock: Callable[[], datetime] = field(default_factory=lambda: _utc_now)
    max_excerpt_chars: int = DEFAULT_MAX_EXCERPT_CHARS

    @classmethod
    def from_env(
        cls,
        env: Mapping[str, str] | None = None,
        *,
        clock: Callable[[], datetime] | None = None,
        max_excerpt_chars: int = DEFAULT_MAX_EXCERPT_CHARS,
    ) -> TraceRecorder:
        values = env if env is not None else os.environ
        path_value = values.get(TRACE_ENV, "").strip()
        if not path_value or path_value.lower() in _FALSE_VALUES:
            return cls(None, clock=clock or _utc_now, max_excerpt_chars=max_excerpt_chars)
        return cls(
            Path(path_value).expanduser(),
            include_body=_env_enabled(values.get(TRACE_BODY_ENV)),
            clock=clock or _utc_now,
            max_excerpt_chars=max_excerpt_chars,
        )

    @property
    def enabled(self) -> bool:
        return self.path is not None

    def emit(
        self,
        *,
        argv: Sequence[str],
        route: TraceRoute | Mapping[str, object] | object | None,
        cwd: str | Path | None = None,
        duration_ms: int | float | None = None,
        duration_seconds: float | None = None,
        exit_code: int | None = None,
        stdout: str | bytes | None = None,
        stderr: str | bytes | None = None,
        stdout_bytes: int | None = None,
        stderr_bytes: int | None = None,
    ) -> bool:
        if self.path is None:
            return False

        record = build_record(
            argv=argv,
            route=route,
            cwd=cwd,
            timestamp=self.clock(),
            duration_ms=duration_ms,
            duration_seconds=duration_seconds,
            exit_code=exit_code,
            stdout=stdout,
            stderr=stderr,
            stdout_bytes=stdout_bytes,
            stderr_bytes=stderr_bytes,
            include_body=self.include_body,
            max_excerpt_chars=self.max_excerpt_chars,
        )
        try:
            append_jsonl(self.path, record)
        except OSError:
            return False
        return True


def build_record(
    *,
    kind: str | None = None,
    argv: Sequence[str],
    route: TraceRoute | Mapping[str, object] | object | None,
    host: str | None = None,
    repo: TraceRepo | Mapping[str, object] | object | None = None,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    timestamp: datetime | None = None,
    duration_ms: int | float | None = None,
    duration_seconds: float | None = None,
    exit_code: int | None = None,
    stdout: str | bytes | None = None,
    stderr: str | bytes | None = None,
    stdout_summary: Mapping[str, object] | None = None,
    stderr_summary: Mapping[str, object] | None = None,
    stdout_bytes: int | None = None,
    stderr_bytes: int | None = None,
    include_body: bool = False,
    max_excerpt_chars: int = DEFAULT_MAX_EXCERPT_CHARS,
) -> dict[str, object]:
    trace_route = _merge_route(route, host=host, repo=repo)
    timestamp_value = timestamp or _utc_now()
    cwd_value = str(cwd) if cwd is not None else os.getcwd()
    argv_value = redact_argv(argv)
    include_output_body = include_body or _env_enabled((env or {}).get(TRACE_BODY_ENV))

    record: dict[str, object] = {
        "timestamp": _format_timestamp(timestamp_value),
        "cwd": cwd_value,
        "argv": argv_value,
        "route": trace_route.to_dict(),
        "host": trace_route.host,
        "repo": trace_route.repo.to_dict() if trace_route.repo else None,
        "duration_ms": _duration_ms(duration_ms, duration_seconds),
        "exit_code": exit_code,
        "stdout": _summary_or_stream_record(
            stdout_summary,
            stdout,
            provided_bytes=stdout_bytes,
            include_body=include_output_body,
            max_excerpt_chars=max_excerpt_chars,
            suppress_body=_is_auth_token_command(argv),
        ),
        "stderr": _summary_or_stream_record(
            stderr_summary,
            stderr,
            provided_bytes=stderr_bytes,
            include_body=include_output_body,
            max_excerpt_chars=max_excerpt_chars,
            suppress_body=False,
        ),
    }
    if kind:
        record["kind"] = kind
    return record


def tracing_enabled(env: Mapping[str, str] | None = None) -> bool:
    values = env if env is not None else os.environ
    value = values.get(TRACE_ENV, "").strip()
    return bool(value) and value.lower() not in _FALSE_VALUES


def suppress_stdout_body(argv: Sequence[str]) -> bool:
    return _is_auth_token_command(argv)


def capture_streams(
    *,
    stdout: TextIO | None = None,
    stderr: TextIO | None = None,
    env: Mapping[str, str] | None = None,
    redact_stdout_body: bool = False,
    max_excerpt_chars: int = DEFAULT_MAX_EXCERPT_CHARS,
) -> tuple[TraceCapture, TraceCapture]:
    values = env if env is not None else os.environ
    include_body = _env_enabled(values.get(TRACE_BODY_ENV))
    return (
        TraceCapture(
            stdout or sys.stdout,
            include_body=include_body,
            redact_body=redact_stdout_body,
            max_excerpt_chars=max_excerpt_chars,
        ),
        TraceCapture(
            stderr or sys.stderr,
            include_body=include_body,
            redact_body=False,
            max_excerpt_chars=max_excerpt_chars,
        ),
    )


def append_trace(record: Mapping[str, object], env: Mapping[str, str] | None = None) -> bool:
    values = env if env is not None else os.environ
    path_value = values.get(TRACE_ENV, "").strip()
    if not path_value or path_value.lower() in _FALSE_VALUES:
        return False
    try:
        append_jsonl(Path(path_value).expanduser(), record)
    except OSError:
        return False
    return True


class TraceCapture:
    def __init__(
        self,
        stream: TextIO,
        *,
        include_body: bool,
        redact_body: bool,
        max_excerpt_chars: int,
    ) -> None:
        self._stream = stream
        self._include_body = include_body
        self._redact_body = redact_body
        self._max_excerpt_chars = max_excerpt_chars
        self._capture_limit = max(0, max_excerpt_chars + 1024)
        self._bytes = 0
        self._captured_chars = 0
        self._body_truncated = False
        self._chunks: list[str] = []

    def write(self, value: str) -> int:
        written = self._stream.write(value)
        self._bytes += len(value.encode("utf-8", errors="replace"))
        if self._include_body and not self._redact_body:
            remaining = self._capture_limit - self._captured_chars
            if remaining > 0:
                excerpt = value[:remaining]
                self._chunks.append(excerpt)
                self._captured_chars += len(excerpt)
            if len(value) > remaining:
                self._body_truncated = True
        return written

    def flush(self) -> None:
        self._stream.flush()

    def summary(self) -> dict[str, object]:
        data: dict[str, object] = {"bytes": self._bytes}
        if self._include_body:
            if self._redact_body and self._bytes:
                data["excerpt"] = AUTH_TOKEN_REDACTED
                data["truncated"] = False
            else:
                excerpt, truncated = _safe_excerpt("".join(self._chunks), self._max_excerpt_chars)
                data["excerpt"] = excerpt
                data["truncated"] = truncated or self._body_truncated
        return data

    def __getattr__(self, name: str) -> object:
        return getattr(self._stream, name)


def route_from_decision(route: TraceRoute | Mapping[str, object] | object | None) -> TraceRoute:
    if isinstance(route, TraceRoute):
        return route
    if route is None:
        return TraceRoute("unknown")

    kind = _route_value(route, "kind")
    reason = _route_value(route, "reason")
    host = _route_value(route, "host")
    repo = _repo_from_value(_route_value(route, "repo"))
    if host is None and repo is not None:
        host = repo.host

    return TraceRoute(
        str(kind) if kind is not None else "unknown",
        str(reason) if reason is not None else "",
        str(host) if host is not None else None,
        repo,
    )


def redact_text(text: str) -> str:
    redacted = _AUTH_HEADER_RE.sub(lambda match: f"{match.group(1)}{match.group(2)} {REDACTED}", text)
    redacted = _TOKEN_PREFIX_RE.sub(REDACTED, redacted)
    redacted = _URL_CREDENTIAL_RE.sub(lambda match: f"{match.group(1)}{REDACTED}{match.group(3)}", redacted)
    redacted = _ENV_ASSIGNMENT_RE.sub(lambda match: f"{match.group(1)}={REDACTED}", redacted)
    return _SENSITIVE_KEY_RE.sub(lambda match: f"{match.group(1)}{match.group(2)}{REDACTED}{match.group(4)}", redacted)


def redact_argv(argv: Sequence[str]) -> list[str]:
    redacted: list[str] = []
    redact_next = False
    redact_header_next = False
    for arg in argv:
        value = str(arg)
        flag, separator, flag_value = value.partition("=")
        if redact_next:
            redacted.append(REDACTED)
            redact_next = False
            continue
        if redact_header_next:
            redacted.append(redact_text(value))
            redact_header_next = False
            continue
        if value in _SENSITIVE_FLAG_NAMES:
            redacted.append(value)
            redact_next = True
            continue
        if separator and flag in _SENSITIVE_FLAG_NAMES:
            redacted.append(f"{flag}={REDACTED}")
            continue
        if value in {"-H", "--header"}:
            redacted.append(value)
            redact_header_next = True
            continue
        if separator and flag in {"-H", "--header"}:
            redacted.append(f"{flag}={redact_text(flag_value)}")
            continue
        redacted.append(redact_text(value))
    return redacted


def append_jsonl(path: Path, record: Mapping[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    line = json.dumps(record, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
    data = line.encode("utf-8")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        while data:
            written = os.write(fd, data)
            data = data[written:]
    finally:
        os.close(fd)


def _merge_route(
    route: TraceRoute | Mapping[str, object] | object | None,
    *,
    host: str | None,
    repo: TraceRepo | Mapping[str, object] | object | None,
) -> TraceRoute:
    trace_route = route_from_decision(route)
    trace_repo = _repo_from_value(repo) or trace_route.repo
    trace_host = host or trace_route.host or (trace_repo.host if trace_repo else None)
    return TraceRoute(trace_route.kind, trace_route.reason, trace_host, trace_repo)


def _summary_or_stream_record(
    summary: Mapping[str, object] | None,
    value: str | bytes | None,
    *,
    provided_bytes: int | None,
    include_body: bool,
    max_excerpt_chars: int,
    suppress_body: bool,
) -> dict[str, object]:
    if summary is not None:
        return dict(summary)
    return _stream_record(
        value,
        provided_bytes=provided_bytes,
        include_body=include_body,
        max_excerpt_chars=max_excerpt_chars,
        suppress_body=suppress_body,
    )


def _stream_record(
    value: str | bytes | None,
    *,
    provided_bytes: int | None,
    include_body: bool,
    max_excerpt_chars: int,
    suppress_body: bool,
) -> dict[str, object]:
    text = _stream_text(value)
    data: dict[str, object] = {
        "bytes": provided_bytes if provided_bytes is not None else _stream_size(value),
    }
    if include_body and value is not None:
        if suppress_body and text:
            data["excerpt"] = AUTH_TOKEN_REDACTED
            data["truncated"] = False
        else:
            excerpt, truncated = _safe_excerpt(text, max_excerpt_chars)
            data["excerpt"] = excerpt
            data["truncated"] = truncated
    return data


def _safe_excerpt(text: str, max_chars: int) -> tuple[str, bool]:
    redacted = redact_text(text)
    if max_chars < 1:
        return "", bool(redacted)
    if len(redacted) <= max_chars:
        return redacted, False
    return redacted[:max_chars], True


def _stream_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def _stream_size(value: str | bytes | None) -> int:
    if value is None:
        return 0
    if isinstance(value, bytes):
        return len(value)
    return len(value.encode("utf-8", errors="replace"))


def _duration_ms(
    duration_ms: int | float | None,
    duration_seconds: float | None,
) -> int | None:
    if duration_ms is not None:
        rounded = int(round(duration_ms))
        return 1 if rounded == 0 and duration_ms > 0 else rounded
    if duration_seconds is not None:
        raw_ms = duration_seconds * 1000
        rounded = int(round(raw_ms))
        return 1 if rounded == 0 and raw_ms > 0 else rounded
    return None


def _route_value(route: Mapping[str, object] | object, name: str) -> object | None:
    if isinstance(route, Mapping):
        return route.get(name)
    return getattr(route, name, None)


def _repo_from_value(value: object | None) -> TraceRepo | None:
    if isinstance(value, TraceRepo):
        return value
    if value is None:
        return None

    host = _repo_value(value, "host")
    owner = _repo_value(value, "owner")
    name = _repo_value(value, "repo") or _repo_value(value, "name")
    full_name = _repo_value(value, "full_name")
    if (owner is None or name is None) and isinstance(full_name, str) and "/" in full_name:
        owner, name = full_name.split("/", 1)

    if host is None or owner is None or name is None:
        return None
    return TraceRepo(str(host), str(owner), str(name))


def _repo_value(repo: Mapping[str, object] | object, name: str) -> object | None:
    if isinstance(repo, Mapping):
        return repo.get(name)
    return getattr(repo, name, None)


def _format_timestamp(value: datetime) -> str:
    if value.tzinfo is None:
        value = value.replace(tzinfo=timezone.utc)
    return value.astimezone(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def _env_enabled(value: str | None) -> bool:
    if value is None:
        return False
    normalized = value.strip().lower()
    if normalized in _FALSE_VALUES:
        return False
    if normalized in _TRUE_VALUES:
        return True
    return bool(normalized)


def _is_auth_token_command(argv: Sequence[str]) -> bool:
    parts = [str(item) for item in argv]
    if parts and Path(parts[0]).name in {"gh", "gh-forgejo-shim", "gfj"}:
        parts = parts[1:]
    return len(parts) >= 2 and parts[0] == "auth" and parts[1] == "token"


def _utc_now() -> datetime:
    return datetime.now(timezone.utc)
