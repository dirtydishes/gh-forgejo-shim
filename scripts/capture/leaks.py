"""Fail if a committed capture holds anything that looks identifying.

    python3 scripts/capture/leaks.py fixtures/clients

It looks for tokens and credentials, IP addresses, emails, home directories, avatar URLs, dates
other than the placeholders' and any host but GitHub's own and reserved test names, in every file under the given paths, and inside the
JSON answers captured logs carry. It prints the file, line and kind of each finding, never the
text, so a leak doesn't reach a CI log. Values redact.py writes as placeholders pass.
"""

import ipaddress
import json
import re
import sys
from pathlib import Path

from redact import embedded

ALLOWED_HOSTS = {"github.com", "api.github.com", "example.com", "example.net", "example.org"}
RESERVED_SUFFIXES = (".test", ".example", ".invalid", ".localhost")
PLACEHOLDER_VALUES = {"", "<redacted>", "<token>"}
TOKEN = re.compile(r"(?:github_pat_|gh[pousr]_|glpat-|gitea_)[A-Za-z0-9_]{8,}")
JWT = re.compile(r"eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}")
HEX = re.compile(r"(?<![0-9A-Fa-f])(?:[0-9A-Fa-f]{64}|[0-9A-Fa-f]{40})(?![0-9A-Fa-f])")
PLACEHOLDER_SHA = re.compile(r"0{24}[0-9a-f]{16}")
AUTHORIZATION = re.compile(r"(?i)authorization:\s*(?:(?:token|bearer|basic)\s+)?([^\s\"',\\]+)")
COOKIE = re.compile(r"(?i)(?:set-)?cookie:\s*([^\n\"\\]*)")
SECRET_ASSIGNMENT = re.compile(r"(?i)\b[\w.-]*(?:token|secret|passw(?:or)?d|credential|api[_-]?key)[\w.-]*=([^&\s\"'\\]+)")
SENSITIVE_KEY = re.compile(r"(?i)token|secret|passw(?:or)?d|passphrase|credential|api[_-]?key|private[_-]?key|cookie")
EMAIL = re.compile(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@([A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?"
                   r"(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)+)")
HOME = re.compile(r"/(?:Users|home)/|[A-Za-z]:\\+Users\\")
IPV4 = re.compile(r"(?<![\d.])\d{1,3}(?:\.\d{1,3}){3}(?!\.?\d)")
IPV6 = re.compile(r"(?<![\w:.])[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7}(?![\w:])")
URL_HOST = re.compile(r"[A-Za-z][A-Za-z0-9+.-]*://(?:[^/\s\"'<>@]*@)?([^/\s\"'<>@?#\\]+)")
USERINFO = re.compile(r"://([^/\s\"'<>@]+)@")
AVATAR = re.compile(r"avatars\.githubusercontent\.com|gravatar\.com|://[^/\s\"'<>]+/avatars?/", re.IGNORECASE)
# A bare host name: labels ending in a common top-level domain, not inside a path (`ChatGPT.app`
# in `/Applications`, `settings.local.json`).
BARE_HOST = re.compile(r"(?<![\w./-])((?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+"
                       r"(?:com|net|org|io|dev|me|ai|co|xyz|cloud|info|biz|app|sh|run|so|is|it|ch|se|fi|no|"
                       r"dk|pl|es|be|at|cz|ie|pt|ru|br|mx|kr|cn|tw|hk|sg|za|au|nz|in|ly|to|cc|ws|fm|am|li|lu|"
                       r"us|uk|de|eu|ca|fr|nl|jp|local|lan|internal|home|arpa|onion|site|online|tech|page|gg|tv|"
                       r"live|space|world|zone|host|link|codes|works|systems|network|tools|email))"
                       r"(?![\w-]|\.[\w-])", re.IGNORECASE)
# Where gh captures name a host without a scheme: `gh auth status`, a git remote in scp form, and
# a host flag or variable in text such as a log. Records' argv and env are read in value_findings.
NAMED_HOST = re.compile(r"Logged in to (\S+) account|(?<![\w/])[\w.-]+@([A-Za-z0-9][\w.-]*):(?!//)|"
                        r"(?:--hostname|--host|GH_HOST)[= ]\s*([^\s\"',]+)")
# Dates other than the placeholders' 2000-01-01; gh's own release date in `gh --version` stays.
GH_VERSION = re.compile(r"gh version \S+ \(\d{4}-\d\d-\d\d\)")
DATE = re.compile(r"\b(\d{4})([-/])(\d\d)\2(\d\d)\b|\b\d\d/(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)/\d{4}\b|"
                  r"\b\d\d (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) \d{4}\b")
HOST_FLAGS = {"--hostname", "--host", "-h"}
REPO_FLAGS = {"--repo", "-R"}
HOST_VARIABLES = {"GH_HOST", "GH_ENTERPRISE_HOST", "GITHUB_SERVER_URL"}


def allowed_host(host):
    host = host.lower().rstrip(".")
    host = host.rsplit(":", 1)[0] if host.count(":") == 1 else host
    return host.startswith("<") or host in ALLOWED_HOSTS or host.endswith(RESERVED_SUFFIXES) or host in {
        suffix[1:] for suffix in RESERVED_SUFFIXES}


def is_ip(text):
    try:
        ipaddress.ip_address(text)
    except ValueError:
        return False
    return True


def text_findings(text):
    """The kinds of identifying data in one string."""
    kinds = set()
    if TOKEN.search(text) or JWT.search(text):
        kinds.add("token")
    if any(not PLACEHOLDER_SHA.fullmatch(match.group(0)) for match in HEX.finditer(text)):
        kinds.add("token or unredacted commit ID")
    for pattern in (AUTHORIZATION, COOKIE, SECRET_ASSIGNMENT, USERINFO):
        if any(match.group(1).strip() not in PLACEHOLDER_VALUES for match in pattern.finditer(text)):
            kinds.add("credential")
    for match in EMAIL.finditer(text):
        if not allowed_host(match.group(1)):
            kinds.add("email")
    if any(match.group(0).replace("/", "-") != "2000-01-01" and match.group(0) != "01 Jan 2000"
           for match in DATE.finditer(GH_VERSION.sub("", text))):
        kinds.add("date")
    if HOME.search(text):
        kinds.add("home directory")
    if any(is_ip(match.group(0)) for pattern in (IPV4, IPV6) for match in pattern.finditer(text)):
        kinds.add("IP address")
    if AVATAR.search(text):
        kinds.add("avatar URL")
    hosts = [match.group(1) for match in URL_HOST.finditer(text)]
    hosts += [match.group(1) for match in BARE_HOST.finditer(text)]
    hosts += [host for match in NAMED_HOST.finditer(text) for host in match.groups() if host]
    if any(not allowed_host(host) and not is_ip(host) for host in hosts):
        kinds.add("host")
    return kinds


def argv_hosts(argv):
    """The hosts an argv names: a host flag's value, and the host of `--repo HOST/OWNER/REPO`."""
    hosts = []
    for index, arg in enumerate(argv):
        flag, equal, value = arg.partition("=")
        if not equal:
            value = argv[index + 1] if index + 1 < len(argv) else ""
        if flag in HOST_FLAGS:
            hosts.append(value)
        elif flag in REPO_FLAGS and value.count("/") >= 2 and "://" not in value:
            hosts.append(value.split("/", 1)[0])
        elif arg.startswith("-R") and len(arg) > 2 and arg[2:].count("/") >= 2:
            hosts.append(arg[2:].split("/", 1)[0])
    return [host for host in hosts if host]


def value_findings(value):
    """The same, through a JSON value and the JSON answers inside its strings."""
    kinds = set()
    if isinstance(value, dict):
        for key, item in value.items():
            kinds |= text_findings(key)
            if SENSITIVE_KEY.search(key) and isinstance(item, str) and item not in PLACEHOLDER_VALUES:
                kinds.add("credential")
            if key in HOST_VARIABLES and isinstance(item, str) and "://" not in item and not allowed_host(item):
                kinds.add("host")
            kinds |= value_findings(item)
    elif isinstance(value, list):
        if all(isinstance(item, str) for item in value) and any(not allowed_host(h) for h in argv_hosts(value)):
            kinds.add("host")
        for item in value:
            kinds |= value_findings(item)
    elif isinstance(value, str):
        kinds |= text_findings(value)
        for item in embedded(value):
            kinds |= value_findings(item)
    return kinds


def file_findings(path):
    """(line, kind) pairs for one file."""
    data = path.read_bytes()
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return [(0, "binary file")]
    if path.suffix == ".json":
        try:
            return [(0, kind) for kind in sorted(value_findings(json.loads(text)))]
        except ValueError:
            pass
    findings = []
    for number, line in enumerate(text.splitlines(), 1):
        kinds = text_findings(line)
        if path.suffix == ".jsonl" and line.strip():
            try:
                kinds |= value_findings(json.loads(line))
            except ValueError:
                kinds.add("invalid JSON line")
        findings.extend((number, kind) for kind in sorted(kinds))
    return findings


def check(paths):
    """Every finding under the paths, as `file:line: kind` lines."""
    files = []
    for path in map(Path, paths):
        files.extend(sorted(p for p in path.rglob("*") if p.is_file()) if path.is_dir() else [path])
    return [f"{path}:{line}: {kind}" for path in files for line, kind in file_findings(path)]


def main(argv=None):
    paths = sys.argv[1:] if argv is None else argv
    if not paths:
        print("usage: leaks.py <file or directory>...", file=sys.stderr)
        return 2
    findings = check(paths)
    for finding in findings:
        print(finding)
    if findings:
        print(f"{len(findings)} findings: redact these with scripts/capture/redact.py", file=sys.stderr)
    return int(bool(findings))


if __name__ == "__main__":
    sys.exit(main())
