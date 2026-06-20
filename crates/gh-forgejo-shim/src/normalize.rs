use serde_json::{json, Map, Value};

use crate::forgejo::RepoRef;

pub const SUPPORTED_JSON_FIELDS: &[&str] = &[
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
];

pub const SUPPORTED_REPO_JSON_FIELDS: &[&str] = &[
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
];

pub const SUPPORTED_ISSUE_JSON_FIELDS: &[&str] = &[
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
];

pub const SUPPORTED_CHECK_JSON_FIELDS: &[&str] = &[
    "bucket",
    "completedAt",
    "conclusion",
    "description",
    "detailsUrl",
    "link",
    "name",
    "startedAt",
    "state",
    "workflow",
];

pub fn normalize_pull(pull: &Value) -> Value {
    let user = object_field(pull, "user");
    let head = object_field(pull, "head");
    let base = object_field(pull, "base");

    json!({
        "additions": normalize_count(pull.get("additions")),
        "deletions": normalize_count(pull.get("deletions")),
        "number": value_or(pull.get("number"), pull.get("id")),
        "title": cloned_or_null(pull.get("title")),
        "state": normalize_state(pull),
        "isDraft": truthy(pull.get("draft").unwrap_or(&Value::Bool(false))),
        "url": value_or(pull.get("html_url"), pull.get("url")),
        "headRefName": object_value(head, "ref"),
        "baseRefName": object_value(base, "ref"),
        "author": {
            "login": object_value_or(user, "login", "username"),
            "name": object_value_or(user, "full_name", "name"),
        },
        "createdAt": cloned_or_null(pull.get("created_at")),
        "updatedAt": cloned_or_null(pull.get("updated_at")),
        "mergeable": normalize_mergeable(pull),
        "mergeStateStatus": truthy_string(pull.get("merge_state_status"))
            .unwrap_or_else(|| "UNKNOWN".to_string())
            .to_uppercase(),
        "reviewDecision": cloned_or_null(pull.get("reviewDecision")),
        "reviewRequests": truthy_value_or(pull.get("reviewRequests"), json!([])),
        "statusCheckRollup": normalize_embedded_status_check_rollup(pull.get("statusCheckRollup")),
    })
}

pub fn with_status_check_rollup(pull: &Value, statuses: &[Value]) -> Value {
    let mut enriched = pull.clone();
    if let Some(object) = enriched.as_object_mut() {
        object.insert(
            "statusCheckRollup".to_string(),
            normalize_status_check_rollup(statuses),
        );
    }
    enriched
}

pub fn normalize_status_check_rollup(statuses: &[Value]) -> Value {
    Value::Array(
        latest_statuses(statuses)
            .into_iter()
            .map(|status| status_to_rollup_item(&status))
            .collect(),
    )
}

pub fn normalize_pr_checks(statuses: &[Value]) -> Value {
    Value::Array(
        latest_statuses(statuses)
            .into_iter()
            .map(|status| status_to_check_item(&status))
            .collect(),
    )
}

pub fn filter_fields<S: AsRef<str>>(data: &Value, fields: &[S]) -> Value {
    filter_supported_fields(data, fields, SUPPORTED_JSON_FIELDS)
}

pub fn filter_repo_fields<S: AsRef<str>>(data: &Value, fields: &[S]) -> Value {
    filter_supported_fields(data, fields, SUPPORTED_REPO_JSON_FIELDS)
}

pub fn filter_issue_fields<S: AsRef<str>>(data: &Value, fields: &[S]) -> Value {
    filter_supported_fields(data, fields, SUPPORTED_ISSUE_JSON_FIELDS)
}

pub fn filter_check_fields<S: AsRef<str>>(data: &Value, fields: &[S]) -> Value {
    filter_supported_fields(data, fields, SUPPORTED_CHECK_JSON_FIELDS)
}

