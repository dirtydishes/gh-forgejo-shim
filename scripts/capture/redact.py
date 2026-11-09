"""Redact client captures before they're committed to fixtures/clients.

    python3 scripts/capture/redact.py --out fixtures/clients \\
        --name <your login>='<user>' --name <repo>='<repo>' --name <forge host>=forge.test \\
        <capture files...>

It takes gh-capture logs (`.jsonl`, with an optional `.marks` beside each), web server access logs
(`.log`), GraphQL documents and their variables (`.graphql`, `.vars`) and stub tables (`.json`), and
writes each under --out with its own name. It refuses anything else, screenshots included.

What it replaces, everywhere: the names given with --name; every login seen in a JSON answer or an
`auth status`; home directories, emails, IP addresses, tokens, Authorization and Cookie values,
avatar URLs, commit IDs, Actions run and job IDs and timestamps. In JSON, a user object keeps only
its login, type and URLs, a commit author loses its name and email, and node IDs and any value
under a sensitive key are hidden. One table serves every file in a run, so a value always maps to the same
placeholder. A record's `ts` becomes `t`, seconds since the file's first record, and its `.marks`
labels become each record's `mark`. An answer repeated within a file is stored once: later copies
say `{"same_as": <line index>}`.

Run leaks.py on the result before committing: it fails on anything that still looks identifying.
"""

import argparse
import ipaddress
import json
import re
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

USER = "<user>"
SUFFIXES = {".jsonl", ".log", ".graphql", ".vars", ".json"}
TOKEN = re.compile(r"(?:github_pat_|gh[pousr]_|glpat-|gitea_)[A-Za-z0-9_]{8,}")
AUTHORIZATION = re.compile(r"(?i)(authorization:\s*(?:(?:token|bearer|basic)\s+)?)[^\s\"',]+")
USERINFO = re.compile(r"(://)[^/\s\"'<>@]+@")
COOKIE = re.compile(r"(?i)((?:set-)?cookie:\s*)[^\n\"]+")
SECRET_ASSIGNMENT = re.compile(r"(?i)\b([\w.-]*(?:token|secret|passw(?:or)?d|credential|api[_-]?key)[\w.-]*=)[^&\s\"']+")
HOME = re.compile(r"/(?:Users|home)/[^/\s\"'<>]+|[A-Za-z]:\\+Users\\+[^\\\s\"'<>]+")
EMAIL = re.compile(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?"
                   r"(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)+")
AVATAR = re.compile(r"https?://(?:avatars\.githubusercontent\.com|(?:secure|www)\.gravatar\.com|"
                    r"[^/\s\"'<>]+/avatars?)/[^\s\"'<>]*")
IPV4 = re.compile(r"(?<![\d.])\d{1,3}(?:\.\d{1,3}){3}(?!\.?\d)")
IPV6 = re.compile(r"(?<![\w:.])[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7}(?![\w:])")
SHA = re.compile(r"(?<![0-9A-Fa-f])(?:[0-9a-f]{64}|[0-9a-f]{40})(?![0-9A-Fa-f])")
# Placeholders this script writes: leaks.py accepts these and nothing else of their shape.
PLACEHOLDER_SHA = re.compile(r"0{24}[0-9a-f]{16}")
ACTIONS = re.compile(r"(/actions/runs/)(\d+)(?:(/jobs?/)(\d+))?")
TIMESTAMP = re.compile(r"\b\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:?\d\d)")
LOG_TIME = re.compile(r"\[(\d\d/[A-Z][a-z]{2}/\d{4}:\d\d:\d\d:\d\d [+-]\d{4})\]")
# gh's `auth status`: "Logged in to <host> account <login> (<source>)".
ACCOUNT = re.compile(r"Logged in to \S+ account (\S+)")
EPOCH = datetime(2000, 1, 1, tzinfo=timezone.utc)
SENSITIVE_KEY = re.compile(r"(?i)token|secret|passw(?:or)?d|passphrase|credential|api[_-]?key|private[_-]?key|cookie")
# A user object (any object with a `login`) keeps only these keys; the rest are hidden. GitHub's
# REST and GraphQL users and Forgejo's each add profile fields of their own (`bio`, `full_name`,
# `websiteUrl`, `pronouns`, `status`), so a list of what to hide would miss the next one.
USER_KEYS = {"login", "username", "type", "__typename", "site_admin", "user_view_type", "gravatar_id",
             "url", "html_url", "resourcePath", "avatar_url", "avatarUrl", "created_at", "updated_at",
             "createdAt", "updatedAt", "followers_url", "following_url", "gists_url", "starred_url",
             "subscriptions_url", "organizations_url", "repos_url", "events_url", "received_events_url"}
