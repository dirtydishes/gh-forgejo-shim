use async_graphql::Request;
use axum::{Json, Router, extract::Query, response::IntoResponse, routing::get};
use fga_core::{forgejo::Forgejo, graphql};
use serde_json::{Value, json};
use std::collections::HashMap;

fn user() -> Value {
    json!({"id":1,"login":"viewer","full_name":"Viewer","avatar_url":"https://forge.test/avatar"})
}
fn label() -> Value {
    json!({"id":1,"name":"needs work","description":null,"color":"aa0000"})
}
fn issue(number: i32, title: &str) -> Value {
    json!({"id":number,"number":number,"title":title,"body":"needle in body","state":"closed","html_url":"https://forge.test/o/r/issues/2","user":user(),"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","labels":[label()]})
}
fn pull(number: i32, merged: bool) -> Value {
    json!({"number":number,"title":"needle","state":"closed","merged":merged,"draft":false,"html_url":"https://forge.test/o/r/pulls/2","head":{"label":"viewer:topic","ref":"topic","sha":"abc","repo":{"owner":user()}},"base":{"label":"main","ref":"main","sha":"def","repo":{"owner":user()}},"user":user(),"merged_at":null})
}
async fn session(upstream: Router) -> (fga_core::forgejo::Session, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    (
        Forgejo::new(url)
            .unwrap()
            .authenticate("token disposable")
            .await
            .unwrap(),
        task,
    )
}
#[tokio::test]
async fn applies_search_filters_across_upstream_pages_before_github_pagination() {
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r/issues",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                assert_eq!(q["type"], "pulls");
                assert_eq!(q["state"], "all");
                assert_eq!(q["created_by"], "viewer");
                assert_eq!(q["assigned_by"], "viewer");
                assert_eq!(q["labels"], "needs work");
                assert_eq!(q["q"], "+\"needle\"");
                assert_eq!(q["sort"], "latest");
                match q["page"].as_str() {
                    "1" => (
                        [("link", "<http://untrusted.invalid/steal>; rel=\"next\"")],
                        Json(json!([issue(4, "does not match title")])),
                    )
                        .into_response(),
                    "2" => Json(json!([issue(2, "needle in title")])).into_response(),
                    _ => panic!("unexpected page"),
                }
            }),
        )
        .route(
            "/api/v1/repos/o/r/pulls",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                assert_eq!(q["poster"], "viewer");
                Json(json!([pull(4, false), pull(2, true)]))
            }),
        );
    let (session, task) = session(upstream).await;
    let query = "repo:o/r type:pr author:@me assignee:@me label:\"needs work\" head:topic base:main ( needle in:title ) sort:created-desc";
    let request = format!(
        "{{ search(type:ISSUE_ADVANCED,query:{},first:1) {{ issueCount nodes {{ ... on PullRequest {{ number }} }} pageInfo {{ hasNextPage }} }} }}",
        serde_json::to_string(query).unwrap()
    );
    let response = graphql::schema()
        .execute(Request::new(request).data(session.clone()))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"search":{"issueCount":1,"nodes":[{"number":2}],"pageInfo":{"hasNextPage":false}}})
    );
    let response = graphql::schema()
        .execute(
            Request::new(
                "{search(type:ISSUE,query:\"repo:o/r type:pr unsupported:x\"){issueCount}}",
            )
            .data(session),
        )
        .await;
    assert_eq!(response.errors.len(), 1);
    assert_eq!(
        response.errors[0].extensions.as_ref().unwrap().get("type"),
        Some(&async_graphql::Value::from("UNPROCESSABLE"))
    );
    task.abort();
}

#[tokio::test]
async fn search_closed_includes_merged_while_repository_closed_excludes_them() {
    let upstream=Router::new().route("/api/v1/user",get(||async {Json(user())}))
        .route("/api/v1/repos/o/r",get(||async {Json(json!({"id":1,"name":"r","full_name":"o/r","owner":{"id":1,"login":"o","avatar_url":"https://forge.test/avatar"},"private":false,"html_url":"https://forge.test/o/r","default_branch":"main","fork":false,"archived":false}))}))
        .route("/api/v1/repos/o/r/issues",get(||async {Json(json!([issue(4,"needle"),issue(2,"needle")]))}))
        .route("/api/v1/repos/o/r/pulls",get(||async {Json(json!([pull(4,false),pull(2,true)]))}));
    let (session, task) = session(upstream).await;
    let response=graphql::schema().execute(Request::new("{search(type:ISSUE,query:\"repo:o/r type:pr state:closed\"){nodes{...on PullRequest{number}}} repository(owner:\"o\",name:\"r\"){pullRequests(states:[CLOSED],first:10){nodes{number}}}}").data(session)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"search":{"nodes":[{"number":4},{"number":2}]},"repository":{"pullRequests":{"nodes":[{"number":4}]}}})
    );
    task.abort();
}

