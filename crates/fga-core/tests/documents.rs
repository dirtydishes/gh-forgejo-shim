//! Fields ChatGPT 22943's documents read beyond gh's own (plan review F7, G4).
use axum::{Json, Router, response::IntoResponse, routing::get};
use fga_core::forgejo::Forgejo;
use serde_json::{Value, json};

fn user(login: &str) -> Value {
    json!({"id":1,"login":login,"full_name":"","avatar_url":format!("https://forge.test/avatars/{login}")})
}

fn pull(author: &str, state: &str) -> Value {
    json!({"number":1,"title":"T","state":state,"merged":false,"draft":false,
        "html_url":"https://forge.test/e2e/oracle/pulls/1","mergeable":true,
        "head":{"ref":"h","label":"h","sha":"abc"},"base":{"ref":"main","label":"main","sha":"def"},
        "user":user(author),"merged_at":null})
}

/// A fake Forgejo where the viewer `e2e` has `push` access, for a PR by `author` in `state`.
fn forge(push: bool, author: &'static str, state: &'static str) -> Router {
    forge_with(push, pull(author, state), false)
}

/// The same forge for any PR, optionally with an auto-merge scheduled by the viewer.
fn forge_with(push: bool, pull: Value, scheduled: bool) -> Router {
    forge_full(push, push, false, pull, scheduled.then_some("e2e"))
}

