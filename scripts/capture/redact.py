"""Redact client captures before they're committed to fixtures/clients.

    python3 scripts/capture/redact.py --out fixtures/clients \\
        --name <your login>='<user>' --name <repo>='<repo>' --name <forge host>=forge.test \\
        <capture files...>

It takes gh-capture logs (`.jsonl`, with an optional `.marks` beside each), web server access logs
(`.log`), GraphQL documents and their variables (`.graphql`, `.vars`) and stub tables (`.json`), and
writes each under --out with its own name. It refuses anything else, screenshots included.

What it replaces, everywhere: the names given with --name; every login seen in a JSON answer or an
`auth status`, the capturing account's as `<user>`; home directories and the local user's name;
emails, IP addresses, tokens and credentials (Authorization, token and Cookie headers, URL
passwords, `key=value` secrets); avatar URLs, commit IDs, node IDs, hex folder names, Actions run
and job IDs, and dates and times in any format. In JSON, a user object keeps only its login, type
and URLs, a commit author loses its name and email, a value under a sensitive key is hidden, and
so is every HTTP header value but a few harmless ones. One table serves every file in a run, so a
value always maps to the same placeholder. A record's `ts` becomes `t`, seconds since the file's
first record, and its `.marks` labels become each record's `mark`. An answer repeated within a
file is stored once: later copies say `{"same_as": <line index>}`.

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
# A raw git header's identity line, as in a signed commit's `verification.payload`, and an armored
# signature or key block, which names its signer.
GIT_IDENT = re.compile(r"(?m)^((?:author|committer|tagger) ).*<[^>\n]*> \d+ [+-]\d{4}$")
ARMOR = re.compile(r"-----BEGIN ([A-Z ]+)-----.*?-----END \1-----", re.DOTALL)
COOKIE = re.compile(r"(?i)((?:set-)?cookie:\s*)[^\n\"]+")
SECRET_ASSIGNMENT = re.compile(r"(?i)\b([\w.-]*(?:token|secret|passw(?:or)?d|credential|api[_-]?key)[\w.-]*=)[^&\s\"']+")
# A home directory, URL-encoded too; its last part is the local user's name.
HOME = re.compile(r"(?:/|%2[Ff])(?:Users|home)(?:/|%2[Ff])([^/%\s\"'<>]+)|[A-Za-z]:\\+Users\\+([^\\\s\"'<>]+)")
SHARED_HOMES = {"shared", "guest"}
# Logins kept as they are: bots and apps, which GraphQL gives without `[bot]`, and words gh and
# GitHub use in paths and commands, which name an organization or the system (`actions/checkout`)
# far more often than a person. Replacing them would break every path; redact.py lists any it
# keeps, for a person to check.
BOTS = {"github-actions", "dependabot", "copilot", "renovate", "ghost", "web-flow", "forgejo-actions",
        "gitea-actions"}
PATH_WORDS = {"actions", "admin", "api", "apps", "checks", "cli", "commits", "compare", "explore", "git",
              "github", "graphql", "issues", "login", "notifications", "orgs", "pr", "pulls", "releases",
              "repos", "runs", "search", "settings", "status", "user", "users"}
# An email, but not a git remote in scp form (`git@github.com:o/r.git`).
EMAIL = re.compile(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?"
                   r"(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)+(?![\w-]|\.[\w-]|:[\w~/])")
AVATAR = re.compile(r"https?://(?:avatars\.githubusercontent\.com|(?:secure|www)\.gravatar\.com|"
                    r"[^/\s\"'<>]+/avatars?)/[^\s\"'<>]*")
IPV4 = re.compile(r"(?<![\d.])\d{1,3}(?:\.\d{1,3}){3}(?!\.?\d)")
IPV6 = re.compile(r"(?<![\w:.-])[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7}(?![\w:-])")
SHA = re.compile(r"(?<![0-9A-Fa-f])(?:[0-9a-f]{64}|[0-9a-f]{40})(?![0-9A-Fa-f])")
# A hex folder name in a path, which may be a build or install ID: `claude-code/2.1.309/<id>/`.
HEX_FOLDER = re.compile(r"(?<=/)(?![0-9a-f]{40}/|[0-9a-f]{64}/)(?=\d*[a-f])[0-9a-f]{8,64}(?=/)")
# Placeholders this script writes: leaks.py accepts these and nothing else of their shape.
PLACEHOLDER_SHA = re.compile(r"0{24}[0-9a-f]{16}")
ACTIONS = re.compile(r"(/actions/runs/)(\d+)(?:(/jobs?/)(\d+))?")
# A date and time: ISO 8601 and RFC 3339, git's `2026-10-30 15:16:57 -0400`, Forgejo's log
# `2026/10/30 15:16:57`, Go's `2026-10-30 15:16:57.123 -0400 EDT`, seconds and zone optional; git's
# and date(1)'s `Fri Oct 30 15:16:57 2026 -0400` and `Fri Oct 30 15:16:57 EDT 2026`; and HTTP's
# `Fri, 30 Oct 2026 19:02:30 GMT` or RFC 2822's `-0400`. A zone's name, `ChST` too, or Go's `-03`
# or `+0545` name, becomes UTC.
ZONE_NAMES = ("UTC|GMT|EST|EDT|CST|CDT|MST|MDT|PST|PDT|AKST|AKDT|HST|BST|CET|CEST|EET|EEST|WET|WEST|IST|"
              "JST|KST|AEST|AEDT|ACST|AWST|NZST|NZDT|MSK")
TIMESTAMP = re.compile(r"(?<!\d)(\d{4})([-/])(\d\d)\2(\d\d)([T ])(\d\d):(\d\d)(?::(\d\d)(?:\.(\d+))?)?"
                       r"( ?)(Z|[+-]\d\d:?\d\d)?( (?:" + ZONE_NAMES + r"|(?:(?<=[+-]\d{4} )|(?<=[+-]\d\d:\d\d ))"
                       r"(?:[A-Z][A-Za-z]{1,4}|[+-]\d\d(?:\d\d)?)))?(?![\w:])")
DAYS = "Mon|Tue|Wed|Thu|Fri|Sat|Sun"
MONTHS = "Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec"
CTIME = re.compile(r"\b(?:" + DAYS + r") (" + MONTHS + r") ( ?\d{1,2}) (\d\d):(\d\d):(\d\d)"
                   r"( (?:[A-Z][A-Za-z]{1,4}|[+-]\d\d(?:\d\d)?))? (\d{4})(?: ([+-]\d{4}))?(?![\w:])")
HTTP_DATE = re.compile(r"\b(?:" + DAYS + r"), (\d\d) (" + MONTHS + r") (\d{4}) (\d\d):(\d\d):(\d\d) (GMT|[+-]\d{4})\b")
# A date alone, as in a search (`closed:>2026-09-30`). Dates that are versions, not times, stay:
# gh's release date in `gh --version`, and GitHub's REST API version (`X-GitHub-Api-Version` and
# the `-Selected` answer).
KEPT_DATES = r"gh version \S+ \(\d{4}-\d\d-\d\d\)|(?i:api-version(?:-selected)?)[:=] *\d{4}-\d\d-\d\d"
DATE = re.compile(r"(" + KEPT_DATES + r")|(?<!\d)\d{4}([-/])\d\d\2\d\d(?!\d)")
# An integer time (epoch seconds or milliseconds) under a key that says so: `reset` in
# `gh api rate_limit`, `created_at`, `timestamp`, `expires`, a JWT's `exp` and `iat`.
TIME_KEY = re.compile(r"(?i)time|date|reset|expire|created|updated|modified|since|until|seen|^(?:exp|iat|nbf)$|"
                      r"(?:^|_)at$|[a-z]At$")
EPOCH_SECONDS = 946684800
LOG_TIME = re.compile(r"\[(\d\d/[A-Z][a-z]{2}/\d{4}:\d\d:\d\d:\d\d [+-]\d{4})\]")
# An access log line's client, ident and user fields, before its time.
LOG_FIELDS = re.compile(r"^(\S+) (\S+) (\S+) (?=\[)")
# gh's `auth status`: "Logged in to <host> account <login> (<source>)".
ACCOUNT = re.compile(r"Logged in to \S+ account (\S+)")
EPOCH = datetime(2000, 1, 1, tzinfo=timezone.utc)
SENSITIVE_KEY = re.compile(r"(?i)token|secret|passw(?:or)?d|passphrase|credential|api[_-]?key|private[_-]?key|cookie")
# A user object (any object with a `login`) keeps only these keys; the rest are hidden. GitHub's
# REST and GraphQL users and Forgejo's each add profile fields of their own (`bio`, `full_name`,
# `websiteUrl`, `pronouns`, `status`), so a list of what to hide would miss the next one.
USER_KEYS = {"login", "username", "type", "__typename", "site_admin", "user_view_type", "url", "html_url", "resourcePath", "avatar_url", "avatarUrl", "created_at", "updated_at",
             "createdAt", "updatedAt", "followers_url", "following_url", "gists_url", "starred_url",
             "subscriptions_url", "organizations_url", "repos_url", "events_url", "received_events_url"}
# A commit's signature and its check, which name the signer by key ID, fingerprint, subject or
# account: only their flags and types stay.
SIGNATURE_KEYS = {"signature", "verification"}
# Keys whose object is a person even without a `login` (GraphQL's `viewer{name bio}`), as is any
# object inside a user's.
USER_HOLDERS = {"viewer", "user", "author", "committer", "owner", "creator", "actor", "sender", "assignee",
                "mergedBy", "merged_by", "closed_by", "enabledBy", "requestedReviewer", "reviewer", "editor"}
# Profile fields in JSON that doesn't parse (cut short, or followed by other output), and the
# logins in it.
PROFILE_TEXT = re.compile(r'("(?:name|full_name|login_name|bio|bioHTML|location|blog|company|email|website|websiteUrl|'
                          r'description|twitter_username|twitterUsername|pronouns|gravatar_id)"\s*:\s*)"(?:[^"\\]|\\.)*"')
LOGIN_TEXT = re.compile(r'"(?:login|username)"\s*:\s*"([^"\\]+)"')
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
        self.kept = []

    def learn(self, values):
        """Pass one: logins, local user names and timestamps from every file, so placeholders are
        fixed before anything is written, and timestamps keep their order. The capturing account
        (gh's viewer) is `<user>`, and no one else is."""
        found = {"viewers": [], "logins": [], "locals": [], "instants": set()}
        for value in values:
            self._collect(value, found)
        known = {name.lower() for name in self.names}
        taken = set(self.names.values())

        def placeholder(viewer):
            """`<user>` for the viewer, if no one has it yet; the next free `<user-N>` otherwise."""
            if viewer and USER not in taken:
                return USER
            number = 2
            while f"<user-{number}>" in taken:
                number += 1
            return f"<user-{number}>"

        for viewer, logins in ((True, found["viewers"]), (False, found["logins"])):
            for login in logins:
                if login.lower() in PATH_WORDS and login.lower() not in known:
                    add(self.kept, login)
                if (login.lower() in known or login.lower() in BOTS | PATH_WORDS or login.endswith("[bot]")
                        or login.startswith("<")):
                    continue
                # `<user>` is only ever the viewer: anyone else is `<user-2>` on.
                self.names[login] = placeholder(viewer)
                taken.add(self.names[login])
                known.add(login.lower())
        for name in found["locals"]:
            if name.lower() not in known and name.lower() not in SHARED_HOMES:
                self.names[name] = "<local-user>"
                known.add(name.lower())
        for index, instant in enumerate(sorted(found["instants"])):
            self.times[instant] = EPOCH + timedelta(seconds=index)

    def _collect(self, value, found):
        if isinstance(value, dict):
            argv = value.get("argv")
            if isinstance(argv, list) and argv[:1] == ["api"] and isinstance(value.get("stdout"), str):
                # `gh api user`, or `/user`: the answer is the viewer's own user object, or with
                # `--jq .login` the bare login.
                if any(arg.strip("/") == "user" for arg in argv[1:]):
                    for answer in embedded(value["stdout"]):
                        if isinstance(answer, dict) and isinstance(answer.get("login"), str):
                            add(found["viewers"], answer["login"])
                    login = value["stdout"].strip().strip('"')
                    if (jq_expression(argv) == ".login" and value.get("exit") == 0 and login != "null"
                            and re.fullmatch(r"[A-Za-z0-9][\w.-]*", login)):
                        add(found["viewers"], login)
            viewer = value.get("viewer")
            if isinstance(viewer, dict) and isinstance(viewer.get("login"), str):
                add(found["viewers"], viewer["login"])
            for key, item in value.items():
                # Forgejo names a commit's author and signer by `username`.
                if key in ("login", "username") and isinstance(item, str) and item:
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
            for match in CTIME.finditer(value):
                found["instants"].add(parse_ctime(match))
            for match in HTTP_DATE.finditer(value):
                found["instants"].add(parse_http(match))
            parsed = embedded(value)
            for item in parsed:
                self._collect(item, found)
            if not parsed:
                for login in LOGIN_TEXT.findall(value):
                    add(found["logins"], login)

    def text(self, value):
        value = ARMOR.sub(r"-----BEGIN \1-----<redacted>-----END \1-----", value)
        value = GIT_IDENT.sub(r"\1<redacted> <email> %d +0000" % EPOCH_SECONDS, value)
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
        value = HEX_FOLDER.sub("<id>", value)
        value = NODE_ID.sub(lambda m: self._node(m.group(0)), value)
        value = ACTIONS.sub(self._actions, value)
        # A time pass one didn't see (none should be left) still becomes the first placeholder.
        value = TIMESTAMP.sub(lambda m: format_time(self.times.get(parse_time(m), EPOCH), m), value)
        value = CTIME.sub(lambda m: format_ctime(self.times.get(parse_ctime(m), EPOCH), m), value)
        value = HTTP_DATE.sub(lambda m: format_http(self.times.get(parse_http(m), EPOCH), m), value)
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

    def value(self, value, person=False):
        """A JSON value: strings by the text rules, and a person's details hidden by key. `person`
        marks a value held under a person's key."""
        if isinstance(value, dict):
            # A user keeps its allowlisted keys; any other object held by a person keeps only its type
            # and a git actor's date.
            user = "login" in value
            keep = USER_KEYS if user else {"__typename", "date"} if person else None
            identity = keep is None and ("email" in value or "name" in value and bool({"user", "date"} & set(value)))
            result = {}
            for key, item in value.items():
                if SENSITIVE_KEY.search(key) and isinstance(item, str) and item:
                    result[key] = "<redacted>"
                elif TIME_KEY.search(key) and isinstance(item, int) and not isinstance(item, bool) and item > 10 ** 9:
                    result[key] = EPOCH_SECONDS * (1000 if item > 10 ** 12 else 1)
                elif key in NODE_KEYS and isinstance(item, str) and item:
                    result[key] = self._node(item)
                elif key in SIGNATURE_KEYS and isinstance(item, dict):
                    result[key] = hidden(item)
                elif keep is not None and key not in keep:
                    result[key] = self.value(item, person=True) if isinstance(item, (dict, list)) else hidden(item)
                elif identity and key in IDENTITY_KEYS:
                    result[key] = hidden(item)
                else:
                    result[key] = self.value(item, person=key in USER_HOLDERS and isinstance(item, dict))
            return result
        if isinstance(value, list):
            return [self.value(item, person) for item in value]
        if person:
            return hidden(value)
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
            return PROFILE_TEXT.sub(r'\1"<redacted>"', self.text(text))
        out = []
        last = 0
        for start, end, item in spans:
            out += [text[last:start], dump_like(self.value(item), text[start:end])]
            last = end
        return "".join(out) + text[last:]


def jq_expression(argv):
    """The expression given to `--jq` or `-q`, stripped, or None."""
    for index, arg in enumerate(argv):
        flag, equal, expression = arg.partition("=")
        if flag in ("--jq", "-q"):
            return (expression if equal else argv[index + 1] if index + 1 < len(argv) else "").strip()
    return None


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


def utc(year, month, day, hour, minute, second, micro, zone):
    """A local time with an optional `Z`, `+HH:MM` or `+HHMM` zone (none: UTC) as a UTC instant."""
    offset = timedelta(0)
    if zone and zone != "Z":
        offset = timedelta(hours=int(zone[1:3]), minutes=int(zone[-2:])) * (1 if zone[0] == "+" else -1)
    return datetime(year, month, day, hour, minute, second, micro, tzinfo=timezone(offset)).astimezone(timezone.utc)


def parse_time(match):
    """A TIMESTAMP match as a UTC instant."""
    year, _, month, day, _, hour, minute, second, fraction, _, zone, _ = match.groups()
    micro = int((fraction or "0")[:6].ljust(6, "0"))
    return utc(int(year), int(month), int(day), int(hour), int(minute), int(second or 0), micro, zone)


def format_time(instant, match):
    """An instant written the way the match wrote its time, in UTC, without a fraction."""
    _, dash, _, _, separator, _, _, second, _, space, zone, name = match.groups()
    text = instant.strftime(f"%Y{dash}%m{dash}%d{separator}%H:%M" + (":%S" if second else ""))
    if zone:
        text += space + ("Z" if zone == "Z" else "+00:00" if ":" in zone else "+0000")
    return text + (" UTC" if name else "")


def parse_http(match):
    """An HTTP or RFC 2822 date (`GMT` or a numeric zone, as `git log --date=rfc` writes) as UTC."""
    day, month, year, hour, minute, second, zone = match.groups()
    return utc(int(year), MONTHS.split("|").index(month) + 1, int(day), int(hour), int(minute), int(second), 0,
               None if zone == "GMT" else zone)


def format_http(instant, match):
    return instant.strftime("%a, %d %b %Y %H:%M:%S ") + ("GMT" if match.group(7) == "GMT" else "+0000")


def parse_ctime(match):
    """A CTIME match as a UTC instant."""
    month, day, hour, minute, second, _, year, zone = match.groups()
    return utc(int(year), MONTHS.split("|").index(month) + 1, int(day), int(hour), int(minute), int(second), 0, zone)


def format_ctime(instant, match):
    """An instant in the match's own ctime form: the day space-padded if it was (date(1)), not if it
    wasn't (git), and a zone's name as UTC."""
    _, day, _, _, _, name, _, zone = match.groups()
    padded = str(instant.day).rjust(2) if day.startswith(" ") else str(instant.day)
    text = instant.strftime("%a %b ") + padded + instant.strftime(" %H:%M:%S")
    return text + (" UTC" if name else "") + instant.strftime(" %Y") + (" +0000" if zone else "")


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
        while index < len(text) and (text[index].isspace() or text[index] == "\ufeff"):
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

    def fields(match):
        return " ".join([match.group(1)] + [field if field == "-" else "<redacted>" for field in match.group(2, 3)]) + " "

    return "".join(redactor.text(LOG_FIELDS.sub(fields, LOG_TIME.sub(relative, line))) + "\n" for line in lines)


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
    if USER not in redactor.names.values():
        parser.error(f"found no capturing account in the captures: pass --name <your login>='{USER}'")
    if redactor.kept:
        print("kept these logins, which are also words in gh's paths; check they aren't people: "
              + ", ".join(redactor.kept), file=sys.stderr)
    args.out.mkdir(parents=True, exist_ok=True)
    for path in files:
        (args.out / redactor.text(path.name)).write_text(redact(path, redactor))
    return 0


if __name__ == "__main__":
    sys.exit(main())