pub fn normalize_repo(repo: &Value, fallback: &RepoRef) -> Value {
    let owner = object_field(repo, "owner");
    let owner_login = object_value_or(owner, "login", "username")
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| fallback.owner.clone());
    let name = truthy_string(repo.get("name")).unwrap_or_else(|| fallback.repo.clone());
    let default_branch = repo.get("default_branch").filter(|value| truthy(value));
    let language = repo
        .get("language")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let topics = repo
        .get("topics")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let permissions = object_field(repo, "permissions");
    let is_private = truthy(repo.get("private").unwrap_or(&Value::Bool(false)));
    let is_internal = truthy(repo.get("internal").unwrap_or(&Value::Bool(false)));
    let topic_nodes = topics
        .iter()
        .filter_map(Value::as_str)
        .map(|topic| json!({ "topic": { "name": topic } }))
        .collect::<Vec<_>>();

    json!({
        "archivedAt": cloned_or_null(repo.get("archived_at")),
        "assignableUsers": [],
        "codeOfConduct": null,
        "contactLinks": [],
        "createdAt": cloned_or_null(repo.get("created_at")),
        "description": cloned_or_null(repo.get("description")),
        "defaultBranchRef": default_branch
            .map(|value| json!({ "name": value.clone() }))
            .unwrap_or(Value::Null),
        "deleteBranchOnMerge": truthy(repo.get("default_delete_branch_after_merge").unwrap_or(&Value::Bool(false))),
        "diskUsage": normalize_count(repo.get("size")),
        "forkCount": normalize_count(repo.get("forks_count")),
        "fundingLinks": [],
        "hasDiscussionsEnabled": false,
        "hasIssuesEnabled": truthy(repo.get("has_issues").unwrap_or(&Value::Bool(false))),
        "hasProjectsEnabled": truthy(repo.get("has_projects").unwrap_or(&Value::Bool(false))),
        "hasWikiEnabled": truthy(repo.get("has_wiki").unwrap_or(&Value::Bool(false))),
        "homepageUrl": truthy_value_or(repo.get("website"), Value::Null),
        "id": cloned_or_null(repo.get("id")),
        "isArchived": truthy(repo.get("archived").unwrap_or(&Value::Bool(false))),
        "isBlankIssuesEnabled": truthy(repo.get("has_issues").unwrap_or(&Value::Bool(false))),
        "isEmpty": truthy(repo.get("empty").unwrap_or(&Value::Bool(false))),
        "isFork": truthy(repo.get("fork").unwrap_or(&Value::Bool(false))),
        "isInOrganization": is_organization(owner),
        "isMirror": truthy(repo.get("mirror").unwrap_or(&Value::Bool(false))),
        "isPrivate": is_private,
        "isSecurityPolicyEnabled": false,
        "isTemplate": truthy(repo.get("template").unwrap_or(&Value::Bool(false))),
        "isUserConfigurationRepository": false,
        "issueTemplates": [],
        "issues": connection(normalize_count(repo.get("open_issues_count")), None),
        "labels": connection(0, None),
        "languages": {
            "nodes": language.map(|name| vec![json!({ "name": name })]).unwrap_or_default(),
            "totalSize": 0,
        },
        "latestRelease": null,
        "licenseInfo": null,
        "mentionableUsers": [],
        "mergeCommitAllowed": truthy(repo.get("allow_merge_commits").unwrap_or(&Value::Bool(false))),
        "milestones": connection(0, None),
        "mirrorUrl": truthy_value_or(repo.get("original_url"), Value::Null),
        "name": name,
        "nameWithOwner": truthy_string(repo.get("full_name")).unwrap_or_else(|| format!("{owner_login}/{name}")),
        "openGraphImageUrl": cloned_or_null(repo.get("avatar_url")),
        "owner": {
            "login": owner_login,
            "type": object_value(owner, "type"),
        },
        "parent": normalize_parent(repo.get("parent")),
        "primaryLanguage": language.map(|name| json!({ "name": name })).unwrap_or(Value::Null),
        "projects": connection(0, None),
        "projectsV2": connection(0, None),
        "pullRequestTemplates": [],
        "pullRequests": connection(normalize_count(repo.get("open_pr_counter")), None),
        "pushedAt": cloned_or_null(repo.get("updated_at")),
        "rebaseMergeAllowed": truthy(repo.get("allow_rebase").unwrap_or(&Value::Bool(false))),
        "repositoryTopics": {
            "nodes": topic_nodes,
            "totalCount": topics.iter().filter(|topic| topic.as_str().is_some()).count(),
        },
        "securityPolicyUrl": null,
        "squashMergeAllowed": truthy(repo.get("allow_squash_merge").unwrap_or(&Value::Bool(false))),
        "sshUrl": cloned_or_null(repo.get("ssh_url")),
        "stargazerCount": normalize_count(repo.get("stars_count")),
        "templateRepository": null,
        "updatedAt": cloned_or_null(repo.get("updated_at")),
        "url": truthy_value_or(repo.get("html_url"), Value::String(fallback.web_base_url())),
        "usesCustomOpenGraphImage": false,
        "viewerCanAdminister": permission_truthy(permissions, "admin"),
        "viewerDefaultCommitEmail": null,
        "viewerDefaultMergeMethod": cloned_or_null(repo.get("default_merge_style")),
        "viewerHasStarred": false,
        "viewerPermission": viewer_permission(permissions).map(Value::String).unwrap_or(Value::Null),
        "viewerPossibleCommitEmails": [],
        "viewerSubscription": null,
        "visibility": visibility(is_private, is_internal),
        "watchers": connection(normalize_count(repo.get("watchers_count")), None),
    })
}