# Who wrote a commit, as git records it: a name beside an email, a date or a user.
IDENTITY_KEYS = {"name", "email"}
NODE_KEYS = {"node_id", "nodeId", "id"}
HTTP_STATUS = re.compile(r"HTTP/[\d.]+ \d{3}")
# Response headers whose values stay (redacted like any text); every other value is hidden, which
# covers dates, request IDs, rate-limit reset times and cookies.
HEADERS = {"content-type", "content-length", "content-encoding", "transfer-encoding", "link", "vary",
           "cache-control", "x-github-media-type", "x-github-api-version-selected", "x-oauth-scopes",
           "x-accepted-oauth-scopes", "x-accepted-github-permissions", "x-ratelimit-limit",
           "x-ratelimit-remaining", "x-ratelimit-used", "x-ratelimit-resource", "x-total-count",
           "x-hasmore"}


class Redactor:
    """The shared table: every file of a run goes through one."""

    def __init__(self, names):
        self.names = dict(names)
        self.shas = {}
        self.runs = {}
        self.jobs = {}
        self.times = {}

    def learn(self, values):
        """Pass one: logins and timestamps from every file, so placeholders are fixed before
        anything is written, and timestamps keep their order."""
        logins = []
        instants = set()
        for value in values:
            self._collect(value, logins, instants)
        known = {name.lower() for name in self.names}
        users = sum(1 for placeholder in self.names.values() if re.fullmatch(r"<user(?:-\d+)?>", placeholder))
        for login in logins:
            if login.lower() in known or login.endswith("[bot]") or login.startswith("<"):
                continue
            users += 1
            self.names[login] = USER if users == 1 else f"<user-{users}>"
            known.add(login.lower())
        for index, instant in enumerate(sorted(instants)):
            self.times[instant] = (EPOCH + timedelta(seconds=index)).strftime("%Y-%m-%dT%H:%M:%SZ")

    def _collect(self, value, logins, instants):
        if isinstance(value, dict):
            for key, item in value.items():
                if key == "login" and isinstance(item, str) and item not in logins:
                    logins.append(item)
                self._collect(item, logins, instants)
        elif isinstance(value, list):
            for item in value:
                self._collect(item, logins, instants)
        elif isinstance(value, str):
            for login in ACCOUNT.findall(value):
                if login not in logins:
                    logins.append(login)
            for match in TIMESTAMP.finditer(value):
                instants.add(parse_time(match.group(0)))
            for parsed in embedded(value):
                self._collect(parsed, logins, instants)

    def text(self, value):
        value = USERINFO.sub(r"\1<redacted>@", value)
        value = HOME.sub("<home>", value)
        value = EMAIL.sub("<email>", value)
        value = TOKEN.sub("<token>", value)
        value = AUTHORIZATION.sub(r"\1<token>", value)
        value = COOKIE.sub(r"\1<redacted>", value)
        value = SECRET_ASSIGNMENT.sub(r"\1<redacted>", value)
        # Longest first, so a host goes before the login inside it. A name ends at anything but a
        # letter or digit: `<repo>-fj` for a checkout named after the repository.
        for name in sorted(self.names, key=len, reverse=True):
            value = re.sub(r"(?<![A-Za-z0-9])" + re.escape(name) + r"(?![A-Za-z0-9])",
                           lambda _, name=name: self.names[name], value, flags=re.IGNORECASE)
        value = AVATAR.sub("<avatar-url>", value)
        value = IPV4.sub(lambda m: "<ip>" if is_ip(m.group(0)) else m.group(0), value)
        value = IPV6.sub(lambda m: "<ip>" if is_ip(m.group(0)) else m.group(0), value)
        value = SHA.sub(self._sha, value)
        value = ACTIONS.sub(self._actions, value)
        return TIMESTAMP.sub(lambda m: self.times.get(parse_time(m.group(0)), "<timestamp>"), value)

    def _sha(self, match):
        sha = match.group(0)
        if PLACEHOLDER_SHA.fullmatch(sha):
            return sha
        return self.shas.setdefault(sha, f"{len(self.shas) + 1:040x}")

    def _actions(self, match):
        run = self.runs.setdefault(match.group(2), str(len(self.runs) + 1))
        if not match.group(3):
            return match.group(1) + run
        return match.group(1) + run + match.group(3) + self.jobs.setdefault(match.group(4), str(len(self.jobs) + 1))

    def value(self, value):
        """A JSON value: strings by the text rules, and a person's details hidden by key."""
        if isinstance(value, dict):
            user = "login" in value
            identity = not user and ("email" in value or "name" in value and bool({"user", "date"} & set(value)))
            result = {}
            for key, item in value.items():
                if SENSITIVE_KEY.search(key) and isinstance(item, str) and item:
                    result[key] = "<redacted>"
                elif key in NODE_KEYS and isinstance(item, str) and item:
                    result[key] = "<node-id>"
                elif user and key not in USER_KEYS or identity and key in IDENTITY_KEYS:
                    result[key] = hidden(item)
                else:
                    result[key] = self.value(item)
            return result
        if isinstance(value, list):
            return [self.value(item) for item in value]
        if isinstance(value, str):
            return self.structured(value)
        return value

    def structured(self, text):
        """A string that may hold JSON answers, or HTTP headers and a body, redacted as such."""
        parts = http_parts(text)
        if parts:
            head, blank, body = parts
            lines = head.split("\n")
            for index, line in enumerate(lines[1:], 1):
                name, colon, rest = line.partition(":")
                if colon:
                    end = "\r" if rest.endswith("\r") else ""
                    kept = name.strip().lower() in HEADERS
                    lines[index] = name + ":" + (self.text(rest) if kept else " <redacted>" + end)
            lines[0] = self.text(lines[0])
            return "\n".join(lines) + blank + self.structured(body)
        spans = json_spans(text)
        if spans is None:
            return self.text(text)
        out = []
        last = 0
        for start, end, item in spans:
            out += [text[last:start], dump_like(self.value(item), text[start:end])]
            last = end
        return "".join(out) + text[last:]


