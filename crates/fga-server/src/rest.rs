use crate::{AppState, Host, rest_error};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use fga_core::{
    forgejo::{Error, Session, WriteError},
    map,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub async fn root(Extension(host): Extension<Arc<Host>>) -> impl IntoResponse {
    let public = host.public_url.as_str().trim_end_matches('/');
    (
        [
            ("x-oauth-scopes", "repo, read:org, workflow"),
            ("x-github-media-type", "github.v3; format=json"),
        ],
        Json(
            json!({"current_user_url": format!("{public}/api/v3/user"), "repository_url": format!("{public}/api/v3/repos/{{owner}}/{{repo}}") }),
        ),
    )
}

pub async fn meta(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({"installed_version": state.ghes_version}))
}

/// Forgejo has no rate-limit API and the gateway sets no limits of its own, so `rate_limit`
/// answers GitHub's overview with GitHub's per-user ceilings and nothing used: `core` and `graphql`
/// per hour, `search` and `code_search` per minute, each renewing when its window would.
/// A script that paces itself by `remaining` never waits.
pub async fn rate_limit() -> impl IntoResponse {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let budget = |limit: u32, window: u64| json!({"limit": limit, "used": 0, "remaining": limit, "reset": now + window});
    Json(json!({
        "resources": {
            "core": budget(5000, 3600),
            "graphql": budget(5000, 3600),
            "search": budget(30, 60),
            "code_search": budget(10, 60),
        },
        "rate": budget(5000, 3600),
    }))
}

pub async fn user(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
) -> impl IntoResponse {
    Json(map::user(&session.viewer, &host.public_url))
}

/// `gh pr merge --delete-branch` deletes the head branch through Forgejo. gh reads 404 and 422 as
/// "already deleted", so only a branch known to be absent gets GitHub's 422; a refusal (default
/// or protected branch, no write access, a 404 that doesn't name the branch) keeps its own code, a
/// name the gateway won't send is a 400, and anything but Forgejo's 204 is a 502.
pub async fn delete_branch(
    Extension(session): Extension<Session>,
    Path((owner, repo, branch)): Path<(String, String, String)>,
) -> Response {
    match session.delete_branch(&owner, &repo, &branch).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(WriteError::Transport(Error::NotFound)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"message": "Reference does not exist",
                "documentation_url": "https://docs.github.com/rest/git/refs#delete-a-reference"})),
        )
            .into_response(),
        Err(WriteError::Transport(error)) => rest_error(error),
        Err(error @ WriteError::UnsendableBranch) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": error.to_string()})),
        )
            .into_response(),
        Err(_) => rest_error(Error::Upstream),
    }
}

pub async fn repository(
    Extension(session): Extension<Session>,
    Path((owner, name)): Path<(String, String)>,
) -> Response {
    let result = async {
        let repo = session.repository(&owner, &name).await?;
        let owner_type = session.owner_type(&repo.owner.login).await?;
        Ok(Json(map::repository(&repo, owner_type)).into_response())
    }
    .await;
    result.unwrap_or_else(rest_error)
}

#[derive(Default, Deserialize)]
pub struct PullsQuery {
    head: Option<String>,
    state: Option<String>,
    per_page: Option<usize>,
    page: Option<usize>,
}

pub async fn pulls(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, name)): Path<(String, String)>,
    query: Result<Query<PullsQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return invalid_query();
    };
    let filter = query.state.as_deref().unwrap_or("open");
    let per_page = query.per_page.unwrap_or(30).clamp(1, 100);
    let page = query.page.unwrap_or(1);
    if !matches!(filter, "open" | "closed" | "all") || page == 0 {
        return invalid_query();
    }
    let pulls = match session
        .pull_requests(&owner, &name, filter, query.head.as_deref(), None)
        .await
    {
        Ok(pulls) => pulls,
        Err(error) => return rest_error(error),
    };
    let start = (page - 1).saturating_mul(per_page);
    let body: Vec<_> = pulls
        .iter()
        .skip(start)
        .take(per_page)
        .map(|pr| map::pull_request(pr, &host.wip_prefixes))
        .collect();
    let last = pulls.len().div_ceil(per_page).max(1);
    let mut headers = HeaderMap::new();
    if last > 1 {
        let mut url = host.public_url.clone();
        url.path_segments_mut()
            .expect("validated base URL")
            .pop_if_empty()
            .extend(["api", "v3", "repos", &owner, &name, "pulls"]);
        let mut links = Vec::new();
        for (rel, number) in [
            ("next", page.checked_add(1).filter(|_| page < last)),
            ("prev", page.checked_sub(1).filter(|n| *n > 0)),
            ("last", Some(last)),
            ("first", Some(1)),
        ] {
            if let Some(number) = number {
                url.set_query(None);
                url.query_pairs_mut()
                    .append_pair("state", filter)
                    .append_pair("per_page", &per_page.to_string())
                    .append_pair("page", &number.to_string());
                if let Some(head) = &query.head {
                    url.query_pairs_mut().append_pair("head", head);
                }
                links.push(format!("<{url}>; rel=\"{rel}\""));
            }
        }
        headers.insert(
            "link",
            links.join(", ").parse().expect("URL is a valid header"),
        );
    }
    (headers, Json(body)).into_response()
}