pub fn normalize_issue(issue: &Value) -> Value {
    let user = object_field(issue, "user");
    let milestone = issue.get("milestone").filter(|value| value.is_object());
    let state = truthy_string(issue.get("state"))
        .map(|value| value.to_uppercase())
        .filter(|value| !value.is_empty());
    let closed = state.as_deref() == Some("CLOSED");

    json!({
        "assignees": list_of_objects(issue.get("assignees"))
            .into_iter()
            .map(|item| normalize_user(item.as_object()))
            .collect::<Vec<_>>(),
        "author": normalize_user(user),
        "body": truthy_value_or(issue.get("body"), Value::String(String::new())),
        "closed": closed,
        "closedAt": cloned_or_null(issue.get("closed_at")),
        "closedByPullRequestsReferences": [],
        "comments": normalize_count(issue.get("comments")),
        "createdAt": cloned_or_null(issue.get("created_at")),
        "id": cloned_or_null(issue.get("id")),
        "isPinned": normalize_count(issue.get("pin_order")) > 0,
        "labels": list_of_objects(issue.get("labels"))
            .into_iter()
            .map(|item| normalize_label(item.as_object()))
            .collect::<Vec<_>>(),
        "milestone": normalize_milestone(milestone),
        "number": value_or(issue.get("number"), issue.get("id")),
        "projectCards": [],
        "projectItems": [],
        "reactionGroups": [],
        "state": state.map(Value::String).unwrap_or(Value::Null),
        "stateReason": null,
        "title": cloned_or_null(issue.get("title")),
        "updatedAt": cloned_or_null(issue.get("updated_at")),
        "url": value_or(issue.get("html_url"), issue.get("url")),
    })
}

pub fn status_for_current_branch<S: AsRef<str>>(pull: Option<&Value>, fields: &[S]) -> Value {
    let normalized = pull.map(|value| filter_fields(&normalize_pull(value), fields));
    json!({
        "currentBranch": normalized.unwrap_or(Value::Null),
        "createdBy": [],
        "needsReview": [],
    })
}

pub fn apply_simple_jq(data: &Value, expression: &str) -> Option<Value> {
    let mut query = expression.trim();
    if query == "." || query.is_empty() {
        return none_if_null(data.clone());
    }
    if query.ends_with("// empty") {
        query = query[..query.len() - "// empty".len()].trim();
    }
    if !query.starts_with('.') {
        return none_if_null(data.clone());
    }

    let mut value = data.clone();
    for part in query[1..].split('.') {
        if part.is_empty() {
            continue;
        }
        if let Value::Array(items) = &value {
            if let Some(key) = part.strip_suffix("[]") {
                value = Value::Array(
                    items
                        .iter()
                        .filter_map(|item| {
                            item.as_object()
                                .map(|object| object.get(key).cloned().unwrap_or(Value::Null))
                        })
                        .collect(),
                );
            } else {
                return None;
            }
        } else if let Value::Object(object) = &value {
            value = object.get(part).cloned().unwrap_or(Value::Null);
        } else {
            return None;
        }
    }
    none_if_null(value)
}

pub fn render_simple_template(data: &Value, template: &str) -> String {
    let mut rendered = String::new();
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            rendered.push_str(&rest[start..]);
            return rendered;
        };
        let token = &after_start[..end];
        let replacement = template_replacement(data, token);
        if let Some(replacement) = replacement {
            rendered.push_str(&replacement);
        } else {
            rendered.push_str(&rest[start..start + 2 + end + 2]);
        }
        rest = &after_start[end + 2..];
    }

    rendered.push_str(rest);
    rendered
}