def hidden(value):
    """A value with nothing left of it but its type: strings `<redacted>`, numbers 0."""
    if isinstance(value, dict):
        return {key: hidden(item) for key, item in value.items()}
    if isinstance(value, list):
        return [hidden(item) for item in value]
    if isinstance(value, str):
        return "<redacted>" if value else value
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return 0
    return value


def parse_time(text):
    """An ISO 8601 time in any of the forms TIMESTAMP matches, as UTC (Python 3.9's
    fromisoformat takes only `+HH:MM` and six-digit fractions)."""
    text = text.replace("Z", "+00:00")
    if re.search(r"[+-]\d{4}$", text):
        text = text[:-2] + ":" + text[-2:]
    text = re.sub(r"\.(\d+)", lambda m: "." + (m.group(1) + "000000")[:6], text)
    return datetime.fromisoformat(text).astimezone(timezone.utc)


def is_ip(text):
    try:
        ipaddress.ip_address(text)
    except ValueError:
        return False
    return True


def json_spans(text):
    """The JSON values a string holds back to back, as (start, end, value): one for most answers,
    one per page for `gh api --paginate`. None unless the string is nothing but JSON values and
    whitespace, at least one of them an object or array."""
    decoder = json.JSONDecoder()
    spans = []
    index = 0
    while True:
        while index < len(text) and text[index].isspace():
            index += 1
        if index == len(text):
            break
        try:
            value, end = decoder.raw_decode(text, index)
        except ValueError:
            return None
        spans.append((index, end, value))
        index = end
    return spans if any(isinstance(value, (dict, list)) for _, _, value in spans) else None


def http_parts(text):
    """`gh api -i` output: (status and header lines, the blank line, the body), or None."""
    if not HTTP_STATUS.match(text):
        return None
    blank = re.search(r"\r?\n\r?\n", text)
    if not blank:
        return text, "", ""
    return text[:blank.start()], blank.group(0), text[blank.end():]


def embedded(text):
    """The JSON values inside a string: an answer, its pages, or the body after HTTP headers."""
    parts = http_parts(text)
    spans = json_spans(parts[2] if parts else text)
    return [value for _, _, value in spans or []]


