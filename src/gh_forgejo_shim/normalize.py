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
    "archivedAt",
    "assignableUsers",
    "codeOfConduct",
    "contactLinks",
    "createdAt",
    "defaultBranchRef",
    "deleteBranchOnMerge",
    "description",
    "diskUsage",
    "forkCount",
    "fundingLinks",
    "hasDiscussionsEnabled",
    "hasIssuesEnabled",
    "hasProjectsEnabled",
    "hasWikiEnabled",
    "homepageUrl",
    "id",
    "isArchived",
    "isBlankIssuesEnabled",
    "isEmpty",
    "isFork",
    "isInOrganization",
    "isMirror",
    "isPrivate",
    "isSecurityPolicyEnabled",
    "isTemplate",
    "isUserConfigurationRepository",
    "issueTemplates",
    "issues",
    "labels",
    "languages",
    "latestRelease",
    "licenseInfo",
    "mentionableUsers",
    "mergeCommitAllowed",
    "milestones",
    "mirrorUrl",
    "name",
    "nameWithOwner",
    "openGraphImageUrl",
    "owner",
    "parent",
    "primaryLanguage",
    "projects",
    "projectsV2",
    "pullRequestTemplates",
    "pullRequests",
    "pushedAt",
    "rebaseMergeAllowed",
    "repositoryTopics",
    "securityPolicyUrl",
    "squashMergeAllowed",
    "sshUrl",
    "stargazerCount",
    "templateRepository",
    "updatedAt",
    "url",
    "usesCustomOpenGraphImage",
    "viewerCanAdminister",
    "viewerDefaultCommitEmail",
    "viewerDefaultMergeMethod",
    "viewerHasStarred",
    "viewerPermission",
    "viewerPossibleCommitEmails",
    "viewerSubscription",
    "visibility",
    "watchers",
)

