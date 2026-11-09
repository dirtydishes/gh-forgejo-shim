//! A document under the complexity limit still can't build a response past the output budget:
//! the report's case, one 60,000-character issue body under many aliases, and the ways round a
//! budget that counted only text.
use async_graphql::Request;
use axum::{Json, Router, routing::get};
use fga_core::forgejo::Forgejo;
use fga_core::graphql::{self, MAX_OUTPUT_BYTES};
use serde_json::{Value, json};

const BODY_LEN: usize = 60_000;

/// Room for the budget's own error and the entries it stops at.
const SLACK: usize = 64 * 1024;

fn user() -> Value {
    json!({"id":1,"login":"o","full_name":"","avatar_url":""})
}

fn issue(number: i32, body: &str) -> Value {
    json!({"id":number,"number":number,"title":"T","body":body,"state":"open",
        "html_url":format!("https://forge.test/o/r/issues/{number}"),"user":user(),
        "created_at":"2026-11-08T00:00:00Z","updated_at":"2026-11-08T00:00:00Z","labels":[],
        "pull_request":null})
}

/// The gateway's response to `query` against a forge whose issues all have `body`: issue 1
/// alone, and a list of 49.
async fn run(body: String, query: String) -> (async_graphql::Response, usize) {
    let one = issue(1, &body);
    let all: Vec<Value> = (1..=49).map(|number| issue(number, &body)).collect();
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
            get(move || async move { Json(one) }),
        )
        .route(
            "/api/v1/repos/o/r/issues",
            get(move || async move { Json(all) }),
        )
        // Slow, so every field that reads comments is waiting on Forgejo when the budget runs out.
        .route(
            "/api/v1/repos/o/r/issues/{number}/comments",
            get(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                Json(json!([]))
            }),
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
    let response = graphql::schema()
        .execute(Request::new(query).data(session))
        .await;
    task.abort();
    let size = serde_json::to_vec(&response).unwrap().len();
    (response, size)
}

/// `body` under `aliases` aliases on issue 1.
async fn aliased_bodies(body: String, aliases: usize) -> (async_graphql::Response, usize) {
    let fields: Vec<String> = (0..aliases).map(|i| format!("a{i}:body")).collect();
    let query = format!(
        "{{repository(owner:\"o\",name:\"r\"){{issue(number:1){{{}}}}}}}",
        fields.join(" ")
    );
    run(body, query).await
}

fn stopped_by_the_budget(response: &async_graphql::Response, size: usize) {
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    let error = serde_json::to_value(&response.errors[0]).unwrap();
    assert_eq!(error["extensions"]["type"], "RESOURCE_LIMITS_EXCEEDED");
    assert!(size <= MAX_OUTPUT_BYTES + SLACK, "{size} bytes");
}

#[tokio::test]
async fn aliased_bodies_stop_at_the_output_budget() {
    // 400 aliases is under the complexity limit, and would build 24 MB.
    let (response, size) = aliased_bodies("A".repeat(BODY_LEN), 400).await;
    stopped_by_the_budget(&response, size);
}

#[tokio::test]
async fn escaped_bodies_stop_at_the_output_budget() {
    // Each control character is six bytes as JSON: 400 aliases would build 144 MB.
    let (response, size) = aliased_bodies("\u{1}".repeat(BODY_LEN), 400).await;
    stopped_by_the_budget(&response, size);
}

#[tokio::test]
async fn aliased_bodies_under_the_budget_are_whole() {
    // 6 MB.
    let (response, size) = aliased_bodies("A".repeat(BODY_LEN), 100).await;

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

/// Errors count too: a failing field under a long alias, on each issue of each issue's repository,
/// gives one error per pair of issues and alias, and each error's path repeats the alias. Unbounded,
/// this builds about 96,000 errors of over 300 bytes each.
#[tokio::test]
async fn errors_stop_at_the_output_budget() {
    let prefix = "e".repeat(250);
    let fields: Vec<String> = (0..40)
        .map(|i| format!("{prefix}{i}:labels(first:-1){{totalCount}}"))
        .collect();
    let query = format!(
        "{{repository(owner:\"o\",name:\"r\"){{issues(first:100){{nodes{{repository{{\
         issues(first:100){{nodes{{{}}}}}}}}}}}}}}}",
        fields.join(" ")
    );

    let (response, size) = run(String::new(), query).await;

    assert!(size <= MAX_OUTPUT_BYTES + SLACK, "{size} bytes");
    assert!(
        response.errors.iter().any(|error| error
            .message
            .starts_with("This query's response would pass")),
        "{} errors: {:?}",
        response.errors.len(),
        response.errors.first().map(|error| &error.message)
    );
}

/// Errors that land after the budget ran out are dropped too. Issue 1's body under 300 aliases
/// runs the budget out at once, while each of 40 fields on each of 49 issues reads that issue's
/// comments from a slow forge and then fails on `first:-1`, so all 1,960 are still in flight.
#[tokio::test]
async fn errors_landing_after_the_budget_are_dropped() {
    let bodies: Vec<String> = (0..300).map(|i| format!("a{i}:body")).collect();
    let fields: Vec<String> = (0..40)
        .map(|i| format!("e{i}:comments(first:-1){{totalCount}}"))
        .collect();
    let query = format!(
        "{{repository(owner:\"o\",name:\"r\"){{issue(number:1){{{}}} \
         issues(first:100){{nodes{{{}}}}}}}}}",
        bodies.join(" "),
        fields.join(" ")
    );

    let (response, size) = run("A".repeat(BODY_LEN), query).await;

    assert!(size <= MAX_OUTPUT_BYTES + SLACK, "{size} bytes");
    assert!(
        response.errors.iter().any(|error| error
            .message
            .starts_with("This query's response would pass")),
        "{} errors: {:?}",
        response.errors.len(),
        response.errors.first().map(|error| &error.message)
    );
}