pub fn render_json_or_jq(data: &Value, jq: Option<&str>, template: Option<&str>) -> Option<String> {
    let value = if let Some(expression) = jq {
        apply_simple_jq(data, expression)?
    } else {
        data.clone()
    };

    if let Some(template) = template {
        return Some(render_simple_template(&value, template));
    }

    if let Some(text) = value.as_str() {
        Some(format!("{text}\n"))
    } else {
        Some(format!("{}\n", python_json_string(&value)))
    }
}

pub fn render_json_or_jq_list(
    data: &[Value],
    jq: Option<&str>,
    template: Option<&str>,
) -> Option<String> {
    render_json_or_jq(&Value::Array(data.to_vec()), jq, template)
}

fn filter_supported_fields<S: AsRef<str>>(data: &Value, fields: &[S], supported: &[&str]) -> Value {
    if fields.is_empty() {
        return data.clone();
    }

    let mut filtered = Map::new();
    for field in fields {
        let field = field.as_ref();
        if supported.contains(&field) {
            filtered.insert(
                field.to_string(),
                data.get(field).cloned().unwrap_or(Value::Null),
            );
        }
    }
    Value::Object(filtered)
}

fn normalize_count(value: Option<&Value>) -> i64 {
    let Some(value) = value else {
        return 0;
    };
    if value.is_boolean() {
        return 0;
    }
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .unwrap_or_default()
}

fn normalize_state(pull: &Value) -> Value {
    if pull.get("merged").is_some_and(truthy) || pull.get("merged_at").is_some_and(truthy) {
        return Value::String("MERGED".to_string());
    }
    pull.get("state")
        .filter(|value| !value.is_null())
        .map(|value| Value::String(value_to_python_string(value).to_uppercase()))
        .unwrap_or(Value::Null)
}

fn normalize_mergeable(pull: &Value) -> String {
    if let Some(raw) = pull.get("mergeable").and_then(Value::as_str) {
        let value = raw.to_uppercase();
        if matches!(value.as_str(), "MERGEABLE" | "CONFLICTING" | "UNKNOWN") {
            return value;
        }
    }
    if pull.get("mergeable").and_then(Value::as_bool) == Some(true) {
        return "MERGEABLE".to_string();
    }
    let merge_state = truthy_string(pull.get("merge_state_status"))
        .unwrap_or_default()
        .to_uppercase();
    if matches!(merge_state.as_str(), "DIRTY" | "CONFLICTING" | "CONFLICT") {
        return "CONFLICTING".to_string();
    }
    "UNKNOWN".to_string()
}

fn normalize_embedded_status_check_rollup(value: Option<&Value>) -> Value {
    match value.and_then(Value::as_array) {
        Some(items) => Value::Array(
            items
                .iter()
                .filter(|item| item.is_object())
                .cloned()
                .collect(),
        ),
        None => json!([]),
    }
}

fn latest_statuses(statuses: &[Value]) -> Vec<Value> {
    let mut latest = std::collections::BTreeMap::<String, Value>::new();
    for status in statuses.iter().filter(|status| status.is_object()) {
        let key = status_name(status);
        let replace = latest
            .get(&key)
            .map(|existing| status_timestamp(status) >= status_timestamp(existing))
            .unwrap_or(true);
        if replace {
            latest.insert(key, status.clone());
        }
    }
    latest.into_values().collect()
}

fn status_to_rollup_item(status: &Value) -> Value {
    let state = status_state(status);
    let target_url = status_url(status);
    json!({
        "__typename": "StatusContext",
        "completedAt": status_completed_at(status, &state),
        "conclusion": status_conclusion(&state),
        "context": status_name(status),
        "description": cloned_or_null(status.get("description")),
        "detailsUrl": target_url.clone(),
        "name": status_name(status),
        "startedAt": cloned_or_null(status.get("created_at")),
        "state": state,
        "targetUrl": target_url,
        "workflowName": status_workflow(status),
    })
}

fn status_to_check_item(status: &Value) -> Value {
    let state = status_state(status);
    let target_url = status_url(status);
    json!({
        "bucket": status_bucket(&state),
        "completedAt": status_completed_at(status, &state),
        "conclusion": status_conclusion(&state),
        "description": cloned_or_null(status.get("description")),
        "detailsUrl": target_url.clone(),
        "link": target_url.clone(),
        "name": status_name(status),
        "startedAt": cloned_or_null(status.get("created_at")),
        "state": check_state(&state),
        "workflow": status_workflow(status),
    })
}

