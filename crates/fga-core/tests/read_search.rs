use async_graphql::Request;
use axum::{Json, Router, extract::Query, response::IntoResponse, routing::get};
use fga_core::{forgejo::Forgejo, graphql};
use serde_json::{Value, json};
use std::collections::HashMap;

fn user() -> Value {
    json!({"id":1,"login":"viewer","full_name":"Viewer","avatar_url":"https://forge.test/avatar"})
}
fn label() -> Value {
    json!({"id":1,"name":"needs work","description":null,"color":"aa0000"})
}
fn issue(number: i32, title: &str) -> Value {
    json!({"id":number,"number":number,"title":title,"body":"needle in body","state":"closed","html_url":"https://forge.test/o/r/issues/2","user":user(),"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","labels":[label()]})
}
fn pull(number: i32, merged: bool) -> Value {
    json!({"number":number,"title":"needle","state":"closed","merged":merged,"draft":false,"html_url":"https://forge.test/o/r/pulls/2","head":{"label":"viewer:topic","ref":"topic","sha":"abc","repo":{"owner":user()}},"base":{"label":"main","ref":"main","sha":"def","repo":{"owner":user()}},"user":user(),"merged_at":null})
}
async fn session(upstream: Router) -> (fga_core::forgejo::Session, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    (
        Forgejo::new(url)
            .unwrap()
            .authenticate("token disposable")
            .await
            .unwrap(),
        task,
    )
}
#[tokio::test]
async fn applies_search_filters_across_upstream_pages_before_github_pagination() {
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r/issues",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                assert_eq!(q["type"], "pulls");
                assert_eq!(q["state"], "all");
                assert_eq!(q["created_by"], "viewer");
                assert_eq!(q["assigned_by"], "viewer");
                assert_eq!(q["labels"], "needs work");
                assert_eq!(q["q"], "+\"needle\"");
                assert_eq!(q["sort"], "latest");
                match q["page"].as_str() {
                    "1" => (
                        [("link", "<http://untrusted.invalid/steal>; rel=\"next\"")],
                        Json(json!([issue(4, "does not match title")])),
                    )
                        .into_response(),
                    "2" => Json(json!([issue(2, "needle in title")])).into_response(),
                    _ => panic!("unexpected page"),
                }
            }),
        )
        .route(
            "/api/v1/repos/o/r/pulls",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                assert_eq!(q["poster"], "viewer");
                Json(json!([pull(4, false), pull(2, true)]))
            }),
        );
    let (session, task) = session(upstream).await;
    let query = "repo:o/r type:pr author:@me assignee:@me label:\"needs work\" head:topic base:main ( needle in:title ) sort:created-desc";
    let request = format!(
        "{{ search(type:ISSUE_ADVANCED,query:{},first:1) {{ issueCount nodes {{ ... on PullRequest {{ number }} }} pageInfo {{ hasNextPage }} }} }}",
        serde_json::to_string(query).unwrap()
    );
    let response = graphql::schema()
        .execute(Request::new(request).data(session.clone()))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"search":{"issueCount":1,"nodes":[{"number":2}],"pageInfo":{"hasNextPage":false}}})
    );
    let response = graphql::schema()
        .execute(
            Request::new(
                "{search(type:ISSUE,query:\"repo:o/r type:pr unsupported:x\"){issueCount}}",
            )
            .data(session),
        )
        .await;
    assert_eq!(response.errors.len(), 1);
    assert_eq!(
        response.errors[0].extensions.as_ref().unwrap().get("type"),
        Some(&async_graphql::Value::from("UNPROCESSABLE"))
    );
    task.abort();
}

