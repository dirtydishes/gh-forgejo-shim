from __future__ import annotations

import os
import shutil
import stat
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

MARKER = "managed by gh-forgejo-shim git recorder"
TRACE_PATH_ENV = "FJ_SHIM_TRACE"
TRACE_ENV = TRACE_PATH_ENV
TRACE_BODY_ENV = "FJ_SHIM_TRACE_BODY"
REAL_GIT_ENV = "FJ_SHIM_REAL_GIT"

__all__ = [
    "GitRecorder",
    "GitRecorderSession",
    "TRACE_BODY_ENV",
    "TRACE_ENV",
    "TRACE_PATH_ENV",
    "create_git_recorder",
    "git_recorder_script",
    "remove_git_recorder",
]


@dataclass(frozen=True)
class GitRecorder:
    """Temporary git recorder wrapper details for one launch environment."""

    wrapper_dir: Path
    wrapper_path: Path
    trace_path: Path
    real_git: Path
    path: str
    env: dict[str, str]


GitRecorderSession = GitRecorder


def create_git_recorder(
    trace_path: str | Path,
    *,
    real_git: str | Path | None = None,
    root_dir: str | Path | None = None,
    wrapper_dir: str | Path | None = None,
    env: Mapping[str, str] | None = None,
) -> GitRecorder:
    """Create a temporary ``git`` executable that records JSONL invocations."""

    values = dict(env if env is not None else os.environ)
    git_path = _resolve_real_git(real_git, values)
    directory = _make_wrapper_dir(root_dir=root_dir, wrapper_dir=wrapper_dir)
    wrapper = directory / "git"
    if wrapper.exists() and not _is_recorder_wrapper(wrapper):
        raise FileExistsError(f"{wrapper} exists and is not managed by gh-forgejo-shim")

    trace = Path(trace_path).expanduser().resolve(strict=False)
    wrapper.write_text(git_recorder_script(git_path, trace), encoding="utf-8")
    wrapper.chmod(wrapper.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    (directory / ".gh-forgejo-shim-git-recorder").write_text(MARKER + "\n", encoding="utf-8")

    path = _prepend_path(directory, values.get("PATH", ""))
    launch_env = dict(values)
    launch_env["PATH"] = path
    launch_env[TRACE_PATH_ENV] = str(trace)
    launch_env[REAL_GIT_ENV] = str(git_path)

    return GitRecorder(
        wrapper_dir=directory,
        wrapper_path=wrapper,
        trace_path=trace,
        real_git=git_path,
        path=path,
        env=launch_env,
    )


def remove_git_recorder(session: GitRecorder | str | Path) -> Path:
    """Remove a temporary git recorder directory created by this module."""

    directory = Path(session.wrapper_dir) if isinstance(session, GitRecorder) else Path(session).expanduser()
    if directory.is_file():
        directory = directory.parent
    if not directory.exists():
        return directory

    wrapper = directory / "git"
    if not _is_recorder_wrapper(wrapper):
        raise FileExistsError(f"{directory} is not a gh-forgejo-shim git recorder directory")
    shutil.rmtree(directory)
    return directory


def git_recorder_script(real_git: str | Path, trace_path: str | Path) -> str:
    """Return the Python script body used for the temporary ``git`` wrapper."""

    return f"""#!{sys.executable}
# {MARKER}
from __future__ import annotations

import datetime
import json
import os
import re
import subprocess
import sys
import threading
import time
from pathlib import Path

REAL_GIT = {str(real_git)!r}
TRACE_PATH_DEFAULT = {str(trace_path)!r}
TRACE_PATH_ENV = {TRACE_PATH_ENV!r}
TRACE_BODY_ENV = {TRACE_BODY_ENV!r}
EXCERPT_LIMIT = 4096
CHUNK_SIZE = 8192
SECRET_REPLACEMENT = "[REDACTED]"

_SECRET_KEY_RE = re.compile(
    r"(?i)(token|secret|pass(?:word)?|passwd|api[-_]?key|authorization|"
    r"auth[-_]?token|credential|client[-_]?secret|access[-_]?token|"
    r"refresh[-_]?token|private[-_]?key)"
)
_AUTH_HEADER_RE = re.compile(r"(?i)(authorization\\s*[:=]\\s*(?:bearer|basic)?\\s+)([^\\s;&]+)")
_SECRET_ASSIGNMENT_RE = re.compile(
    r"(?i)(\\b(?:token|secret|password|passwd|api[-_]?key|authorization|"
    r"auth[-_]?token|credential|client[-_]?secret|access[-_]?token|"
    r"refresh[-_]?token|private[-_]?key)\\b\\s*[:=]\\s*)([^\\s;&]+)"
)
_URL_USERINFO_RE = re.compile(r"(?i)\\b([a-z][a-z0-9+.-]*://)([^/\\s:@]+)(?::[^/\\s@]*)?@")
_KNOWN_TOKEN_RE = re.compile(
    r"(?i)\\b("
    r"github_pat_[A-Za-z0-9_]+|"
    r"gh[pousr]_[A-Za-z0-9_]+|"
    r"glpat-[A-Za-z0-9_-]+|"
    r"gitea_[A-Za-z0-9_-]+"
    r")\\b"
)


def _utc_timestamp() -> str:
    return datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z")


def _redact_text(value: str) -> str:
    redacted = _URL_USERINFO_RE.sub(r"\\1" + SECRET_REPLACEMENT + "@", value)
    redacted = _AUTH_HEADER_RE.sub(r"\\1" + SECRET_REPLACEMENT, redacted)
    redacted = _SECRET_ASSIGNMENT_RE.sub(r"\\1" + SECRET_REPLACEMENT, redacted)
    return _KNOWN_TOKEN_RE.sub(SECRET_REPLACEMENT, redacted)


def _redact_argv(args: list[str]) -> list[str]:
    redacted: list[str] = []
    redact_next = False
    for arg in args:
        if redact_next:
            redacted.append(SECRET_REPLACEMENT)
            redact_next = False
            continue

        key, separator, _value = arg.partition("=")
        if key.startswith("-") and _SECRET_KEY_RE.search(key):
            redacted.append(key + separator + SECRET_REPLACEMENT if separator else key)
            redact_next = not separator
            continue
        if separator and _SECRET_KEY_RE.search(key):
            redacted.append(key + separator + SECRET_REPLACEMENT)
            continue

        redacted.append(_redact_text(arg))
    return redacted


def _safe_decode(value: bytes) -> str:
    return value.decode("utf-8", errors="replace")


def _forward_stream(source, destination, include_excerpt: bool) -> tuple[int, bytes]:
    total = 0
    remaining = EXCERPT_LIMIT
    excerpt: list[bytes] = []
    while True:
        chunk = source.read(CHUNK_SIZE)
        if not chunk:
            break
        total += len(chunk)
        if include_excerpt and remaining > 0:
            piece = chunk[:remaining]
            excerpt.append(piece)
            remaining -= len(piece)
        try:
            destination.write(chunk)
            destination.flush()
        except BrokenPipeError:
            pass
    return total, b"".join(excerpt)


def _start_forwarder(source, destination, key: str, include_excerpt: bool, stats: dict[str, object]):
    def run() -> None:
        size, excerpt = _forward_stream(source, destination, include_excerpt)
        stats[key + "_bytes"] = size
        if include_excerpt:
            stats[key + "_excerpt"] = excerpt

    thread = threading.Thread(target=run)
    thread.start()
    return thread


def _append_record(record: dict[str, object]) -> None:
    trace_path = Path(os.environ.get(TRACE_PATH_ENV) or TRACE_PATH_DEFAULT)
    try:
        trace_path.parent.mkdir(parents=True, exist_ok=True)
        with trace_path.open("a", encoding="utf-8") as trace:
            trace.write(json.dumps(record, sort_keys=True, separators=(",", ":")) + "\\n")
    except OSError:
        pass


def _record_base(args: list[str]) -> dict[str, object]:
    safe_argv = _redact_argv(args)
    return {{
        "schema": "gh-forgejo-shim.git-recorder.v1",
        "kind": "git",
        "timestamp": _utc_timestamp(),
        "cwd": _redact_text(os.getcwd()),
        "argv": safe_argv,
        "command_argv": ["git", *safe_argv],
    }}


def _finish_record(
    record: dict[str, object],
    *,
    started: float,
    exit_code: int,
    stdout_bytes: int,
    stderr_bytes: int,
    stdout_excerpt: bytes = b"",
    stderr_excerpt: bytes = b"",
) -> None:
    include_excerpt = os.environ.get(TRACE_BODY_ENV) == "1"
    duration = time.monotonic() - started
    record["duration"] = round(duration, 6)
    record["duration_ms"] = round(duration * 1000, 3)
    record["exit_code"] = exit_code
    record["stdout_bytes"] = stdout_bytes
    record["stderr_bytes"] = stderr_bytes
    record["stdout"] = {{"bytes": stdout_bytes}}
    record["stderr"] = {{"bytes": stderr_bytes}}
    if include_excerpt:
        stdout_text = _redact_text(_safe_decode(stdout_excerpt))
        stderr_text = _redact_text(_safe_decode(stderr_excerpt))
        record["stdout_excerpt"] = stdout_text
        record["stderr_excerpt"] = stderr_text
        record["stdout"]["excerpt"] = stdout_text
        record["stderr"]["excerpt"] = stderr_text


def _error_record(record: dict[str, object], started: float, message: bytes) -> int:
    try:
        sys.stderr.buffer.write(message)
        sys.stderr.buffer.flush()
    except BrokenPipeError:
        pass

    _finish_record(
        record,
        started=started,
        exit_code=127,
        stdout_bytes=0,
        stderr_bytes=len(message),
        stderr_excerpt=message[:EXCERPT_LIMIT],
    )
    _append_record(record)
    return 127


def main() -> int:
    started = time.monotonic()
    args = sys.argv[1:]
    record = _record_base(args)
    include_excerpt = os.environ.get(TRACE_BODY_ENV) == "1"

    try:
        process = subprocess.Popen(
            [REAL_GIT, *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as exc:
        message = f"gh-forgejo-shim git recorder: unable to run real git: {{exc}}\\n".encode()
        return _error_record(record, started, message)

    stats: dict[str, object] = {{}}
    assert process.stdout is not None
    assert process.stderr is not None
    threads = [
        _start_forwarder(process.stdout, sys.stdout.buffer, "stdout", include_excerpt, stats),
        _start_forwarder(process.stderr, sys.stderr.buffer, "stderr", include_excerpt, stats),
    ]
    exit_code = process.wait()
    for thread in threads:
        thread.join()

    stdout_excerpt = stats.get("stdout_excerpt", b"")
    stderr_excerpt = stats.get("stderr_excerpt", b"")
    _finish_record(
        record,
        started=started,
        exit_code=int(exit_code),
        stdout_bytes=int(stats.get("stdout_bytes", 0)),
        stderr_bytes=int(stats.get("stderr_bytes", 0)),
        stdout_excerpt=stdout_excerpt if isinstance(stdout_excerpt, bytes) else b"",
        stderr_excerpt=stderr_excerpt if isinstance(stderr_excerpt, bytes) else b"",
    )
    _append_record(record)
    return int(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
"""


def _resolve_real_git(real_git: str | Path | None, env: Mapping[str, str]) -> Path:
    configured = real_git if real_git is not None else env.get(REAL_GIT_ENV)
    if configured:
        return Path(configured).expanduser().resolve(strict=False)
    found = shutil.which("git", path=env.get("PATH"))
    if found is None:
        raise FileNotFoundError(f"could not find git; set {REAL_GIT_ENV}")
    return Path(found).expanduser().resolve(strict=False)


def _make_wrapper_dir(*, root_dir: str | Path | None, wrapper_dir: str | Path | None) -> Path:
    if wrapper_dir is not None:
        directory = Path(wrapper_dir).expanduser().resolve(strict=False)
        directory.mkdir(parents=True, exist_ok=True)
        return directory
    root = str(Path(root_dir).expanduser()) if root_dir is not None else None
    return Path(tempfile.mkdtemp(prefix="gh-forgejo-shim-git-", dir=root)).resolve(strict=False)


def _prepend_path(directory: Path, path_value: str) -> str:
    if not path_value:
        return str(directory)
    return os.pathsep.join([str(directory), path_value])


def _is_recorder_wrapper(wrapper: Path) -> bool:
    try:
        text = wrapper.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False
    return MARKER in text
