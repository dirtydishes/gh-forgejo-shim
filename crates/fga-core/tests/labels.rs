//! `gh label list`, and the label edits of `gh pr edit` and `gh issue edit`. The documents are gh
//! 2.103.3's own: `LabelList` (`pkg/cmd/label/http.go`), `RepositoryLabelList`
//! (`api/queries_repo.go`), and `LabelAdd` and `LabelRemove` (`pkg/cmd/pr/shared/editable_http.go`).
use async_graphql::{Request, Variables};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query},
    http::StatusCode,
    routing::{delete, get, post},
};
use fga_core::{forgejo::Forgejo, graphql, ids::node_id};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const LABEL_LIST: &str = r#"fragment label on Label{color,description,name}
	query LabelList($owner: String!, $repo: String!, $limit: Int!, $endCursor: String, $query: String, $orderBy: LabelOrder) {
		repository(owner: $owner, name: $repo) {
			labels(first: $limit, after: $endCursor, query: $query, orderBy: $orderBy) {
				totalCount,
				nodes {
					...label
				}
				pageInfo {
					hasNextPage
					endCursor
				}
			}
		}
	}"#;
const REPOSITORY_LABEL_LIST: &str = "query RepositoryLabelList($endCursor:String$name:String!$owner:String!){repository(owner: $owner, name: $name){labels(first: 100, orderBy: {field: NAME, direction: ASC}, after: $endCursor){nodes{id,name},pageInfo{hasNextPage,endCursor}}}}";
const LABEL_ADD: &str = "mutation LabelAdd($input:AddLabelsToLabelableInput!){addLabelsToLabelable(input: $input){__typename}}";
const LABEL_REMOVE: &str = "mutation LabelRemove($input:RemoveLabelsFromLabelableInput!){removeLabelsFromLabelable(input: $input){__typename}}";

fn user() -> Value {
    json!({"id":1,"login":"o","full_name":"","avatar_url":""})
}
fn label(id: i64, name: &str, color: &str, description: &str) -> Value {
    json!({"id":id,"name":name,"color":color,"description":description,"exclusive":false,
        "is_archived":false,"url":format!("https://forge.test/api/v1/repos/o/r/labels/{id}")})
}
fn issue(number: i32, pull: bool) -> Value {
    json!({"id":number,"number":number,"title":"T","body":"","state":"open",
        "html_url":format!("https://forge.test/o/r/issues/{number}"),"user":user(),
        "created_at":"2026-11-03T00:00:00Z","updated_at":"2026-11-03T00:00:00Z",
        "labels":[label(3, "bug", "ee0701", "Something broke")],
        "pull_request":if pull { json!({"merged":false}) } else { Value::Null }})
}
fn pull() -> Value {
    json!({"number":1,"title":"T","state":"open","merged":false,"draft":false,
    "html_url":"https://forge.test/o/r/pulls/1","head":{"ref":"head","label":"head","sha":"abc"},
    "base":{"ref":"main","label":"main","sha":"def"},"user":user(),"merged_at":null,"labels":[]})
}

