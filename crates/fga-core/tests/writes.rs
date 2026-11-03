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
fn repo() -> Value {
    json!({"id":1,"name":"r","full_name":"o/r","owner":{"id":1,"login":"o","avatar_url":""},
    "private":false,"html_url":"","default_branch":"main","fork":false,"archived":false,
    "default_merge_style":"squash","allow_merge_commits":true,"allow_squash_merge":true,"allow_rebase":true})
}
fn router() -> Router {
    Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route("/api/v1/repos/o/r", get(|| async { Json(repo()) }))
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
                        if mutation == "mergePullRequest" {
                            assert_eq!(input["head_commit_id"], "abc");
                            assert_eq!(input["merge_when_checks_succeed"], false);
                        } else {
                            assert!(input.get("head_commit_id").is_none());
                        }
                        assert_eq!(input["MergeTitleField"], "headline");
                        assert_eq!(input["MergeMessageField"], "body");
                        assert_eq!(input["delete_branch_after_merge"], false);
                        (status, Json(body))
                    }
                }),
            );
            // Auto-merge refuses an expected head before writing.
            let head = if mutation == "mergePullRequest" {
                r#"expectedHeadOid:"abc","#
            } else {
                ""
            };
            let response = execute(app, format!(r#"mutation {{ {mutation}(input: {{pullRequestId: "{}", mergeMethod:{graphql_method}, {head} commitHeadline:"headline", commitBody:"body"}}) {{clientMutationId}} }}"#, node_id("PullRequest", "o/r", 1))).await;
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

/// Forgejo refuses a merge with "Please try again later" while it is still checking a new PR.
#[tokio::test]
async fn merge_retries_while_forgejo_is_still_checking() {
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = router().route(
        "/api/v1/repos/o/r/pulls/1/merge",
        axum::routing::post({
            let attempts = attempts.clone();
            move || async move {
                match attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                    0 | 1 => (
                        StatusCode::METHOD_NOT_ALLOWED,
                        Json(api_error("Please try again later")),
                    ),
                    _ => (StatusCode::OK, Json(Value::Null)),
                }
            }
        }),
    );
    let query = format!(
        r#"mutation {{mergePullRequest(input:{{pullRequestId:"{}", mergeMethod:SQUASH}}){{pullRequest{{number}}}}}}"#,
        node_id("PullRequest", "o/r", 1)
    );
    let response = execute(app, query).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
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
async fn timeline_is_read_whole_and_uses_latest_event_even_in_the_same_second() {
    let event =
        |id, kind| json!({"id":id,"type":kind,"created_at":"2026-11-01T00:00:00Z","user":user()});
    // Forgejo filters the timeline after paging it, so it must be read in one request; it
    // returns `null` for an empty timeline.
    for (events, scheduled) in [
        (
            json!([
                event(1, "pull_scheduled_merge"),
                event(3, "pull_cancel_scheduled_merge"),
                event(2, "pull_scheduled_merge")
            ]),
            false,
        ),
        (
            json!([
                event(1, "pull_cancel_scheduled_merge"),
                event(3, "pull_scheduled_merge"),
                event(2, "pull_cancel_scheduled_merge")
            ]),
            true,
        ),
        (Value::Null, false),
    ] {
        let app = router().route(
            "/api/v1/repos/o/r/issues/1/timeline",
            get(
                move |Query(query): Query<HashMap<String, String>>| async move {
                    assert!(!query.contains_key("page"), "timeline must not be paged");
                    Json(events)
                },
            ),
        );
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

/// Forgejo schedules any style and its worker later drops a disallowed one, so refuse it first.
#[tokio::test]
async fn disallowed_merge_styles_are_refused_before_writing() {
    let app = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r",
            get(|| async {
                let mut repo = repo();
                repo["allow_merge_commits"] = json!(false);
                Json(repo)
            }),
        )
        .route(
            "/api/v1/repos/o/r/pulls/1/merge",
            axum::routing::post(|| async { panic!("must not write") as StatusCode }),
        );
    let query = format!(
        r#"mutation {{enablePullRequestAutoMerge(input:{{pullRequestId:"{}", mergeMethod:MERGE}}){{pullRequest{{number}}}}}}"#,
        node_id("PullRequest", "o/r", 1)
    );
    let response = execute(app, query).await;
    assert_eq!(
        response.errors[0].message,
        "Merge commits are not allowed on this repository."
    );
}

/// A schedule stores no expected head, so auto-merge refuses one before writing.
#[tokio::test]
async fn auto_merge_refuses_an_expected_head() {
    let app = router().route(
        "/api/v1/repos/o/r/pulls/1/merge",
        axum::routing::post(|| async { panic!("must not write") as StatusCode }),
    );
    let query = format!(
        r#"mutation {{enablePullRequestAutoMerge(input:{{pullRequestId:"{}", mergeMethod:SQUASH, expectedHeadOid:"abc"}}){{pullRequest{{number}}}}}}"#,
        node_id("PullRequest", "o/r", 1)
    );
    let response = execute(app, query).await;
    assert_eq!(
        response.errors[0].message,
        "Forgejo can't tie an auto-merge to a head commit; merge without auto-merge to check the head"
    );
}

#[tokio::test]
async fn forgejo_state_refusals_keep_their_reason() {
    let app = router().route(
        "/api/v1/repos/o/r/pulls/1",
        axum::routing::patch(|| async {
            (
                StatusCode::PRECONDITION_FAILED,
                Json(api_error(
                    "cannot close this pull request because it still has open dependencies",
                )),
            )
        }),
    );
    let query = format!(
        r#"mutation {{closePullRequest(input:{{pullRequestId:"{}"}}){{pullRequest{{number}}}}}}"#,
        node_id("PullRequest", "o/r", 1)
    );
    let response = execute(app, query).await;
    assert_eq!(
        response.errors[0].message,
        "Pull Request still has open dependencies"
    );
}

/// Ready removes only the draft marker; a title still opening with a prefix would stay a draft.
#[tokio::test]
async fn ready_removes_one_prefix_and_refuses_an_unrepresentable_title() {
    let app = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r/pulls/1",
            get(|| async {
                let mut pull = pull();
                pull["title"] = json!("WIP: [WIP] keep this");
                pull["draft"] = json!(true);
                Json(pull)
            })
            .patch(|| async { panic!("must not write") as StatusCode }),
        );
    let query = format!(
        r#"mutation {{markPullRequestReadyForReview(input:{{pullRequestId:"{}"}}){{pullRequest{{number}}}}}}"#,
        node_id("PullRequest", "o/r", 1)
    );
    let response = execute(app, query).await;
    assert_eq!(
        response.errors[0].message,
        "The title still starts with a WIP prefix, so Forgejo would keep this a draft"
    );
}

/// A forge whose WIP prefix the gateway doesn't know (Forgejo derives draft from server-configured
/// title prefixes, `models/issues/pull.go:798`, which no API exposes). Records every PATCHed title.
/// `concurrent`: another editor retitles the PR before Forgejo re-reads it for the PATCH's reply.
fn draft_prefix_forge(
    title: &str,
    concurrent: Option<&'static str>,
) -> (Router, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let current = std::sync::Arc::new(std::sync::Mutex::new(title.to_owned()));
    let patched = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let state = |title: &str| {
        let mut pull = pull();
        pull["title"] = json!(title);
        pull["draft"] = json!(title.starts_with("DRAFT:"));
        Json(pull)
    };
    let (read, write, log) = (current.clone(), current, patched.clone());
    let app = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r/pulls/1",
            get(move || async move { state(&read.lock().unwrap()) }).patch(
                move |Json(body): Json<Value>| async move {
                    let written = body["title"].as_str().unwrap().to_owned();
                    log.lock().unwrap().push(written.clone());
                    let title = concurrent.map_or(written, str::to_owned);
                    *write.lock().unwrap() = title.clone();
                    (StatusCode::CREATED, state(&title))
                },
            ),
        );
    (app, patched)
}

/// gh prints success for any payload, so a draft change Forgejo didn't make must fail. Forgejo's
/// edit has no conditional write, so the gateway never rolls the title back over what may by then
/// be someone else's edit; it reports the title Forgejo now shows (parallax r1#1, r2#1, r2#2).
#[tokio::test]
async fn draft_changes_fail_and_report_the_title_when_the_forge_uses_another_prefix() {
    let mutation = |name: &str| {
        format!(
            r#"mutation {{{name}(input:{{pullRequestId:"{}"}}){{pullRequest{{isDraft}}}}}}"#,
            node_id("PullRequest", "o/r", 1)
        )
    };
    let message = "Forgejo didn't change the draft state: the forge's WIP title prefixes differ from the gateway's";

    let (app, patched) = draft_prefix_forge("T", None);
    let response = execute(app, mutation("convertPullRequestToDraft")).await;
    assert_eq!(
        response.errors[0].message,
        format!("{message}; the title is now \"WIP: T\"")
    );
    assert_eq!(*patched.lock().unwrap(), ["WIP: T"]);

    let (app, patched) = draft_prefix_forge("T", Some("someone else's title"));
    let response = execute(app, mutation("convertPullRequestToDraft")).await;
    assert_eq!(
        response.errors[0].message,
        format!("{message}; the title is now \"someone else's title\"")
    );
    assert_eq!(*patched.lock().unwrap(), ["WIP: T"]);

    let (app, patched) = draft_prefix_forge("DRAFT: T", None);
    let response = execute(app, mutation("markPullRequestReadyForReview")).await;
    assert_eq!(response.errors[0].message, message);
    assert_eq!(*patched.lock().unwrap(), Vec::<String>::new());
}

/// Forgejo's edit changes the title and state before it checks a new base exists
/// (`routers/api/v1/repo/pull.go:649-759`), so a missing base must fail before any write.
#[tokio::test]
async fn an_edit_to_a_missing_base_changes_nothing() {
    let patched = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = patched.clone();
    let app = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r/pulls/1",
            get(|| async { Json(pull()) }).patch(move |Json(body): Json<Value>| async move {
                log.lock().unwrap().push(body);
                (StatusCode::CREATED, Json(pull()))
            }),
        )
        // URL building drops `.` and `..` segments, so `.` reaches the branch list (parallax r1#1).
        .route(
            "/api/v1/repos/o/r/branches",
            get(|| async { Json(json!([{"name": "release/next"}])) }),
        )
        .route(
            "/api/v1/repos/o/r/branches/{*branch}",
            get(
                |axum::extract::Path(branch): axum::extract::Path<String>| async move {
                    if branch == "release/next" {
                        Json(json!({"name": branch})).into_response()
                    } else {
                        (StatusCode::NOT_FOUND, Json(api_error("branch not found"))).into_response()
                    }
                },
            ),
        );
    let edit = |base: &str| {
        format!(
            r#"mutation {{updatePullRequest(input:{{pullRequestId:"{}",title:"X",state:CLOSED,baseRefName:"{base}"}}){{pullRequest{{number}}}}}}"#,
            node_id("PullRequest", "o/r", 1)
        )
    };
    for missing in ["missing", ".", "release/../next", "release/./next"] {
        let response = execute(app.clone(), edit(missing)).await;
        assert_eq!(response.errors[0].message, "Not Found", "{missing}");
        assert_eq!(*patched.lock().unwrap(), Vec::<Value>::new(), "{missing}");
    }

    let response = execute(app, edit("release/next")).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        *patched.lock().unwrap(),
        [json!({"title":"X","state":"closed","base":"release/next"})]
    );
}