#[tokio::test]
async fn search_closed_includes_merged_while_repository_closed_excludes_them() {
    let upstream=Router::new().route("/api/v1/user",get(||async {Json(user())}))
        .route("/api/v1/repos/o/r",get(||async {Json(json!({"id":1,"name":"r","full_name":"o/r","owner":{"id":1,"login":"o","avatar_url":"https://forge.test/avatar"},"private":false,"html_url":"https://forge.test/o/r","default_branch":"main","fork":false,"archived":false}))}))
        .route("/api/v1/repos/o/r/issues",get(||async {Json(json!([issue(4,"needle"),issue(2,"needle")]))}))
        .route("/api/v1/repos/o/r/pulls",get(||async {Json(json!([pull(4,false),pull(2,true)]))}));
    let (session, task) = session(upstream).await;
    let response=graphql::schema().execute(Request::new("{search(type:ISSUE,query:\"repo:o/r type:pr state:closed\"){nodes{...on PullRequest{number}}} repository(owner:\"o\",name:\"r\"){pullRequests(states:[CLOSED],first:10){nodes{number}}}}").data(session)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"search":{"nodes":[{"number":4},{"number":2}]},"repository":{"pullRequests":{"nodes":[{"number":4}]}}})
    );
    task.abort();
}

/// PR web URLs, and URLs under a PR, take GitHub's `/pull/{n}` layout; other web URLs keep
/// Forgejo's (MAPPING §8).
#[tokio::test]
async fn pull_request_urls_use_githubs_layout() {
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route("/api/v1/repos/o/r", get(|| async { Json(json!({"id":1,"name":"r","full_name":"o/r","owner":{"id":1,"login":"o","avatar_url":"https://forge.test/avatar"},"private":false,"html_url":"https://forge.test/o/r","default_branch":"main","fork":false,"archived":false})) }))
        .route("/api/v1/repos/o/r/pulls/2", get(|| async { Json(pull(2, false)) }))
        .route(
            "/api/v1/repos/o/r/issues/2/comments",
            get(|| async {
                Json(json!([{"id":7,"user":user(),"body":"hi","created_at":"2026-01-01T00:00:00Z",
                    "html_url":"https://forge.test/o/r/pulls/2#issuecomment-7"}]))
            }),
        )
        // Forgejo's issue routes answer for pull requests too, with the PR's URL.
        .route(
            "/api/v1/repos/o/r/issues/2",
            get(|| async {
                let mut pr = issue(2, "needle");
                pr["html_url"] = json!("https://forge.test/o/r/pulls/2");
                Json(pr)
            }),
        )
        .route("/api/v1/repos/o/r/issues/4", get(|| async { Json(issue(4, "plain")) }));
    let (session, task) = session(upstream).await;
    let response = graphql::schema()
        .execute(
            Request::new(
                r#"{repository(owner:"o",name:"r"){url
                    pullRequest(number:2){url comments(first:1){nodes{url}}}
                    asIssue:issue(number:2){url} issue(number:4){url}}}"#,
            )
            .data(session),
        )
        .await;
    task.abort();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"repository":{"url":"https://forge.test/o/r",
            "pullRequest":{"url":"https://forge.test/o/r/pull/2",
                "comments":{"nodes":[{"url":"https://forge.test/o/r/pull/2#issuecomment-7"}]}},
            "asIssue":{"url":"https://forge.test/o/r/pull/2"},
            "issue":{"url":"https://forge.test/o/r/issues/2"}}})
    );
}