fn status_name(status: &Value) -> String {
    truthy_string(status.get("context"))
        .or_else(|| truthy_string(status.get("name")))
        .or_else(|| truthy_string(status.get("title")))
        .unwrap_or_else(|| "status".to_string())
}

fn status_workflow(status: &Value) -> String {
    truthy_string(status.get("workflow"))
        .or_else(|| truthy_string(status.get("workflow_name")))
        .or_else(|| truthy_string(status.get("context")))
        .unwrap_or_else(|| status_name(status))
}

fn status_url(status: &Value) -> Value {
    status
        .get("target_url")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            status
                .get("url")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            status
                .get("html_url")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        })
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null)
}

fn status_timestamp(status: &Value) -> String {
    status
        .get("updated_at")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            status
                .get("created_at")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_default()
        .to_string()
}

fn status_state(status: &Value) -> String {
    let raw = truthy_string(status.get("state"))
        .or_else(|| truthy_string(status.get("status")))
        .unwrap_or_default()
        .to_lowercase();
    match raw.as_str() {
        "success" | "successful" | "ok" | "pass" | "passed" => "SUCCESS".to_string(),
        "failure" | "failed" => "FAILURE".to_string(),
        "error" | "cancelled" | "canceled" | "warning" => "ERROR".to_string(),
        _ => "PENDING".to_string(),
    }
}

fn status_conclusion(state: &str) -> Value {
    match state {
        "SUCCESS" => Value::String("success".to_string()),
        "FAILURE" | "ERROR" => Value::String("failure".to_string()),
        _ => Value::Null,
    }
}

fn status_bucket(state: &str) -> &'static str {
    match state {
        "SUCCESS" => "pass",
        "FAILURE" | "ERROR" => "fail",
        _ => "pending",
    }
}

fn check_state(state: &str) -> &'static str {
    match state {
        "SUCCESS" | "FAILURE" | "ERROR" => "completed",
        _ => "pending",
    }
}

fn status_completed_at(status: &Value, state: &str) -> Value {
    if !matches!(state, "SUCCESS" | "FAILURE" | "ERROR") {
        return Value::Null;
    }
    status
        .get("updated_at")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            status
                .get("created_at")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        })
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null)
}

fn is_organization(owner: Option<&Map<String, Value>>) -> bool {
    let owner_type = owner
        .and_then(|owner| owner.get("type"))
        .map(value_to_python_string)
        .unwrap_or_default()
        .to_lowercase();
    matches!(owner_type.as_str(), "organization" | "org")
}

fn normalize_parent(parent: Option<&Value>) -> Value {
    let Some(parent) = parent.filter(|value| value.is_object()) else {
        return Value::Null;
    };
    let owner = object_field(parent, "owner");
    let name = parent.get("name");
    let owner_login = object_value_or(owner, "login", "username");
    let name_with_owner = truthy_string(parent.get("full_name")).or_else(|| {
        let owner_login = owner_login.as_str()?;
        let name = truthy_string(name)?;
        Some(format!("{owner_login}/{name}"))
    });

    json!({
        "name": cloned_or_null(name),
        "nameWithOwner": name_with_owner.map(Value::String).unwrap_or(Value::Null),
        "url": value_or(parent.get("html_url"), parent.get("url")),
    })
}

fn viewer_permission(permissions: Option<&Map<String, Value>>) -> Option<String> {
    if permission_truthy(permissions, "admin") {
        return Some("ADMIN".to_string());
    }
    if permission_truthy(permissions, "push") {
        return Some("WRITE".to_string());
    }
    if permission_truthy(permissions, "pull") {
        return Some("READ".to_string());
    }
    None
}

fn visibility(is_private: bool, is_internal: bool) -> &'static str {
    if is_internal {
        "INTERNAL"
    } else if is_private {
        "PRIVATE"
    } else {
        "PUBLIC"
    }
}

fn normalize_user(user: Option<&Map<String, Value>>) -> Value {
    json!({
        "login": object_value_or(user, "login", "username"),
        "name": object_value_or(user, "full_name", "name"),
    })
}

