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

    def get_current_user(self, host: str) -> dict[str, Any]:
        return self._request_json("GET", f"https://{_host_for_url(host)}/api/v1/user", None)

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

    def get_pull_diff(self, repo: RepoRef, number: int, *, patch: bool = False) -> str:
        diff_type = "patch" if patch else "diff"
        return self._request_text("GET", f"{repo.api_base_url}/pulls/{number}.{diff_type}", None)

    def list_pull_files(self, repo: RepoRef, number: int) -> list[dict[str, Any]]:
        files = self._request_json("GET", f"{repo.api_base_url}/pulls/{number}/files", None)
        if not isinstance(files, list):
            return []
        return [item for item in files if isinstance(item, dict)]

    def list_commit_statuses(self, repo: RepoRef, sha: str) -> list[dict[str, Any]]:
        statuses = self._request_json("GET", f"{repo.api_base_url}/statuses/{_quote(sha)}", None)
        if isinstance(statuses, dict) and isinstance(statuses.get("statuses"), list):
            statuses = statuses["statuses"]
        if not isinstance(statuses, list):
            return []
        return [item for item in statuses if isinstance(item, dict)]

    def get_repo(self, repo: RepoRef) -> dict[str, Any]:
        return self._request_json("GET", repo.api_base_url, None)

    def list_branches(
        self,
        repo: RepoRef,
        *,
        limit: int | None = None,
        page: int | None = None,
    ) -> list[dict[str, Any]]:
        if limit is None and page is None:
            branches: list[dict[str, Any]] = []
            page_size = 100
            for page_number in range(1, 101):
                batch = self.list_branches(repo, limit=page_size, page=page_number)
                if not batch:
                    break
                branches.extend(batch)
            return branches

        params: dict[str, object] = {}
        if limit is not None:
            params["limit"] = limit
        if page is not None:
            params["page"] = page
        query_string = urllib.parse.urlencode(params)
        url = f"{repo.api_base_url}/branches"
        if query_string:
            url = f"{url}?{query_string}"
        branches = self._request_json("GET", url, None)
        if not isinstance(branches, list):
            return []
        return [item for item in branches if isinstance(item, dict)]

    def get_branch(self, repo: RepoRef, branch: str) -> dict[str, Any]:
        return self._request_json("GET", f"{repo.api_base_url}/branches/{_quote(branch)}", None)

    def create_branch(self, repo: RepoRef, *, new_branch: str, old_branch: str) -> dict[str, Any]:
        payload = {
            "new_branch_name": new_branch,
            "old_branch_name": old_branch,
        }
        return self._request_json("POST", f"{repo.api_base_url}/branches", payload)

    def list_issues(
        self,
        repo: RepoRef,
        *,
        state: str = "open",
        labels: tuple[str, ...] = (),
        query: str | None = None,
        milestone: str | None = None,
        author: str | None = None,
        assignee: str | None = None,
        mentioned: str | None = None,
        limit: int | None = None,
    ) -> list[dict[str, Any]]:
        params: dict[str, object] = {"state": state, "type": "issues"}
        if labels:
            params["labels"] = ",".join(labels)
        if query:
            params["q"] = query
        if milestone:
            params["milestones"] = milestone
        if author and author != "@me":
            params["created_by"] = author
        if assignee and assignee != "@me":
            params["assigned_by"] = assignee
        if mentioned and mentioned != "@me":
            params["mentioned_by"] = mentioned
        if limit is not None:
            params["limit"] = limit
        query_string = urllib.parse.urlencode(params)
        issues = self._request_json("GET", f"{repo.api_base_url}/issues?{query_string}", None)
        if not isinstance(issues, list):
            return []
        return [item for item in issues if isinstance(item, dict)]

    def get_issue(self, repo: RepoRef, number: int) -> dict[str, Any]:
        return self._request_json("GET", f"{repo.api_base_url}/issues/{number}", None)

    def list_labels(self, repo: RepoRef) -> list[dict[str, Any]]:
        labels = self._request_json("GET", f"{repo.api_base_url}/labels", None)
        if not isinstance(labels, list):
            return []
        return [item for item in labels if isinstance(item, dict)]

    def create_issue(
        self,
        repo: RepoRef,
        *,
        title: str,
        body: str = "",
        assignees: tuple[str, ...] = (),
        labels: tuple[int, ...] = (),
        milestone: int | None = None,
    ) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "title": title,
            "body": body,
        }
        if assignees:
            payload["assignees"] = list(assignees)
        if labels:
            payload["labels"] = list(labels)
        if milestone is not None:
            payload["milestone"] = milestone
        return self._request_json("POST", f"{repo.api_base_url}/issues", payload)

    def create_issue_comment(self, repo: RepoRef, number: int, *, body: str) -> dict[str, Any]:
        return self._request_json("POST", f"{repo.api_base_url}/issues/{number}/comments", {"body": body})

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
        response_data = self._request(method, url, payload, accept="application/json")
        if not response_data:
            return {}
        try:
            return json.loads(response_data.decode("utf-8"))
        except json.JSONDecodeError as exc:
            raise ForgejoError("Forgejo API returned invalid JSON") from exc

    def _request_text(
        self,
        method: str,
        url: str,
        payload: dict[str, Any] | None,
    ) -> str:
        return self._request(method, url, payload, accept="text/plain").decode("utf-8", errors="replace")

    def _request(
        self,
        method: str,
        url: str,
        payload: dict[str, Any] | None,
        *,
        accept: str,
    ) -> bytes:
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        headers = {
            "Accept": accept,
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

        return response_data


def _quote(value: str) -> str:
    return urllib.parse.quote(value, safe="")


def _host_for_url(host: str) -> str:
    parsed = urllib.parse.urlparse(host)
    if parsed.scheme and parsed.netloc:
        return parsed.netloc
    return host.strip().strip("/")


def _head_matches(item: dict[str, Any], head: str) -> bool:
    head_data = item.get("head")
    if not isinstance(head_data, dict):
        return False
    ref = head_data.get("ref")
    label = head_data.get("label")
    return ref == head or label == head or (isinstance(label, str) and label.endswith(f":{head}"))