/// PR web URLs, and URLs under a PR, take GitHub's `/pull/{n}` layout; other web URLs keep
/// Forgejo's (MAPPING §8).
#[tokio::test]
async fn pull_request_urls_use_githubs_layout() {
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route("/api/v1/repos/o/r", get(|| async { Json(json!({"id":1,"name":"r","full_name":"o/r","owner":{"id":1,"login":"o","avatar_url":"https://forge.test/avatar"},"private":false,"html_url":"https://forge.test/o/r","default_branch":"main","fork":false,"archived":false})) }))
        .route("/api/v1/repos/o/r/pulls/2", get(|| async { Json(pull(2, false)) }))
        .route(
            "/api/v1/repos/o/r/issues/2/comments",
            get(|| async {
                Json(json!([{"id":7,"user":user(),"body":"hi","created_at":"2026-01-01T00:00:00Z",
                    "html_url":"https://forge.test/o/r/pulls/2#issuecomment-7"}]))
            }),
        )
        // Forgejo's issue routes answer for pull requests too, with the PR's URL.
        .route(
            "/api/v1/repos/o/r/issues/2",
            get(|| async {
                let mut pr = issue(2, "needle");
                pr["html_url"] = json!("https://forge.test/o/r/pulls/2");
                Json(pr)
            }),
        )
        .route("/api/v1/repos/o/r/issues/4", get(|| async { Json(issue(4, "plain")) }));
    let (session, task) = session(upstream).await;
    let response = graphql::schema()
        .execute(
            Request::new(
                r#"{repository(owner:"o",name:"r"){url
                    pullRequest(number:2){url comments(first:1){nodes{url}}}
                    asIssue:issue(number:2){url} issue(number:4){url}}}"#,
            )
            .data(session),
        )
        .await;
    task.abort();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"repository":{"url":"https://forge.test/o/r",
            "pullRequest":{"url":"https://forge.test/o/r/pull/2",
                "comments":{"nodes":[{"url":"https://forge.test/o/r/pull/2#issuecomment-7"}]}},
            "asIssue":{"url":"https://forge.test/o/r/pull/2"},
            "issue":{"url":"https://forge.test/o/r/issues/2"}}})
    );
}

/// gh's `--json projectCards` selection on `pr list` and `issue list`. Forgejo has no classic
/// projects, so both answer an empty connection, as the built-in path's `[]` did.
#[tokio::test]
async fn project_cards_are_an_empty_connection() {
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r",
            get(|| async {
                Json(json!({"id":1,"name":"r","full_name":"o/r","private":false,
                    "owner":{"id":1,"login":"o","avatar_url":"https://forge.test/avatar"},
                    "html_url":"https://forge.test/o/r","default_branch":"main","fork":false,
                    "archived":false,"permissions":{"admin":false,"push":false,"pull":true}}))
            }),
        )
        .route(
            "/api/v1/repos/o/r/pulls/2",
            get(|| async { Json(pull(2, false)) }),
        )
        .route(
            "/api/v1/repos/o/r/issues/4",
            get(|| async { Json(issue(4, "plain")) }),
        );
    let (session, task) = session(upstream).await;
    let cards = "projectCards(first:100){nodes{project{name}column{name}},totalCount}";
    let response = graphql::schema()
        .execute(
            Request::new(format!(
                r#"{{repository(owner:"o",name:"r"){{
                    pullRequest(number:2){{{cards}}} issue(number:4){{{cards}}}}}}}"#
            ))
            .data(session),
        )
        .await;
    task.abort();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let empty = json!({"projectCards":{"nodes":[],"totalCount":0}});
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"repository":{"pullRequest":empty,"issue":empty}})
    );
}