fn normalize_label(label: Option<&Map<String, Value>>) -> Value {
    json!({
        "color": object_value(label, "color"),
        "description": object_value(label, "description"),
        "id": object_value(label, "id"),
        "name": object_value(label, "name"),
    })
}

fn normalize_milestone(milestone: Option<&Value>) -> Value {
    let Some(milestone) = milestone else {
        return Value::Null;
    };
    json!({
        "number": value_or(milestone.get("id"), milestone.get("number")),
        "title": cloned_or_null(milestone.get("title")),
        "state": truthy_string(milestone.get("state"))
            .map(|value| Value::String(value.to_uppercase()))
            .unwrap_or(Value::Null),
    })
}

fn list_of_objects(value: Option<&Value>) -> Vec<&Value> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter(|item| item.is_object()).collect())
        .unwrap_or_default()
}

fn object_field<'a>(value: &'a Value, key: &str) -> Option<&'a Map<String, Value>> {
    value.get(key).and_then(Value::as_object)
}

fn object_value(object: Option<&Map<String, Value>>, key: &str) -> Value {
    object
        .and_then(|object| object.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

fn object_value_or(object: Option<&Map<String, Value>>, primary: &str, fallback: &str) -> Value {
    object
        .and_then(|object| object.get(primary))
        .filter(|value| truthy(value))
        .cloned()
        .or_else(|| {
            object
                .and_then(|object| object.get(fallback))
                .filter(|value| truthy(value))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

fn permission_truthy(permissions: Option<&Map<String, Value>>, key: &str) -> bool {
    permissions
        .and_then(|permissions| permissions.get(key))
        .is_some_and(truthy)
}

fn connection(total_count: i64, nodes: Option<Vec<Value>>) -> Value {
    json!({
        "nodes": nodes.unwrap_or_default(),
        "totalCount": total_count,
    })
}

fn value_or(primary: Option<&Value>, fallback: Option<&Value>) -> Value {
    primary
        .filter(|value| truthy(value))
        .cloned()
        .or_else(|| fallback.filter(|value| truthy(value)).cloned())
        .unwrap_or(Value::Null)
}

fn truthy_value_or(value: Option<&Value>, fallback: Value) -> Value {
    value
        .filter(|value| truthy(value))
        .cloned()
        .unwrap_or(fallback)
}

fn cloned_or_null(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or(Value::Null)
}

fn truthy_string(value: Option<&Value>) -> Option<String> {
    let value = value?;
    truthy(value).then(|| value_to_python_string(value))
}

fn value_to_python_string(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Array(_) | Value::Object(_) => python_json_string(value),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => {
            number.as_i64().is_some_and(|value| value != 0)
                || number.as_u64().is_some_and(|value| value != 0)
                || number.as_f64().is_some_and(|value| value != 0.0)
        }
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn none_if_null(value: Value) -> Option<Value> {
    (!value.is_null()).then_some(value)
}

fn template_replacement(data: &Value, token: &str) -> Option<String> {
    let token = token.trim();
    let query = token.strip_prefix('.')?;
    if !query.is_empty() && !valid_template_query(query) {
        return None;
    }
    let value = if query.is_empty() {
        Some(data)
    } else {
        lookup_dotted(data, query)
    };
    Some(template_value_to_string(value))
}

fn valid_template_query(query: &str) -> bool {
    query
        .split('.')
        .all(|part| !part.is_empty() && valid_identifier(part))
}

fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphanumeric() || first == '_')
        && chars.all(|char| char.is_ascii_alphanumeric() || char == '_' || char == '.')
}

fn lookup_dotted<'a>(data: &'a Value, query: &str) -> Option<&'a Value> {
    let mut value = data;
    for part in query.split('.') {
        if part.is_empty() {
            continue;
        }
        let object = value.as_object()?;
        value = object.get(part)?;
    }
    Some(value)
}

fn template_value_to_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(true)) => "true".to_string(),
        Some(Value::Bool(false)) => "false".to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(value @ (Value::Array(_) | Value::Object(_))) => python_json_string(value),
    }
}