fn invalid_query() -> Response {
    (
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({"message": "Validation Failed"})),
    )
        .into_response()
}

// gh pr status reads this REST resource for its mergeability detail.
pub async fn pull(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, number)): Path<(String, String, i32)>,
    headers: HeaderMap,
) -> Response {
    if headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',').any(|v| {
                v.trim().starts_with("application/vnd.github.v3.diff")
                    || v.trim().starts_with("application/vnd.github.diff")
            })
        })
    {
        return match session
            .text(
                &["repos", &owner, &repo, "pulls", &format!("{number}.diff")],
                &[],
            )
            .await
        {
            Ok(diff) => ([("content-type", "text/plain; charset=utf-8")], diff).into_response(),
            Err(e) => rest_error(e),
        };
    }
    match session.pull_request(&owner, &repo, number).await {
        Ok(pr) => {
            let mut value = map::pull_request(&pr, &host.wip_prefixes);
            // MAPPING §2: Forgejo's false never proves a conflict, so REST reports null, not false.
            let mergeable = map::mergeable(&pr) == map::MergeableState::Mergeable;
            value["mergeable"] = json!(mergeable.then_some(true));
            Json(value).into_response()
        }
        Err(e) => rest_error(e),
    }
}

/// GitHub's combined status for a commit, from Forgejo's statuses through the same mapping as
/// GraphQL's `statusCheckRollup`. Pagination parameters are accepted; every status is returned.
pub async fn commit_status(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> Response {
    let result = async {
        let statuses = session.statuses(&owner, &repo, &sha).await?;
        let checks = map::checks(&statuses, &owner, &repo, &host.public_url);
        Ok(Json(map::combined_status(&checks, &sha)).into_response())
    }
    .await;
    result.unwrap_or_else(rest_error)
}

/// GitHub's check runs for a commit: Forgejo Actions jobs, from the same mapping.
pub async fn check_runs(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> Response {
    let result = async {
        let statuses = session.statuses(&owner, &repo, &sha).await?;
        let checks = map::checks(&statuses, &owner, &repo, &host.public_url);
        Ok(Json(map::check_runs(&checks, &sha)).into_response())
    }
    .await;
    result.unwrap_or_else(rest_error)
}

pub async fn commit_pulls(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> Response {
    let result = async {
        let mut pulls = session
            .pull_requests(&owner, &repo, "all", None, None)
            .await?;
        pulls.retain(|p| p.head.sha == sha);
        // Forgejo's singular route also knows merged commits. Preserve additional
        // open PRs sharing the head, which that route alone cannot represent.
        match session
            .get::<serde_json::Value>(&["repos", &owner, &repo, "commits", &sha, "pull"], &[])
            .await
        {
            Ok(value) => {
                let pr: fga_core::forgejo::PullRequest = serde_json::from_value(value)
                    .map_err(|_| fga_core::forgejo::Error::Upstream)?;
                if !pulls.iter().any(|p| p.number == pr.number) {
                    pulls.push(pr);
                }
            }
            Err(fga_core::forgejo::Error::NotFound) if pulls.is_empty() => {
                session
                    .get::<serde_json::Value>(
                        &["repos", &owner, &repo, "git", "commits", &sha],
                        &[],
                    )
                    .await?;
            }
            Err(fga_core::forgejo::Error::NotFound) => {}
            Err(e) => return Err(e),
        }
        pulls.sort_by_key(|p| p.number);
        Ok(Json(
            pulls
                .iter()
                .map(|p| map::pull_request(p, &host.wip_prefixes))
                .collect::<Vec<_>>(),
        )
        .into_response())
    }
    .await;
    result.unwrap_or_else(rest_error)
}

pub async fn readme(
    Extension(session): Extension<Session>,
    Path((owner, repo)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let entries = session
            .get::<serde_json::Value>(&["repos", &owner, &repo, "contents"], &[])
            .await?;
        let entries = entries
            .as_array()
            .ok_or(fga_core::forgejo::Error::Upstream)?;
        let entry = entries
            .iter()
            .find(|e| {
                e["type"] == "file"
                    && e["name"]
                        .as_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case("README.md"))
            })
            .or_else(|| {
                entries.iter().find(|e| {
                    e["type"] == "file"
                        && e["name"]
                            .as_str()
                            .is_some_and(|name| name.eq_ignore_ascii_case("README"))
                })
            })
            .ok_or(fga_core::forgejo::Error::NotFound)?;
        let path = entry["path"]
            .as_str()
            .ok_or(fga_core::forgejo::Error::Upstream)?;
        let value = session
            .get::<serde_json::Value>(&["repos", &owner, &repo, "contents", path], &[])
            .await?;
        if headers
            .get("accept")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| h.contains("raw"))
        {
            Ok((
                [("content-type", "text/plain; charset=utf-8")],
                session.file_text(&owner, &repo, path).await?,
            )
                .into_response())
        } else {
            Ok(Json(value).into_response())
        }
    }
    .await;
    result.unwrap_or_else(rest_error)
}
