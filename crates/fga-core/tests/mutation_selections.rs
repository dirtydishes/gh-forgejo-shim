//! GraphQL merges the selections that share a response name before it runs any of them, so a
//! mutation selected twice with the same arguments, here through two fragments, writes once and
//! answers with both selections' fields of that one result.
use async_graphql::Request;
use axum::{
    Json, Router,
    extract::Path,
    http::StatusCode,
    routing::{get, post},
};
use fga_core::{forgejo::Forgejo, graphql, ids::node_id};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn user() -> Value {
    json!({"id":1,"login":"o","full_name":"","avatar_url":""})
}

/// A forge with one repository and one issue, #2, that counts the issues and comments posted to
/// it and numbers each after the count.
fn forge(posts: Arc<Mutex<Vec<String>>>) -> Router {
    let (issues, comments) = (posts.clone(), posts);
    Router::new()
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
            "/api/v1/repos/o/r/issues/2",
            get(|| async {
                Json(json!({"id":2,"number":2,"title":"T","body":"","state":"open",
                    "html_url":"https://forge.test/o/r/issues/2","user":user(),
                    "created_at":"2026-11-03T00:00:00Z","updated_at":"2026-11-03T00:00:00Z",
                    "labels":[]}))
            }),
        )
        .route(
            "/api/v1/repos/o/r/issues",
            post(move || {
                let number = {
                    let mut posts = issues.lock().unwrap();
                    posts.push("issue".to_owned());
                    posts.len() + 6
                };
                async move {
                    (
                        StatusCode::CREATED,
                        Json(json!({"id":number * 10,"number":number,"title":"Bug","body":"",
                            "state":"open","html_url":format!("https://forge.test/o/r/issues/{number}"),
                            "user":user(),"created_at":"2026-11-03T00:00:00Z",
                            "updated_at":"2026-11-03T00:00:00Z","labels":[]})),
                    )
                }
            }),
        )
        .route(
            "/api/v1/repos/o/r/issues/{number}/comments",
            post(move |Path(issue): Path<i32>| {
                let id = {
                    let mut posts = comments.lock().unwrap();
                    posts.push(format!("comment on {issue}"));
                    posts.len() + 8
                };
                async move {
                    (
                        StatusCode::CREATED,
                        Json(json!({"id":id,"user":user(),"body":"hi",
                            "created_at":"2026-11-03T00:00:00Z",
                            "html_url":format!("https://forge.test/o/r/issues/{issue}#issuecomment-{id}")})),
                    )
                }
            }),
        )
}

/// What the gateway answers `query`, and what it posted to the forge meanwhile.
async fn run(query: &str) -> (async_graphql::Response, Vec<String>) {
    let posts = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let router = forge(posts.clone());
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
    let posts = posts.lock().unwrap().clone();
    (response, posts)
}

fn create_input() -> String {
    format!(
        r#"{{repositoryId:"{}",title:"Bug"}}"#,
        node_id("Repository", "o/r", 1)
    )
}

fn comment_input() -> String {
    format!(
        r#"{{subjectId:"{}",body:"hi"}}"#,
        node_id("Issue", "o/r", 2)
    )
}

#[tokio::test]
async fn an_issue_created_through_two_fragments_is_created_once() {
    let input = create_input();
    let (response, posts) = run(&format!(
        "mutation{{...A ...B}}
        fragment A on Mutation{{createIssue(input:{input}){{issue{{id}}}}}}
        fragment B on Mutation{{createIssue(input:{input}){{issue{{number}}}}}}"
    ))
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(posts, ["issue"]);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"createIssue":{"issue":{"id":node_id("Issue", "o/r", 7),"number":7}}})
    );
}

#[tokio::test]
async fn a_comment_added_through_two_fragments_is_added_once() {
    let input = comment_input();
    let (response, posts) = run(&format!(
        "mutation{{...A ...B}}
        fragment A on Mutation{{addComment(input:{input}){{commentEdge{{node{{url}}}}}}}}
        fragment B on Mutation{{addComment(input:{input}){{commentEdge{{node{{body}}}}}}}}"
    ))
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(posts, ["comment on 2"]);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"addComment":{"commentEdge":{"node":{
            "url":"https://forge.test/o/r/issues/2#issuecomment-9","body":"hi"}}}})
    );
}

/// The same mutation under one name with other arguments can't merge, so it writes nothing: one
/// selected directly and one through a fragment, which async-graphql's own rule doesn't compare.
#[tokio::test]
async fn one_name_with_other_arguments_writes_nothing() {
    let input = create_input();
    let other = input.replace("Bug", "Other");
    let (response, posts) = run(&format!(
        "mutation{{createIssue(input:{input}){{issue{{id}}}} ...B}}
        fragment B on Mutation{{createIssue(input:{other}){{issue{{number}}}}}}"
    ))
    .await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    assert!(
        response.errors[0]
            .message
            .starts_with("Fields \"input\" conflict because they have differing arguments"),
        "{}",
        response.errors[0].message
    );
    assert_eq!(posts, Vec::<String>::new());
}