/// One page of `items`, as Forgejo's label lists send it: at most `cap` items whatever `limit`
/// asks (its `MAX_RESPONSE_ITEMS`), and no `Link` header, only `X-Total-Count`.
fn paged(items: &[Value], query: &HashMap<String, String>, cap: usize) -> Json<Value> {
    let get = |key: &str, default: usize| {
        query
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let size = get("limit", cap).min(cap);
    let page = get("page", 1).max(1);
    Json(json!(
        items
            .iter()
            .skip((page - 1) * size)
            .take(size)
            .collect::<Vec<_>>()
    ))
}

/// The forge's shape: the organization's labels (`None` for a user's repository), whether the
/// repository is archived, and the most items Forgejo sends a page.
struct Shape {
    organization: Option<Vec<Value>>,
    archived: bool,
    cap: usize,
}

/// A forge with repository `o/r`, issue #2 and pull request #1. Its labels are `Wip` (1), `bug`
/// (3), `kind/feature` (7) and `9` (9); an organization `o`, when there is one, adds `triage` (5).
/// It records each label write as `METHOD number body-or-id`.
fn forge(organization: bool, writes: Arc<Mutex<Vec<String>>>) -> Router {
    let organization = organization.then(|| vec![label(5, "triage", "00ff00", "Needs a look")]);
    forge_shaped(
        Shape {
            organization,
            archived: false,
            cap: 50,
        },
        writes,
    )
}
fn forge_shaped(shape: Shape, writes: Arc<Mutex<Vec<String>>>) -> Router {
    let (added, removed) = (writes.clone(), writes);
    let Shape {
        organization,
        archived,
        cap,
    } = shape;
    let labels = vec![
        label(1, "Wip", "aaaaaa", ""),
        label(3, "bug", "ee0701", "Something broke"),
        label(7, "kind/feature", "0052cc", "New work"),
        label(9, "9", "cccccc", "Numbered"),
    ];
    let mut router = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/o/r",
            get(move || async move {
                Json(
                    json!({"id":1,"name":"r","full_name":"o/r","private":false,"owner":user(),
                    "html_url":"https://forge.test/o/r","default_branch":"main","fork":false,
                    "archived":archived,"has_issues":true,
                    "permissions":{"admin":false,"push":true,"pull":true}}),
                )
            }),
        )
        .route(
            "/api/v1/repos/o/r/labels",
            get(
                move |Query(query): Query<HashMap<String, String>>| async move {
                    paged(&labels, &query, cap)
                },
            ),
        )
        .route(
            "/api/v1/repos/o/r/issues/2",
            get(|| async { Json(issue(2, false)) }),
        )
        .route(
            "/api/v1/repos/o/r/issues/1",
            get(|| async { Json(issue(1, true)) }),
        )
        .route("/api/v1/repos/o/r/pulls/1", get(|| async { Json(pull()) }))
        .route(
            "/api/v1/repos/o/r/issues/{number}/labels",
            post(move |Path(number): Path<i32>, body: Bytes| {
                let body: Value = serde_json::from_slice(&body).unwrap();
                added.lock().unwrap().push(format!("POST {number} {body}"));
                async { Json(json!([])) }
            }),
        )
        .route(
            "/api/v1/repos/o/r/issues/{number}/labels/{id}",
            delete(move |Path((number, id)): Path<(i32, String)>| {
                removed
                    .lock()
                    .unwrap()
                    .push(format!("DELETE {number} {id}"));
                async { StatusCode::NO_CONTENT }
            }),
        );
    if let Some(shared) = organization {
        router = router.route(
            "/api/v1/orgs/o/labels",
            get(
                move |Query(query): Query<HashMap<String, String>>| async move {
                    paged(&shared, &query, cap)
                },
            ),
        );
    }
    router
}

