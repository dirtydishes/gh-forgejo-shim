//! `gh repo create` (gh 2.103.3) without a template, license, gitignore or README creates the
//! repository with one mutation, `RepositoryCreate` in `pkg/cmd/repo/create/http.go`, after reading
//! the owner's ID from `GET users/{owner}` when the name has one. The mutation below is gh's own,
//! and the inputs are what gh sends: `name`, `visibility`, `hasIssuesEnabled` and
//! `hasWikiEnabled` always, the rest only when set.
use async_graphql::{Request, Variables};
use axum::{
    Json, Router,
    extract::{Json as Body, Path},
    http::StatusCode,
    routing::{get, patch, post},
};
use fga_core::{forgejo::Forgejo, graphql, ids::node_id};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const GH_REPOSITORY_CREATE: &str = "
	mutation RepositoryCreate($input: CreateRepositoryInput!) {
		createRepository(input: $input) {
			repository {
				id
				name
				owner { login }
				url
			}
		}
	}";

fn account(id: i64, login: &str) -> Value {
    json!({"id":id,"login":login,"full_name":"","avatar_url":""})
}

/// A repository as Forgejo returns it from a create or an edit.
fn created(owner: Value, name: &str, id: i64, body: &Value) -> Value {
    let login = owner["login"].as_str().unwrap().to_owned();
    json!({"id":id,"name":name,"full_name":format!("{login}/{name}"),"owner":owner,
        "private":body["private"].as_bool().unwrap_or(false),
        "html_url":format!("https://forge.test/{login}/{name}"),"default_branch":"main",
        "fork":false,"archived":false,"has_issues":true,"has_wiki":true,
        "description":body["description"].as_str().unwrap_or("")})
}

/// What the fake Forgejo was sent, as `METHOD path body`.
type Sent = Arc<Mutex<Vec<(String, Value)>>>;

/// A Forgejo where `o` (ID 1) is the viewer and belongs to `acme` (ID 5); `taken` already exists
/// for both. `PATCH` of a repository named `stuck` fails.
async fn forge() -> (
    Sent,
    fga_core::forgejo::Session,
    tokio::task::JoinHandle<()>,
) {
    let sent: Sent = Arc::new(Mutex::new(Vec::new()));
    let (personal, org, edit) = (sent.clone(), sent.clone(), sent.clone());
    let create = |sent: Sent, owner: Value, path: &'static str| {
        move |Body(body): Body<Value>| {
            sent.lock()
                .unwrap()
                .push((format!("POST {path}"), body.clone()));
            async move {
                let name = body["name"].as_str().unwrap().to_owned();
                if name == "taken" {
                    return (
                        StatusCode::CONFLICT,
                        Json(
                            json!({"message":"The repository with the same name already exists."}),
                        ),
                    );
                }
                (StatusCode::CREATED, Json(created(owner, &name, 9, &body)))
            }
        }
    };
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(account(1, "o")) }))
        .route(
            "/api/v1/user/orgs",
            get(|| async { Json(json!([{"id":5,"name":"acme","username":"acme"}])) }),
        )
        .route("/api/v1/orgs/o", get(|| async { StatusCode::NOT_FOUND }))
        .route(
            "/api/v1/orgs/acme",
            get(|| async { Json(json!({"id":5,"name":"acme","username":"acme"})) }),
        )
        .route(
            "/api/v1/user/repos",
            post(create(personal, account(1, "o"), "/user/repos")),
        )
        .route(
            "/api/v1/orgs/acme/repos",
            post(create(org, account(5, "acme"), "/orgs/acme/repos")),
        )
        .route(
            "/api/v1/repos/{owner}/{name}",
            patch(
                move |Path((owner, name)): Path<(String, String)>, Body(body): Body<Value>| {
                    edit.lock()
                        .unwrap()
                        .push((format!("PATCH /repos/{owner}/{name}"), body.clone()));
                    async move {
                        if name == "stuck" {
                            return (StatusCode::FORBIDDEN, Json(json!({"message":"no"})));
                        }
                        let mut repository = created(account(1, &owner), &name, 9, &json!({}));
                        for field in ["has_issues", "has_wiki"] {
                            if let Some(value) = body.get(field) {
                                repository[field] = value.clone();
                            }
                        }
                        (StatusCode::OK, Json(repository))
                    }
                },
            ),
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
    (sent, session, task)
}

async fn create(session: &fga_core::forgejo::Session, input: Value) -> async_graphql::Response {
    let request = Request::new(GH_REPOSITORY_CREATE)
        .variables(Variables::from_json(json!({ "input": input })))
        .data(session.clone());
    graphql::schema().execute(request).await
}