def dump_like(value, original):
    """Re-encode in the original's style: gh writes compact JSON, a stub may not."""
    parsed = json.loads(original)
    separators = (",", ":")
    if json.dumps(parsed, ensure_ascii=False) == original:
        separators = (", ", ": ")
    return json.dumps(value, ensure_ascii=False, separators=separators)


def read_marks(path):
    """`MARK <label> HH:MM:SS` lines, local time as the capture's `ts` has it."""
    if not path.exists():
        return []
    marks = []
    for line in path.read_text().splitlines():
        parts = line.split()
        if len(parts) != 3 or parts[0] != "MARK" or not re.fullmatch(r"\d\d:\d\d:\d\d", parts[2]):
            raise ValueError(f"{path.name}: not a mark line: {line!r}")
        marks.append((parts[2], parts[1]))
    return sorted(marks)


def records(path, redactor, marks):
    """A gh-capture log, redacted: `ts` becomes `t`, marks become `mark`, repeats `same_as`."""
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    stamps = [datetime.strptime(row.pop("ts"), "%Y-%m-%dT%H:%M:%S%z") for row in rows]
    start = min(stamps) if stamps else None
    seen = {}
    out = []
    for index, (row, stamp) in enumerate(zip(rows, stamps)):
        row = redactor.value(row)
        row["t"] = int((stamp - start).total_seconds())
        clock = stamp.strftime("%H:%M:%S")
        labels = [label for time, label in marks if time <= clock]
        if labels:
            row["mark"] = redactor.text(labels[-1])
        stdout = row.get("stdout")
        if isinstance(stdout, str) and stdout:
            if stdout in seen:
                row["stdout"] = {"same_as": seen[stdout]}
            else:
                seen[stdout] = index
        out.append(json.dumps(row, ensure_ascii=False, sort_keys=True))
    return "".join(line + "\n" for line in out)


def access_log(path, redactor):
    """Combined-format access log lines: times become `[+Ns]` from the first line."""
    lines = path.read_text().splitlines()
    times = [datetime.strptime(m.group(1), "%d/%b/%Y:%H:%M:%S %z") for m in map(LOG_TIME.search, lines) if m]
    start = min(times) if times else None

    def relative(match):
        return "[+%ds]" % (datetime.strptime(match.group(1), "%d/%b/%Y:%H:%M:%S %z") - start).total_seconds()

    return "".join(redactor.text(LOG_TIME.sub(relative, line)) + "\n" for line in lines)


def learnable(path):
    """What pass one reads from a file: its records, or its text."""
    text = path.read_text()
    if path.suffix == ".jsonl":
        rows = [json.loads(line) for line in text.splitlines() if line.strip()]
        for row in rows:
            row.pop("ts", None)
        return rows
    if path.suffix == ".json":
        return json.loads(text)
    return LOG_TIME.sub("", text)


def redact(path, redactor):
    if path.suffix == ".jsonl":
        return records(path, redactor, read_marks(path.with_suffix(".marks")))
    if path.suffix == ".log":
        return access_log(path, redactor)
    if path.suffix == ".json":
        return json.dumps(redactor.value(json.loads(path.read_text())), ensure_ascii=False, indent=2) + "\n"
    return redactor.text(path.read_text())


def name_pair(text):
    value, equal, placeholder = text.partition("=")
    if not equal or not value or not placeholder:
        raise argparse.ArgumentTypeError("--name takes VALUE=PLACEHOLDER")
    return value, placeholder


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--name", action="append", default=[], type=name_pair,
                        help="replace VALUE (any case) with PLACEHOLDER: a host, repository or login")
    parser.add_argument("files", nargs="+", type=Path)
    args = parser.parse_args(argv)
    files = [path for path in args.files if path.suffix != ".marks"]
    refused = [path for path in files if path.suffix not in SUFFIXES]
    if refused:
        parser.error("refusing files it can't redact: " + ", ".join(str(path) for path in refused))
    redactor = Redactor(args.name)
    redactor.learn([learnable(path) for path in files])
    args.out.mkdir(parents=True, exist_ok=True)
    for path in files:
        (args.out / redactor.text(path.name)).write_text(redact(path, redactor))
    return 0


if __name__ == "__main__":
    sys.exit(main())