async fn execute(router: Router, request: Request) -> async_graphql::Response {
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
    let response = graphql::schema().execute(request.data(session)).await;
    task.abort();
    response
}
fn request(document: &str, variables: Value) -> Request {
    Request::new(document).variables(Variables::from_json(variables))
}
async fn list(organization: bool, variables: Value) -> Value {
    let response = execute(
        forge(organization, Default::default()),
        request(LABEL_LIST, variables),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()["repository"]["labels"].clone()
}
fn names(labels: &Value) -> Vec<&str> {
    labels["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect()
}

/// gh's default `label list`: created order, which Forgejo's IDs keep, the organization's labels
/// among the repository's, and an empty `--search`.
#[tokio::test]
async fn label_list_pages_repository_and_organization_labels_in_created_order() {
    let created = json!({"field":"CREATED_AT","direction":"ASC"});
    let first = list(
        true,
        json!({"owner":"o","repo":"r","limit":3,"query":"","orderBy":created}),
    )
    .await;
    assert_eq!(names(&first), ["Wip", "bug", "triage"]);
    assert_eq!(first["totalCount"], 5);
    assert_eq!(first["pageInfo"]["hasNextPage"], true);
    assert_eq!(
        first["nodes"][1],
        json!({"color":"ee0701","description":"Something broke","name":"bug"})
    );
    let rest = list(
        true,
        json!({"owner":"o","repo":"r","limit":3,"query":"","orderBy":created,
            "endCursor":first["pageInfo"]["endCursor"]}),
    )
    .await;
    assert_eq!(names(&rest), ["kind/feature", "9"]);
    assert_eq!(rest["pageInfo"]["hasNextPage"], false);
}

/// `--search` matches names and descriptions, ignoring case; `--sort name --order desc` sorts by
/// name, ignoring case.
#[tokio::test]
async fn label_list_searches_and_sorts_by_name() {
    let created = json!({"field":"CREATED_AT","direction":"ASC"});
    for (query, expected) in [
        ("BROKE", vec!["bug"]),
        ("tri", vec!["triage"]),
        ("feature", vec!["kind/feature"]),
    ] {
        let labels = list(
            true,
            json!({"owner":"o","repo":"r","limit":30,"query":query,"orderBy":created}),
        )
        .await;
        assert_eq!(names(&labels), expected, "{query}");
        assert_eq!(labels["totalCount"], expected.len(), "{query}");
    }
    let labels = list(
        true,
        json!({"owner":"o","repo":"r","limit":30,"query":"",
            "orderBy":{"field":"NAME","direction":"DESC"}}),
    )
    .await;
    assert_eq!(
        names(&labels),
        ["Wip", "triage", "kind/feature", "bug", "9"]
    );
}

/// A user's repository has no organization labels: Forgejo answers 404 for the organization.
#[tokio::test]
async fn a_user_repository_lists_only_its_own_labels() {
    let labels = list(false, json!({"owner":"o","repo":"r","limit":30})).await;
    assert_eq!(names(&labels), ["Wip", "bug", "kind/feature", "9"]);
}

/// `RepositoryLabelList` resolves `--add-label` and `--remove-label` names to IDs, and the IDs it
/// returns name the repository they were read through, as the labels on an issue do.
#[tokio::test]
async fn label_ids_name_their_repository() {
    let response = execute(
        forge(true, Default::default()),
        request(
            REPOSITORY_LABEL_LIST,
            json!({"owner":"o","name":"r","endCursor":null}),
        ),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let nodes = response.data.into_json().unwrap()["repository"]["labels"]["nodes"].clone();
    // By name: 9, bug, kind/feature, triage, Wip.
    assert_eq!(
        nodes[1],
        json!({"id":node_id("Label", "o/r", 3),"name":"bug"})
    );
    assert_eq!(
        nodes[3],
        json!({"id":node_id("Label", "o/r", 5),"name":"triage"})
    );

    let response = execute(
        forge(true, Default::default()),
        Request::new(
            r#"{repository(owner:"o",name:"r"){issue(number:2){labels(first:10){nodes{id}}}}}"#,
        ),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["repository"]["issue"]["labels"]["nodes"][0]["id"],
        node_id("Label", "o/r", 3)
    );
}

async fn mutate(
    document: &str,
    labelable: String,
    labels: &[String],
) -> (async_graphql::Response, Vec<String>) {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let response = execute(
        forge(true, writes.clone()),
        request(
            document,
            json!({"input":{"labelableId":labelable,"labelIds":labels}}),
        ),
    )
    .await;
    let writes = writes.lock().unwrap().clone();
    (response, writes)
}

/// Adds go to the issue's labels in one request, by Forgejo ID, for a pull request and an issue
/// alike; removes go one label at a time.
#[tokio::test]
async fn labels_are_added_and_removed_on_pull_requests_and_issues() {
    let bug = node_id("Label", "o/r", 3);
    let triage = node_id("Label", "o/r", 5);
    let (response, writes) = mutate(
        LABEL_ADD,
        node_id("PullRequest", "o/r", 1),
        &[bug.clone(), triage.clone()],
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap(),
        json!({"addLabelsToLabelable":{"__typename":"AddLabelsToLabelablePayload"}})
    );
    assert_eq!(writes, [r#"POST 1 {"labels":[3,5]}"#]);

    let (response, writes) = mutate(LABEL_REMOVE, node_id("Issue", "o/r", 2), &[bug, triage]).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(writes, ["DELETE 2 3", "DELETE 2 5"]);
}

/// Nothing is written when a label or the issue can't be resolved: a label read through another
/// repository, one this repository and its organization don't have, an issue ID for a pull request,
/// or a pull request that doesn't exist. Each answers GitHub's "Not Found".
#[tokio::test]
async fn unresolvable_ids_write_nothing() {
    let bug = node_id("Label", "o/r", 3);
    for (labelable, label) in [
        (
            node_id("PullRequest", "o/r", 1),
            node_id("Label", "o/other", 3),
        ),
        (
            node_id("PullRequest", "o/r", 1),
            node_id("Label", "o/r", 99),
        ),
        (node_id("PullRequest", "o/r", 1), node_id("Issue", "o/r", 3)),
        (node_id("Issue", "o/r", 1), bug.clone()),
        (node_id("PullRequest", "o/r", 4), bug.clone()),
    ] {
        for document in [LABEL_ADD, LABEL_REMOVE] {
            let (response, writes) =
                mutate(document, labelable.clone(), &[bug.clone(), label.clone()]).await;
            assert_eq!(response.errors.len(), 1, "{label}");
            assert_eq!(response.errors[0].message, "Not Found", "{label}");
            assert!(writes.is_empty(), "{writes:?}");
        }
    }
}

/// Forgejo's remove reads its last path segment as a label name before an ID, so removing label 9
/// would remove a label named "9" instead, when one exists. Here label 9 is that label, so the
/// remove goes ahead; a label whose ID another label has as its name is refused unwritten.
#[tokio::test]
async fn a_remove_that_forgejo_would_read_as_another_label_is_refused() {
    let (response, writes) = mutate(
        LABEL_REMOVE,
        node_id("Issue", "o/r", 2),
        &[node_id("Label", "o/r", 9)],
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(writes, ["DELETE 2 9"]);

    let writes = Arc::new(Mutex::new(Vec::new()));
    let router = forge_shaped(
        Shape {
            organization: Some(vec![label(5, "3", "00ff00", "")]),
            archived: false,
            cap: 50,
        },
        writes.clone(),
    );
    let response = execute(
        router,
        request(
            LABEL_REMOVE,
            json!({"input":{"labelableId":node_id("Issue", "o/r", 2),"labelIds":[node_id("Label", "o/r", 3)]}}),
        ),
    )
    .await;
    assert_eq!(response.errors.len(), 1);
    assert_eq!(
        response.errors[0].message,
        "Forgejo would read label 3 as the label named \"3\"; remove it on the forge"
    );
    assert!(writes.lock().unwrap().is_empty());
}

/// A token without `read:organization` can't see the organization's labels, but Forgejo's remove
/// still reads them by name: an organization label named "3" would go in place of label 3. So with
/// the organization's labels hidden, a remove is refused unwritten, naming the scope; the list and
/// adds go on with the repository's own labels (parallax sol-r1).
#[tokio::test]
async fn hidden_organization_labels_refuse_a_remove() {
    let hidden = || async {
        (
            StatusCode::FORBIDDEN,
            Json(
                json!({"message":"token does not have at least one of required scope(s): [read:organization]"}),
            ),
        )
    };
    let writes = Arc::new(Mutex::new(Vec::new()));
    let router = forge(false, writes.clone()).route("/api/v1/orgs/o/labels", get(hidden));
    let response = execute(
        router,
        request(
            LABEL_REMOVE,
            json!({"input":{"labelableId":node_id("Issue", "o/r", 2),"labelIds":[node_id("Label", "o/r", 3)]}}),
        ),
    )
    .await;
    assert_eq!(response.errors.len(), 1);
    assert_eq!(
        response.errors[0].message,
        "Forgejo may read label 3 as the name of an organization label this token can't read; give the token read:organization"
    );
    assert!(writes.lock().unwrap().is_empty());

    let router = forge(false, writes.clone()).route("/api/v1/orgs/o/labels", get(hidden));
    let response = execute(
        router,
        request(
            LABEL_ADD,
            json!({"input":{"labelableId":node_id("Issue", "o/r", 2),"labelIds":[node_id("Label", "o/r", 3)]}}),
        ),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(*writes.lock().unwrap(), [r#"POST 2 {"labels":[3]}"#]);

    let router = forge(false, Default::default()).route("/api/v1/orgs/o/labels", get(hidden));
    let response = execute(
        router,
        request(LABEL_LIST, json!({"owner":"o","repo":"r","limit":30})),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        names(&response.data.into_json().unwrap()["repository"]["labels"]),
        ["Wip", "bug", "kind/feature", "9"]
    );
}

/// Forgejo sends at most its `MAX_RESPONSE_ITEMS` labels a page, which may be fewer than the
/// gateway asks for, and no `Link`. Both lists are read to the end, so the organization label named
/// "3" on the third page still stops the remove of label 3 (parallax sol-r1#1).
#[tokio::test]
async fn short_pages_are_read_to_the_end() {
    let shape = || Shape {
        organization: Some(vec![
            label(11, "1", "00ff00", ""),
            label(12, "2", "00ff00", ""),
            label(13, "3", "00ff00", ""),
        ]),
        archived: false,
        cap: 1,
    };
    let response = execute(
        forge_shaped(shape(), Default::default()),
        request(LABEL_LIST, json!({"owner":"o","repo":"r","limit":30})),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        names(&response.data.into_json().unwrap()["repository"]["labels"]),
        ["Wip", "bug", "kind/feature", "9", "1", "2", "3"]
    );

    let writes = Arc::new(Mutex::new(Vec::new()));
    let response = execute(
        forge_shaped(shape(), writes.clone()),
        request(
            LABEL_REMOVE,
            json!({"input":{"labelableId":node_id("Issue", "o/r", 2),"labelIds":[node_id("Label", "o/r", 3)]}}),
        ),
    )
    .await;
    assert_eq!(response.errors.len(), 1);
    assert_eq!(
        response.errors[0].message,
        "Forgejo would read label 3 as the label named \"3\"; remove it on the forge"
    );
    assert!(writes.lock().unwrap().is_empty());
}

/// An archived repository is read-only on GitHub, and Forgejo's label routes don't check: both
/// mutations refuse it unwritten, with GitHub's message (parallax sol-r1#2).
#[tokio::test]
async fn an_archived_repository_takes_no_label_writes() {
    for document in [LABEL_ADD, LABEL_REMOVE] {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let router = forge_shaped(
            Shape {
                organization: None,
                archived: true,
                cap: 50,
            },
            writes.clone(),
        );
        let response = execute(
            router,
            request(
                document,
                json!({"input":{"labelableId":node_id("Issue", "o/r", 2),"labelIds":[node_id("Label", "o/r", 3)]}}),
            ),
        )
        .await;
        assert_eq!(response.errors.len(), 1, "{document}");
        assert_eq!(
            response.errors[0].message,
            "Repository was archived so is read-only."
        );
        assert!(writes.lock().unwrap().is_empty(), "{document}");
    }
}
