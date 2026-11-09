"""Fail if a committed capture holds anything that looks identifying.

    python3 scripts/capture/leaks.py fixtures/clients

It looks for tokens and credentials, IP addresses, emails, home directories, avatar URLs and any
host but GitHub's own and reserved test names, in every file under the given paths, and inside the
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
# A bare host name: labels ending in a common top-level domain. `.app` and `.sh` are left out:
# they name macOS bundles and scripts far more often than hosts in a capture.
BARE_HOST = re.compile(r"(?<![\w.-])((?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+"
                       r"(?:com|net|org|io|dev|me|ai|co|xyz|cloud|info|biz|us|uk|de|eu|ca|fr|nl|jp|"
                       r"local|lan|internal|home|arpa|onion|site|online|tech|page|gg|tv))(?![\w-])", re.IGNORECASE)


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
    if HOME.search(text):
        kinds.add("home directory")
    if any(is_ip(match.group(0)) for pattern in (IPV4, IPV6) for match in pattern.finditer(text)):
        kinds.add("IP address")
    if AVATAR.search(text):
        kinds.add("avatar URL")
    hosts = [match.group(1) for match in URL_HOST.finditer(text)]
    hosts += [match.group(1) for match in BARE_HOST.finditer(text)]
    if any(not allowed_host(host) and not is_ip(host) for host in hosts):
        kinds.add("host")
    return kinds


def value_findings(value):
    """The same, through a JSON value and the JSON answers inside its strings."""
    kinds = set()
    if isinstance(value, dict):
        for key, item in value.items():
            kinds |= text_findings(key)
            if SENSITIVE_KEY.search(key) and isinstance(item, str) and item not in PLACEHOLDER_VALUES:
                kinds.add("credential")
            kinds |= value_findings(item)
    elif isinstance(value, list):
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
