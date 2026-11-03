//! The Issue fields gh 2.103.3's `issue list` and `issue view --json` ask beyond the gateway's first
//! set: `isPinned` and `closedByPullRequestsReferences`. The selection is gh's own
//! (`issueClosedByPullRequestsReferences` in `api/query_builder.go`).
use async_graphql::Request;
use axum::{Json, Router, routing::get};
use fga_core::{forgejo::Forgejo, graphql, ids::node_id};
use serde_json::{Value, json};

const GH_CLOSED_BY: &str = "closedByPullRequestsReferences(first: 100){nodes{id,number,url,repository{id,name,owner{id,login}}}pageInfo{hasNextPage,endCursor}}";

fn user() -> Value {
    json!({"id":1,"login":"viewer","full_name":"Viewer","avatar_url":"https://forge.test/avatar"})
}
fn issue(number: i32, pin_order: i32) -> Value {
    json!({"id":number,"number":number,"title":"T","body":"","state":"open",
        "html_url":format!("https://forge.test/o/r/issues/{number}"),"user":user(),
        "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","labels":[],
        "pin_order":pin_order})
}
fn pull(owner: &str, number: i32, state: &str, merged: bool) -> Value {
    json!({"number":number,"title":"fix","state":state,"merged":merged,"draft":false,
        "html_url":format!("https://forge.test/{owner}/r/pulls/{number}"),
        "head":{"label":"topic","ref":"topic","sha":"abc","repo":null},
        "base":{"label":"main","ref":"main","sha":"def","repo":null},"user":user(),"merged_at":null})
}
fn repository(owner: &str) -> Value {
    json!({"id":1,"name":"r","full_name":format!("{owner}/r"),"private":false,
        "owner":{"id":1,"login":owner,"avatar_url":""},"html_url":format!("https://forge.test/{owner}/r"),
        "default_branch":"main","fork":false,"archived":false,
        "permissions":{"admin":false,"push":true,"pull":true}})
}
/// A cross-reference on the timeline, made from PR (or issue, when `pull` is `None`) `number` in
/// `owner/r`, which is `state` and, for a PR, merged or not.
fn reference(
    id: i64,
    kind: &str,
    action: &str,
    owner: &str,
    number: i32,
    state: &str,
    pull: Option<bool>,
) -> Value {
    json!({"id":id,"type":kind,"created_at":"2026-01-02T00:00:00Z","user":user(),"ref_action":action,
        "ref_issue":{"number":number,"state":state,
            "pull_request":pull.map_or(Value::Null, |merged| json!({"merged":merged})),
            "repository":{"name":"r","owner":owner}}})
}

#[tokio::test]
async fn issue_pins_and_closing_pull_requests_come_from_forgejo() {
    let open = Some(false);
    let timeline = json!([
        {"id":1,"type":"comment","created_at":"2026-01-01T00:00:00Z","user":user()},
        // An open PR whose description says `closes #4`.
        reference(2, "pull_ref", "closes", "o", 10, "open", open),
        // A closed, unmerged PR: GitHub's default leaves it out.
        reference(3, "pull_ref", "closes", "o", 11, "closed", open),
        // The keyword was edited away.
        reference(4, "pull_ref", "neutered", "o", 12, "open", open),
        // A mention, not a closing reference.
        reference(5, "pull_ref", "none", "o", 13, "open", open),
        // `closes #4` in a comment on an open PR: Forgejo closes #4 when it merges.
        reference(6, "comment_ref", "closes", "o", 14, "open", open),
        // `closes` in the description, then `reopens` in a later comment: the newest decides.
        reference(7, "pull_ref", "closes", "o", 17, "open", open),
        reference(8, "comment_ref", "reopens", "o", 17, "open", open),
        // An open PR in another repository.
        reference(9, "pull_ref", "closes", "f", 20, "open", open),
        // A PR the token can't read: left out, not an error.
        reference(10, "pull_ref", "closes", "o", 15, "open", open),
        // An issue, not a PR.
        reference(11, "issue_ref", "closes", "o", 16, "open", None),
        // A merged PR: GitHub lists it by default.
        reference(12, "pull_ref", "closes", "o", 18, "closed", Some(true)),
        // PR 10 again, from a comment: listed once, where it first appeared.
        reference(13, "comment_ref", "closes", "o", 10, "open", open),
    ]);
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route("/api/v1/repos/o/r", get(|| async { Json(repository("o")) }))
        .route("/api/v1/repos/f/r", get(|| async { Json(repository("f")) }))
        .route(
            "/api/v1/repos/o/r/issues/4",
            get(|| async { Json(issue(4, 1)) }),
        )
        .route(
            "/api/v1/repos/o/r/issues/5",
            get(|| async { Json(issue(5, 0)) }),
        )
        .route(
            "/api/v1/repos/o/r/issues/4/timeline",
            get(move || async move { Json(timeline) }),
        )
        .route(
            "/api/v1/repos/o/r/issues/5/timeline",
            get(|| async { Json(Value::Null) }),
        )
        .route(
            "/api/v1/repos/o/r/pulls/10",
            get(|| async { Json(pull("o", 10, "open", false)) }),
        )
        .route(
            "/api/v1/repos/o/r/pulls/14",
            get(|| async { Json(pull("o", 14, "open", false)) }),
        )
        .route(
            "/api/v1/repos/o/r/pulls/17",
            get(|| async { Json(pull("o", 17, "open", false)) }),
        )
        .route(
            "/api/v1/repos/o/r/pulls/18",
            get(|| async { Json(pull("o", 18, "closed", true)) }),
        )
        .route(
            "/api/v1/repos/f/r/pulls/20",
            get(|| async { Json(pull("f", 20, "open", false)) }),
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
    let query = format!(
        r#"{{repository(owner:"o",name:"r"){{pinned:issue(number:4){{isPinned {GH_CLOSED_BY}}} plain:issue(number:5){{isPinned {GH_CLOSED_BY}}}}}}}"#
    );
    let response = graphql::schema()
        .execute(Request::new(query).data(session))
        .await;
    task.abort();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    // Whole nodes, as gh selects them: the PR, and its base repository and that repository's owner.
    let node = |owner: &str, number: i32| {
        json!({"id":node_id("PullRequest", &format!("{owner}/r"), number),
            "number":number,"url":format!("https://forge.test/{owner}/r/pull/{number}"),
            "repository":{"id":node_id("Repository", &format!("{owner}/r"), 1),
                "name":"r","owner":{"id":node_id("User", "", 1),"login":owner}}})
    };
    let data = response.data.into_json().unwrap();
    let pinned = &data["repository"]["pinned"];
    assert_eq!(pinned["isPinned"], json!(true));
    let closed_by = &pinned["closedByPullRequestsReferences"];
    assert_eq!(
        closed_by["nodes"],
        json!([node("o", 10), node("o", 14), node("f", 20), node("o", 18)])
    );
    assert_eq!(closed_by["pageInfo"]["hasNextPage"], json!(false));
    let plain = &data["repository"]["plain"];
    assert_eq!(plain["isPinned"], json!(false));
    assert_eq!(plain["closedByPullRequestsReferences"]["nodes"], json!([]));
}