SUPPORTED_ISSUE_JSON_FIELDS = (
    "assignees",
    "author",
    "body",
    "closed",
    "closedAt",
    "closedByPullRequestsReferences",
    "comments",
    "createdAt",
    "id",
    "isPinned",
    "labels",
    "milestone",
    "number",
    "projectCards",
    "projectItems",
    "reactionGroups",
    "state",
    "stateReason",
    "title",
    "updatedAt",
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
    language = repo.get("language")
    topics = repo.get("topics") if isinstance(repo.get("topics"), list) else []
    permissions = repo.get("permissions") if isinstance(repo.get("permissions"), dict) else {}
    is_private = bool(repo.get("private", False))
    is_internal = bool(repo.get("internal", False))
    return {
        "archivedAt": repo.get("archived_at"),
        "assignableUsers": [],
        "codeOfConduct": None,
        "contactLinks": [],
        "createdAt": repo.get("created_at"),
        "description": repo.get("description"),
        "defaultBranchRef": {"name": default_branch} if default_branch else None,
        "deleteBranchOnMerge": bool(repo.get("default_delete_branch_after_merge", False)),
        "diskUsage": _normalize_count(repo.get("size")),
        "forkCount": _normalize_count(repo.get("forks_count")),
        "fundingLinks": [],
        "hasDiscussionsEnabled": False,
        "hasIssuesEnabled": bool(repo.get("has_issues", False)),
        "hasProjectsEnabled": bool(repo.get("has_projects", False)),
        "hasWikiEnabled": bool(repo.get("has_wiki", False)),
        "homepageUrl": repo.get("website") or None,
        "id": repo.get("id"),
        "isArchived": bool(repo.get("archived", False)),
        "isBlankIssuesEnabled": bool(repo.get("has_issues", False)),
        "isEmpty": bool(repo.get("empty", False)),
        "isFork": bool(repo.get("fork", False)),
        "isInOrganization": _is_organization(owner),
        "isMirror": bool(repo.get("mirror", False)),
        "isPrivate": is_private,
        "isSecurityPolicyEnabled": False,
        "isTemplate": bool(repo.get("template", False)),
        "isUserConfigurationRepository": False,
        "issueTemplates": [],
        "issues": _connection(_normalize_count(repo.get("open_issues_count"))),
        "labels": _connection(0),
        "languages": {
            "nodes": [{"name": language}] if isinstance(language, str) and language else [],
            "totalSize": 0,
        },
        "latestRelease": None,
        "licenseInfo": None,
        "mentionableUsers": [],
        "mergeCommitAllowed": bool(repo.get("allow_merge_commits", False)),
        "milestones": _connection(0),
        "mirrorUrl": repo.get("original_url") or None,
        "name": name,
        "nameWithOwner": repo.get("full_name") or f"{owner_login}/{name}",
        "openGraphImageUrl": repo.get("avatar_url"),
        "owner": {
            "login": owner_login,
            "type": owner.get("type"),
        },
        "parent": _normalize_parent(repo.get("parent")),
        "primaryLanguage": {"name": language} if isinstance(language, str) and language else None,
        "projects": _connection(0),
        "projectsV2": _connection(0),
        "pullRequestTemplates": [],
        "pullRequests": _connection(_normalize_count(repo.get("open_pr_counter"))),
        "pushedAt": repo.get("updated_at"),
        "rebaseMergeAllowed": bool(repo.get("allow_rebase", False)),
        "repositoryTopics": {
            "nodes": [{"topic": {"name": topic}} for topic in topics if isinstance(topic, str)],
            "totalCount": len([topic for topic in topics if isinstance(topic, str)]),
        },
        "securityPolicyUrl": None,
        "squashMergeAllowed": bool(repo.get("allow_squash_merge", False)),
        "sshUrl": repo.get("ssh_url"),
        "stargazerCount": _normalize_count(repo.get("stars_count")),
        "templateRepository": None,
        "updatedAt": repo.get("updated_at"),
        "url": repo.get("html_url") or getattr(fallback, "web_base_url"),
        "usesCustomOpenGraphImage": False,
        "viewerCanAdminister": bool(permissions.get("admin", False)),
        "viewerDefaultCommitEmail": None,
        "viewerDefaultMergeMethod": repo.get("default_merge_style"),
        "viewerHasStarred": False,
        "viewerPermission": _viewer_permission(permissions),
        "viewerPossibleCommitEmails": [],
        "viewerSubscription": None,
        "visibility": _visibility(is_private=is_private, is_internal=is_internal),
        "watchers": _connection(_normalize_count(repo.get("watchers_count"))),
    }


def filter_repo_fields(data: dict[str, Any], fields: tuple[str, ...]) -> dict[str, Any]:
    if not fields:
        return data
    return {field: data.get(field) for field in fields if field in SUPPORTED_REPO_JSON_FIELDS}


def normalize_issue(issue: dict[str, Any]) -> dict[str, Any]:
    user = issue.get("user") if isinstance(issue.get("user"), dict) else {}
    milestone = issue.get("milestone") if isinstance(issue.get("milestone"), dict) else None
    state = str(issue.get("state") or "").upper() or None
    closed = state == "CLOSED"
    return {
        "assignees": [_normalize_user(item) for item in _list_of_dicts(issue.get("assignees"))],
        "author": _normalize_user(user),
        "body": issue.get("body") or "",
        "closed": closed,
        "closedAt": issue.get("closed_at"),
        "closedByPullRequestsReferences": [],
        "comments": _normalize_count(issue.get("comments")),
        "createdAt": issue.get("created_at"),
        "id": issue.get("id"),
        "isPinned": _normalize_count(issue.get("pin_order")) > 0,
        "labels": [_normalize_label(item) for item in _list_of_dicts(issue.get("labels"))],
        "milestone": _normalize_milestone(milestone),
        "number": issue.get("number") or issue.get("id"),
        "projectCards": [],
        "projectItems": [],
        "reactionGroups": [],
        "state": state,
        "stateReason": None,
        "title": issue.get("title"),
        "updatedAt": issue.get("updated_at"),
        "url": issue.get("html_url") or issue.get("url"),
    }


def filter_issue_fields(data: dict[str, Any], fields: tuple[str, ...]) -> dict[str, Any]:
    if not fields:
        return data
    return {field: data.get(field) for field in fields if field in SUPPORTED_ISSUE_JSON_FIELDS}


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


def _connection(total_count: int, nodes: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    return {
        "nodes": [] if nodes is None else nodes,
        "totalCount": total_count,
    }


def _is_organization(owner: dict[str, Any]) -> bool:
    owner_type = str(owner.get("type") or "").lower()
    return owner_type in {"organization", "org"}


def _normalize_parent(parent: Any) -> dict[str, Any] | None:
    if not isinstance(parent, dict):
        return None
    owner = parent.get("owner") if isinstance(parent.get("owner"), dict) else {}
    name = parent.get("name")
    owner_login = owner.get("login") or owner.get("username")
    name_with_owner = parent.get("full_name")
    if not name_with_owner and owner_login and name:
        name_with_owner = f"{owner_login}/{name}"
    return {
        "name": name,
        "nameWithOwner": name_with_owner,
        "url": parent.get("html_url") or parent.get("url"),
    }


def _viewer_permission(permissions: dict[str, Any]) -> str | None:
    if permissions.get("admin"):
        return "ADMIN"
    if permissions.get("push"):
        return "WRITE"
    if permissions.get("pull"):
        return "READ"
    return None


def _visibility(*, is_private: bool, is_internal: bool) -> str:
    if is_internal:
        return "INTERNAL"
    return "PRIVATE" if is_private else "PUBLIC"


def _normalize_user(user: dict[str, Any]) -> dict[str, Any]:
    return {
        "login": user.get("login") or user.get("username"),
        "name": user.get("full_name") or user.get("name"),
    }


def _normalize_label(label: dict[str, Any]) -> dict[str, Any]:
    return {
        "color": label.get("color"),
        "description": label.get("description"),
        "id": label.get("id"),
        "name": label.get("name"),
    }


def _normalize_milestone(milestone: dict[str, Any] | None) -> dict[str, Any] | None:
    if milestone is None:
        return None
    return {
        "number": milestone.get("id") or milestone.get("number"),
        "title": milestone.get("title"),
        "state": str(milestone.get("state") or "").upper() or None,
    }


def _list_of_dicts(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, dict)]