/// Forgejo's create ignores maintainer edits, so a fork PR gets them in a follow-up edit.
#[tokio::test]
async fn fork_prs_get_maintainer_edits_after_create() {
    let fork = || {
        let mut pull = pull();
        pull["head"]["repo"] = json!({"name":"r","owner":{"id":2,"login":"fork","avatar_url":""}});
        pull
    };
    let app = router()
        .route(
            "/api/v1/repos/o/r/pulls",
            axum::routing::post(move |Json(body): Json<Value>| async move {
                assert!(body.get("allow_maintainer_edit").is_none());
                (StatusCode::CREATED, Json(fork()))
            }),
        )
        .route(
            "/api/v1/repos/o/r/pulls/1",
            axum::routing::patch(move |Json(body): Json<Value>| async move {
                assert_eq!(body, json!({"allow_maintainer_edit": true}));
                let mut pull = fork();
                pull["allow_maintainer_edit"] = json!(true);
                (StatusCode::CREATED, Json(pull))
            }),
        );
    let query = format!(
        r#"mutation {{createPullRequest(input:{{repositoryId:"{}", baseRefName:"main", headRefName:"fork:head", title:"T"}}){{pullRequest{{maintainerCanModify}}}}}}"#,
        node_id("Repository", "o/r", 1)
    );
    let response = execute(app, query).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"createPullRequest":{"pullRequest":{"maintainerCanModify":true}}})
    );
}

