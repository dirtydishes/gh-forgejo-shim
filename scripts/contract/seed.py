"""Seed a disposable real Forgejo, run a command, and remove only this run's resources."""

import base64
import http.client
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

from common import REPO, ROOT

IMAGE = "codeberg.org/forgejo/forgejo:15"


def docker(*args):
    return subprocess.run(["docker", *args], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout.decode().strip()


class Forgejo:
    def __init__(self, base):
        self.base = base
        self.token = None
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def api(self, path, data=None, method=None, basic=None):
        headers = {"Content-Type": "application/json"}
        if self.token:
            headers["Authorization"] = f"token {self.token}"
        if basic:
            headers["Authorization"] = "Basic " + base64.b64encode(basic.encode()).decode()
        request = urllib.request.Request(
            self.base + "/api/v1/" + path,
            data=json.dumps(data).encode() if data is not None else None,
            headers=headers, method=method,
        )
        with self.opener.open(request, timeout=20) as response:
            body = response.read()
        return json.loads(body) if body else None


def wait_until(description, predicate, attempts=90):
    for _ in range(attempts):
        try:
            if predicate():
                return
        except (urllib.error.URLError, TimeoutError, ConnectionError, http.client.RemoteDisconnected):
            pass
        time.sleep(1)
    raise RuntimeError(f"timed out waiting for {description}")


def seed(forgejo, directory):
    token = forgejo.api("users/dirtydishes/tokens", {"name": "gfjs-contract", "scopes": ["all"]}, basic="dirtydishes:gfjs-disposable-password")
    forgejo.token = token["sha1"]
    forgejo.api("user/repos", {"name": "gfj-oracle", "private": False, "default_branch": "main"})
    scenarios = json.loads((ROOT / "contract/scenarios.json").read_text())
    checkout = directory / "seed"
    checkout.mkdir()
    env = {
        "PATH": os.environ["PATH"], "HOME": str(directory), "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull, "GIT_TERMINAL_PROMPT": "0",
    }

    def git(*args, extra=None):
        result = subprocess.run(["git", *args], cwd=checkout, env=env | (extra or {}),
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if result.returncode:
            # git errors can echo Authorization headers. Never include them in a report.
            raise RuntimeError(f"seed git {args[0]} failed with exit {result.returncode}")
        return result.stdout.decode().strip()

    git("init", "-q", "-b", "main")
    git("config", "user.name", "dirtydishes")
    git("config", "user.email", "gfjs@example.invalid")
    (checkout / "README.md").write_text("Contract twin for dirtydishes/gfj-oracle.\n")
    git("add", ".")
    git("commit", "-qm", "main")
    for scenario in scenarios:
        git("checkout", "-qb", scenario["headRefName"], "main")
        (checkout / f"scenario-{scenario['number']}.txt").write_text(scenario["headRefName"] + "\n")
        git("add", ".")
        git("commit", "-qm", scenario["title"])
    git("push", "-q", f"{forgejo.base}/{REPO}.git", "main", *(s["headRefName"] for s in scenarios), extra={
        "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "http.extraHeader",
        "GIT_CONFIG_VALUE_0": f"Authorization: token {forgejo.token}",
    })
    wait_until("Forgejo post-receive metadata", lambda: not forgejo.api(f"repos/{REPO}")["empty"], attempts=30)
    pulls = {}
    for scenario in scenarios:
        pull = forgejo.api(f"repos/{REPO}/pulls", {
            "base": scenario["baseRefName"], "head": scenario["headRefName"],
            "title": scenario["title"], "body": scenario["body"],
        })
        number = scenario["number"]
        if pull["number"] != number or pull["draft"] != scenario["isDraft"]:
            raise RuntimeError(f"Forgejo did not reproduce scenario {number}'s number/draft")
        pulls[number] = pull
        if scenario["state"] == "MERGED":
            wait_until(f"PR {number} mergeability", lambda: forgejo.api(f"repos/{REPO}/pulls/{number}")["mergeable"], attempts=30)
            forgejo.api(f"repos/{REPO}/pulls/{number}/merge", {"Do": "merge", "delete_branch_after_merge": False})
        elif scenario["state"] == "CLOSED":
            forgejo.api(f"repos/{REPO}/pulls/{number}", {"state": "closed"}, method="PATCH")
    for number in (1, 2):
        for index, job in enumerate(("check", "lint"), start=1):
            forgejo.api(f"repos/{REPO}/statuses/{pulls[number]['head']['sha']}", {
                "context": f"ci / {job} (pull_request)",
                "target_url": f"/{REPO}/actions/runs/{number}/jobs/{index}",
                "state": "failure" if number == 2 and job == "check" else "success",
                "description": "Has finished running",
            })
    # Validate scenario state and source data, so a seeding bug cannot be accepted as an xfail.
    for scenario in scenarios:
        number = scenario["number"]
        pull = forgejo.api(f"repos/{REPO}/pulls/{number}")
        state = "MERGED" if pull["merged"] else pull["state"].upper()
        actual = (pull["number"], pull["title"], pull["body"], state, pull["draft"], pull["head"]["ref"], pull["base"]["ref"])
        expected = tuple(scenario[k] for k in ("number", "title", "body", "state", "isDraft", "headRefName", "baseRefName"))
        if actual != expected:
            raise RuntimeError(f"Forgejo scenario {number} differs from committed GitHub scenario")
        statuses = forgejo.api(f"repos/{REPO}/commits/{pull['head']['sha']}/statuses")
        if number in (1, 2):
            observed = {s["context"]: s["status"] for s in statuses}
            expected_statuses = {
                "ci / check (pull_request)": "failure" if number == 2 else "success",
                "ci / lint (pull_request)": "success",
            }
            if observed != expected_statuses or any(not s["target_url"].startswith(f"/{REPO}/actions/runs/") for s in statuses):
                raise RuntimeError(f"Forgejo scenario {number} statuses differ")
        elif number == 5 and statuses:
            raise RuntimeError("scenario 5 must not have checks")
    config_home = directory / "seed-home"
    config_dir = config_home / ".config/gh-forgejo-shim"
    config_dir.mkdir(parents=True)
    (config_dir / "config.toml").write_text(
        f'hosts = ["forge.test"]\n[host."forge.test"]\napi_url = "{forgejo.base}"\ntoken_env = "GFJS_TOKEN"\n'
    )
    return {"GFJS_TOKEN": forgejo.token, "GFJS_SEED_HOME": str(config_home), "GFJS_WORK_DIR": str(directory)}


def main():
    if len(sys.argv) < 2:
        raise ValueError("usage: seed-forgejo.sh <command> [args...]; seeds, runs, then tears down")
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    docker("info")
    name = f"gfjs-contract-{uuid.uuid4().hex[:12]}"
    volume = name + "-data"
    cache = ROOT / "target/contract"
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=name + "-", dir=cache) as temporary:
        try:
            docker("network", "create", name)
            docker("volume", "create", volume)
            docker("run", "-d", "--name", name, "--network", name, "--publish", "127.0.0.1::3000", "-v", f"{volume}:/data",
                   "-e", "FORGEJO__security__INSTALL_LOCK=true", "-e", "FORGEJO__database__DB_TYPE=sqlite3",
                   "-e", "FORGEJO__server__ROOT_URL=https://forge.test/", "-e", "FORGEJO__server__DOMAIN=forge.test",
                   "-e", "FORGEJO__service__DISABLE_REGISTRATION=true", "-e", "FORGEJO__mailer__ENABLED=false",
                   "-e", "FORGEJO__repository__DEFAULT_BRANCH=main",
                   "-e", "FORGEJO__repository.pull-request__WORK_IN_PROGRESS_PREFIXES=scenario: draft", IMAGE)
            port = docker("port", name, "3000/tcp").rsplit(":", 1)[1]
            forgejo = Forgejo(f"http://127.0.0.1:{port}")
            wait_until("Forgejo 15 readiness", lambda: forgejo.api("version"))
            version = forgejo.api("version")["version"]
            if not version.startswith("15."):
                raise RuntimeError(f"expected Forgejo 15, got {version}")
            docker("exec", "--user", "git", name, "forgejo", "--config", "/data/gitea/conf/app.ini", "admin", "user", "create",
                   "--username", "dirtydishes", "--password", "gfjs-disposable-password", "--email", "gfjs@example.invalid",
                   "--admin", "--must-change-password=false")
            env = os.environ | seed(forgejo, Path(temporary))
            print("seeded Forgejo 15: PRs 1-7, Actions-style statuses on 1 and 2; PR 3 is stale and untested", flush=True)
            return subprocess.run(sys.argv[1:], cwd=ROOT, env=env).returncode
        finally:
            for args in (("rm", "-fv", name), ("network", "rm", name), ("volume", "rm", volume)):
                subprocess.run(["docker", *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (subprocess.CalledProcessError, urllib.error.URLError) as error:
        # Do not dump HTTP bodies, request headers, or environments containing tokens.
        print(f"contract setup failed: {type(error).__name__}", file=sys.stderr)
        sys.exit(1)