/// gh's `--json projectCards` selection on `pr list` and `issue list`. Forgejo has no classic
/// projects, so both answer an empty connection, as the built-in path's `[]` did.
#[tokio::test]
async fn project_cards_are_an_empty_connection() {
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r",
            get(|| async {
                Json(json!({"id":1,"name":"r","full_name":"o/r","private":false,
                    "owner":{"id":1,"login":"o","avatar_url":"https://forge.test/avatar"},
                    "html_url":"https://forge.test/o/r","default_branch":"main","fork":false,
                    "archived":false,"permissions":{"admin":false,"push":false,"pull":true}}))
            }),
        )
        .route(
            "/api/v1/repos/o/r/pulls/2",
            get(|| async { Json(pull(2, false)) }),
        )
        .route(
            "/api/v1/repos/o/r/issues/4",
            get(|| async { Json(issue(4, "plain")) }),
        );
    let (session, task) = session(upstream).await;
    let cards = "projectCards(first:100){nodes{project{name}column{name}},totalCount}";
    let response = graphql::schema()
        .execute(
            Request::new(format!(
                r#"{{repository(owner:"o",name:"r"){{
                    pullRequest(number:2){{{cards}}} issue(number:4){{{cards}}}}}}}"#
            ))
            .data(session),
        )
        .await;
    task.abort();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let empty = json!({"projectCards":{"nodes":[],"totalCount":0}});
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"repository":{"pullRequest":empty,"issue":empty}})
    );
}