/// A refused maintainer-edit setting still returns the created PR, with an error naming it.
#[tokio::test]
async fn refused_maintainer_edits_report_the_created_pr() {
    let fork = || {
        let mut pull = pull();
        pull["head"]["repo"] = json!({"name":"r","owner":{"id":2,"login":"fork","avatar_url":""}});
        pull
    };
    let app = router()
        .route(
            "/api/v1/repos/o/r/pulls",
            axum::routing::post(move || async move { (StatusCode::CREATED, Json(fork())) }),
        )
        .route(
            "/api/v1/repos/o/r/pulls/1",
            axum::routing::patch(|| async { StatusCode::FORBIDDEN }),
        );
    let query = format!(
        r#"mutation {{createPullRequest(input:{{repositoryId:"{}", baseRefName:"main", headRefName:"fork:head", title:"T"}}){{pullRequest{{number maintainerCanModify}}}}}}"#,
        node_id("Repository", "o/r", 1)
    );
    let response = execute(app, query).await;
    assert_eq!(
        response.errors[0].message,
        "Created https://forge.test/o/r/pull/1, but Forgejo refused to let maintainers edit its branch"
    );
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"createPullRequest":{"pullRequest":{"number":1,"maintainerCanModify":false}}})
    );
}

/// Forgejo always schedules and only a later event wakes its worker, so auto-merge merges a ready
/// PR at once and schedules only after a refusal such an event can lift. A direct merge leaves
/// the style to Forgejo and needs no repository read.
#[tokio::test]
async fn auto_merge_merges_ready_prs_and_schedules_after_a_policy_refusal() {
    let refusal = (
        StatusCode::METHOD_NOT_ALLOWED,
        "not allowed to merge [reason: Does not have enough approvals]",
    );
    let denied = (
        StatusCode::METHOD_NOT_ALLOWED,
        "User not allowed to merge PR",
    );
    let checking = (StatusCode::METHOD_NOT_ALLOWED, "Please try again later");
    let raced = (StatusCode::CONFLICT, "merge push out of date");
    let (ok, scheduled) = ((StatusCode::OK, ""), (StatusCode::CREATED, ""));
    // (mutation, Forgejo's replies, merge_when_checks_succeed per request, error)
    let cases = [
        ("enablePullRequestAutoMerge", vec![ok], vec![false], None),
        (
            "enablePullRequestAutoMerge",
            vec![refusal, scheduled],
            vec![false, true],
            None,
        ),
        // Still checking: the direct attempt retries, then the schedule is accepted.
        (
            "enablePullRequestAutoMerge",
            vec![checking, checking, checking, checking, scheduled],
            vec![false, false, false, false, true],
            None,
        ),
        // A push that lost a race to the base gets one fresh direct attempt.
        (
            "enablePullRequestAutoMerge",
            vec![raced, ok],
            vec![false, false],
            None,
        ),
        (
            "enablePullRequestAutoMerge",
            vec![raced, raced],
            vec![false, false],
            Some("Base branch was modified. Review and try the merge again."),
        ),
        (
            "enablePullRequestAutoMerge",
            vec![denied],
            vec![false],
            Some("User does not have permission to merge this pull request"),
        ),
        ("mergePullRequest", vec![ok], vec![false], None),
    ];
    for (mutation, replies, sent, error) in cases {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut app = Router::new()
            .route("/api/v1/user", get(|| async { Json(user()) }))
            .route("/api/v1/repos/o/r/pulls/1", get(|| async { Json(pull()) }))
            .route(
                "/api/v1/repos/o/r/pulls/1/merge",
                axum::routing::post({
                    let seen = seen.clone();
                    move |Json(input): Json<Value>| async move {
                        let mut seen = seen.lock().unwrap();
                        seen.push(input["merge_when_checks_succeed"] == true);
                        let (status, message) = replies[seen.len() - 1];
                        (status, Json(api_error(message)))
                    }
                }),
            );
        app = if mutation == "mergePullRequest" {
            app.route(
                "/api/v1/repos/o/r",
                get(|| async { panic!("direct merges need no repository read") as StatusCode }),
            )
        } else {
            app.route("/api/v1/repos/o/r", get(|| async { Json(repo()) }))
        };
        let query = format!(
            r#"mutation {{ {mutation}(input:{{pullRequestId:"{}", mergeMethod:SQUASH}}){{pullRequest{{number}}}}}}"#,
            node_id("PullRequest", "o/r", 1)
        );
        let response = execute(app, query).await;
        assert_eq!(
            response.errors.first().map(|e| e.message.as_str()),
            error,
            "{mutation} {sent:?}"
        );
        assert_eq!(*seen.lock().unwrap(), sent, "{mutation}");
    }
}