fn python_json_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(_) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string()),
        Value::Array(items) => {
            let items = items
                .iter()
                .map(python_json_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{items}]")
        }
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            let entries = keys
                .into_iter()
                .filter_map(|key| {
                    let key_json = serde_json::to_string(key).ok()?;
                    let value = object.get(key)?;
                    Some(format!("{key_json}: {}", python_json_string(value)))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{entries}}}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_forgejo_pull_to_github_shape() {
        let pull = json!({
            "number": 12,
            "title": "Add shim",
            "state": "open",
            "draft": true,
            "html_url": "https://git.example.com/o/r/pulls/12",
            "head": {"ref": "feature"},
            "base": {"ref": "main"},
            "user": {"login": "alice", "full_name": "Alice"},
            "created_at": "2026-05-31T00:00:00Z",
            "updated_at": "2026-05-31T01:00:00Z",
            "mergeable": true,
            "merge_state_status": "CLEAN"
        });

        let normalized = normalize_pull(&pull);

        assert_eq!(normalized["number"], 12);
        assert_eq!(normalized["state"], "OPEN");
        assert_eq!(normalized["isDraft"], true);
        assert_eq!(normalized["headRefName"], "feature");
        assert_eq!(normalized["baseRefName"], "main");
        assert_eq!(normalized["author"]["login"], "alice");
        assert_eq!(normalized["additions"], 0);
        assert_eq!(normalized["deletions"], 0);
        assert_eq!(normalized["mergeable"], "MERGEABLE");
        assert_eq!(normalized["mergeStateStatus"], "CLEAN");
        assert_eq!(normalized["statusCheckRollup"], json!([]));
    }

    #[test]
    fn normalizes_merged_and_unknown_mergeable_values() {
        let normalized = normalize_pull(&json!({
            "number": 12,
            "state": "closed",
            "merged_at": "2026-05-31T02:00:00Z",
            "mergeable": false
        }));

        assert_eq!(normalized["state"], "MERGED");
        assert_eq!(normalized["mergeable"], "UNKNOWN");
    }

    #[test]
    fn filters_supported_json_fields() {
        let data = json!({"number": 1, "title": "T", "unknown": "x"});

        assert_eq!(
            filter_fields(&data, &["number", "unknown"]),
            json!({"number": 1})
        );
    }

    #[test]
    fn status_for_current_branch_wraps_pull_like_gh_status() {
        let status = status_for_current_branch(
            Some(&json!({
                "number": 12,
                "title": "Add shim",
                "state": "open",
                "html_url": "https://git.example.com/o/r/pulls/12",
                "head": {"ref": "feature"}
            })),
            &["number", "title", "url"],
        );

        assert_eq!(
            status,
            json!({
                "currentBranch": {
                    "number": 12,
                    "title": "Add shim",
                    "url": "https://git.example.com/o/r/pulls/12"
                },
                "createdBy": [],
                "needsReview": []
            })
        );
    }

    #[test]
    fn normalizes_forgejo_commit_statuses_to_check_rows() {
        let statuses = vec![
            json!({
                "context": "ci/test",
                "description": "old result",
                "state": "failure",
                "created_at": "2026-06-01T00:00:00Z",
                "updated_at": "2026-06-01T00:01:00Z"
            }),
            json!({
                "context": "ci/test",
                "description": "new result",
                "state": "success",
                "target_url": "https://git.example.com/owner/repo/actions/runs/2",
                "created_at": "2026-06-01T00:02:00Z",
                "updated_at": "2026-06-01T00:03:00Z"
            }),
        ];

        let checks = normalize_pr_checks(&statuses);

        assert_eq!(
            checks,
            json!([
                {
                    "bucket": "pass",
                    "completedAt": "2026-06-01T00:03:00Z",
                    "conclusion": "success",
                    "description": "new result",
                    "detailsUrl": "https://git.example.com/owner/repo/actions/runs/2",
                    "link": "https://git.example.com/owner/repo/actions/runs/2",
                    "name": "ci/test",
                    "startedAt": "2026-06-01T00:02:00Z",
                    "state": "completed",
                    "workflow": "ci/test"
                }
            ])
        );
    }

    #[test]
    fn normalizes_forgejo_commit_statuses_to_rollup_items() {
        let rollup = normalize_status_check_rollup(&[json!({
            "context": "deploy",
            "description": "waiting",
            "state": "pending",
            "created_at": "2026-06-01T00:00:00Z"
        })]);

        assert_eq!(
            rollup,
            json!([
                {
                    "__typename": "StatusContext",
                    "completedAt": null,
                    "conclusion": null,
                    "context": "deploy",
                    "description": "waiting",
                    "detailsUrl": null,
                    "name": "deploy",
                    "startedAt": "2026-06-01T00:00:00Z",
                    "state": "PENDING",
                    "targetUrl": null,
                    "workflowName": "deploy"
                }
            ])
        );
    }

    #[test]
    fn normalizes_repository_rich_fields() {
        let repo = json!({
            "id": 99,
            "name": "repo",
            "full_name": "owner/repo",
            "html_url": "https://git.example.com/owner/repo",
            "ssh_url": "git@git.example.com:owner/repo.git",
            "default_branch": "main",
            "private": false,
            "has_issues": true,
            "has_projects": true,
            "has_wiki": false,
            "allow_merge_commits": true,
            "allow_rebase": true,
            "allow_squash_merge": true,
            "forks_count": 2,
            "stars_count": 3,
            "watchers_count": 4,
            "open_issues_count": 5,
            "open_pr_counter": 6,
            "language": "Python",
            "permissions": {"pull": true, "push": true, "admin": false},
            "owner": {"login": "owner"}
        });

        let normalized = normalize_repo(&repo, &RepoRef::new("git.example.com", "owner", "repo"));
        let fields = filter_repo_fields(
            &normalized,
            &[
                "id",
                "hasIssuesEnabled",
                "hasProjectsEnabled",
                "hasWikiEnabled",
                "forkCount",
                "stargazerCount",
                "watchers",
                "primaryLanguage",
                "viewerPermission",
                "visibility",
                "pullRequests",
                "issues",
                "mergeCommitAllowed",
                "rebaseMergeAllowed",
                "squashMergeAllowed",
            ],
        );

        assert_eq!(
            fields,
            json!({
                "forkCount": 2,
                "hasIssuesEnabled": true,
                "hasProjectsEnabled": true,
                "hasWikiEnabled": false,
                "id": 99,
                "issues": {"nodes": [], "totalCount": 5},
                "mergeCommitAllowed": true,
                "primaryLanguage": {"name": "Python"},
                "pullRequests": {"nodes": [], "totalCount": 6},
                "rebaseMergeAllowed": true,
                "squashMergeAllowed": true,
                "stargazerCount": 3,
                "viewerPermission": "WRITE",
                "visibility": "PUBLIC",
                "watchers": {"nodes": [], "totalCount": 4}
            })
        );
    }

    #[test]
    fn normalizes_issue_to_github_shape() {
        let issue = json!({
            "number": 13,
            "title": "Fix it",
            "body": "Details",
            "state": "closed",
            "html_url": "https://git.example.com/owner/repo/issues/13",
            "user": {"login": "alice"},
            "labels": [{"id": 2, "name": "bug", "color": "ee0000"}],
            "comments": 2
        });

        let normalized = normalize_issue(&issue);

        assert_eq!(
            filter_issue_fields(&normalized, &["number", "title", "body", "state", "closed"]),
            json!({
                "body": "Details",
                "closed": true,
                "number": 13,
                "state": "CLOSED",
                "title": "Fix it"
            })
        );
    }

    #[test]
    fn applies_small_jq_subset() {
        let data = json!({
            "currentBranch": {
                "url": "https://git.example.com/owner/repo/pulls/12"
            }
        });

        assert_eq!(
            apply_simple_jq(&data, ".currentBranch.url"),
            Some(json!("https://git.example.com/owner/repo/pulls/12"))
        );
        assert_eq!(apply_simple_jq(&data, ".missing // empty"), None);
    }

    #[test]
    fn renders_template_fields_and_json_values() {
        let data = json!({
            "url": "https://git.example.com/owner/repo/pulls/12",
            "currentBranch": {"number": 12},
            "ok": true
        });

        assert_eq!(
            render_simple_template(&data, "{{.url}}"),
            "https://git.example.com/owner/repo/pulls/12"
        );
        assert_eq!(
            render_simple_template(&data, "{{.currentBranch.number}} {{.ok}}"),
            "12 true"
        );
        assert_eq!(
            render_json_or_jq(&data, Some(".url"), Some("{{.}}")),
            Some("https://git.example.com/owner/repo/pulls/12".to_string())
        );
    }

    #[test]
    fn renders_python_style_sorted_json_output() {
        let data = json!({"b": 2, "a": [true, null]});

        assert_eq!(
            render_json_or_jq(&data, None, None),
            Some("{\"a\": [true, null], \"b\": 2}\n".to_string())
        );
    }
}
