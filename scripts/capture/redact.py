"""Redact client captures before they're committed to fixtures/clients.

    python3 scripts/capture/redact.py --out fixtures/clients \\
        --name <your login>='<user>' --name <repo>='<repo>' --name <forge host>=forge.test \\
        <capture files...>

It takes gh-capture logs (`.jsonl`, with an optional `.marks` beside each), web server access logs
(`.log`), GraphQL documents and their variables (`.graphql`, `.vars`) and stub tables (`.json`), and
writes each under --out with its own name. It refuses anything else, screenshots included.

What it replaces, everywhere: the names given with --name; every login seen in a JSON answer or an
`auth status`; home directories and the local user's name wherever it appears; emails, IP addresses, tokens, Authorization and Cookie values,
avatar URLs, commit IDs, Actions run and job IDs and timestamps. In JSON, a user object keeps only
its login, type and URLs, a commit author loses its name and email, node IDs are numbered and any
value under a sensitive key is hidden. One table serves every file in a run, so a value always maps to the same
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
# Tokens by their prefix (GitHub, GitLab, Forgejo, npm, PyPI, OpenAI and Anthropic, Slack, AWS)
# and JSON web tokens. gh's masked `gho_****` stays.
TOKEN = re.compile(r"(?<![A-Za-z0-9_-])(?:(?:github_pat_|gh[pousr]_|glpat-|glrt-|gitea_|npm_|pypi-|sk-|xox[abposr]-)"
                   r"[A-Za-z0-9_-]{8,}|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}(?:\.[A-Za-z0-9_-]+)?)")
# A header that carries a token by itself, as Forgejo's and GitLab's `PRIVATE-TOKEN` does.
TOKEN_HEADER = re.compile(r"(?i)\b((?:private-token|x-api-key|api-key|x-auth-token|x-access-token|x-gitea-otp):\s*)"
                          r"[^\s\"',\\]+")
AUTHORIZATION = re.compile(r"(?i)(authorization:\s*(?:(?:token|bearer|basic)\s+)?)[^\s\"',]+")
USERINFO = re.compile(r"(://)[^/\s\"'<>@]+@")
COOKIE = re.compile(r"(?i)((?:set-)?cookie:\s*)[^\n\"]+")
SECRET_ASSIGNMENT = re.compile(r"(?i)\b([\w.-]*(?:token|secret|passw(?:or)?d|credential|api[_-]?key)[\w.-]*=)[^&\s\"']+")
# A home directory, URL-encoded too; its last part is the local user's name.
HOME = re.compile(r"(?:/|%2[Ff])(?:Users|home)(?:/|%2[Ff])([^/%\s\"'<>]+)|[A-Za-z]:\\+Users\\+([^\\\s\"'<>]+)")
SHARED_HOMES = {"shared", "guest"}
# Logins of bots and apps, which GraphQL gives without `[bot]`: kept, as they name no one.
BOTS = {"github-actions", "dependabot", "copilot", "renovate", "ghost", "web-flow", "forgejo-actions",
        "gitea-actions"}
EMAIL = re.compile(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?"
                   r"(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)+")
AVATAR = re.compile(r"https?://(?:avatars\.githubusercontent\.com|(?:secure|www)\.gravatar\.com|"
                    r"[^/\s\"'<>]+/avatars?)/[^\s\"'<>]*")
IPV4 = re.compile(r"(?<![\d.])\d{1,3}(?:\.\d{1,3}){3}(?!\.?\d)")
IPV6 = re.compile(r"(?<![\w:.-])[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7}(?![\w:-])")
SHA = re.compile(r"(?<![0-9A-Fa-f])(?:[0-9a-f]{64}|[0-9a-f]{40})(?![0-9A-Fa-f])")
# Placeholders this script writes: leaks.py accepts these and nothing else of their shape.
PLACEHOLDER_SHA = re.compile(r"0{24}[0-9a-f]{16}")
ACTIONS = re.compile(r"(/actions/runs/)(\d+)(?:(/jobs?/)(\d+))?")
# A date and time: ISO 8601 and RFC 3339, git's `2026-10-30 15:16:57 -0400`, Forgejo's log
# `2026/10/30 15:16:57`, with or without a zone, and HTTP's `Fri, 30 Oct 2026 19:02:30 GMT`.
TIMESTAMP = re.compile(r"\b(\d{4})([-/])(\d\d)\2(\d\d)([T ])(\d\d):(\d\d):(\d\d)(?:\.(\d+))?"
                       r"( ?)(Z|[+-]\d\d:?\d\d)?(?![\d:])")
HTTP_DATE = re.compile(r"\b(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun), \d\d (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) "
                       r"\d{4} \d\d:\d\d:\d\d GMT")
HTTP_FORMAT = "%a, %d %b %Y %H:%M:%S GMT"
# A date alone, as in a search (`closed:>2026-09-30`), but not gh's own release date.
DATE = re.compile(r"(gh version \S+ \(\d{4}-\d\d-\d\d\))|\b\d{4}([-/])\d\d\2\d\d\b")
# An integer time under a key that says so (`reset` in `gh api rate_limit`, `created_at`).
TIME_KEY = re.compile(r"(?i)reset|(?:^|_)(?:at|time|date)$|[a-z](?:At|Time|Date)$")
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
# GitHub's node IDs in text, such as a mutation's argv: `PR_kwDO…`, and the older base64 `MDQ6…`.
NODE_ID = re.compile(r"\b(?:[A-Z]{1,5}_kw[A-Za-z0-9_-]{4,}|MD[A-Za-z0-9+/]{10,}={0,2})(?![\w+/=-])")
JSON_STYLES = [{"separators": (",", ":")}, {"separators": (", ", ": ")}, {"indent": 2}, {"indent": 4}]
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
        self.nodes = {}
        self.runs = {}
        self.jobs = {}
        self.times = {}

    def learn(self, values):
        """Pass one: logins, local user names and timestamps from every file, so placeholders are
        fixed before anything is written, and timestamps keep their order. The capturing account
        (gh's viewer) is `<user>` whichever login appears first."""
        found = {"viewers": [], "logins": [], "locals": [], "instants": set()}
        for value in values:
            self._collect(value, found)
        known = {name.lower() for name in self.names}
        users = sum(1 for placeholder in self.names.values() if re.fullmatch(r"<user(?:-\d+)?>", placeholder))
        for login in found["viewers"] + found["logins"]:
            if login.lower() in known or login.lower() in BOTS or login.endswith("[bot]") or login.startswith("<"):
                continue
            users += 1
            self.names[login] = USER if users == 1 else f"<user-{users}>"
            known.add(login.lower())
        for name in found["locals"]:
            if name.lower() not in known and name.lower() not in SHARED_HOMES:
                self.names[name] = "<local-user>"
                known.add(name.lower())
        for index, instant in enumerate(sorted(found["instants"])):
            self.times[instant] = EPOCH + timedelta(seconds=index)

    def _collect(self, value, found):
        if isinstance(value, dict):
            if isinstance(value.get("argv"), list) and value["argv"][:1] == ["api"] and isinstance(value.get("stdout"), str):
                # `gh api user`, or `/user`: the answer is the viewer's own user object.
                if any(arg.strip("/") == "user" for arg in value["argv"][1:]):
                    for answer in embedded(value["stdout"]):
                        if isinstance(answer, dict) and isinstance(answer.get("login"), str):
                            add(found["viewers"], answer["login"])
            viewer = value.get("viewer")
            if isinstance(viewer, dict) and isinstance(viewer.get("login"), str):
                add(found["viewers"], viewer["login"])
            for key, item in value.items():
                if key == "login" and isinstance(item, str):
                    add(found["logins"], item)
                self._collect(item, found)
        elif isinstance(value, list):
            for item in value:
                self._collect(item, found)
        elif isinstance(value, str):
            for match in HOME.finditer(value):
                add(found["locals"], match.group(1) or match.group(2))
            for login in ACCOUNT.findall(value):
                add(found["viewers"], login)
            for match in TIMESTAMP.finditer(value):
                found["instants"].add(parse_time(match))
            for match in HTTP_DATE.finditer(value):
                found["instants"].add(datetime.strptime(match.group(0), HTTP_FORMAT).replace(tzinfo=timezone.utc))
            for parsed in embedded(value):
                self._collect(parsed, found)

    def text(self, value):
        value = USERINFO.sub(r"\1<redacted>@", value)
        value = HOME.sub("<home>", value)
        value = EMAIL.sub(lambda m: "<email>" if is_email(m.group(0)) else m.group(0), value)
        value = TOKEN.sub("<token>", value)
        value = AUTHORIZATION.sub(r"\1<token>", value)
        value = TOKEN_HEADER.sub(r"\1<token>", value)
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
        value = NODE_ID.sub(lambda m: self._node(m.group(0)), value)
        value = ACTIONS.sub(self._actions, value)
        # A time pass one didn't see (none should be left) still becomes the first placeholder.
        value = TIMESTAMP.sub(lambda m: format_time(self.times.get(parse_time(m), EPOCH), m), value)
        value = HTTP_DATE.sub(lambda m: self.times.get(datetime.strptime(m.group(0), HTTP_FORMAT).replace(
            tzinfo=timezone.utc), EPOCH).strftime(HTTP_FORMAT), value)
        return DATE.sub(lambda m: m.group(1) or "2000{0}01{0}01".format(m.group(2)), value)

    def _sha(self, match):
        sha = match.group(0)
        if PLACEHOLDER_SHA.fullmatch(sha):
            return sha
        return self.shas.setdefault(sha, f"{len(self.shas) + 1:040x}")

    def _node(self, node):
        if node.startswith("<node-id"):
            return node
        return self.nodes.setdefault(node, f"<node-id-{len(self.nodes) + 1}>")

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
                elif TIME_KEY.search(key) and isinstance(item, int) and not isinstance(item, bool) and item > 10 ** 9:
                    result[key] = int(EPOCH.timestamp()) * (1000 if item > 10 ** 12 else 1)
                elif key in NODE_KEYS and isinstance(item, str) and item:
                    result[key] = self._node(item)
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


def add(items, item):
    if item not in items:
        items.append(item)


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


def parse_time(match):
    """A TIMESTAMP match as a UTC instant; one without a zone counts as UTC."""
    year, _, month, day, _, hour, minute, second, fraction, _, zone = match.groups()
    offset = timedelta(0)
    if zone and zone != "Z":
        offset = timedelta(hours=int(zone[1:3]), minutes=int(zone[-2:])) * (1 if zone[0] == "+" else -1)
    micro = int((fraction or "0")[:6].ljust(6, "0"))
    local = datetime(int(year), int(month), int(day), int(hour), int(minute), int(second), micro,
                     tzinfo=timezone(offset))
    return local.astimezone(timezone.utc)


def format_time(instant, match):
    """An instant written the way the match wrote its time, in UTC, without a fraction."""
    _, dash, _, _, separator, _, _, _, _, space, zone = match.groups()
    text = instant.strftime(f"%Y{dash}%m{dash}%d{separator}%H:%M:%S")
    if zone:
        text += space + ("Z" if zone == "Z" else "+00:00" if ":" in zone else "+0000")
    return text


def is_ip(text):
    """An IP address; an IPv6 one needs two groups of digits, so `::add-mask::` isn't one."""
    try:
        address = ipaddress.ip_address(text)
    except ValueError:
        return False
    return address.version == 4 or sum(1 for group in text.split(":") if group) >= 2


def is_email(text):
    """An email: its domain ends in a name, not a version (`ripgrep@14.1.0`)."""
    return bool(re.search(r"\.[A-Za-z0-9-]*[A-Za-z][A-Za-z0-9-]*$", text.rsplit("@", 1)[-1]))


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
    """Re-encode in the original's style, when one of the usual ones reproduces it exactly: gh's
    compact JSON, Python's spaced, pretty-printed, with non-ASCII escaped, or with Go's escaping of
    `<`, `>` and `&`. Compact otherwise."""
    parsed = json.loads(original)
    for style in JSON_STYLES:
        for ascii_only in (False, True):
            for escape in (False, True):
                if dumps(parsed, style, ascii_only, escape) == original:
                    return dumps(value, style, ascii_only, escape)
    return dumps(value, JSON_STYLES[0], False, False)


def dumps(value, style, ascii_only, escape):
    text = json.dumps(value, ensure_ascii=ascii_only, **style)
    return text.replace("<", "\\u003c").replace(">", "\\u003e").replace("&", "\\u0026") if escape else text


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
