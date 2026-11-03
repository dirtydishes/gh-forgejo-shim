//! `gh issue create` (gh 2.103.3) creates the issue with one mutation, `IssueCreate` in
//! `api/queries_issue.go`, after reading the repository's ID. The mutation below is gh's own.
use async_graphql::{Request, Variables};
use axum::{
    Json, Router,
    extract::Json as Body,
    http::StatusCode,
    routing::{get, post},
};
use fga_core::{forgejo::Forgejo, graphql, ids::node_id};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const GH_ISSUE_CREATE: &str = "
	mutation IssueCreate($input: CreateIssueInput!) {
		createIssue(input: $input) {
			issue {
				id
				url
			}
		}
	}";

fn user() -> Value {
    json!({"id":1,"login":"o","full_name":"","avatar_url":""})
}

#[tokio::test]
async fn issue_create_makes_a_forgejo_issue() {
    let posted = Arc::new(Mutex::new(Vec::new()));
    let record = posted.clone();
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r",
            get(|| async {
                Json(
                    json!({"id":1,"name":"r","full_name":"o/r","private":false,"owner":user(),
                    "html_url":"https://forge.test/o/r","default_branch":"main","fork":false,
                    "archived":false,"has_issues":true,
                    "permissions":{"admin":false,"push":true,"pull":true}}),
                )
            }),
        )
        .route(
            "/api/v1/repos/o/r/issues",
            post(move |Body(body): Body<Value>| {
                record.lock().unwrap().push(body);
                async {
                    (
                        StatusCode::CREATED,
                        Json(
                            json!({"id":70,"number":7,"title":"Bug","body":"Steps","state":"open",
                            "html_url":"https://forge.test/o/r/issues/7","user":user(),
                            "created_at":"2026-11-03T00:00:00Z","updated_at":"2026-11-03T00:00:00Z",
                            "labels":[]}),
                        ),
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let session = Forgejo::new(base)
        .unwrap()
        .authenticate("token disposable")
        .await
        .unwrap();
    let create = |repository: i32| {
        let input = json!({"input":{"repositoryId":node_id("Repository", "o/r", repository),
            "title":"Bug","body":"Steps"}});
        Request::new(GH_ISSUE_CREATE)
            .variables(Variables::from_json(input))
            .data(session.clone())
    };
    let response = graphql::schema().execute(create(1)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"createIssue":{"issue":{"id":node_id("Issue", "o/r", 7),
            "url":"https://forge.test/o/r/issues/7"}}})
    );
    // A Repository ID for another repository at that name writes nothing.
    let response = graphql::schema().execute(create(2)).await;
    assert_eq!(response.errors[0].message, "Not Found");
    task.abort();
    assert_eq!(
        *posted.lock().unwrap(),
        [json!({"title":"Bug","body":"Steps"})]
    );
}