/// The same forge where the viewer may merge into `main` only if `can_merge` (a writer outside a
/// merge allowlist may not), in a repository that may be archived, with an auto-merge scheduled
/// by `scheduler`.
fn forge_full(
    push: bool,
    can_merge: bool,
    archived: bool,
    pull: Value,
    scheduler: Option<&'static str>,
) -> Router {
    let timeline = match scheduler {
        Some(login) => {
            json!([{"id":1,"type":"pull_scheduled_merge","created_at":"2026-11-01T00:00:00Z","user":user(login)}])
        }
        None => json!([]),
    };
    Router::new()
        .route("/api/v1/user", get(|| async { Json(user("e2e")) }))
        .route(
            "/api/v1/repos/e2e/oracle",
            get(move || async move {
                Json(json!({"id":1,"name":"oracle","full_name":"e2e/oracle","owner":user("e2e"),
                    "private":false,"html_url":"https://forge.test/e2e/oracle","default_branch":"main",
                    "fork":false,"archived":archived,"permissions":{"admin":false,"push":push,"pull":true}}))
            }),
        )
        .route("/api/v1/repos/e2e/oracle/pulls/1", get(move || async move { Json(pull) }))
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/reviews",
            get(|| async {
                Json(json!([
                    {"id":7,"user":user("other"),"body":"ok","state":"APPROVED","commit_id":"abc",
                     "submitted_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:01Z",
                     "html_url":"https://forge.test/e2e/oracle/pulls/1#issuecomment-70","comments_count":2},
                    {"id":8,"user":user("e2e"),"body":"draft","state":"PENDING","commit_id":"older",
                     "submitted_at":"2026-11-02T00:00:00Z","updated_at":"2026-11-02T00:00:00Z",
                     "html_url":"","pull_request_url":"https://forge.test/e2e/oracle/pulls/1","comments_count":1},
                    // Someone else's pending review: Forgejo shows it to site admins, GitHub to no one.
                    {"id":9,"user":user("other"),"body":"secret","state":"PENDING","commit_id":"abc",
                     "submitted_at":"2026-11-02T00:00:00Z","updated_at":"2026-11-02T00:00:00Z","html_url":"","comments_count":1},
                    // A plain approval: no code comments, so no comments read.
                    {"id":10,"user":user("third"),"body":"","state":"APPROVED","commit_id":"abc",
                     "submitted_at":"2026-11-03T00:00:00Z","updated_at":"2026-11-03T00:00:00Z",
                     "html_url":"https://forge.test/e2e/oracle/pulls/1#issuecomment-100","comments_count":0}
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
        .route("/api/v1/repos/e2e/oracle/issues/1/timeline", get(move || async move { Json(timeline) }))
        // Forgejo's conversations: review 7 has a reply on a.rs:3 (new side); the viewer's pending
        // review 8, made on an older head, has a resolved comment on old-side line 5 of b.rs. Forgejo
        // records a.rs's line as 3, its line in the commit that last changed it; the hunk shows it
        // was line 7 when the reviewer commented.
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/reviews/7/comments",
            // Forgejo's order varies between requests; this one is newest first.
            get(|| async {
                Json(json!([
                    with_hunk(review_comment(71, 7, "e2e", "a.rs", 3, 0, None), A_HUNK),
                    with_hunk(review_comment(70, 7, "other", "a.rs", 3, 0, None), A_HUNK)
                ]))
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/reviews/9/comments",
            get(|| async { Json(json!([review_comment(90, 9, "other", "c.rs", 1, 0, None)])) }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/reviews/10/comments",
            get(|| async { panic!("a review without code comments needs no read") as Json<Value> }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/reviews/8/comments",
            get(|| async { Json(json!([review_comment(80, 8, "e2e", "b.rs", 0, 5, Some("other"))])) }),
        )
        // A required check still pending keeps the PR BLOCKED, so auto-merge can be enabled.
        .route(
            "/api/v1/repos/e2e/oracle/branches/main",
            get(move || async move {
                Json(json!({"name":"main","protected":true,"required_approvals":0,
                    "enable_status_check":true,"status_check_contexts":["ci"],"user_can_merge":can_merge}))
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

/// A hunk ending on a.rs's line 7, two lines below where Forgejo's blame puts it.
const A_HUNK: &str = "@@ -5,2 +5,3 @@\n a\n+b\n c";

fn with_hunk(mut comment: Value, diff_hunk: &str) -> Value {
    comment["diff_hunk"] = json!(diff_hunk);
    comment
}

/// A one-line hunk ending on the comment's line: a context line on the new side, or a removed
/// line on the old side.
fn hunk(position: u64, original_position: u64) -> String {
    if position > 0 {
        format!("@@ -{position},1 +{position},1 @@\n x")
    } else {
        format!("@@ -{original_position},1 +{original_position},0 @@\n-x")
    }
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
        "path":path,"commit_id":"abc","original_commit_id":"abc","diff_hunk":hunk(position, original_position),
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
            pending:reviews(first:1,author:"E2E",states:PENDING){nodes{body state submittedAt}}
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
        // Forgejo sends a pending review's creation time as submitted_at; GitHub gives it none.
        json!([{"body":"draft","state":"PENDING","submittedAt":null}])
    );
    assert_eq!(
        pull["reviews"]["nodes"],
        json!([
            {"author":{"login":"other"},"createdAt":"2026-11-01T00:00:00Z",
             "url":"https://forge.test/e2e/oracle/pull/1#issuecomment-70","isMinimized":false,"viewerCanUpdate":false},
            // A pending review has no URL of its own; another user's pending review isn't listed.
            {"author":{"login":"e2e"},"createdAt":"2026-11-02T00:00:00Z",
             "url":"https://forge.test/e2e/oracle/pull/1","isMinimized":false,"viewerCanUpdate":true},
            {"author":{"login":"third"},"createdAt":"2026-11-03T00:00:00Z",
             "url":"https://forge.test/e2e/oracle/pull/1#issuecomment-100","isMinimized":false,"viewerCanUpdate":false}
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
        json!([{"comments":{"totalCount":2}},{"comments":{"totalCount":1}},{"comments":{"totalCount":0}}])
    );
    let review = |id| fga_core::ids::node_id("PullRequestReview", "e2e/oracle", id);
    let comment = |id: i64, login: &str, review_id: i64, state: &str, diff_hunk: &str| {
        json!({"author":{"login":login},"body":format!("c{id}"),"bodyHTML":format!("<p>c{id}</p>"),
            "diffHunk":diff_hunk,"state":state,
            "url":format!("https://forge.test/e2e/oracle/pull/1/files#issuecomment-{id}"),
            "pullRequestReview":{"id":review(review_id)},"viewerCanUpdate":login == "e2e"})
    };
    // a.rs's thread is on the line the reviewer saw. b.rs's review was made on an older head, so
    // its line may have moved or gone: no current line, as an outdated thread on GitHub.
    assert_eq!(
        pull["reviewThreads"],
        json!({"totalCount":2,"nodes":[
            {"id":fga_core::ids::node_id("PullRequestReviewThread", "e2e/oracle", 70),"path":"a.rs",
             "diffSide":"RIGHT","line":7,"originalLine":7,"startLine":null,
             "isResolved":false,"viewerCanResolve":false,"viewerCanUnresolve":false,
             "comments":{"nodes":[comment(70, "other", 7, "SUBMITTED", A_HUNK), comment(71, "e2e", 7, "SUBMITTED", A_HUNK)]}},
            {"id":fga_core::ids::node_id("PullRequestReviewThread", "e2e/oracle", 80),"path":"b.rs",
             "diffSide":"LEFT","line":null,"originalLine":5,"startLine":null,
             "isResolved":true,"viewerCanResolve":false,"viewerCanUnresolve":false,
             "comments":{"nodes":[comment(80, "e2e", 8, "PENDING", &hunk(0, 5))]}}
        ]})
    );
    // The PR's author, or a writer, may resolve any conversation; a thread's own starter without
    // push access may not (Forgejo's CanMarkConversation; Opus interim o1#1).
    let marks = |pull: &Value| -> Vec<Value> {
        pull["reviewThreads"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| json!([t["viewerCanResolve"], t["viewerCanUnresolve"]]))
            .collect()
    };
    let threads = r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){
        reviewThreads(first:10){nodes{viewerCanResolve viewerCanUnresolve}}}}}"#;
    let author = query(forge(false, "e2e", "open"), threads).await;
    assert_eq!(marks(&author), [json!([true, false]), json!([false, true])]);
    let writer = query(forge(true, "other", "open"), threads).await;
    assert_eq!(marks(&writer), [json!([true, false]), json!([false, true])]);
}

/// The viewer flags in the states the first test doesn't reach (Opus interim o1#5, o1#6).
#[tokio::test]
async fn viewer_can_covers_schedules_merged_and_draft_prs_and_writers_reviews() {
    let flags = |pull: Value| {
        json!([
            pull["viewerCanClose"],
            pull["viewerCanReopen"],
            pull["viewerCanEnableAutoMerge"],
            pull["viewerCanDisableAutoMerge"]
        ])
    };
    // A schedule: no enabling it again; its scheduler may disable it even without push access.
    for push in [true, false] {
        let scheduled = query(forge_with(push, pull("other", "open"), true), VIEWER_CAN).await;
        assert_eq!(
            flags(scheduled),
            json!([push, false, false, true]),
            "push {push}"
        );
    }
    let mut merged = pull("other", "closed");
    merged["merged"] = json!(true);
    assert_eq!(
        flags(query(forge_with(true, merged, false), VIEWER_CAN).await),
        json!([true, false, false, false])
    );
    let mut draft = pull("other", "open");
    draft["draft"] = json!(true);
    assert_eq!(
        flags(query(forge_with(true, draft, false), VIEWER_CAN).await),
        json!([true, false, false, false])
    );
    // A writer may edit anyone's review body, as with comments.
    let reviews = query(
        forge(true, "other", "open"),
        r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){reviews(first:10){nodes{viewerCanUpdate}}}}}"#,
    )
    .await;
    assert_eq!(
        reviews["reviews"]["nodes"],
        json!([{"viewerCanUpdate":true},{"viewerCanUpdate":true},{"viewerCanUpdate":true}])
    );
}

/// Scheduling or cancelling a merge needs permission to merge into the base branch, which a merge
/// allowlist can withhold from a writer, and Forgejo refuses both in an archived repository.
#[tokio::test]
async fn auto_merge_follows_the_merge_allowlist_and_archiving() {
    let flags = |pull: Value| {
        json!([
            pull["viewerCanEnableAutoMerge"],
            pull["viewerCanDisableAutoMerge"]
        ])
    };
    let open = || pull("other", "open");
    // A writer outside the allowlist: no enabling, and no cancelling another user's schedule.
    let outside = query(forge_full(true, false, false, open(), None), VIEWER_CAN).await;
    assert_eq!(flags(outside), json!([false, false]));
    let outside = query(
        forge_full(true, false, false, open(), Some("other")),
        VIEWER_CAN,
    )
    .await;
    assert_eq!(flags(outside), json!([false, false]));
    // The branch summary says "may merge" from the user's own access, but a token that can't write
    // to this repository (one limited to others) can't merge here: Forgejo checks the token.
    let token = query(
        forge_full(false, true, false, open(), Some("other")),
        VIEWER_CAN,
    )
    .await;
    assert_eq!(flags(token), json!([false, false]));
    // A writer may cancel another user's schedule after the base branch is deleted: the summary is
    // gone, and write access decides.
    let mut gone = open();
    gone["base"] = json!({"ref":"gone","label":"gone","sha":"def"});
    let gone = query(
        forge_full(true, true, false, gone, Some("other")),
        VIEWER_CAN,
    )
    .await;
    assert_eq!(gone["viewerCanDisableAutoMerge"], true);
    // Its scheduler may cancel it without being allowed to merge, unless the repository is archived.
    let own = query(
        forge_full(false, false, false, open(), Some("e2e")),
        VIEWER_CAN,
    )
    .await;
    assert_eq!(flags(own), json!([false, true]));
    let archived = query(
        forge_full(true, true, true, open(), Some("e2e")),
        VIEWER_CAN,
    )
    .await;
    assert_eq!(flags(archived), json!([false, false]));
    let archived = query(forge_full(true, true, true, open(), None), VIEWER_CAN).await;
    assert_eq!(flags(archived), json!([false, false]));
}

/// Forgejo refuses every write in an archived repository (`mustNotBeArchived`), and GitHub reports
/// every write flag false there, the author's included.
#[tokio::test]
async fn an_archived_repository_offers_no_writes() {
    let pull = query(
        forge_full(true, true, true, pull("e2e", "closed"), None),
        r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){
            viewerCanUpdate viewerCanClose viewerCanReopen
            comments(first:10){nodes{viewerCanUpdate viewerCanDelete}}
            reviews(first:10){nodes{viewerCanUpdate}}
            reviewThreads(first:10){nodes{viewerCanResolve viewerCanUnresolve
                comments(first:10){nodes{viewerCanUpdate}}}}}}}"#,
    )
    .await;
    fn collect(value: &Value, flags: &mut Vec<(String, Value)>) {
        match value {
            Value::Object(map) => map.iter().for_each(|(k, v)| {
                if k.starts_with("viewerCan") {
                    flags.push((k.clone(), v.clone()));
                } else {
                    collect(v, flags);
                }
            }),
            Value::Array(items) => items.iter().for_each(|v| collect(v, flags)),
            _ => {}
        }
    }
    let mut flags = Vec::new();
    collect(&pull, &mut flags);
    assert!(flags.len() > 8, "{flags:?}");
    assert!(flags.iter().all(|(_, v)| v == false), "{flags:?}");
}

/// Forgejo's answers to a token with repository and user scope only, as Forgejo 15 gives them:
/// every `/issues/…` route, `/labels` and `/milestones` need `read:issue`, and `/orgs/…` needs
/// `read:organization` (403 even for a user, where a full token gets 404).
fn without_issue_scope(forge: Router, status: axum::http::StatusCode) -> Router {
    forge.layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| async move {
            let path = request.uri().path().to_owned();
            let scope = if path.starts_with("/api/v1/orgs/") {
                Some("read:organization")
            } else if path.contains("/issues") || path.ends_with("/labels") || path.ends_with("/milestones") {
                Some("read:issue")
            } else {
                None
            };
            match scope {
                Some(scope) if status == axum::http::StatusCode::FORBIDDEN => (
                    status,
                    Json(json!({"message":format!("token does not have at least one of required scope(s): [{scope}]"),
                        "url":"https://forge.test/api/swagger"})),
                )
                    .into_response(),
                Some(_) if path.contains("/issues") => status.into_response(),
                _ => next.run(request).await,
            }
        },
    ))
}

async fn run(app: Router, document: &str) -> async_graphql::Response {
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
    response
}

/// The fields of ChatGPT's PR documents that Forgejo serves only under `read:issue` or
/// `read:organization`.
const SCOPED: &str = r#"{repository(owner:"e2e",name:"oracle"){owner{__typename login}
    pullRequest(number:1){number viewerCanEnableAutoMerge viewerCanDisableAutoMerge
    autoMergeRequest{enabledAt} comments(first:10){totalCount nodes{body}} reviews(first:10){totalCount}}}}"#;

