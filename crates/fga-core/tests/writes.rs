use async_graphql::Request;
use axum::{Json, Router, extract::Query, http::StatusCode, response::IntoResponse, routing::get};
use fga_core::{forgejo::Forgejo, graphql, ids::node_id};
use serde_json::{Value, json};
use std::collections::HashMap;

fn user() -> Value {
    json!({"id":1,"login":"dirtydishes","full_name":"dirtydishes","avatar_url":""})
}
fn pull() -> Value {
    json!({"number":1,"title":"T","state":"open","merged":false,"draft":false,
    "html_url":"https://forge.test/o/r/pulls/1","head":{"ref":"head","label":"head","sha":"abc"},
    "base":{"ref":"main","label":"main","sha":"def"},"user":user(),"merged_at":null})
}
async fn execute(router: Router, query: String) -> async_graphql::Response {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
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
    response
}
fn router() -> Router {
    Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route("/api/v1/repos/o/r/pulls/1", get(|| async { Json(pull()) }))
}

#[tokio::test]
async fn merge_errors_keep_the_github_type_message_and_mutation_path() {
    for (status, body, kind, message) in [
        (
            StatusCode::CONFLICT,
            json!({"message":"head out of date"}),
            "UNPROCESSABLE",
            "Head branch was modified. Review and try the merge again.",
        ),
        (
            StatusCode::METHOD_NOT_ALLOWED,
            json!({}),
            "UNPROCESSABLE",
            "Pull Request has merge conflicts",
        ),
        (
            StatusCode::CONFLICT,
            json!({"message":"already scheduled"}),
            "UNPROCESSABLE",
            "Pull Request is not mergeable",
        ),
        (
            StatusCode::FORBIDDEN,
            json!({}),
            "FORBIDDEN",
            "Resource not accessible by token",
        ),
        (StatusCode::NOT_FOUND, json!({}), "NOT_FOUND", "Not Found"),
    ] {
        let app = router().route(
            "/api/v1/repos/o/r/pulls/1/merge",
            axum::routing::post(move |Json(input): Json<Value>| {
                let body = body.clone();
                async move {
                    assert_eq!(input["Do"], "squash");
                    assert_eq!(input["head_commit_id"], "abc");
                    assert_eq!(input["MergeTitleField"], "headline");
                    assert_eq!(input["MergeMessageField"], "body");
                    assert_eq!(input["delete_branch_after_merge"], false);
                    (status, Json(body))
                }
            }),
        );
        let response = execute(app, format!(r#"mutation {{ mergePullRequest(input: {{pullRequestId: "{}", mergeMethod:SQUASH, expectedHeadOid:"abc", commitHeadline:"headline", commitBody:"body"}}) {{clientMutationId}} }}"#, node_id("PullRequest", "o/r", 1))).await;
        assert_eq!(response.errors.len(), 1);
        let error = &response.errors[0];
        assert_eq!(error.message, message);
        assert_eq!(
            error.extensions.as_ref().unwrap().get("type"),
            Some(&async_graphql::Value::from(kind))
        );
        assert_eq!(
            error.path,
            vec![async_graphql::PathSegment::Field("mergePullRequest".into())]
        );
    }
}

#[tokio::test]
async fn disable_is_idempotent_but_missing_prs_still_fail() {
    let app = router().route(
        "/api/v1/repos/o/r/pulls/1/merge",
        axum::routing::delete(|| async { StatusCode::NOT_FOUND }),
    );
    let query = format!(
        r#"mutation {{disablePullRequestAutoMerge(input:{{pullRequestId:"{}"}}){{pullRequest{{number}}}}}}"#,
        node_id("PullRequest", "o/r", 1)
    );
    let response = execute(app, query).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["disablePullRequestAutoMerge"]["pullRequest"]["number"],
        1
    );
    let response = execute(router(), format!(r#"mutation {{disablePullRequestAutoMerge(input:{{pullRequestId:"{}"}}){{pullRequest{{number}}}}}}"#, node_id("PullRequest", "o/r", 999))).await;
    assert_eq!(
        response.errors[0].extensions.as_ref().unwrap().get("type"),
        Some(&async_graphql::Value::from("NOT_FOUND"))
    );
}

#[tokio::test]
async fn timeline_reads_every_page_and_uses_latest_event_even_in_the_same_second() {
    for (last_kind, scheduled) in [
        ("pull_cancel_scheduled_merge", false),
        ("pull_scheduled_merge", true),
    ] {
        let app = router()
            .route("/api/v1/repos/o/r", get(|| async { Json(json!({"id":1,"name":"r","full_name":"o/r","owner":{"id":1,"login":"o","avatar_url":""},"private":false,"html_url":"","default_branch":"main","fork":false,"archived":false,"default_merge_style":"squash"})) }))
            .route("/api/v1/repos/o/r/issues/1/timeline", get(move |Query(query): Query<HashMap<String,String>>| async move {
                let event = |id, kind| json!({"id":id,"type":kind,"created_at":"2026-11-01T00:00:00Z","user":user()});
                match query["page"].as_str() {
                    "1" => ([("link", "<http://untrusted.invalid>; rel=\"next\"")], Json(json!([event(1,"pull_scheduled_merge")]))).into_response(),
                    "2" => Json(json!([event(3,last_kind), event(2,"pull_scheduled_merge")])).into_response(),
                    _ => panic!("unexpected page"),
                }
            }));
        let response = execute(app, "{repository(owner:\"o\",name:\"r\"){pullRequest(number:1){autoMergeRequest{enabledAt enabledBy{login} mergeMethod commitHeadline commitBody authorEmail}}}}".into()).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        let auto = &data["repository"]["pullRequest"]["autoMergeRequest"];
        if scheduled {
            assert_eq!(
                *auto,
                json!({"enabledAt":"2026-11-01T00:00:00Z","enabledBy":{"login":"dirtydishes"},"mergeMethod":"SQUASH","commitHeadline":null,"commitBody":null,"authorEmail":null})
            );
        } else {
            assert!(auto.is_null());
        }
    }
}