/// `gh issue comment` sends the issue's own ID and `gh pr comment` the PR's; each comments only on
/// what its ID names. An issue ID whose number is a pull request writes nothing.
#[tokio::test]
async fn comments_go_to_issues_and_pull_requests_by_their_own_ids() {
    use axum::{extract::Path, routing::post};
    use std::sync::{Arc, Mutex};
    let posted = Arc::new(Mutex::new(Vec::new()));
    let record = posted.clone();
    let issue = |number: i32, pull: bool| {
        json!({"id":number,"number":number,"title":"T","body":"","state":"open",
            "html_url":format!("https://forge.test/o/r/issues/{number}"),"user":user(),
            "created_at":"2026-11-03T00:00:00Z","updated_at":"2026-11-03T00:00:00Z","labels":[],
            "pull_request":if pull { json!({"merged":false}) } else { Value::Null }})
    };
    let router = router()
        .route("/api/v1/repos/o/r/issues/1", get(move || async move { Json(issue(1, true)) }))
        .route("/api/v1/repos/o/r/issues/2", get(move || async move { Json(issue(2, false)) }))
        .route(
            "/api/v1/repos/o/r/issues/{number}/comments",
            post(move |Path(number): Path<i32>| {
                record.lock().unwrap().push(number);
                async move {
                    (
                        StatusCode::CREATED,
                        Json(json!({"id":9,"user":user(),"body":"hi","created_at":"2026-11-03T00:00:00Z",
                            "html_url":if number == 1 {
                                // Forgejo links a PR's comment under /pulls/.
                                "https://forge.test/o/r/pulls/1#issuecomment-9".to_owned()
                            } else {
                                format!("https://forge.test/o/r/issues/{number}#issuecomment-9")
                            }})),
                    )
                }
            }),
        );
    let comment = |kind: &str, number: i32| {
        format!(
            r#"mutation{{addComment(input:{{subjectId:"{}",body:"hi"}}){{commentEdge{{node{{url}}}}}}}}"#,
            node_id(kind, "o/r", number)
        )
    };
    // gh reads back only the comment's URL: an issue's stays under /issues/, a PR's takes GitHub's
    // /pull/ layout (MAPPING §8).
    for (kind, number, url) in [
        ("Issue", 2, "https://forge.test/o/r/issues/2#issuecomment-9"),
        (
            "PullRequest",
            1,
            "https://forge.test/o/r/pull/1#issuecomment-9",
        ),
    ] {
        let response = execute(router.clone(), comment(kind, number)).await;
        assert!(response.errors.is_empty(), "{kind}: {:?}", response.errors);
        assert_eq!(
            response.data.into_json().unwrap(),
            json!({"addComment":{"commentEdge":{"node":{"url":url}}}})
        );
    }
    // Each kind of ID names only its own kind: an issue ID for a PR, a PR ID for an issue.
    for (kind, number) in [("Issue", 1), ("PullRequest", 2)] {
        let response = execute(router.clone(), comment(kind, number)).await;
        assert_eq!(response.errors[0].message, "Not Found", "{kind} {number}");
    }
    assert_eq!(*posted.lock().unwrap(), [2, 1]);
}
