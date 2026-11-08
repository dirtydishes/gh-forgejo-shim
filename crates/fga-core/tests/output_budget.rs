//! A document under the complexity limit still can't build a response past the output budget:
//! the report's case, one 60,000-character issue body under many aliases.
use async_graphql::Request;
use axum::{Json, Router, routing::get};
use fga_core::forgejo::Forgejo;
use fga_core::graphql::{self, MAX_OUTPUT_BYTES};
use serde_json::{Value, json};

const BODY_LEN: usize = 60_000;

fn user() -> Value {
    json!({"id":1,"login":"o","full_name":"","avatar_url":""})
}

/// The response to `body` under `aliases` aliases, and its size as the gateway sends it.
async fn aliased_bodies(aliases: usize) -> (async_graphql::Response, usize) {
    let issue = json!({"id":1,"number":1,"title":"T","body":"A".repeat(BODY_LEN),"state":"open",
        "html_url":"https://forge.test/o/r/issues/1","user":user(),
        "created_at":"2026-11-08T00:00:00Z","updated_at":"2026-11-08T00:00:00Z","labels":[],
        "pull_request":null});
    let router = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r",
            get(|| async {
                Json(
                    json!({"id":1,"name":"r","full_name":"o/r","owner":user(),"private":false,
                    "html_url":"","default_branch":"main","fork":false,"archived":false}),
                )
            }),
        )
        .route(
            "/api/v1/repos/o/r/issues/1",
            get(move || async move { Json(issue) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let session = Forgejo::new(base)
        .unwrap()
        .authenticate("token disposable")
        .await
        .unwrap();
    let fields: Vec<String> = (0..aliases).map(|i| format!("a{i}:body")).collect();
    let query = format!(
        "{{repository(owner:\"o\",name:\"r\"){{issue(number:1){{{}}}}}}}",
        fields.join(" ")
    );
    let response = graphql::schema()
        .execute(Request::new(query).data(session))
        .await;
    task.abort();
    let size = serde_json::to_vec(&response).unwrap().len();
    (response, size)
}

#[tokio::test]
async fn aliased_bodies_stop_at_the_output_budget() {
    // 400 aliases is under the complexity limit, and would build 24 MB.
    let (response, size) = aliased_bodies(400).await;

    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    let error = serde_json::to_value(&response.errors[0]).unwrap();
    assert_eq!(error["extensions"]["type"], "RESOURCE_LIMITS_EXCEEDED");
    assert!(size <= MAX_OUTPUT_BYTES, "{size} bytes");
}

#[tokio::test]
async fn aliased_bodies_under_the_budget_are_whole() {
    // 6 MB.
    let (response, size) = aliased_bodies(100).await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let issue = data["repository"]["issue"].as_object().unwrap();
    assert_eq!(issue.len(), 100);
    assert!(
        issue
            .values()
            .all(|body| body.as_str().unwrap().len() == BODY_LEN)
    );
    assert!(size > 100 * BODY_LEN, "{size} bytes");
}