/// A token without issue or organization scope still reads the PR. GitHub answers a reader who
/// can't act `false` and `null` rather than an error, so the gateway does too where Forgejo won't
/// let the token look: no schedule it can see, so none to offer or cancel; no conversation; and
/// the owner read as a user. A server fault on those routes stays an error.
#[tokio::test]
async fn a_token_without_issue_scope_still_reads_the_pr() {
    use axum::http::StatusCode;
    // A writer's PR with a merge the viewer scheduled, so a full token sees all of it.
    let full = run(forge_with(true, pull("other", "open"), true), SCOPED).await;
    assert!(full.errors.is_empty(), "{:?}", full.errors);
    assert_eq!(
        full.data.into_json().unwrap()["repository"],
        json!({"owner":{"__typename":"User","login":"e2e"},"pullRequest":{"number":1,
            "viewerCanEnableAutoMerge":false,"viewerCanDisableAutoMerge":true,
            "autoMergeRequest":{"enabledAt":"2026-11-01T00:00:00Z"},
            "comments":{"totalCount":2,"nodes":[{"body":"hi"},{"body":"hi"}]},"reviews":{"totalCount":3}}})
    );
    let limited = run(
        without_issue_scope(
            forge_with(true, pull("other", "open"), true),
            StatusCode::FORBIDDEN,
        ),
        SCOPED,
    )
    .await;
    assert!(limited.errors.is_empty(), "{:?}", limited.errors);
    assert_eq!(
        limited.data.into_json().unwrap()["repository"],
        json!({"owner":{"__typename":"User","login":"e2e"},"pullRequest":{"number":1,
            "viewerCanEnableAutoMerge":false,"viewerCanDisableAutoMerge":false,"autoMergeRequest":null,
            "comments":{"totalCount":0,"nodes":[]},"reviews":{"totalCount":3}}})
    );
    let broken = run(
        without_issue_scope(
            forge_with(true, pull("other", "open"), true),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        SCOPED,
    )
    .await;
    assert!(
        !broken.errors.is_empty(),
        "a server fault must stay an error"
    );
}

/// ChatGPT desktop's six PR documents, as its gh sent them (redacted from a trace: the public
/// oracle repository, PR 1 and field names only), pointed at this file's forge.
fn chatgpt_document(n: u32) -> (String, Value) {
    let read = |ext: &str| {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/chatgpt/doc{n}.{ext}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("dirtydishes", "e2e")
        .replace("gfj-oracle", "oracle")
    };
    (
        read("graphql"),
        serde_json::from_str(&read("json")).unwrap(),
    )
}

async fn run_with(app: Router, document: &str, variables: Value) -> async_graphql::Response {
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
                .variables(async_graphql::Variables::from_json(variables))
                .data(session)
                .data("https://forge.test".parse::<url::Url>().unwrap()),
        )
        .await;
    server.abort();
    response
}

