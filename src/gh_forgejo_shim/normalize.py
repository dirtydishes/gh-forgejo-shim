from __future__ import annotations

from typing import Any

SUPPORTED_JSON_FIELDS = (
    "additions",
    "deletions",
    "number",
    "title",
    "state",
    "isDraft",
    "url",
    "headRefName",
    "baseRefName",
    "author",
    "createdAt",
    "updatedAt",
    "mergeable",
    "mergeStateStatus",
    "reviewDecision",
    "reviewRequests",
    "statusCheckRollup",
)

SUPPORTED_REPO_JSON_FIELDS = (
    "description",
    "defaultBranchRef",
    "isPrivate",
    "name",
    "nameWithOwner",
    "owner",
    "sshUrl",
    "url",
)


def normalize_pull(pull: dict[str, Any]) -> dict[str, Any]:
    user = pull.get("user") if isinstance(pull.get("user"), dict) else {}
    head = pull.get("head") if isinstance(pull.get("head"), dict) else {}
    base = pull.get("base") if isinstance(pull.get("base"), dict) else {}
    state = _normalize_state(pull)

    return {
        "additions": _normalize_count(pull.get("additions")),
        "deletions": _normalize_count(pull.get("deletions")),
        "number": pull.get("number") or pull.get("id"),
        "title": pull.get("title"),
        "state": state,
        "isDraft": bool(pull.get("draft", False)),
        "url": pull.get("html_url") or pull.get("url"),
        "headRefName": head.get("ref"),
        "baseRefName": base.get("ref"),
        "author": {
            "login": user.get("login") or user.get("username"),
            "name": user.get("full_name") or user.get("name"),
        },
        "createdAt": pull.get("created_at"),
        "updatedAt": pull.get("updated_at"),
        "mergeable": _normalize_mergeable(pull),
        "mergeStateStatus": str(pull.get("merge_state_status") or "UNKNOWN").upper(),
        "reviewDecision": pull.get("reviewDecision"),
        "reviewRequests": pull.get("reviewRequests") or [],
        "statusCheckRollup": pull.get("statusCheckRollup") or [],
    }


def _normalize_count(value: Any) -> int:
    if isinstance(value, bool):
        return 0
    if isinstance(value, int):
        return value
    return 0


def _normalize_state(pull: dict[str, Any]) -> str | None:
    if pull.get("merged") or pull.get("merged_at"):
        return "MERGED"
    state = pull.get("state")
    return str(state).upper() if state is not None else None


def _normalize_mergeable(pull: dict[str, Any]) -> str:
    raw = pull.get("mergeable")
    if isinstance(raw, str):
        value = raw.upper()
        if value in {"MERGEABLE", "CONFLICTING", "UNKNOWN"}:
            return value
    if raw is True:
        return "MERGEABLE"
    merge_state = str(pull.get("merge_state_status") or "").upper()
    if merge_state in {"DIRTY", "CONFLICTING", "CONFLICT"}:
        return "CONFLICTING"
    return "UNKNOWN"


def filter_fields(data: dict[str, Any], fields: tuple[str, ...]) -> dict[str, Any]:
    if not fields:
        return data
    return {field: data.get(field) for field in fields if field in SUPPORTED_JSON_FIELDS}


def normalize_repo(repo: dict[str, Any], fallback: Any) -> dict[str, Any]:
    owner = repo.get("owner") if isinstance(repo.get("owner"), dict) else {}
    owner_login = owner.get("login") or owner.get("username") or getattr(fallback, "owner")
    name = repo.get("name") or getattr(fallback, "repo")
    default_branch = repo.get("default_branch")
    return {
        "description": repo.get("description"),
        "defaultBranchRef": {"name": default_branch} if default_branch else None,
        "isPrivate": bool(repo.get("private", False)),
        "name": name,
        "nameWithOwner": repo.get("full_name") or f"{owner_login}/{name}",
        "owner": {
            "login": owner_login,
            "type": owner.get("type"),
        },
        "sshUrl": repo.get("ssh_url"),
        "url": repo.get("html_url") or getattr(fallback, "web_base_url"),
    }


def filter_repo_fields(data: dict[str, Any], fields: tuple[str, ...]) -> dict[str, Any]:
    if not fields:
        return data
    return {field: data.get(field) for field in fields if field in SUPPORTED_REPO_JSON_FIELDS}


def status_for_current_branch(
    pull: dict[str, Any] | None,
    fields: tuple[str, ...],
) -> dict[str, Any]:
    normalized = None if pull is None else filter_fields(normalize_pull(pull), fields)
    return {
        "currentBranch": normalized,
        "createdBy": [],
        "needsReview": [],
    }
