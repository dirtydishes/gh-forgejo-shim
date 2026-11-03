use crate::{Host, rest_error};
use axum::{
    Extension, Json,
    extract::{Path, Query, RawQuery},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use fga_core::{
    actions,
    forgejo::{Error, Session},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunQuery {
    page: Option<usize>,
    per_page: Option<usize>,
    branch: Option<String>,
    event: Option<String>,
    status: Option<String>,
    head_sha: Option<String>,
    exclude_pull_requests: Option<bool>,
}
/// The query, with a repeated parameter taking its last value as GitHub does: Claude desktop sends
/// `per_page=100&per_page=100`, which a plain `Query` refuses as a duplicate field.
fn last_wins<T: serde::de::DeserializeOwned>(raw: Option<String>) -> Option<T> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        pairs.retain(|(seen, _)| *seen != key);
        pairs.push((key.into_owned(), value.into_owned()));
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(&pairs)
        .finish();
    let uri: axum::http::Uri = format!("/?{query}").parse().ok()?;
    Query::<T>::try_from_uri(&uri)
        .ok()
        .map(|Query(query)| query)
}
fn invalid() -> Response {
    (
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({"message":"Validation Failed"})),
    )
        .into_response()
}
fn links(
    host: &Host,
    owner: &str,
    repo: &str,
    resource: &str,
    pagination: (usize, usize),
    total: u64,
    query: &[(&str, String)],
) -> HeaderMap {
    let (page, limit) = pagination;
    let mut headers = HeaderMap::new();
    let mut url = host.public_url.clone();
    url.path_segments_mut()
        .expect("validated public URL")
        .pop_if_empty()
        .extend(["api", "v3", "repos", owner, repo, "actions", resource]);
    let mut links = Vec::new();
    let last = (total as usize).div_ceil(limit).max(1);
    for (rel, page) in [
        ("next", (page < last).then_some(page.saturating_add(1))),
        ("prev", page.checked_sub(1).filter(|p| *p > 0)),
    ] {
        if let Some(page) = page {
            url.set_query(None);
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())))
                .append_pair("page", &page.to_string())
                .append_pair("per_page", &limit.to_string());
            links.push(format!("<{url}>; rel=\"{rel}\""));
        }
    }
    if !links.is_empty() {
        headers.insert(
            "link",
            links.join(", ").parse().expect("URL is a valid header"),
        );
    }
    headers
}

/// gh follows a run's `jobs_url` and reads its artifacts. Forgejo 15's API has neither per run, so
/// both point under the gateway's API, where they answer empty lists.
fn with_links(host: &Host, owner: &str, repo: &str, mut run: Value) -> Value {
    let id = run["id"].to_string();
    for (field, resource) in [("jobs_url", "jobs"), ("artifacts_url", "artifacts")] {
        let mut url = host.public_url.clone();
        url.path_segments_mut()
            .expect("validated public URL")
            .pop_if_empty()
            .extend([
                "api", "v3", "repos", owner, repo, "actions", "runs", &id, resource,
            ]);
        run[field] = json!(url.as_str());
    }
    run
}