/// gh pr checkout fetches `refs/heads/{headRefName}` from the head repository, or the PR's own ref
/// `refs/pull/N/head` from the base when there's none. The gateway reports a head repository only
/// while the head branch exists and its tip is the PR's head, so gh never checks out a branch that
/// isn't there, was recreated, or has moved since (a closed PR; a PR reopened through the API). The
/// owner stays as GitHub reports it; an open PR's `isCrossRepository` is true unless the branch is the
/// PR's head, so `gh pr close -d`/`merge -d` never delete a branch that isn't it. gh deletes no remote
/// branch for a closed or merged PR, so one from the same repository is not cross-repository, as on
/// GitHub, and `gh pr list` shows its branch without an owner. `headRefName` is always Forgejo's own
/// name, so no made-up name can equal a real branch's.
#[tokio::test]
async fn a_head_branch_that_is_gone_has_no_head_repository() {
    let repo = |owner: &str| json!({"name":"r","owner":{"id":1,"login":owner,"avatar_url":""}});
    // (label, ref, head repository owner, head SHA, state, merged, the PR ref's SHA if readable)
    type Case = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        bool,
        Option<&'static str>,
    );
    let cases: [Case; 15] = [
        ("feature", "feature", "o", "aaa", "open", false, Some("aaa")),
        (
            "feature",
            "refs/pull/2/head",
            "o",
            "aaa",
            "open",
            false,
            Some("aaa"),
        ),
        (
            "",
            "refs/pull/3/head",
            "o",
            "aaa",
            "open",
            false,
            Some("aaa"),
        ),
        ("feature", "feature", "f", "aaa", "open", false, Some("aaa")),
        (
            "feature",
            "refs/pull/5/head",
            "f",
            "aaa",
            "open",
            false,
            Some("aaa"),
        ),
        (
            "refs/pull/6/head",
            "refs/pull/6/head",
            "o",
            "aaa",
            "open",
            false,
            Some("aaa"),
        ),
        // Merged with its branch still at the PR's head: the branch can still move before gh
        // fetches it, and the PR's own ref can't, so only the ref is offered.
        (
            "feature",
            "feature",
            "o",
            "aaa",
            "closed",
            true,
            Some("aaa"),
        ),
        // Closed, then the branch recreated at another commit.
        (
            "feature",
            "feature",
            "o",
            "ccc",
            "closed",
            false,
            Some("aaa"),
        ),
        // Reopened through the API after the branch moved: the PR's ref stayed where it was.
        ("feature", "feature", "o", "ccc", "open", false, Some("aaa")),
        (
            "refs/refs/topic",
            "refs/refs/topic",
            "o",
            "aaa",
            "open",
            false,
            Some("aaa"),
        ),
        // The PR's ref can't be read: unknown counts as moved.
        ("feature", "feature", "o", "aaa", "open", false, None),
        // A real branch named like a ref keeps its name, so it can't be taken for `topic`, but
        // counts as gone: gh fetches the PR's own ref.
        (
            "refs/topic",
            "refs/topic",
            "o",
            "aaa",
            "open",
            false,
            Some("aaa"),
        ),
        ("topic", "topic", "o", "aaa", "open", false, Some("aaa")),
        // r5#3: a real branch named like an AGit PR's ref without `refs/`. No name is made up, so
        // it can't be taken for AGit PR 3.
        (
            "pull/3/head",
            "pull/3/head",
            "o",
            "aaa",
            "open",
            false,
            Some("aaa"),
        ),
        // r5#2, the GitHub-equivalent residue: a closed PR from `refs/topic` keeps that name and
        // gets no head repository, as on GitHub once its fork is deleted.
        (
            "refs/topic",
            "refs/topic",
            "o",
            "aaa",
            "closed",
            false,
            Some("aaa"),
        ),
    ];
    let pulls: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(i, (label, reference, owner, sha, state, merged, _))| {
            let mut source = pull(i as i32 + 1, *merged);
            source["state"] = json!(state);
            source["head"] = json!({"label":label,"ref":reference,"sha":sha,"repo":repo(owner)});
            source
        })
        .collect();
    let repository = |owner: &'static str| {
        get(move || async move {
            Json(
                json!({"id":1,"name":"r","full_name":format!("{owner}/r"),"private":false,
                "owner":{"id":1,"login":owner,"avatar_url":"https://forge.test/avatar"},
                "html_url":format!("https://forge.test/{owner}/r"),"default_branch":"main",
                "fork":owner != "o","archived":false,
                "permissions":{"admin":false,"push":true,"pull":true}}),
            )
        })
    };
    let listed = Value::Array(pulls.clone());
    let mut upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route("/api/v1/repos/o/r", repository("o"))
        .route("/api/v1/repos/f/r", repository("f"))
        .route(
            "/api/v1/repos/o/r/pulls",
            get(move || async move { Json(listed) }),
        );
    for (number, (source, case)) in (1..).zip(pulls.into_iter().zip(cases)) {
        upstream = upstream.route(
            &format!("/api/v1/repos/o/r/pulls/{number}"),
            get(move || async move { Json(source) }),
        );
        if let Some(sha) = case.6 {
            let reference = format!("refs/pull/{number}/head");
            upstream = upstream.route(
                &format!("/api/v1/repos/o/r/git/refs/pull/{number}/head"),
                get(move || async move {
                    Json(json!({"ref":reference,"object":{"type":"commit","sha":sha}}))
                }),
            );
        }
    }
    let (session, task) = session(upstream).await;
    let fields = "headRefName isCrossRepository headRepository{name} headRepositoryOwner{login}";
    let query = (1..=cases.len())
        .map(|n| format!("p{n}:pullRequest(number:{n}){{{fields}}}"))
        .chain([
            "agit:pullRequests(headRefName:\"refs/pull/3/head\",first:5){nodes{number}}".into(),
            "pull3:pullRequests(headRefName:\"pull/3/head\",first:5){nodes{number}}".into(),
            "topic:pullRequests(headRefName:\"topic\",first:5){nodes{number}}".into(),
        ])
        .collect::<Vec<_>>()
        .join(" ");
    let response = graphql::schema()
        .execute(
            Request::new(format!(r#"{{repository(owner:"o",name:"r"){{{query}}}}}"#)).data(session),
        )
        .await;
    task.abort();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let head = |name: &str, cross: bool, repository: Value, owner: &str| {
        json!({"headRefName":name,"isCrossRepository":cross,"headRepository":repository,
            "headRepositoryOwner":{"login":owner}})
    };
    let r = json!({"name":"r"});
    assert_eq!(
        response.data.into_json().unwrap()["repository"],
        json!({
            "p1":head("feature", false, r.clone(), "o"),
            "p2":head("feature", true, Value::Null, "o"),
            // An AGit PR reports Forgejo's own ref name.
            "p3":head("refs/pull/3/head", true, Value::Null, "o"),
            "p4":head("feature", true, r.clone(), "f"),
            "p5":head("feature", true, Value::Null, "f"),
            // A known limit: a gone branch named `refs/…` keeps its name.
            "p6":head("refs/pull/6/head", true, Value::Null, "o"),
            "p7":head("feature", false, Value::Null, "o"),
            "p8":head("feature", false, Value::Null, "o"),
            "p9":head("feature", true, Value::Null, "o"),
            "p10":head("refs/refs/topic", true, Value::Null, "o"),
            "p11":head("feature", true, Value::Null, "o"),
            // gh pr list --head takes the name gh was shown.
            "p12":head("refs/topic", true, Value::Null, "o"),
            "p13":head("topic", false, r.clone(), "o"),
            "p14":head("pull/3/head", false, r, "o"),
            "p15":head("refs/topic", false, Value::Null, "o"),
            "agit":{"nodes":[{"number":3}]},
            "pull3":{"nodes":[{"number":14}]},
            "topic":{"nodes":[{"number":13}]},
        })
    );
}

/// ChatGPT desktop finds a branch's PR with `gh pr list --head <branch> --author @me --state all
/// --json number,url,state,headRefName`, which gh 2.103.3 sends as a search: its query text is the
/// oracle capture's, and the variables are those gh sent the gateway. A token without issue scope
/// can't read Forgejo's issue list, which such a PR-only query doesn't need: it is answered from
/// the pull list alone, newest first, as search orders by default.
#[tokio::test]
async fn a_pr_only_search_needs_no_issue_scope() {
    let captured: Value = serde_json::from_str(include_str!(
        "../../../fixtures/oracle/list-me-head.requests.json"
    ))
    .unwrap();
    let query = captured[0]["query"].as_str().unwrap();
    assert!(query.contains("query PullRequestSearch"), "{query}");
    let variables =
        json!({"limit":30,"q":"author:@me head:topic repo:o/r type:pr","type":"ISSUE_ADVANCED"});
    let mine = |number: i32, head: &str, merged: bool| {
        let mut pr = pull(number, merged);
        pr["head"]["ref"] = json!(head);
        pr["head"]["label"] = json!(head);
        pr["html_url"] = json!(format!("https://forge.test/o/r/pulls/{number}"));
        if !merged {
            pr["state"] = json!("open");
        }
        pr
    };
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r/issues",
            get(|| async {
                (
                    axum::http::StatusCode::FORBIDDEN,
                    Json(json!({"message":"token does not have at least one of required scope(s): [read:issue]"})),
                )
            }),
        )
        .route(
            "/api/v1/repos/o/r/pulls",
            get(move |Query(q): Query<HashMap<String, String>>| async move {
                assert_eq!((q["state"].as_str(), q["poster"].as_str()), ("all", "viewer"));
                // Someone else's PR from the same branch, as a Forgejo ignoring `poster` would send.
                let mut theirs = mine(10, "topic", false);
                theirs["user"]["login"] = json!("someone");
                let listed = if q.get("page").is_none_or(|p| p == "1") {
                    json!([theirs, mine(9, "topic", false), mine(8, "other", false), mine(7, "topic", true)])
                } else {
                    json!([])
                };
                Json(listed)
            }),
        );
    let (session, task) = session(upstream).await;
    let response = graphql::schema()
        .execute(
            Request::new(query)
                .variables(async_graphql::Variables::from_json(variables))
                .data(session),
        )
        .await;
    task.abort();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["search"],
        json!({"issueCount":2,"nodes":[
            {"number":9,"url":"https://forge.test/o/r/pull/9","state":"OPEN","headRefName":"topic"},
            {"number":7,"url":"https://forge.test/o/r/pull/7","state":"MERGED","headRefName":"topic"}],
            "pageInfo":{"hasNextPage":false,"endCursor":"ZmdhOmN1cnNvcjox"}})
    );
}