/// Every one of ChatGPT's documents answers a token without issue scope, where two failed before
/// (doc2's auto-merge fields, doc4's conversation). Those fields read as nothing to act on and no
/// comments; every other field matches a full token's answer.
#[tokio::test]
async fn chatgpt_documents_answer_a_token_without_issue_scope() {
    use axum::http::StatusCode;
    // A writer's open PR with a merge the viewer scheduled, a comment thread, reviews and a commit.
    let forge = || {
        forge_with(true, pull("other", "open"), true).route(
            "/api/v1/repos/e2e/oracle/pulls/1/commits",
            get(|| async {
                let author = json!({"name":"T","email":"t@example.invalid","date":"2026-11-01T00:00:00Z"});
                Json(json!([{"sha":"abc","author":user("e2e"),"commit":{"message":"m","author":author,"committer":author}}]))
            }),
        )
    };
    let pr = |data: &Value| {
        let repository = &data["repository"];
        if repository.is_null() {
            data["p0"]["pullRequest"].clone()
        } else {
            repository["pullRequest"].clone()
        }
    };
    for n in 1..=6 {
        let (document, variables) = chatgpt_document(n);
        let full = run_with(forge(), &document, variables.clone()).await;
        assert!(
            full.errors.is_empty(),
            "doc{n} with full scope: {:?}",
            full.errors
        );
        let limited = run_with(
            without_issue_scope(forge(), StatusCode::FORBIDDEN),
            &document,
            variables,
        )
        .await;
        assert!(
            limited.errors.is_empty(),
            "doc{n} without issue scope: {:?}",
            limited.errors
        );
        let (mut full, mut limited) = (
            full.data.into_json().unwrap(),
            limited.data.into_json().unwrap(),
        );
        match n {
            2 => {
                assert_eq!(
                    (
                        pr(&full)["autoMergeRequest"].is_object(),
                        &pr(&full)["viewerCanDisableAutoMerge"]
                    ),
                    (true, &json!(true)),
                    "doc2 with full scope sees the schedule"
                );
                let scoped = pr(&limited);
                assert_eq!(
                    (
                        &scoped["autoMergeRequest"],
                        &scoped["viewerCanEnableAutoMerge"],
                        &scoped["viewerCanDisableAutoMerge"]
                    ),
                    (&Value::Null, &json!(false), &json!(false))
                );
            }
            4 => {
                assert_eq!(pr(&full)["comments"]["nodes"].as_array().unwrap().len(), 2);
                assert_eq!(
                    pr(&limited)["comments"],
                    json!({"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}})
                );
            }
            _ => {}
        }
        for data in [&mut full, &mut limited] {
            let pull = &mut data["repository"]["pullRequest"];
            if let Some(fields) = pull.as_object_mut() {
                for field in [
                    "autoMergeRequest",
                    "viewerCanEnableAutoMerge",
                    "viewerCanDisableAutoMerge",
                    "comments",
                ] {
                    fields.remove(field);
                }
            }
        }
        assert_eq!(
            limited, full,
            "doc{n}: the rest of the answer matches a full token's"
        );
    }
}

/// Only Forgejo's missing-scope 403 reads as "nothing this token may see". Any other refusal on
/// the same routes, such as a 403 for a user Forgejo won't show the conversation to, or a token
/// revoked mid-request (401), stays an error rather than turning into empty data.
#[tokio::test]
async fn only_a_missing_scope_refusal_reads_as_empty() {
    use axum::http::StatusCode;
    let refusing = |status: StatusCode| {
        forge_with(true, pull("other", "open"), true).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| async move {
                let path = request.uri().path().to_owned();
                if path.contains("/issues") || path.starts_with("/api/v1/orgs/") {
                    (status, Json(json!({"message":"you may not see this"}))).into_response()
                } else {
                    next.run(request).await
                }
            },
        ))
    };
    for status in [StatusCode::FORBIDDEN, StatusCode::UNAUTHORIZED] {
        let response = run(refusing(status), SCOPED).await;
        let failed: Vec<String> = response
            .errors
            .iter()
            .map(|error| format!("{:?}", error.path.last()))
            .collect();
        assert!(
            ["autoMergeRequest", "comments", "owner"]
                .iter()
                .all(|field| failed.iter().any(|path| path.contains(field))),
            "{status}: {failed:?}"
        );
    }
}

