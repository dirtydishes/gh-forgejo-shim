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

// Forgejo v15 ctx.Error serializes obj as message, never the title argument.
fn api_error(message: &str) -> Value {
    json!({"message":message,"url":"https://forge.test/api/swagger"})
}

#[tokio::test]
async fn merge_errors_keep_the_github_type_message_and_mutation_path() {
    let conflicts = "Pull Request has merge conflicts";
    let not_mergeable = "Pull Request is not mergeable";
    let mut cases = vec![
        (
            StatusCode::METHOD_NOT_ALLOWED,
            api_error("User not allowed to merge PR"),
            "FORBIDDEN",
            "User does not have permission to merge this pull request",
        ),
        // pull.go:942 passes an empty obj for the "PR already merged" title.
        (
            StatusCode::METHOD_NOT_ALLOWED,
            api_error(""),
            "UNPROCESSABLE",
            "Pull Request is already merged",
        ),
        (
            StatusCode::METHOD_NOT_ALLOWED,
            api_error("Work in progress PRs cannot be merged"),
            "UNPROCESSABLE",
            "Pull Request is still a draft",
        ),
        (
            StatusCode::METHOD_NOT_ALLOWED,
            api_error("Please try again later"),
            "UNPROCESSABLE",
            conflicts,
        ),
        (
            StatusCode::METHOD_NOT_ALLOWED,
            api_error("not allowed to merge [reason: Does not have enough approvals]"),
            "UNPROCESSABLE",
            "Protected branch hook declined",
        ),
        (
            StatusCode::METHOD_NOT_ALLOWED,
            api_error("won't sign: nokey"),
            "UNPROCESSABLE",
            "Commits must have verified signatures.",
        ),
        // Conflict errors are JSON-encoded Go structs, not ctx.Error messages.
        (
            StatusCode::CONFLICT,
            json!({"Style":"merge","StdOut":"CONFLICT (content): Merge conflict in file.txt\n","StdErr":"","Err":{}}),
            "UNPROCESSABLE",
            conflicts,
        ),
        (
            StatusCode::CONFLICT,
            json!({"Style":"rebase","CommitSHA":"abc","StdOut":"","StdErr":"error: could not apply abc\n","Err":{}}),
            "UNPROCESSABLE",
            conflicts,
        ),
        (
            StatusCode::CONFLICT,
            json!({"Style":"merge","StdOut":"","StdErr":"fatal: refusing to merge unrelated histories\n","Err":{}}),
            "UNPROCESSABLE",
            conflicts,
        ),
        (
            StatusCode::CONFLICT,
            json!({"ID":1,"IssueID":2,"HeadRepoID":3,"BaseRepoID":3,"HeadBranch":"head","BaseBranch":"main"}),
            "UNPROCESSABLE",
            "Pull Request is already merged",
        ),
        (
            StatusCode::CONFLICT,
            api_error("merge push out of date"),
            "UNPROCESSABLE",
            "Base branch was modified. Review and try the merge again.",
        ),
        (
            StatusCode::CONFLICT,
            api_error("head out of date"),
            "UNPROCESSABLE",
            "Head branch was modified. Review and try the merge again.",
        ),
        (
            StatusCode::CONFLICT,
            api_error("PushRejected without remote error message"),
            "UNPROCESSABLE",
            "Pull Request merge failed",
        ),
        (
            StatusCode::CONFLICT,
            api_error("PushRejected with remote message: protected branch update failed"),
            "UNPROCESSABLE",
            "Pull Request merge failed",
        ),
        (
            StatusCode::CONFLICT,
            api_error(
                "pull request is already scheduled to auto merge when checks succeed [pull_id: 1]",
            ),
            "UNPROCESSABLE",
            "Auto merge is already enabled",
        ),
        (
            StatusCode::FORBIDDEN,
            api_error("Forbidden"),
            "FORBIDDEN",
            "Resource not accessible by token",
        ),
        (
            StatusCode::NOT_FOUND,
            api_error("Not Found"),
            "NOT_FOUND",
            "Not Found",
        ),
        // Unknown messages and incomplete structs keep the prior status fallback.
        (
            StatusCode::METHOD_NOT_ALLOWED,
            api_error("unknown merge failure"),
            "UNPROCESSABLE",
            conflicts,
        ),
        (
            StatusCode::METHOD_NOT_ALLOWED,
            json!({}),
            "UNPROCESSABLE",
            conflicts,
        ),
        (
            StatusCode::CONFLICT,
            api_error("unknown merge failure"),
            "UNPROCESSABLE",
            not_mergeable,
        ),
        (
            StatusCode::CONFLICT,
            api_error(""),
            "UNPROCESSABLE",
            not_mergeable,
        ),
        (
            StatusCode::CONFLICT,
            json!({"Style":"merge"}),
            "UNPROCESSABLE",
            not_mergeable,
        ),
        // Recognize messages only under their documented status code.
        (
            StatusCode::CONFLICT,
            api_error("User not allowed to merge PR"),
            "UNPROCESSABLE",
            not_mergeable,
        ),
        (
            StatusCode::METHOD_NOT_ALLOWED,
            api_error("head out of date"),
            "UNPROCESSABLE",
            conflicts,
        ),
    ];
    for (method, message) in [
        ("merge", "Merge commits are not allowed on this repository."),
        (
            "squash",
            "Squash merges are not allowed on this repository.",
        ),
        (
            "rebase",
            "Rebase merges are not allowed on this repository.",
        ),
    ] {
        cases.push((
            StatusCode::METHOD_NOT_ALLOWED,
            api_error(&format!(
                "{method} is not an allowed merge style for this repository"
            )),
            "UNPROCESSABLE",
            message,
        ));
    }
    let mut failures = Vec::new();
    for (status, body, kind, message) in cases {
        for mutation in ["mergePullRequest", "enablePullRequestAutoMerge"] {
            let description = format!("{mutation}: {status} {body}");
            let body = body.clone();
            let method = body["message"]
                .as_str()
                .and_then(|m| m.strip_suffix(" is not an allowed merge style for this repository"))
                .unwrap_or("squash")
                .to_owned();
            let graphql_method = method.to_uppercase();
            let app = router().route(
                "/api/v1/repos/o/r/pulls/1/merge",
                axum::routing::post(move |Json(input): Json<Value>| {
                    let body = body.clone();
                    let method = method.clone();
                    async move {
                        assert_eq!(input["Do"], method);
                        assert_eq!(input["head_commit_id"], "abc");
                        assert_eq!(input["MergeTitleField"], "headline");
                        assert_eq!(input["MergeMessageField"], "body");
                        assert_eq!(input["delete_branch_after_merge"], false);
                        assert_eq!(
                            input["merge_when_checks_succeed"],
                            mutation == "enablePullRequestAutoMerge"
                        );
                        (status, Json(body))
                    }
                }),
            );
            let response = execute(app, format!(r#"mutation {{ {mutation}(input: {{pullRequestId: "{}", mergeMethod:{graphql_method}, expectedHeadOid:"abc", commitHeadline:"headline", commitBody:"body"}}) {{clientMutationId}} }}"#, node_id("PullRequest", "o/r", 1))).await;
            assert_eq!(response.errors.len(), 1, "{description}");
            let error = &response.errors[0];
            let actual_kind = error.extensions.as_ref().unwrap().get("type");
            let expected_kind = async_graphql::Value::from(kind);
            let expected_path = vec![async_graphql::PathSegment::Field(mutation.into())];
            if error.message != message
                || actual_kind != Some(&expected_kind)
                || error.path != expected_path
            {
                failures.push(format!(
                    "{description}: expected {kind} {expected_path:?} {message:?}, got {error:?}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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
