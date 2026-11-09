//! A field that fails on the gateway's side names itself by the schema's type and field, for
//! fga-server's usage counts; an ordinary answer such as Not Found names nothing.
use async_graphql::Request;
use axum::{Json, Router, http::StatusCode, routing::get};
use fga_core::forgejo::Forgejo;
use fga_core::graphql::{self, FAILED_FIELD};
use serde_json::{Value, json};

fn user() -> Value {
    json!({"id":1,"login":"o","full_name":"","avatar_url":""})
}

#[tokio::test]
async fn a_failed_field_is_named_by_the_schema_never_by_its_alias() {
    let issue = json!({"id":3,"number":3,"title":"T","body":"","state":"open",
        "html_url":"https://forge.test/o/r/issues/3","user":user(),
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
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        )
        .route(
            "/api/v1/repos/o/r/issues/2",
            get(|| async { StatusCode::NOT_FOUND }),
        )
        .route(
            "/api/v1/repos/o/r/issues/3",
            get(move || async move { Json(issue) }),
        )
        // Not JSON: an answer the gateway can't read.
        .route(
            "/api/v1/repos/o/r/issues/3/comments",
            get(|| async { "<html>" }),
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
    let query = r#"{ acmeRepo: repository(owner: "o", name: "r") {
        acmeBroken: issue(number: 1) { title }
        acmeMissing: issue(number: 2) { title }
        acmeFine: issue(number: 3) { title acmeThread: comments(first: 1) { totalCount } } } }"#;
    let response = graphql::schema()
        .execute(Request::new(query).data(session))
        .await;
    task.abort();
    let mut named: Vec<(String, Value, Value)> = response
        .errors
        .iter()
        .map(|error| {
            let error = serde_json::to_value(error).unwrap();
            (
                error["path"].to_string(),
                error["extensions"]["type"].clone(),
                error["extensions"][FAILED_FIELD].clone(),
            )
        })
        .collect();
    named.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        named,
        vec![
            (
                json!(["acmeRepo", "acmeBroken"]).to_string(),
                json!("INTERNAL"),
                json!("Repository.issue")
            ),
            (
                json!(["acmeRepo", "acmeFine", "acmeThread"]).to_string(),
                json!("INTERNAL"),
                json!("Issue.comments")
            ),
            (
                json!(["acmeRepo", "acmeMissing"]).to_string(),
                json!("NOT_FOUND"),
                Value::Null
            ),
        ]
    );
}
