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
use std::{collections::BTreeMap, sync::Arc};

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
    resource: &[&str],
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
        .extend(["api", "v3", "repos", owner, repo, "actions"])
        .extend(resource);
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

/// gh follows a run's `jobs_url`, and builds the artifacts path itself; `artifacts_url` is there for
/// GitHub's shape. Forgejo 15's API has neither per run, so both point under the gateway's API,
/// where they answer empty lists.
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
    list(&host, &session, &owner, &repo, raw, None).await
}

/// One workflow's runs, which `gh run list --workflow` reads once it has the workflow's ID.
pub async fn workflow_runs(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, selector)): Path<(String, String, String)>,
    RawQuery(raw): RawQuery,
) -> Response {
    list(&host, &session, &owner, &repo, raw, Some(&selector)).await
}

async fn list(
    host: &Host,
    session: &Session,
    owner: &str,
    repo: &str,
    raw: Option<String>,
    workflow: Option<&str>,
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
        let workflows = session.workflows(owner, repo).await?;
        let mut resource = vec!["runs"];
        if let Some(selector) = workflow {
            // Forgejo keys a run's workflow by its bare file name.
            let found = find_workflow(session, owner, repo, selector, &workflows).await?;
            let file = found.path.rsplit('/').next().unwrap_or_default();
            upstream.push(("workflow_id", file.to_owned()));
            resource = vec!["workflows", selector, "runs"];
        }
        let runs = session
            .action_runs(
                owner,
                repo,
                &upstream,
                (page - 1).saturating_mul(limit),
                limit,
            )
            .await?;
        // A run of a workflow that exists only on its own branch is named from there, so it never
        // fails the page.
        let named = futures_util::future::join_all(
            runs.workflow_runs
                .iter()
                .map(|run| session.workflow_for(owner, repo, run, &workflows)),
        )
        .await;
        let mut mapped = Vec::new();
        for (run, workflow) in runs.workflow_runs.iter().zip(&named) {
            mapped.push(with_links(host, owner, repo, actions::run(run, workflow)?));
        }
        Ok((
            links(
                host,
                owner,
                repo,
                &resource,
                (page, limit),
                runs.total_count,
                &public,
            ),
            Json(json!({"total_count":runs.total_count,"workflow_runs":mapped})),
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
            links(
                &host,
                &owner,
                &repo,
                &["workflows"],
                (page, limit),
                total,
                &[],
            ),
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
        let workflow = session.workflow_for(&owner, &repo, &run, &workflows).await;
        let mapped = with_links(&host, &owner, &repo, actions::run(&run, &workflow)?);
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
    Path((owner, repo, selector)): Path<(String, String, String)>,
) -> Response {
    let result = async {
        let workflows = session.workflows(&owner, &repo).await?;
        find_workflow(&session, &owner, &repo, &selector, &workflows).await
    }
    .await;
    match result {
        Ok(workflow) => Json(workflow).into_response(),
        Err(error) => rest_error(error),
    }
}

/// GitHub's `{workflow_id}`: a workflow's ID, or its file name for a workflow on the default branch.
async fn find_workflow(
    session: &Session,
    owner: &str,
    repo: &str,
    selector: &str,
    workflows: &BTreeMap<String, actions::Workflow>,
) -> Result<actions::Workflow, Error> {
    let Ok(id) = selector.parse::<i64>() else {
        return workflows.get(selector).cloned().ok_or(Error::NotFound);
    };
    if let Some(workflow) = workflows.values().find(|v| v.id == id) {
        return Ok(workflow.clone());
    }
    // gh's `run view` and `run watch` look up the workflow a run names (`GetRun` in
    // `pkg/cmd/run/shared/shared.go`) and stop on a 404. A workflow that exists only on a branch
    // isn't among the default branch's, but its ID is its file name's hash, so find a recent run of
    // it and name it as that run's list entry does.
    let recent = session.action_runs(owner, repo, &[], 0, 50).await?;
    let run = recent
        .workflow_runs
        .iter()
        .find(|run| actions::workflow_id(&run.workflow_id) == id)
        .ok_or(Error::NotFound)?;
    Ok(session.workflow_for(owner, repo, run, workflows).await)
}