/// A forge whose PR and its 24 comments each hold a 60 KiB Markdown body.
fn forge_with_long_bodies() -> Router {
    let body = "# Heading\n\nsome *text*\n\n".repeat(60 * 1024 / 25);
    let mut long_pull = pull("other", "open");
    long_pull["body"] = json!(body);
    let comments: Vec<Value> = (1..=24)
        .map(|id| {
            json!({"id":id,"user":user("other"),"body":body,"created_at":"2026-11-01T00:00:00Z",
                "html_url":format!("https://forge.test/e2e/oracle/pulls/1#issuecomment-{id}")})
        })
        .collect();
    forge_with(true, long_pull, false).layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let comments = comments.clone();
            async move {
                if request.uri().path() == "/api/v1/repos/e2e/oracle/issues/1/comments" {
                    Json(json!(comments)).into_response()
                } else {
                    next.run(request).await
                }
            }
        },
    ))
}

/// The forge ChatGPT's documents run against: a writer's open PR with a merge the viewer
/// scheduled, a comment thread, reviews and a commit.
fn chatgpt_forge() -> Router {
    forge_with(true, pull("other", "open"), true).route(
        "/api/v1/repos/e2e/oracle/pulls/1/commits",
        get(|| async {
            let author = json!({"name":"T","email":"t@example.invalid","date":"2026-11-01T00:00:00Z"});
            Json(json!([{"sha":"abc","author":user("e2e"),"commit":{"message":"m","author":author,"committer":author}}]))
        }),
    )
}