pub async fn runs(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
) -> Response {
    let Some(q) = last_wins::<RunQuery>(raw) else {
        return invalid();
    };
    let page = q.page.unwrap_or(1);
    let limit = q.per_page.unwrap_or(30).clamp(1, 100);
    if page == 0 {
        return invalid();
    }
    let mut upstream = Vec::new();
    let mut public = Vec::new();
    if let Some(branch) = q.branch {
        upstream.push(("ref", format!("refs/heads/{branch}")));
        public.push(("branch", branch));
    }
    for (key, value) in [("event", q.event), ("head_sha", q.head_sha)] {
        if let Some(value) = value {
            upstream.push((key, value.clone()));
            public.push((key, value));
        }
    }
    if let Some(status) = q.status {
        let native: &[&str] = match status.as_str() {
            "queued" => &["waiting", "blocked", "unknown"],
            "in_progress" => &["running"],
            "completed" => &["success", "failure", "cancelled", "skipped"],
            "success" => &["success"],
            "failure" => &["failure"],
            "cancelled" => &["cancelled"],
            "skipped" => &["skipped"],
            _ => return invalid(),
        };
        upstream.extend(native.iter().map(|value| ("status", (*value).to_owned())));
        public.push(("status", status));
    }
    if let Some(exclude) = q.exclude_pull_requests {
        public.push(("exclude_pull_requests", exclude.to_string()));
    }
    let result = async {
        let runs = session
            .action_runs(
                &owner,
                &repo,
                &upstream,
                (page - 1).saturating_mul(limit),
                limit,
            )
            .await?;
        let workflows = session.workflows(&owner, &repo).await?;
        let mapped: Result<Vec<Value>, Error> = runs
            .workflow_runs
            .iter()
            .map(|run| {
                let workflow = workflows.get(&run.workflow_id).ok_or(Error::NotFound)?;
                Ok(with_links(
                    &host,
                    &owner,
                    &repo,
                    actions::run(run, workflow)?,
                ))
            })
            .collect();
        Ok((
            links(
                &host,
                &owner,
                &repo,
                "runs",
                (page, limit),
                runs.total_count,
                &public,
            ),
            Json(json!({"total_count":runs.total_count,"workflow_runs":mapped?})),
        )
            .into_response())
    }
    .await;
    result.unwrap_or_else(rest_error)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowQuery {
    page: Option<usize>,
    per_page: Option<usize>,
}
pub async fn workflows(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo)): Path<(String, String)>,
    query: Result<Query<WorkflowQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(q)) = query else {
        return invalid();
    };
    let page = q.page.unwrap_or(1);
    let limit = q.per_page.unwrap_or(30).clamp(1, 100);
    if page == 0 {
        return invalid();
    }
    let result = async {
        let workflows = session.workflows(&owner, &repo).await?;
        let total = workflows.len() as u64;
        let workflows: Vec<_> = workflows
            .into_values()
            .skip((page - 1).saturating_mul(limit))
            .take(limit)
            .collect();
        Ok((
            links(&host, &owner, &repo, "workflows", (page, limit), total, &[]),
            Json(json!({"total_count":total,"workflows":workflows})),
        )
            .into_response())
    }
    .await;
    result.unwrap_or_else(rest_error)
}

async fn source(
    session: &Session,
    owner: &str,
    repo: &str,
    id: i64,
) -> Result<actions::Run, Error> {
    let value = session
        .get::<serde_json::Value>(
            &["repos", owner, repo, "actions", "runs", &id.to_string()],
            &[],
        )
        .await?;
    serde_json::from_value(value).map_err(|_| Error::Upstream)
}

pub async fn run(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> Response {
    let result = async {
        let run = source(&session, &owner, &repo, id).await?;
        let workflows = session.workflows(&owner, &repo).await?;
        let workflow = workflows.get(&run.workflow_id).ok_or(Error::NotFound)?;
        let mapped = with_links(&host, &owner, &repo, actions::run(&run, workflow)?);
        Ok(Json(mapped).into_response())
    }
    .await;
    result.unwrap_or_else(rest_error)
}

/// A run's jobs: none through Forgejo 15's API, for a run that exists.
pub async fn jobs(
    Extension(session): Extension<Session>,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> Response {
    match source(&session, &owner, &repo, id).await {
        Ok(_) => Json(json!({"total_count": 0, "jobs": []})).into_response(),
        Err(error) => rest_error(error),
    }
}

/// A run's artifacts: none through Forgejo 15's API, for a run that exists.
pub async fn artifacts(
    Extension(session): Extension<Session>,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> Response {
    match source(&session, &owner, &repo, id).await {
        Ok(_) => Json(json!({"total_count": 0, "artifacts": []})).into_response(),
        Err(error) => rest_error(error),
    }
}

pub async fn workflow(
    Extension(session): Extension<Session>,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> Response {
    let result = async {
        let workflows = session.workflows(&owner, &repo).await?;
        let workflow = workflows
            .into_values()
            .find(|v| v.id == id)
            .ok_or(Error::NotFound)?;
        Ok(Json(workflow).into_response())
    }
    .await;
    result.unwrap_or_else(rest_error)
}
