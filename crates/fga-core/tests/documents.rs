//! Fields ChatGPT 22943's documents read beyond gh's own (plan review F7, G4).
use axum::{Json, Router, routing::get};
use fga_core::forgejo::Forgejo;
use serde_json::{Value, json};

fn user(login: &str) -> Value {
    json!({"id":1,"login":login,"full_name":"","avatar_url":format!("https://forge.test/avatars/{login}")})
}

/// A fake Forgejo where the viewer `e2e` has `push` access, for a PR by `author` in `state`.
fn forge(push: bool, author: &'static str, state: &'static str) -> Router {
    let pull = json!({"number":1,"title":"T","state":state,"merged":false,"draft":false,
        "html_url":"https://forge.test/e2e/oracle/pulls/1","mergeable":true,
        "head":{"ref":"h","label":"h","sha":"abc"},"base":{"ref":"main","label":"main","sha":"def"},
        "user":user(author),"merged_at":null});
    Router::new()
        .route("/api/v1/user", get(|| async { Json(user("e2e")) }))
        .route(
            "/api/v1/repos/e2e/oracle",
            get(move || async move {
                Json(json!({"id":1,"name":"oracle","full_name":"e2e/oracle","owner":user("e2e"),
                    "private":false,"html_url":"https://forge.test/e2e/oracle","default_branch":"main",
                    "fork":false,"archived":false,"permissions":{"admin":false,"push":push,"pull":true}}))
            }),
        )
        .route("/api/v1/repos/e2e/oracle/pulls/1", get(move || async move { Json(pull) }))
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/reviews",
            get(|| async {
                Json(json!([
                    {"id":7,"user":user("other"),"body":"ok","state":"APPROVED","commit_id":"abc",
                     "submitted_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:01Z",
                     "html_url":"https://forge.test/e2e/oracle/pulls/1#issuecomment-70"},
                    {"id":8,"user":user("e2e"),"body":"draft","state":"PENDING","commit_id":"abc",
                     "submitted_at":null,"updated_at":"2026-11-02T00:00:00Z",
                     "html_url":"https://forge.test/e2e/oracle/pulls/1#issuecomment-80"}
                ]))
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/issues/1/comments",
            get(|| async {
                let comment = |id: i64, login: &str| json!({"id":id,"user":user(login),"body":"hi",
                    "created_at":"2026-11-01T00:00:00Z","html_url":format!("https://forge.test/e2e/oracle/pulls/1#issuecomment-{id}")});
                Json(json!([comment(1, "e2e"), comment(2, "other")]))
            }),
        )
        .route("/api/v1/repos/e2e/oracle/issues/1/timeline", get(|| async { Json(json!([])) }))
        // A required check still pending keeps the PR BLOCKED, so auto-merge can be enabled.
        .route(
            "/api/v1/repos/e2e/oracle/branches/main",
            get(|| async {
                Json(json!({"name":"main","protected":true,"required_approvals":0,
                    "enable_status_check":true,"status_check_contexts":["ci"]}))
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/commits/abc/statuses",
            get(|| async {
                Json(json!([{"id":1,"context":"ci","status":"pending","target_url":"","description":"",
                    "created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:00Z"}]))
            }),
        )
}

async fn query(app: Router, document: &str) -> Value {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let session = Forgejo::new(base.parse().unwrap())
        .unwrap()
        .authenticate("token disposable")
        .await
        .unwrap();
    let response = fga_core::graphql::schema()
        .execute(
            async_graphql::Request::new(document)
                .data(session)
                .data("https://forge.test".parse::<url::Url>().unwrap()),
        )
        .await;
    server.abort();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()["repository"]["pullRequest"].clone()
}

const VIEWER_CAN: &str = r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){
    viewerCanUpdate viewerCanClose viewerCanReopen viewerCanEnableAutoMerge viewerCanDisableAutoMerge}}}"#;

/// GitHub's twin answers `viewerCanClose` by permission alone, and offers auto-merge only while
/// the PR can't merge yet.
#[tokio::test]
async fn viewer_can_follows_permission_state_and_merge_readiness() {
    let writer = query(forge(true, "other", "open"), VIEWER_CAN).await;
    assert_eq!(
        writer,
        json!({"viewerCanUpdate":true,"viewerCanClose":true,"viewerCanReopen":false,
            "viewerCanEnableAutoMerge":true,"viewerCanDisableAutoMerge":false})
    );
    let author = query(forge(false, "e2e", "closed"), VIEWER_CAN).await;
    assert_eq!(
        author,
        json!({"viewerCanUpdate":true,"viewerCanClose":true,"viewerCanReopen":true,
            "viewerCanEnableAutoMerge":false,"viewerCanDisableAutoMerge":false})
    );
    let reader = query(forge(false, "other", "closed"), VIEWER_CAN).await;
    assert_eq!(
        reader,
        json!({"viewerCanUpdate":false,"viewerCanClose":false,"viewerCanReopen":false,
            "viewerCanEnableAutoMerge":false,"viewerCanDisableAutoMerge":false})
    );
}

#[tokio::test]
async fn timeline_fields_for_reviews_comments_and_commits() {
    let pull = query(
        forge(false, "other", "open"),
        r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){
            author{login avatarUrl(size:48)}
            pending:reviews(first:1,author:"E2E",states:PENDING){nodes{body state}}
            reviews(first:10){nodes{author{login} createdAt url isMinimized viewerCanUpdate}}
            comments(first:10){nodes{viewerCanDelete viewerCanUpdate}}
        }}}"#,
    )
    .await;
    assert_eq!(
        pull["author"],
        json!({"login":"other","avatarUrl":"https://forge.test/avatars/other"})
    );
    assert_eq!(
        pull["pending"]["nodes"],
        json!([{"body":"draft","state":"PENDING"}])
    );
    assert_eq!(
        pull["reviews"]["nodes"],
        json!([
            {"author":{"login":"other"},"createdAt":"2026-11-01T00:00:00Z",
             "url":"https://forge.test/e2e/oracle/pulls/1#issuecomment-70","isMinimized":false,"viewerCanUpdate":false},
            {"author":{"login":"e2e"},"createdAt":"2026-11-02T00:00:00Z",
             "url":"https://forge.test/e2e/oracle/pulls/1#issuecomment-80","isMinimized":false,"viewerCanUpdate":true}
        ])
    );
    // Without push access, only the viewer's own comment can be changed.
    assert_eq!(
        pull["comments"]["nodes"],
        json!([
            {"viewerCanDelete":true,"viewerCanUpdate":true},
            {"viewerCanDelete":false,"viewerCanUpdate":false}
        ])
    );
}

#[tokio::test]
async fn commit_urls_use_the_public_web_base() {
    let app = forge(true, "other", "open").route(
        "/api/v1/repos/e2e/oracle/pulls/1/commits",
        get(|| async {
            let author = json!({"name":"T","email":"t@example.invalid","date":"2026-11-01T00:00:00Z"});
            Json(json!([{"sha":"abc","author":null,"commit":{"message":"m","author":author,"committer":author}}]))
        }),
    );
    let pull = query(
        app,
        r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){commits(last:1){nodes{commit{url}}}}}}"#,
    )
    .await;
    assert_eq!(
        pull["commits"]["nodes"][0]["commit"]["url"],
        "https://forge.test/e2e/oracle/commit/abc"
    );
}
