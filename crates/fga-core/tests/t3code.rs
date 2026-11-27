//! T3 Code's pull request documents, as it sends them for a Forgejo host, answer with Forgejo's
//! values. T3 Code decodes each answer strictly, so a field it reads that comes back null where
//! it expects a value fails the read as surely as an unknown field.
use axum::{
    Json, Router,
    extract::{RawQuery, Request},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use fga_core::forgejo::Forgejo;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const DOCUMENTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/clients/t3code-0.1.13-documents"
);

fn document(name: &str) -> String {
    std::fs::read_to_string(format!("{DOCUMENTS}/{name}.graphql")).expect(name)
}

fn user(login: &str) -> Value {
    json!({"id":1,"login":login,"full_name":"","avatar_url":format!("https://forge.test/avatars/{login}")})
}

const HEAD: &str = "1111111111111111111111111111111111111111";
const BASE: &str = "2222222222222222222222222222222222222222";
const PARENT: &str = "3333333333333333333333333333333333333333";

/// A fake Forgejo with one open PR, `feat` into `main`, whose head runs one Actions job (run 7,
/// `state`) and carries one plain status. Requests to routes it lacks are kept in `missed`.
fn forge(state: &'static str, missed: Arc<Mutex<Vec<String>>>) -> Router {
    let repo = || json!({"name":"oracle","full_name":"e2e/oracle","owner":user("e2e"),"id":1});
    Router::new()
        .route("/api/v1/user", get(|| async { Json(user("e2e")) }))
        .route(
            "/api/v1/repos/e2e/oracle",
            get(|| async {
                Json(json!({"id":1,"name":"oracle","full_name":"e2e/oracle","owner":user("e2e"),
                    "private":false,"html_url":"https://forge.test/e2e/oracle","default_branch":"main",
                    "fork":false,"archived":false,"allow_merge_commits":true,"allow_rebase":true,
                    "allow_squash_merge":false,"permissions":{"admin":false,"push":true,"pull":true}}))
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1",
            get(move || async move {
                Json(json!({"number":1,"title":"T","body":"","state":"open","merged":false,"draft":false,
                    "html_url":"https://forge.test/e2e/oracle/pulls/1","mergeable":true,
                    "created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-02T00:00:00Z",
                    "head":{"ref":"feat","label":"feat","sha":HEAD,"repo":repo()},
                    "base":{"ref":"main","label":"main","sha":BASE,"repo":repo()},
                    "user":user("e2e"),"merged_at":null,"labels":[],"requested_reviewers":[],
                    "requested_reviewers_teams":[{"id":4,"name":"core","organization":{"id":2,"name":"e2e-org"}}]}))
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/git/refs/pull/1/head",
            get(|| async { Json(json!([{"ref":"refs/pull/1/head","object":{"sha":HEAD}}])) }),
        )
        .route(
            &format!("/api/v1/repos/e2e/oracle/commits/{HEAD}/statuses"),
            get(move || async move {
                Json(json!([
                    {"id":1,"context":"ci / test (pull_request)","status":state,
                     "description": if state == "pending" { "Running" } else { "Successful in 9s" },
                     "target_url":"/e2e/oracle/actions/runs/7/jobs/0",
                     "created_at":"2026-11-02T00:00:00Z","updated_at":"2026-11-02T00:01:00Z"},
                    {"id":2,"context":"deploy","status":"success","description":"ok","target_url":"",
                     "created_at":"2026-11-02T00:00:00Z","updated_at":"2026-11-02T00:00:00Z"}
                ]))
            }),
        )
        .route(
            // Two base commits the head lacks, counted in `X-Total-Count`; a listing of one.
            "/api/v1/repos/e2e/oracle/commits",
            get(|RawQuery(query): RawQuery| async move {
                let query = query.unwrap_or_default();
                let counted = query.contains(&format!("sha={BASE}"))
                    && query.contains(&format!("not={HEAD}"))
                    && query.contains("limit=1")
                    && query.contains("files=false");
                if !counted {
                    return (StatusCode::NOT_FOUND, Json(json!({"message":query}))).into_response();
                }
                ([("x-total-count", "2")], Json(json!([]))).into_response()
            }),
        )
        .route("/api/v1/repos/e2e/oracle/branch_protections", get(|| async { Json(json!([])) }))
        .route(
            "/api/v1/repos/e2e/oracle/branches/main",
            get(|| async {
                Json(json!({"name":"main","protected":false,"required_approvals":0,
                    "enable_status_check":false,"status_check_contexts":[],"user_can_merge":true}))
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/commits",
            get(|| async {
                Json(json!([{"sha":HEAD,"html_url":"","author":user("e2e"),"committer":user("e2e"),
                    "parents":[{"sha":PARENT}],
                    "commit":{"message":"Add a test\n\nBody","author":{"name":"E","email":"e@forge.test","date":"2026-11-02T00:00:00Z"},
                        "committer":{"name":"E","email":"e@forge.test","date":"2026-11-02T00:00:00Z"}}}]))
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/issues/1/comments",
            get(|| async {
                Json(json!([
                    {"id":10,"user":user("other"),"body":"first","created_at":"2026-11-01T00:00:00Z",
                     "updated_at":"2026-11-03T00:00:00Z","html_url":"https://forge.test/e2e/oracle/pulls/1#issuecomment-10"},
                    {"id":11,"user":user("e2e"),"body":"second","created_at":"2026-11-02T00:00:00Z",
                     // Edited at 23:30 UTC, in a zone two hours ahead: later as text than the
                     // first comment's edit, and earlier in time.
                     "updated_at":"2026-11-03T01:30:00+02:00","html_url":"https://forge.test/e2e/oracle/pulls/1#issuecomment-11"}
                ]))
            }),
        )
        .route(
            "/api/v1/repos/e2e/oracle/pulls/1/reviews",
            get(|| async {
                Json(json!([{"id":7,"user":user("other"),"body":"ok","state":"APPROVED","commit_id":HEAD,
                    "submitted_at":"2026-11-02T00:00:00Z","updated_at":"2026-11-02T00:00:00Z",
                    "html_url":"https://forge.test/e2e/oracle/pulls/1#issuecomment-70","comments_count":0}]))
            }),
        )
        .route("/api/v1/repos/e2e/oracle/issues/1/timeline", get(|| async { Json(json!([])) }))
        .fallback(move |request: Request| {
            let missed = missed.clone();
            async move {
                missed.lock().unwrap().push(request.uri().to_string());
                (StatusCode::NOT_FOUND, Json(json!({"message":"not found"}))).into_response()
            }
        })
}

/// Runs `name` with T3 Code's variables for a Forgejo host, as it sends them, against a forge
/// whose job is in `state`.
async fn read(name: &str, state: &'static str, variables: Value) -> Value {
    execute(name, &document(name), state, variables).await
}

/// Runs a document of the test's own against the forge.
async fn run(query: &str) -> Value {
    execute("document", query, "pending", json!({})).await
}

async fn execute(name: &str, query: &str, state: &'static str, variables: Value) -> Value {
    let missed = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = forge(state, missed.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let session = Forgejo::new(base.parse().unwrap())
        .unwrap()
        .authenticate("token disposable")
        .await
        .unwrap();
    let response = fga_core::graphql::schema()
        .execute(
            async_graphql::Request::new(query)
                .variables(async_graphql::Variables::from_json(variables))
                .data(session)
                .data("https://forge.test".parse::<url::Url>().unwrap()),
        )
        .await;
    server.abort();
    assert!(
        response.errors.is_empty(),
        "{name}: {:?}; missed {:?}",
        response.errors,
        missed.lock().unwrap()
    );
    serde_json::to_value(response.data).unwrap()
}

fn pull() -> Value {
    json!({"owner":"e2e","name":"oracle","number":1})
}

fn with(extra: Value) -> Value {
    let mut variables = pull();
    variables
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    variables
}

/// `getPullRequestDetail`, which `pull_request_read` and each poll of a watch read.
fn core_variables() -> Value {
    with(
        json!({"headRef":"refs/pull/1/head","required":false,"requestedReviewers":100,"mergeQueue":false}),
    )
}

#[tokio::test]
async fn the_detail_read_carries_checks_with_their_runs() {
    let data = read("query-PullRequestCore", "pending", core_variables()).await;
    let repository = &data["repository"];
    assert_eq!(repository["autoMergeAllowed"], true);
    let pr = &repository["pullRequest"];
    assert_eq!(pr["viewerDidAuthor"], true);
    assert_eq!(pr["viewerCanUpdateBranch"], false);
    assert!(pr["headRef"]["id"].is_string(), "{}", pr["headRef"]);
    assert_eq!(pr["baseRef"]["compare"]["behindBy"], 2);
    assert_eq!(
        pr["reviewRequests"]["nodes"][0]["requestedReviewer"],
        json!({"__typename":"Team","slug":"core","combinedSlug":"e2e-org/core","teamName":"core","teamAvatarUrl":null})
    );
    let contexts = &pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"];
    let run = &contexts[0];
    assert_eq!(run["__typename"], "CheckRun");
    assert_eq!(run["name"], "test");
    assert_eq!(run["status"], "IN_PROGRESS");
    let workflow_run = &run["checkSuite"]["workflowRun"];
    assert_eq!(workflow_run["event"], "pull_request");
    assert_eq!(workflow_run["runNumber"], 7);
    assert_eq!(workflow_run["workflow"]["name"], "ci");
    assert!(workflow_run["workflow"]["id"].is_string());
    assert_eq!(contexts[1]["__typename"], "StatusContext");
    assert_eq!(contexts[1]["context"], "deploy");
}

/// A watch wakes when a check finishes: the detail read shows the job completed.
#[tokio::test]
async fn a_finished_job_reads_as_completed() {
    let data = read("query-PullRequestCore", "success", core_variables()).await;
    let run = &data["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]
        ["contexts"]["nodes"][0];
    assert_eq!(run["status"], "COMPLETED");
    assert_eq!(run["conclusion"], "SUCCESS");
}

#[tokio::test]
async fn the_check_pages_read_the_same_runs() {
    let data = read(
        "query-PullRequestCheckContexts",
        "pending",
        with(json!({"required":false,"after":null})),
    )
    .await;
    let pr = &data["repository"]["pullRequest"];
    assert_eq!(pr["headRefOid"], HEAD);
    let contexts = &pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"];
    assert_eq!(
        contexts["nodes"][0]["checkSuite"]["workflowRun"]["runNumber"],
        7
    );
    assert_eq!(contexts["pageInfo"]["hasNextPage"], false);
}

#[tokio::test]
async fn the_watch_fingerprint_counts_checks_and_edits() {
    let data = read(
        "query-PullRequestWatchFingerprint",
        "pending",
        with(json!({"mergeQueue":false})),
    )
    .await;
    let pr = &data["repository"]["pullRequest"];
    assert_eq!(pr["headRefOid"], HEAD);
    // Newest edit first, by time rather than text.
    assert_eq!(
        pr["comments"],
        json!({"totalCount":2,"nodes":[{"lastEditedAt":"2026-11-03T00:00:00Z"},{"lastEditedAt":"2026-11-03T01:30:00+02:00"}]})
    );
    assert_eq!(
        pr["reviews"],
        json!({"totalCount":1,"nodes":[{"lastEditedAt":null}]})
    );
    assert_eq!(counted(&data, "IN_PROGRESS"), 1);
    assert_eq!(counted(&data, "SUCCESS"), 0);
    let statuses = &pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["statusContextCountsByState"];
    assert!(
        statuses
            .as_array()
            .unwrap()
            .contains(&json!({"state":"SUCCESS","count":1}))
    );
}

/// The fingerprint's count of check runs in `state`.
fn counted(data: &Value, state: &str) -> i64 {
    data["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]
        ["contexts"]["checkRunCountsByState"]
        .as_array()
        .unwrap()
        .iter()
        .find(|count| count["state"] == state)
        .map_or(0, |count| count["count"].as_i64().unwrap())
}

/// The fingerprint changes when the job finishes, which sends the watch to read the detail.
#[tokio::test]
async fn the_fingerprint_changes_when_a_job_finishes() {
    let variables = || with(json!({"mergeQueue":false}));
    let running = read("query-PullRequestWatchFingerprint", "pending", variables()).await;
    let finished = read("query-PullRequestWatchFingerprint", "success", variables()).await;
    assert_ne!(running, finished);
    assert_eq!(counted(&finished, "IN_PROGRESS"), 0);
    assert_eq!(counted(&finished, "SUCCESS"), 1);
}

#[tokio::test]
async fn a_watch_on_github_reads_no_merge_queue_events() {
    let data = read(
        "query-PullRequestWatchFingerprint",
        "pending",
        with(json!({"mergeQueue":true})),
    )
    .await;
    assert_eq!(
        data["repository"]["pullRequest"]["mergeQueueRemovals"],
        json!({"filteredCount":0})
    );
}

#[tokio::test]
async fn the_activity_read_lists_commits_comments_and_reviews() {
    let data = read(
        "query-PullRequestActivity",
        "pending",
        with(json!({"head":true,"withComments":true,"commentsAfter":null,"withReviews":true,"reviewsAfter":null})),
    )
    .await;
    let pr = &data["repository"]["pullRequest"];
    let commit = &pr["commits"]["nodes"][0]["commit"];
    assert!(commit["id"].is_string());
    assert_eq!(commit["oid"], HEAD);
    assert_eq!(commit["parents"]["totalCount"], 1);
    assert_eq!(commit["messageHeadline"], "Add a test");
    assert_eq!(
        pr["comments"]["nodes"][0]["lastEditedAt"],
        "2026-11-03T00:00:00Z"
    );
    assert_eq!(
        pr["comments"]["nodes"][1]["lastEditedAt"],
        "2026-11-03T01:30:00+02:00"
    );
    assert_eq!(pr["reviews"]["nodes"][0]["state"], "APPROVED");
}

#[tokio::test]
async fn the_summary_reads_with_stacks_off() {
    let data = read(
        "query-PullRequestSummary",
        "pending",
        with(json!({"stacks":false})),
    )
    .await;
    let pr = &data["repository"]["pullRequest"];
    assert_eq!(pr["headRefName"], "feat");
    assert_eq!(
        pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"],
        "PENDING"
    );
}

/// Forgejo has no stacked pull requests or merge queue: asked anyway, both read as none.
#[tokio::test]
async fn stacks_read_as_none() {
    let data = read("query-PullRequestStackMembership", "pending", pull()).await;
    assert_eq!(
        data["repository"]["pullRequest"],
        json!({"stack":null,"stackEntry":null})
    );
}

#[tokio::test]
async fn the_branch_compare_reads_behind_by() {
    let data = read(
        "query-PullRequestActionState",
        "pending",
        with(json!({"headRef":"refs/pull/1/head"})),
    )
    .await;
    let pr = &data["repository"]["pullRequest"];
    assert_eq!(pr["isMergeQueueEnabled"], false);
    assert_eq!(pr["baseRef"]["compare"]["behindBy"], 2);
}

/// Only a pull request's own ref compares against its base: a ref the client chooses would let
/// one document make Forgejo walk any history, once per alias. The fake forge answers no other
/// count, so a request for one would fail the read.
#[tokio::test]
async fn only_the_pull_requests_own_ref_compares() {
    let document = |head: &str| {
        format!(
            "{{repository(owner:\"e2e\",name:\"oracle\"){{pullRequest(number:1){{baseRef{{compare(headRef:\"{head}\"){{behindBy}}}}}}}}}}"
        )
    };
    for (head, expected) in [
        ("refs/pull/1/head", json!({"behindBy":2})),
        ("refs/pull/2/head", Value::Null),
        ("refs/heads/feat", Value::Null),
        (HEAD, Value::Null),
    ] {
        let data = run(&document(head)).await;
        assert_eq!(
            data["repository"]["pullRequest"]["baseRef"]["compare"], expected,
            "{head}"
        );
    }
}
