from __future__ import annotations

import json
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any


class ForgejoError(RuntimeError):
    pass


@dataclass(frozen=True)
class RepoRef:
    host: str
    owner: str
    repo: str

    @property
    def web_base_url(self) -> str:
        return f"https://{self.host}/{self.owner}/{self.repo}"

    @property
    def api_base_url(self) -> str:
        return f"https://{self.host}/api/v1/repos/{_quote(self.owner)}/{_quote(self.repo)}"


class ForgejoClient:
    def __init__(self, token: str | None = None, *, timeout: float = 30) -> None:
        self.token = token
        self.timeout = timeout

    def create_pull(
        self,
        repo: RepoRef,
        *,
        title: str,
        body: str,
        base: str,
        head: str,
        draft: bool = False,
    ) -> dict[str, Any]:
        payload = {
            "title": title,
            "body": body,
            "base": base,
            "head": head,
            "draft": draft,
        }
        return self._request_json("POST", f"{repo.api_base_url}/pulls", payload)

    def get_pull(self, repo: RepoRef, number: int) -> dict[str, Any]:
        return self._request_json("GET", f"{repo.api_base_url}/pulls/{number}", None)

    def get_repo(self, repo: RepoRef) -> dict[str, Any]:
        return self._request_json("GET", repo.api_base_url, None)

    def list_pulls(
        self,
        repo: RepoRef,
        *,
        state: str = "open",
        head: str | None = None,
    ) -> list[dict[str, Any]]:
        query = urllib.parse.urlencode({"state": state})
        pulls = self._request_json("GET", f"{repo.api_base_url}/pulls?{query}", None)
        if not isinstance(pulls, list):
            return []
        if head is None:
            return [item for item in pulls if isinstance(item, dict)]
        return [
            item
            for item in pulls
            if isinstance(item, dict) and _head_matches(item, head)
        ]

    def _request_json(
        self,
        method: str,
        url: str,
        payload: dict[str, Any] | None,
    ) -> Any:
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        headers = {
            "Accept": "application/json",
            "User-Agent": "gh-forgejo-shim",
        }
        if data is not None:
            headers["Content-Type"] = "application/json"
        if self.token:
            headers["Authorization"] = f"token {self.token}"
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                response_data = response.read()
        except urllib.error.HTTPError as exc:
            message = exc.read().decode("utf-8", errors="replace")
            raise ForgejoError(f"Forgejo API returned HTTP {exc.code}: {message}") from exc
        except urllib.error.URLError as exc:
            raise ForgejoError(f"Forgejo API request failed: {exc.reason}") from exc

        if not response_data:
            return {}
        try:
            return json.loads(response_data.decode("utf-8"))
        except json.JSONDecodeError as exc:
            raise ForgejoError("Forgejo API returned invalid JSON") from exc


def _quote(value: str) -> str:
    return urllib.parse.quote(value, safe="")


def _head_matches(item: dict[str, Any], head: str) -> bool:
    head_data = item.get("head")
    if not isinstance(head_data, dict):
        return False
    ref = head_data.get("ref")
    label = head_data.get("label")
    return ref == head or label == head or (isinstance(label, str) and label.endswith(f":{head}"))