/// A document's bodies share one render budget: past it, bodies come back escaped instead of
/// rendered, so a long timeline can't make one request expensive.
#[tokio::test]
async fn bodies_past_the_request_render_budget_come_back_escaped() {
    let pull = query(
        forge_with_long_bodies(),
        r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){
            bodyHTML comments(first:30){nodes{bodyHTML}}}}}"#,
    )
    .await;
    let mut bodies = vec![pull["bodyHTML"].as_str().unwrap().to_owned()];
    for comment in pull["comments"]["nodes"].as_array().unwrap() {
        bodies.push(comment["bodyHTML"].as_str().unwrap().to_owned());
    }
    let rendered = bodies
        .iter()
        .filter(|html| html.starts_with("<h1>Heading</h1>"))
        .count();
    let escaped = bodies
        .iter()
        .filter(|html| html.starts_with("<p># Heading</p>"))
        .count();
    assert_eq!(rendered + escaped, 25, "every body is one or the other");
    // The budget is 1 MiB; each body here draws about 220 KiB: its length, 4 bytes for each of
    // its nodes, and its rendered HTML.
    assert!((3..=6).contains(&rendered), "{rendered} rendered");
}

/// Rendering happens in the gateway, so ChatGPT's PR documents make the same Forgejo requests as
/// when bodies were only escaped.
#[tokio::test]
async fn rendering_bodies_makes_no_extra_upstream_requests() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut counts = Vec::new();
    for n in [1, 4] {
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let forge = chatgpt_forge().layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::Relaxed);
                    next.run(request).await
                }
            },
        ));
        let (document, variables) = chatgpt_document(n);
        let response = run_with(forge, &document, variables).await;
        assert!(response.errors.is_empty(), "doc{n}: {:?}", response.errors);
        counts.push(requests.load(Ordering::Relaxed));
    }
    // doc1 (body) and doc4 (the timeline), counted on the escaping gateway before rendering.
    assert_eq!(counts, [3, 8]);
}