#[tokio::test]
async fn a_private_repository_is_created_for_the_viewer() {
    let (sent, session, task) = forge().await;
    // `gh repo create o/new --private`, then `gh repo create new --private`, which sends no owner.
    for owner in [Some(node_id("User", "", 1)), None] {
        let mut input = json!({"name":"new","visibility":"PRIVATE",
            "hasIssuesEnabled":true,"hasWikiEnabled":true});
        if let Some(owner) = owner {
            input["ownerId"] = json!(owner);
        }
        let response = create(&session, input).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            response.data.into_json().unwrap(),
            json!({"createRepository":{"repository":{"id":node_id("Repository", "o/new", 9),
                "name":"new","owner":{"login":"o"},"url":"https://forge.test/o/new"}}})
        );
    }
    task.abort();
    // Forgejo turns on issues and the wiki for a new repository, as asked, so nothing is edited.
    let create = (
        "POST /user/repos".to_owned(),
        json!({"name":"new","private":true}),
    );
    assert_eq!(*sent.lock().unwrap(), [create.clone(), create]);
}

#[tokio::test]
async fn an_organization_repository_is_created_in_the_organization() {
    let (sent, session, task) = forge().await;
    let response = create(
        &session,
        json!({"name":"new","visibility":"PUBLIC","description":"Tools",
            "ownerId":node_id("Organization", "", 5),"hasIssuesEnabled":true,"hasWikiEnabled":true}),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["createRepository"]["repository"]["owner"],
        json!({"login":"acme"})
    );
    task.abort();
    assert_eq!(
        *sent.lock().unwrap(),
        [(
            "POST /orgs/acme/repos".to_owned(),
            json!({"name":"new","private":false,"description":"Tools"})
        )]
    );
}

/// Forgejo's create takes no homepage and no issue or wiki switches, so the gateway edits the new
/// repository for those, and only those that differ from what Forgejo made.
#[tokio::test]
async fn a_homepage_and_disabled_issues_and_wiki_are_set_after_creating() {
    let (sent, session, task) = forge().await;
    let response = create(
        &session,
        json!({"name":"new","visibility":"PRIVATE","homepageUrl":"https://example.test",
            "hasIssuesEnabled":false,"hasWikiEnabled":true}),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    // GitHub's default for `hasWikiEnabled` is false, so a client that leaves it out gets none.
    let response = create(&session, json!({"name":"bare","visibility":"PRIVATE"})).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    task.abort();
    assert_eq!(
        sent.lock().unwrap()[1..],
        [
            (
                "PATCH /repos/o/new".to_owned(),
                json!({"website":"https://example.test","has_issues":false})
            ),
            (
                "POST /user/repos".to_owned(),
                json!({"name":"bare","private":true})
            ),
            ("PATCH /repos/o/bare".to_owned(), json!({"has_wiki":false})),
        ]
    );
}

/// The repository exists once Forgejo makes it, so a refused edit returns it with an error that
/// names it, rather than an error that invites creating it again.
#[tokio::test]
async fn a_refused_edit_still_returns_the_new_repository() {
    let (_, session, task) = forge().await;
    let response = create(
        &session,
        json!({"name":"stuck","visibility":"PRIVATE","hasIssuesEnabled":false,"hasWikiEnabled":true}),
    )
    .await;
    task.abort();
    assert_eq!(
        response.data.into_json().unwrap()["createRepository"]["repository"]["url"],
        "https://forge.test/o/stuck"
    );
    assert_eq!(
        response.errors[0].message,
        "Created https://forge.test/o/stuck, but Forgejo refused its homepage, issues or wiki setting"
    );
}

/// What Forgejo can't make, or the viewer can't make there, fails before anything is written.
#[tokio::test]
async fn what_forgejo_cant_create_is_refused_and_nothing_is_written() {
    let (sent, session, task) = forge().await;
    let base =
        json!({"name":"new","visibility":"PRIVATE","hasIssuesEnabled":true,"hasWikiEnabled":true});
    let with = |field: &str, value: Value| {
        let mut input = base.clone();
        input[field] = value;
        input
    };
    for (input, message) in [
        (
            with("visibility", json!("INTERNAL")),
            "Forgejo has no internal repositories; create it as private or public",
        ),
        (
            with("teamId", json!("VGVhbQ==")),
            "The gateway can't give a team access to a new repository yet",
        ),
        // Another user's account, and an organization the viewer isn't in.
        (
            with("ownerId", json!(node_id("User", "", 2))),
            "Forgejo creates repositories only in your account or an organization you belong to",
        ),
        (
            with("ownerId", json!(node_id("Organization", "", 6))),
            "Forgejo creates repositories only in your account or an organization you belong to",
        ),
        (with("ownerId", json!("garbage")), "Not Found"),
        (
            with("ownerId", json!(node_id("Repository", "o/r", 1))),
            "Not Found",
        ),
    ] {
        let response = create(&session, input.clone()).await;
        assert_eq!(response.errors[0].message, message, "{input}");
    }
    assert!(sent.lock().unwrap().is_empty());
    // A name already in use is Forgejo's 409, which GitHub words this way.
    let response = create(&session, with("name", json!("taken"))).await;
    task.abort();
    assert_eq!(
        response.errors[0].message,
        "Name already exists on this account"
    );
}
