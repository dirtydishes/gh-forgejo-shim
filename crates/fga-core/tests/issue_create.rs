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

/// What `gh issue create` can't do through the gateway fails before anything is written: gh's
/// input once it has looked up labels, assignees or projects; an archived repository, with
/// GitHub's wording; and a repository whose issues live in an external tracker, which reports
/// `hasIssuesEnabled: false` so gh stops before creating.
#[tokio::test]
async fn issue_create_refuses_what_it_cant_do_and_writes_nothing() {
    let posted = Arc::new(Mutex::new(0));
    let record = posted.clone();
    let repository = |name: &str, id: i64, archived: bool, external: bool| {
        let mut source = json!({"id":id,"name":name,"full_name":format!("o/{name}"),"private":false,
            "owner":user(),"html_url":format!("https://forge.test/o/{name}"),"default_branch":"main",
            "fork":false,"archived":archived,"has_issues":true,
            "permissions":{"admin":false,"push":true,"pull":true}});
        if external {
            source["external_tracker"] = json!({"external_tracker_url":"https://tracker.example/",
                "external_tracker_format":"https://tracker.example/{index}","external_tracker_style":"numeric"});
        }
        source
    };
    let (plain, archived, external) = (
        repository("r", 1, false, false),
        repository("arch", 3, true, false),
        repository("ext", 4, false, true),
    );
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route("/api/v1/repos/o/r", get(move || async move { Json(plain) }))
        .route(
            "/api/v1/repos/o/arch",
            get(move || async move { Json(archived) }),
        )
        .route(
            "/api/v1/repos/o/ext",
            get(move || async move { Json(external) }),
        )
        .fallback(post(move || {
            *record.lock().unwrap() += 1;
            async { StatusCode::CREATED }
        }));
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
    let schema = graphql::schema();
    let run = |document: &str, variables: Value| {
        schema.execute(
            Request::new(document)
                .variables(Variables::from_json(variables))
                .data(session.clone()),
        )
    };
    // gh's input after resolving a label: it also sends empty assignee and project lists.
    let metadata = json!({"input":{"repositoryId":node_id("Repository", "o/r", 1),"title":"Bug",
        "body":"Steps","labelIds":["L"],"assigneeIds":[],"projectIds":[]}});
    let response = run(GH_ISSUE_CREATE, metadata).await;
    let refusal = &response.errors[0].message;
    assert!(
        refusal.contains("unknown field") && refusal.contains("CreateIssueInput"),
        "{refusal}"
    );
    let archived = json!({"input":{"repositoryId":node_id("Repository", "o/arch", 3),
        "title":"Bug","body":"Steps"}});
    let response = run(GH_ISSUE_CREATE, archived).await;
    assert_eq!(
        response.errors[0].message,
        "Repository was archived so is read-only."
    );
    let response = run(
        r#"{plain:repository(owner:"o",name:"r"){hasIssuesEnabled}
            ext:repository(owner:"o",name:"ext"){hasIssuesEnabled}}"#,
        json!({}),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"plain":{"hasIssuesEnabled":true},"ext":{"hasIssuesEnabled":false}})
    );
    task.abort();
    assert_eq!(*posted.lock().unwrap(), 0, "something was written");
}
