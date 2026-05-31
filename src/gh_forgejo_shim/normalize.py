from __future__ import annotations

from typing import Any

SUPPORTED_JSON_FIELDS = (
    "additions",
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


def normalize_pull(pull: dict[str, Any]) -> dict[str, Any]:
    user = pull.get("user") if isinstance(pull.get("user"), dict) else {}
    head = pull.get("head") if isinstance(pull.get("head"), dict) else {}
    base = pull.get("base") if isinstance(pull.get("base"), dict) else {}
    state = pull.get("state")

    return {
        "number": pull.get("number") or pull.get("id"),
        "title": pull.get("title"),
        "state": str(state).upper() if state is not None else None,
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
        "mergeable": pull.get("mergeable"),
        "mergeStateStatus": pull.get("merge_state_status") or "UNKNOWN",
        "reviewDecision": pull.get("reviewDecision"),
        "reviewRequests": pull.get("reviewRequests") or [],
        "statusCheckRollup": pull.get("statusCheckRollup") or [],
    }


def filter_fields(data: dict[str, Any], fields: tuple[str, ...]) -> dict[str, Any]:
    if not fields:
        return data
    return {field: data.get(field) for field in fields if field in SUPPORTED_JSON_FIELDS}


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
