use axum::{
    Json, Router,
    extract::Query,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use fga_core::forgejo::{Error, Forgejo};
use serde_json::{Value, json};
use std::collections::HashMap;

fn user() -> Value {
    json!({"id":1,"login":"e2e","full_name":"Test User","avatar_url":"https://forge.test/avatar/1"})
}
fn pr(number: i32, owner: &str, branch: &str) -> Value {
    json!({"number":number,"title":"test","state":"open","merged":false,"draft":false,"html_url":format!("https://forge.test/e2e/oracle/pulls/{number}"),"head":{"label":branch,"ref":branch,"sha":"abc","repo":{"owner":user_with_login(owner)}},"base":{"label":"main","ref":"main","sha":"def","repo":{"owner":user()}},"user":user(),"merged_at":null})
}
fn user_with_login(login: &str) -> Value {
    let mut value = user();
    value["login"] = json!(login);
    value
}

#[tokio::test]
async fn forwards_auth_and_filters_across_short_upstream_pages() {
    let upstream = Router::new()
        .route(
            "/api/v1/user",
            get(|headers: HeaderMap| async move {
                assert_eq!(headers["authorization"], "bearer disposable");
                Json(user())
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/pulls",
            get(
                |headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
                    assert_eq!(headers["authorization"], "bearer disposable");
                    assert_eq!(query["state"], "all");
                    match query["page"].as_str() {
                        // A server can clamp limit below our requested 50; Link remains authoritative.
                        "1" => (
                            [("link", "<http://untrusted.invalid/steal>; rel=\"next\"")],
                            Json(json!([pr(4, "other", "s/pass"), null])),
                        )
                            .into_response(),
                        "2" => (
                            [("link", "<http://untrusted.invalid/steal>; rel=\"next\"")],
                            Json(json!([pr(3, "e2e", "s/other")])),
                        )
                            .into_response(),
                        "3" => Json(json!([pr(1, "E2E", "s/pass"), pr(2, "e2e", "s/pass")]))
                            .into_response(),
                        _ => panic!("unexpected page"),
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let forgejo = Forgejo::new(url).unwrap();
    assert!(matches!(
        forgejo.authenticate("basic invalid").await,
        Err(Error::Unauthorized)
    ));
    let session = forgejo.authenticate("bearer disposable").await.unwrap();
    let prs = session
        .pull_requests("e2e", "oracle", "all", Some("e2e:s/pass"), None)
        .await
        .unwrap();
    assert_eq!(prs.iter().map(|p| p.number).collect::<Vec<_>>(), [2, 1]);
    assert_eq!(prs[0].head.label(), "e2e:s/pass");
    task.abort();
}

#[derive(Clone, Default)]
struct Counters {
    users: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pulls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    statuses: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    repos: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

async fn memo_user(
    axum::extract::State(hits): axum::extract::State<Counters>,
    headers: HeaderMap,
) -> axum::response::Response {
    hits.users.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if headers["authorization"] == "token invalid" {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    Json(user()).into_response()
}
async fn memo_repo(axum::extract::State(hits): axum::extract::State<Counters>) -> Json<Value> {
    hits.repos.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Json(
        json!({"id":1,"name":"oracle","full_name":"e2e/oracle","owner":user(),"private":true,
        "html_url":"https://forge.test/e2e/oracle","default_branch":"main","fork":false,"archived":false,
        "permissions":{"admin":true,"push":true,"pull":true}}),
    )
}
async fn memo_pr(axum::extract::State(hits): axum::extract::State<Counters>) -> Json<Value> {
    hits.pulls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut p = pr(1, "e2e", "s/pass");
    p["mergeable"] = json!(true);
    Json(p)
}
fn pull_commit(sha: &str) -> Value {
    let author =
        json!({"name":"Test User","email":"test@example.invalid","date":"2026-10-30T00:00:00Z"});
    json!({"sha":sha,"author":user(),"commit":{"message":sha,"author":author,"committer":author}})
}

async fn memo_commits() -> Json<Value> {
    Json(json!([pull_commit("abc")]))
}
async fn memo_statuses(axum::extract::State(hits): axum::extract::State<Counters>) -> Json<Value> {
    hits.statuses
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Json(json!([
        {"id":1,"context":"ci / lint (push)","status":"success","target_url":"/e2e/oracle/actions/runs/1/jobs/1",
         "description":"Successful in 2s","created_at":"2026-10-30T00:00:00Z","updated_at":"2026-10-30T00:00:02Z"},
        {"id":2,"context":"external","status":"success","target_url":"","description":"passed",
         "created_at":"2026-10-30T00:00:00Z","updated_at":"2026-10-30T00:00:02Z"}
    ]))
}
async fn memo_protection() -> Json<Value> {
    Json(
        json!([{"rule_name":"main","enable_status_check":true,"status_check_contexts":["ci / * (push)","external"]}]),
    )
}

#[tokio::test]
async fn caches_identity_per_token_but_keeps_request_data_fresh_and_coalesces_aliases() {
    use std::sync::atomic::Ordering;
    let counts = Counters::default();
    let upstream = Router::new()
        .route("/api/v1/user", get(memo_user))
        .route("/api/v1/repos/e2e/oracle", get(memo_repo))
        .route("/api/v1/repos/e2e/oracle/pulls/1", get(memo_pr))
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/commits",
            get(memo_commits),
        )
        .route(
            "/api/v1/repos/e2e/oracle/commits/abc/statuses",
            get(memo_statuses),
        )
        .route(
            "/api/v1/repos/e2e/oracle/branch_protections",
            get(memo_protection),
        )
        .with_state(counts.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let forgejo = Forgejo::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    for _ in 0..2 {
        assert!(matches!(
            forgejo.authenticate("token invalid").await,
            Err(Error::Unauthorized)
        ));
    }
    assert_eq!(
        counts.users.load(Ordering::SeqCst),
        2,
        "failed auth is not cached"
    );
    let one = forgejo.authenticate("token good").await.unwrap();
    let two = forgejo.authenticate("bearer good").await.unwrap();
    assert_eq!(
        counts.users.load(Ordering::SeqCst),
        3,
        "normalized token hash reuses identity"
    );
    forgejo.authenticate("token other").await.unwrap();
    assert_eq!(
        counts.users.load(Ordering::SeqCst),
        4,
        "different tokens do not share identity"
    );
    let id = fga_core::ids::node_id("PullRequest", "e2e/oracle", 1);
    let query = r#"query {
        a:node(id:"NODE_ID") { ...on PullRequest {number mergeStateStatus} }
        b:node(id:"NODE_ID") {...on PullRequest {number mergeStateStatus}}
        repository(owner:"e2e",name:"oracle") { pullRequest(number:1) {
            commits(last:1) {nodes {commit {statusCheckRollup {state contexts(first:1) {
                checkRunCount statusContextCount totalCount pageInfo {hasNextPage endCursor}
                nodes { ...on CheckRun {name isRequired(pullRequestId:"NODE_ID")} }
            }}}}}
        }}
    }"#
    .replace("NODE_ID", &id);
    let schema = fga_core::graphql::schema();
    let response = schema
        .execute(
            async_graphql::Request::new(query)
                .data(one.clone())
                .data("https://forge.test".parse::<url::Url>().unwrap()),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["a"]["mergeStateStatus"], "CLEAN");
    let contexts = &data["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]
        ["contexts"];
    assert_eq!(contexts["checkRunCount"], 1);
    assert_eq!(contexts["statusContextCount"], 1);
    assert_eq!(contexts["totalCount"], 2);
    assert_eq!(contexts["pageInfo"]["hasNextPage"], true);
    assert_eq!(contexts["nodes"][0]["isRequired"], true);
    let cursor = contexts["pageInfo"]["endCursor"].as_str().unwrap();
    let request=async_graphql::Request::new(r#"query($after:String){repository(owner:"e2e",name:"oracle"){pullRequest(number:1){commits(last:1){nodes{commit{statusCheckRollup{contexts(first:1,after:$after){pageInfo{hasNextPage} nodes{...on StatusContext{context isRequired(pullRequestId:"NODE_ID")}}}}}}}}}}"#.replace("NODE_ID",&id))
        .variables(async_graphql::Variables::from_json(json!({"after":cursor}))).data(one.clone()).data("https://forge.test".parse::<url::Url>().unwrap());
    let response = schema.execute(request).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let contexts = &data["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]
        ["contexts"];
    assert_eq!(contexts["nodes"][0]["context"], "external");
    assert_eq!(contexts["nodes"][0]["isRequired"], true);
    assert_eq!(contexts["pageInfo"]["hasNextPage"], false);
    assert_eq!(
        counts.pulls.load(Ordering::SeqCst),
        1,
        "concurrent aliases share PR fetch"
    );
    assert_eq!(
        counts.statuses.load(Ordering::SeqCst),
        1,
        "concurrent fields share statuses"
    );
    // Captured documents must pass GraphQL validation even when a test upstream
    // intentionally lacks the data endpoints needed to resolve every field.
    let documents =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gh-pr-read");
    for path in std::fs::read_dir(documents).unwrap() {
        let path = path.unwrap().path();
        let request=async_graphql::Request::new(std::fs::read_to_string(&path).unwrap())
            .variables(async_graphql::Variables::from_json(json!({"owner":"e2e","repo":"oracle","name":"oracle","number":1,"pr_number":1,"id":id,"headRefName":"s/pass","viewerQuery":"repo:e2e/oracle state:open is:pr author:e2e","reviewerQuery":"repo:e2e/oracle state:open review-requested:e2e"})))
            .data(one.clone()).data("https://forge.test".parse::<url::Url>().unwrap());
        let response = schema.execute(request).await;
        for e in response.errors {
            assert!(
                e.extensions
                    .as_ref()
                    .is_some_and(|ext| ext.get("type").is_some()),
                "{} validation error: {}",
                path.display(),
                e.message
            );
        }
    }
    two.pull_request("e2e", "oracle", 1).await.unwrap();
    two.statuses("e2e", "oracle", "abc").await.unwrap();
    assert_eq!(
        counts.pulls.load(Ordering::SeqCst),
        2,
        "new request fetches current PR data"
    );
    assert_eq!(
        counts.statuses.load(Ordering::SeqCst),
        2,
        "new request fetches current statuses"
    );
    assert_eq!(counts.repos.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn graphql_draft_title_uses_configured_prefixes_and_mergeability_is_unknown() {
    let upstream = Router::new()
        .route("/api/v1/user", get(memo_user))
        .route("/api/v1/repos/e2e/oracle", get(memo_repo))
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1",
            get(|| async {
                let mut p = pr(1, "e2e", "s/draft");
                p["title"] = json!("[draft]  scenario");
                p["draft"] = json!(true);
                p["mergeable"] = json!(true);
                Json(p)
            }),
        )
        .with_state(Counters::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let forgejo = Forgejo::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let session = forgejo.authenticate("token good").await.unwrap();
    let schema = fga_core::graphql::schema();
    for (prefixes, expected) in [(None, "[draft]  scenario"), (Some("[DRAFT]"), "scenario")] {
        let mut request = async_graphql::Request::new(
            r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){title isDraft mergeable}}}"#,
        )
        .data(session.clone());
        if let Some(prefixes) = prefixes {
            request = request.data(fga_core::map::WipPrefixes::from_csv(prefixes));
        }
        let response = schema.execute(request).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(data["repository"]["pullRequest"]["title"], expected);
        assert_eq!(data["repository"]["pullRequest"]["isDraft"], true);
        assert_eq!(data["repository"]["pullRequest"]["mergeable"], "UNKNOWN");
    }
    server.abort();
}

#[tokio::test]
async fn pr_commits_are_oldest_first_so_last_one_is_the_head() {
    // Forgejo pages `git log base..head`, newest first (`routers/api/v1/repo/pull.go`
    // GetPullRequestCommits, `modules/git/repo_compare.go` GetCompareInfo).
    let upstream = Router::new()
        .route("/api/v1/user", get(memo_user))
        .route("/api/v1/repos/e2e/oracle", get(memo_repo))
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1",
            get(|| async { Json(pr(1, "e2e", "s/two")) }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/commits",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                match query["page"].as_str() {
                    "1" => (
                        [("link", "<http://untrusted.invalid>; rel=\"next\"")],
                        Json(json!([pull_commit("head"), pull_commit("middle")])),
                    )
                        .into_response(),
                    "2" => Json(json!([pull_commit("oldest")])).into_response(),
                    _ => panic!("unexpected page"),
                }
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/commits/{sha}/statuses",
            get(|axum::extract::Path(sha): axum::extract::Path<String>| async move {
                let state = if sha == "head" { "success" } else { "failure" };
                Json(json!([{"id":1,"context":"ci","status":state,"target_url":"","description":"",
                    "created_at":"2026-10-30T00:00:00Z","updated_at":"2026-10-30T00:00:02Z"}]))
            }),
        )
        .with_state(Counters::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let forgejo = Forgejo::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let session = forgejo.authenticate("token good").await.unwrap();
    // gh reads checks through `commits(last: 1)` (cli/cli `api/query_builder.go`).
    let response = fga_core::graphql::schema()
        .execute(
            async_graphql::Request::new(
                r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){
                    all:commits(first:10){nodes{commit{oid}}}
                    last:commits(last:1){nodes{commit{oid statusCheckRollup{state}}}}
                }}}"#,
            )
            .data(session)
            .data("https://forge.test".parse::<url::Url>().unwrap()),
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let pull = &data["repository"]["pullRequest"];
    assert_eq!(
        pull["all"]["nodes"],
        json!([
            {"commit":{"oid":"oldest"}},
            {"commit":{"oid":"middle"}},
            {"commit":{"oid":"head"}}
        ])
    );
    assert_eq!(
        pull["last"]["nodes"],
        json!([{"commit":{"oid":"head","statusCheckRollup":{"state":"SUCCESS"}}}])
    );
    server.abort();
}

#[tokio::test]
async fn retries_ambiguous_mergeability_once_and_uses_reverse_compare_for_behind() {
    use std::sync::{Arc, Mutex};
    let hits = Arc::new(Mutex::new(HashMap::<i32, usize>::new()));
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/e2e/oracle/pulls/{number}",
            get({
                let hits = hits.clone();
                move |axum::extract::Path(number): axum::extract::Path<i32>| {
                    let hits = hits.clone();
                    async move {
                        let mut map = hits.lock().unwrap();
                        let n = map.entry(number).or_default();
                        *n += 1;
                        let mut p = pr(number, "e2e", "s/pass");
                        p["mergeable"] = json!(number == 1 && *n == 2);
                        p["draft"] = json!(number == 3);
                        Json(p)
                    }
                }
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/compare/{refs}",
            get(
                |axum::extract::Path(refs): axum::extract::Path<String>| async move {
                    Json(match refs.as_str() {
                        // The head has 3 commits the base lacks; the base has none the head lacks.
                        "base...head" => json!({"total_commits":3}),
                        "head...base" => json!({"total_commits":0}),
                        _ => panic!("unexpected compare"),
                    })
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let forgejo = Forgejo::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let session = forgejo.authenticate("token good").await.unwrap();
    let p = session.pull_request("e2e", "oracle", 1).await.unwrap();
    assert_eq!(
        fga_core::map::mergeable(&p),
        fga_core::map::MergeableState::Mergeable
    );
    let p = session.pull_request("e2e", "oracle", 2).await.unwrap();
    assert_eq!(
        fga_core::map::mergeable(&p),
        fga_core::map::MergeableState::Unknown
    );
    // Drafts always report false and map to UNKNOWN, so they skip the retry.
    session.pull_request("e2e", "oracle", 3).await.unwrap();
    session.pull_request("e2e", "oracle", 1).await.unwrap();
    assert_eq!(
        *hits.lock().unwrap(),
        HashMap::from([(1, 2), (2, 2), (3, 1)])
    );
    // behind(base, head) asks whether the head lacks base commits.
    assert!(
        !session
            .behind("e2e", "oracle", "base", "head")
            .await
            .unwrap()
    );
    assert!(
        session
            .behind("e2e", "oracle", "head", "base")
            .await
            .unwrap()
    );
    server.abort();
}

fn comment(id: i64) -> Value {
    json!({"id":id,"body":"x","user":user(),"created_at":"2026-10-30T00:00:00Z",
        "updated_at":"2026-10-30T00:00:00Z","html_url":format!("https://forge.test/e2e/oracle/pulls/1#issuecomment-{id}")})
}

fn status(id: i64) -> Value {
    json!({"id":id,"context":format!("ci-{id}"),"status":"success","target_url":"",
        "description":"","created_at":"2026-10-30T00:00:00Z","updated_at":"2026-10-30T00:00:00Z"})
}

/// Endpoints whose swagger has no `page` return the whole list for every query. Reads of them
/// must stop after one request, and paged reads of a misbehaving endpoint must stop at the cap.
#[tokio::test]
async fn unpaged_endpoints_are_read_once_and_paging_is_bounded() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let comment_hits = std::sync::Arc::new(AtomicUsize::new(0));
    let status_hits = std::sync::Arc::new(AtomicUsize::new(0));
    let run_pages = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/e2e/oracle/issues/1/comments",
            get({
                let hits = comment_hits.clone();
                move || async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Json(Value::Array((1..=60).map(comment).collect()))
                }
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/commits/abc/statuses",
            get({
                let hits = status_hits.clone();
                move || async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Json(Value::Array((1..=50).map(status).collect()))
                }
            }),
        )
        // Forgejo's links mark the last page even when it is full.
        .route(
            "/api/v1/repos/e2e/oracle/commits/full/statuses",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                let page: i64 = query["page"].parse().unwrap();
                let rel = if page < 200 { "next" } else { "prev" };
                (
                    [("link", format!("<http://untrusted.invalid>; rel=\"{rel}\""))],
                    Json(Value::Array((1..=50).map(|i| status(page * 50 + i)).collect())),
                )
            }),
        )
        // Forgejo pages reviews before hiding others' pending ones, so they are read unpaged.
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/reviews",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                assert!(!query.contains_key("page"), "reviews must not be paged");
                Json(json!([{"id":1,"user":user(),"body":"","state":"APPROVED","submitted_at":null,"commit_id":"abc"}]))
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/actions/runs",
            get({
                let hits = run_pages.clone();
                move |Query(query): Query<HashMap<String, String>>| async move {
                    hits.lock().unwrap().push(query["page"].clone());
                    let run = json!({"id":1,"workflow_id":"ci.yml","index_in_repo":1,"title":"x","prettyref":"main","commit_sha":"abc","status":"waiting","event":"push","html_url":"","created":"","updated":"","started":""});
                    Json(json!({"total_count":2_000_000,"workflow_runs":vec![run; 50]}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let forgejo = Forgejo::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let session = forgejo.authenticate("token good").await.unwrap();
    let comments = session.comments("e2e", "oracle", 1).await.unwrap();
    assert_eq!(comments.len(), 60);
    assert_eq!(comment_hits.load(Ordering::SeqCst), 1);
    assert!(matches!(
        session.statuses("e2e", "oracle", "abc").await,
        Err(Error::Upstream)
    ));
    assert_eq!(status_hits.load(Ordering::SeqCst), 200);
    let full = session.statuses("e2e", "oracle", "full").await.unwrap();
    assert_eq!(full.len(), 200 * 50);
    assert_eq!(session.reviews("e2e", "oracle", 1).await.unwrap().len(), 1);
    // Only the upstream page holding the requested window is read, however deep it is.
    let runs = session
        .action_runs("e2e", "oracle", &[], 1_000_010, 30)
        .await
        .unwrap();
    assert_eq!(runs.workflow_runs.len(), 30);
    assert_eq!(*run_pages.lock().unwrap(), ["20001"]);
    server.abort();
}

/// `/branch_protections` is admin-only, so others read the branch's summary of its rule, whose
/// name Forgejo hides from non-admins (`services/convert/convert.go:88-99`). A token limited to
/// some repositories gets 403 even when the repository reports admin rights. Team review
/// requests arrive with a null user.
#[tokio::test]
async fn non_admins_read_protection_from_the_branch_and_team_requests_parse() {
    for admin in [false, true] {
        let mut base = pr(1, "e2e", "s/pass");
        base["base"]["ref"] = json!("release/1");
        base["base"]["label"] = json!("release/1");
        base["mergeable"] = json!(true);
        let upstream = Router::new()
            .route("/api/v1/user", get(|| async { Json(user()) }))
            .route(
                "/api/v1/repos/e2e/oracle",
                get(move || async move {
                    Json(json!({"id":1,"name":"oracle","full_name":"e2e/oracle","owner":user(),"private":false,
                        "html_url":"https://forge.test/e2e/oracle","default_branch":"main","fork":false,"archived":false,
                        "permissions":{"admin":admin,"push":true,"pull":true}}))
                }),
            )
            .route(
                "/api/v1/repos/e2e/oracle/branch_protections",
                get(|| async { StatusCode::FORBIDDEN }),
            )
            .route(
                "/api/v1/repos/e2e/oracle/branches/release/1",
                get(|| async {
                    Json(json!({"name":"release/1","protected":true,"effective_branch_protection_name":"",
                        "required_approvals":1,"enable_status_check":false,"status_check_contexts":null}))
                }),
            )
            .route(
                "/api/v1/repos/e2e/oracle/branches/main",
                get(|| async {
                    Json(json!({"name":"main","protected":false,"effective_branch_protection_name":"","status_check_contexts":[]}))
                }),
            )
            .route(
                "/api/v1/repos/e2e/oracle/pulls/1",
                get(move || {
                    let base = base.clone();
                    async move { Json(base) }
                }),
            )
            .route(
                "/api/v1/repos/e2e/oracle/commits/abc/statuses",
                get(|| async { Json(json!([])) }),
            )
            .route(
                "/api/v1/repos/e2e/oracle/pulls/1/reviews",
                get(|| async {
                    Json(json!([
                        {"id":1,"user":user(),"body":"","state":"APPROVED","submitted_at":"2026-10-30T00:00:01Z","commit_id":"abc","official":true},
                        {"id":2,"user":null,"body":"","state":"REQUEST_REVIEW","submitted_at":"2026-10-30T00:00:02Z","commit_id":"abc","official":true}
                    ]))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let session = Forgejo::new(url)
            .unwrap()
            .authenticate("token t")
            .await
            .unwrap();
        assert!(
            session
                .protection("e2e", "oracle", "main")
                .await
                .unwrap()
                .is_none()
        );
        let response = fga_core::graphql::schema()
            .execute(
                async_graphql::Request::new(
                    r#"{repository(owner:"e2e",name:"oracle"){pullRequest(number:1){
                        reviewDecision mergeStateStatus baseRef{branchProtectionRule{requiredApprovingReviewCount}}
                        reviews(first:10){totalCount} latestReviews(first:10){totalCount}}}}"#,
                )
                .data(session)
                .data("https://forge.test".parse::<url::Url>().unwrap()),
            )
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        // The visible gates pass, but hidden flags may still block, so neither the review decision
        // nor the merge state is known.
        assert_eq!(
            response.data.into_json().unwrap()["repository"]["pullRequest"],
            json!({"reviewDecision":null,"mergeStateStatus":"UNKNOWN",
                "baseRef":{"branchProtectionRule":{"requiredApprovingReviewCount":1}},
                "reviews":{"totalCount":1},"latestReviews":{"totalCount":1}}),
            "admin: {admin}"
        );
        server.abort();
    }
}
