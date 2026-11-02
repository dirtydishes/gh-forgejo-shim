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
        // Forgejo's conversations: review 7 has a reply on a.rs:3 (new side); the viewer's pending
        // review 8 has a resolved comment on old-side line 5 of b.rs.
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/reviews/7/comments",
            get(|| async {
                Json(json!([
                    review_comment(70, 7, "other", "a.rs", 3, 0, None),
                    review_comment(71, 7, "e2e", "a.rs", 3, 0, None)
                ]))
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/reviews/8/comments",
            get(|| async { Json(json!([review_comment(80, 8, "e2e", "b.rs", 0, 5, Some("other"))])) }),
        )
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

fn review_comment(
    id: i64,
    review: i64,
    login: &str,
    path: &str,
    position: u64,
    original_position: u64,
    resolver: Option<&str>,
) -> Value {
    json!({"id":id,"body":format!("c{id}"),"user":user(login),"resolver":resolver.map(user),
        "pull_request_review_id":review,"created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:00Z",
        "path":path,"commit_id":"abc","original_commit_id":"abc","diff_hunk":"@@ -1 +1 @@",
        "position":position,"original_position":original_position,
        "html_url":format!("https://forge.test/e2e/oracle/pulls/1/files#issuecomment-{id}"),
        "pull_request_url":"https://forge.test/e2e/oracle/pulls/1"})
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
            comments(first:10){nodes{viewerCanDelete viewerCanUpdate bodyHTML}}
            bodyHTML
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
            {"viewerCanDelete":true,"viewerCanUpdate":true,"bodyHTML":"<p>hi</p>"},
            {"viewerCanDelete":false,"viewerCanUpdate":false,"bodyHTML":"<p>hi</p>"}
        ])
    );
    assert_eq!(pull["bodyHTML"], "");
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

/// Forgejo has no thread object: a conversation is the comments of one review on one path and
/// signed line (`models/issues/comment_code.go:34-49`), resolved on its first comment.
#[tokio::test]
async fn review_threads_follow_forgejo_conversations() {
    let pull = query(
        forge(false, "other", "open"),
        r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){
            reviews(first:10){nodes{comments(first:1){totalCount}}}
            reviewThreads(first:10){totalCount nodes{id path diffSide line originalLine startLine
                isResolved viewerCanResolve viewerCanUnresolve
                comments(first:10){nodes{author{login} body bodyHTML diffHunk state url
                    pullRequestReview{id} viewerCanUpdate}}}}
        }}}"#,
    )
    .await;
    assert_eq!(
        pull["reviews"]["nodes"],
        json!([{"comments":{"totalCount":2}},{"comments":{"totalCount":1}}])
    );
    let review = |id| fga_core::ids::node_id("PullRequestReview", "e2e/oracle", id);
    let comment = |id: i64, login: &str, review_id: i64, state: &str| {
        json!({"author":{"login":login},"body":format!("c{id}"),"bodyHTML":format!("<p>c{id}</p>"),
            "diffHunk":"@@ -1 +1 @@","state":state,
            "url":format!("https://forge.test/e2e/oracle/pulls/1/files#issuecomment-{id}"),
            "pullRequestReview":{"id":review(review_id)},"viewerCanUpdate":login == "e2e"})
    };
    assert_eq!(
        pull["reviewThreads"],
        json!({"totalCount":2,"nodes":[
            {"id":fga_core::ids::node_id("PullRequestReviewThread", "e2e/oracle", 70),"path":"a.rs",
             "diffSide":"RIGHT","line":3,"originalLine":3,"startLine":null,
             "isResolved":false,"viewerCanResolve":false,"viewerCanUnresolve":false,
             "comments":{"nodes":[comment(70, "other", 7, "SUBMITTED"), comment(71, "e2e", 7, "SUBMITTED")]}},
            {"id":fga_core::ids::node_id("PullRequestReviewThread", "e2e/oracle", 80),"path":"b.rs",
             "diffSide":"LEFT","line":5,"originalLine":5,"startLine":null,
             "isResolved":true,"viewerCanResolve":false,"viewerCanUnresolve":true,
             "comments":{"nodes":[comment(80, "e2e", 8, "PENDING")]}}
        ]})
    );
}