/// A missing scope that hides data logs one warning per process naming it, whichever fields and
/// requests it hides data from; a full token logs none. The forge's address tells this test's
/// warnings from other tests'.
#[tokio::test]
async fn a_scope_that_hides_data_warns_once_per_process() {
    use axum::http::StatusCode;
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    assert!(fga_core::scopes::set_sink(|message| {
        SEEN.lock().unwrap().push(message.to_owned())
    }));
    let serve = |app: Router| async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    };
    let ask = |base: String| async move {
        let session = Forgejo::new(base.parse().unwrap())
            .unwrap()
            .authenticate("token disposable")
            .await
            .unwrap();
        let response = fga_core::graphql::schema()
            .execute(
                async_graphql::Request::new(SCOPED)
                    .data(session)
                    .data("https://forge.test".parse::<url::Url>().unwrap()),
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
    };
    // Fields resolve concurrently, so the warnings arrive in either order.
    let seen = |base: &str| -> Vec<String> {
        let mut seen: Vec<String> = SEEN
            .lock()
            .unwrap()
            .iter()
            .filter(|message| message.contains(base))
            .cloned()
            .collect();
        seen.sort();
        seen
    };

    let full = serve(forge_with(true, pull("other", "open"), true)).await;
    ask(full.clone()).await;
    assert_eq!(seen(&full), Vec::<String>::new());

    let limited = serve(without_issue_scope(
        forge_with(true, pull("other", "open"), true),
        StatusCode::FORBIDDEN,
    ))
    .await;
    // Comments, the auto-merge schedule (read three times) and the owner, in two requests.
    ask(limited.clone()).await;
    ask(limited.clone()).await;
    assert_eq!(
        seen(&limited),
        vec![
            format!(
                "warning: {limited}: the token lacks read:issue, so issues can't be read, and a \
                 pull request's comments and scheduled auto-merge read as none; give it \
                 write:issue (README: Token scopes)"
            ),
            format!(
                "warning: {limited}: the token lacks read:organization, so a repository an \
                 organization owns reads as owned by a user; give it read:organization (README: \
                 Token scopes)"
            ),
        ],
        "one warning per scope"
    );
}
