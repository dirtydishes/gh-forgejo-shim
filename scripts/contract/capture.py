"""Read-only captures from the GitHub twin with a checksum-pinned official gh."""

import hashlib
import io
import json
import os
import platform
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

from common import REPO, ROOT, load_cases, oracle_argv

VERSION = "2.103.3"


def official_gh(destination):
    machine = platform.machine().lower()
    arch = {"aarch64": "arm64", "arm64": "arm64", "x86_64": "amd64", "amd64": "amd64"}[machine]
    system = {"Darwin": "macOS", "Linux": "linux"}[platform.system()]
    suffix = "zip" if system == "macOS" else "tar.gz"
    name = f"gh_{VERSION}_{system}_{arch}.{suffix}"
    checksums = dict(line.split()[::-1] for line in (ROOT / "contract/gh-checksums.txt").read_text().splitlines())
    archive = subprocess.run(
        ["curl", "-fsSL", f"https://github.com/cli/cli/releases/download/v{VERSION}/{name}"],
        check=True, stdout=subprocess.PIPE,
    ).stdout
    if hashlib.sha256(archive).hexdigest() != checksums[name]:
        raise ValueError("official gh archive checksum mismatch")
    # Extract only the executable, never arbitrary archive paths.
    if suffix == "zip":
        with zipfile.ZipFile(io.BytesIO(archive)) as bundle:
            members = [n for n in bundle.namelist() if n.endswith("/bin/gh") or n == "bin/gh"]
            data = bundle.read(members[0]) if len(members) == 1 else None
    else:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as bundle:
            members = [m for m in bundle.getmembers() if m.name.endswith("/bin/gh") and m.isfile()]
            data = bundle.extractfile(members[0]).read() if len(members) == 1 else None
    if data is None:
        raise ValueError("official archive must contain exactly one gh executable")
    binary = destination / "gh"
    binary.write_bytes(data)
    binary.chmod(0o755)
    return binary


def main():
    cases = load_cases(ROOT / "contract/cases")  # Validate ALL cases before reading the login.
    cache = ROOT / "target/contract"
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="capture-", dir=cache) as temporary:
        directory = Path(temporary)
        binary = official_gh(directory)
        login_gh = shutil.which("gh")
        if not login_gh:
            raise ValueError("the host gh login is required")
        token_result = subprocess.run(
            [login_gh, "auth", "token", "--hostname", "github.com"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        if token_result.returncode or not token_result.stdout.strip():
            raise ValueError("could not read the host gh login for github.com")
        token = token_result.stdout.decode().strip()
        env = {
            "PATH": os.environ["PATH"], "HOME": str(directory), "GH_CONFIG_DIR": str(directory / "config"),
            "GH_TOKEN": token, "GH_HOST": "github.com", "GH_REPO": f"github.com/{REPO}",
            "GH_PROMPT_DISABLED": "1", "GH_PAGER": "cat", "GH_NO_UPDATE_NOTIFIER": "1",
            "NO_COLOR": "1", "TERM": "dumb", "LC_ALL": "C", "TZ": "UTC",
            "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        }

        def read(argv, extra_env=None):
            result = subprocess.run(
                [str(binary), *argv], cwd=directory, env=env | (extra_env or {}),
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=90,
            )
            # Refuse to persist even an accidental credential echo.
            if token.encode() in result.stdout or token.encode() in result.stderr:
                raise ValueError("capture refused output containing the login token")
            return {"exit": result.returncode, "stdout": result.stdout.decode(), "stderr": result.stderr.decode()}

        version = read(["--version"])
        if version["exit"] or not version["stdout"].startswith(f"gh version {VERSION} "):
            raise ValueError("downloaded gh version does not match")
        # These are also read-only allowlisted pr view commands against the same twin.
        scenarios = []
        for number in range(1, 8):
            result = read(["pr", "view", str(number), "--json", "number,title,body,state,isDraft,headRefName,baseRefName"])
            if result["exit"]:
                raise ValueError(f"could not capture scenario {number}")
            scenarios.append(json.loads(result["stdout"]))
        captures = [(path, case | {"oracle": read(oracle_argv(case), case.get("env"))}) for path, case in cases]
        # Only write after every request completes. No token enters these files.
        for path, case in captures:
            path.write_text(json.dumps(case, indent=2) + "\n")
            print(f"captured {case['id']}: exit {case['oracle']['exit']}")
        (ROOT / "contract/scenarios.json").write_text(json.dumps(scenarios, indent=2) + "\n")
        (ROOT / "contract/oracle-version.txt").write_text(version["stdout"])


if __name__ == "__main__":
    main()
