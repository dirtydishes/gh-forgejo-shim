//! `gh run list --workflow` (gh 2.103.3). gh resolves the selector first: a number or a `.yml`/
//! `.yaml` file name with `GET actions/workflows/{selector}`, anything else by name from
//! `GET actions/workflows` (`FindWorkflow` in `pkg/cmd/workflow/shared/shared.go`). It then lists
//! that workflow's runs from `GET actions/workflows/{ID}/runs` (`GetRuns` in
//! `pkg/cmd/run/shared/shared.go`). The requests below are gh's own.
use axum::{Json, Router, extract::RawQuery, routing::get};
use fga_core::{actions::workflow_id, forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
};

fn run(id: i64, workflow: &str, sha: &str) -> Value {
    json!({"id":id,"workflow_id":workflow,"index_in_repo":id,"title":"t",
        "prettyref":"refs/heads/main","commit_sha":sha,"status":"success","event":"push",
        "html_url":format!("https://forge.test/o/r/actions/runs/{id}"),
        "created":"2026-11-03T00:00:00Z","updated":"2026-11-03T00:00:09Z",
        "started":"2026-11-03T00:00:01Z"})
}

/// A gateway in front of a Forgejo whose default branch has `ci.yml` and `lint.yml`, with two runs
/// of `ci.yml`, one of `lint.yml` and one of `new.yml`, which exists only on a branch. Forgejo filters runs by
/// `workflow_id`, a bare file name; the queries it was sent are returned.
async fn gateway() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let (addr, queries, _) = gateway_with_paths().await;
    (addr, queries)
}

/// The same gateway, with every path its Forgejo was asked for.
#[allow(clippy::type_complexity)]
async fn gateway_with_paths() -> (SocketAddr, Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>) {
    let queries = Arc::new(Mutex::new(Vec::new()));
    let record = queries.clone();
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let paths = asked.clone();
    // Blocked runs of a fork's pull request: 6 waits for approval, 5 was approved, 7 waits on
    // something else.
    let blocked = |id: i64, need_approval: bool, approved_by: i64| {
        let mut run = run(id, "fork.yml", "c6");
        run["status"] = json!("blocked");
        run["need_approval"] = json!(need_approval);
        run["approved_by"] = json!(approved_by);
        run
    };
    let all = [
        blocked(7, false, 0),
        blocked(6, true, 0),
        blocked(5, true, 1),
        run(4, "ci.yml", "c4"),
        run(3, "ci.yml", "c3"),
        run(2, "new.yml", "c2"),
        run(1, "lint.yml", "c1"),
    ];
    let upstream = Router::new()
        .route(
            "/api/v1/user",
            get(|| async { Json(json!({"id":1,"login":"e2e","full_name":"","avatar_url":""})) }),
        )
        .route(
            "/api/v1/repos/o/r",
            get(|| async {
                Json(
                    json!({"id":1,"name":"r","full_name":"o/r","default_branch":"main",
                    "owner":{"id":1,"login":"o","full_name":"","avatar_url":""},"private":false,
                    "html_url":"https://forge.test/o/r","fork":false,"archived":false}),
                )
            }),
        )
        .route(
            "/api/v1/repos/o/r/actions/runs",
            get(move |RawQuery(query): RawQuery| {
                let query = query.unwrap_or_default();
                record.lock().unwrap().push(query.clone());
                let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
                    .into_owned()
                    .collect();
                let values = |name: &str| -> Vec<String> {
                    pairs
                        .iter()
                        .filter(|(key, _)| key == name)
                        .map(|(_, value)| value.clone())
                        .collect()
                };
                let (wanted, statuses, sha) =
                    (values("workflow_id"), values("status"), values("head_sha"));
                let runs: Vec<Value> = all
                    .iter()
                    .filter(|run| wanted.iter().all(|w| run["workflow_id"] == *w.as_str()))
                    .filter(|run| {
                        statuses.is_empty() || statuses.iter().any(|s| run["status"] == *s.as_str())
                    })
                    .filter(|run| sha.iter().all(|s| run["commit_sha"] == *s.as_str()))
                    .cloned()
                    .collect();
                async move { Json(json!({"total_count":runs.len(),"workflow_runs":runs})) }
            }),
        )
        .route(
            "/api/v1/repos/o/r/contents/.forgejo/workflows",
            get(|| async {
                Json(json!([{"name":"ci.yml","type":"file"},{"name":"lint.yml","type":"file"}]))
            }),
        )
        .route(
            "/api/v1/repos/o/r/contents/.forgejo/workflows/ci.yml",
            // "name: ci\non: push\n"
            get(|| async { Json(json!({"content":"bmFtZTogY2kKb246IHB1c2gK"})) }),
        )
        .route(
            "/api/v1/repos/o/r/contents/.forgejo/workflows/lint.yml",
            // "name: lint\non: push\n"
            get(|| async { Json(json!({"content":"bmFtZTogbGludApvbjogcHVzaAo="})) }),
        )
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let paths = paths.clone();
                async move {
                    paths.lock().unwrap().push(request.uri().path().to_owned());
                    next.run(request).await
                }
            },
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let host = Host::new(
        "https://forge.test".parse().unwrap(),
        Forgejo::new(upstream_url.parse().unwrap()).unwrap(),
        WipPrefixes::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(vec![host], "3.19.0".into()).unwrap();
    tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
    (addr, queries, asked)
}

/// The status, the `Link` header and the JSON body.
async fn get_json(addr: SocketAddr, path: &str) -> (u16, String, Value) {
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: forge.test\r\nAuthorization: token t\r\nConnection: close\r\n\r\n"
    );
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
    .await
    .unwrap();
    let code = response.split(' ').nth(1).unwrap().parse().unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let link = head
        .lines()
        .find_map(|line| line.strip_prefix("link: "))
        .unwrap_or_default()
        .to_owned();
    let json = &body[body.find('{').unwrap()..=body.rfind('}').unwrap()];
    (code, link, serde_json::from_str(json).unwrap())
}

fn names(page: &Value) -> Vec<&str> {
    page["workflow_runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn run_list_by_workflow_file_name_asks_forgejo_for_that_file() {
    let (addr, queries) = gateway().await;
    let (code, _, workflow) = get_json(addr, "/api/v3/repos/o/r/actions/workflows/ci.yml").await;
    assert_eq!(code, 200, "{workflow}");
    assert_eq!(
        (&workflow["id"], &workflow["name"], &workflow["path"]),
        (
            &json!(workflow_id("ci.yml")),
            &json!("ci"),
            &json!(".forgejo/workflows/ci.yml")
        )
    );
    let path = format!(
        "/api/v3/repos/o/r/actions/workflows/{}/runs?per_page=20&exclude_pull_requests=true",
        workflow["id"]
    );
    let (code, _, page) = get_json(addr, &path).await;
    assert_eq!(code, 200, "{page}");
    assert_eq!(names(&page), ["ci", "ci"]);
    assert!(
        queries
            .lock()
            .unwrap()
            .iter()
            .any(|q| q.contains("workflow_id=ci.yml")),
        "{queries:?}"
    );
}

/// A workflow's runs page links to its own next page, as GitHub's does, carrying gh's filters. gh
/// follows it only for a `--limit` above 100, its largest `per_page`.
#[tokio::test]
async fn a_workflows_runs_link_to_their_next_page() {
    let (addr, _) = gateway().await;
    let id = workflow_id("ci.yml");
    let (code, link, page) = get_json(
        addr,
        &format!(
            "/api/v3/repos/o/r/actions/workflows/{id}/runs?per_page=1&exclude_pull_requests=true"
        ),
    )
    .await;
    assert_eq!(code, 200, "{page}");
    assert_eq!(page["total_count"], 2);
    assert!(
        link.contains(&format!(
            "<https://forge.test/api/v3/repos/o/r/actions/workflows/{id}/runs?exclude_pull_requests=true&page=2&per_page=1>; rel=\"next\""
        )),
        "{link}"
    );
}

/// gh resolves a workflow by name from the list, then asks for its runs by ID. A branch-only
/// workflow's ID is found through a recent run of it, as `GET actions/workflows/{id}` finds it.
#[tokio::test]
async fn run_list_by_name_or_branch_only_id_filters_by_file() {
    let (addr, queries) = gateway().await;
    let (code, _, listed) = get_json(
        addr,
        "/api/v3/repos/o/r/actions/workflows?per_page=100&page=1",
    )
    .await;
    assert_eq!(code, 200, "{listed}");
    let lint = listed["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "lint")
        .unwrap();
    let (code, _, page) = get_json(
        addr,
        &format!(
            "/api/v3/repos/o/r/actions/workflows/{}/runs?per_page=20&exclude_pull_requests=true",
            lint["id"]
        ),
    )
    .await;
    assert_eq!(code, 200, "{page}");
    assert_eq!(names(&page), ["lint"]);
    let (code, _, page) = get_json(
        addr,
        &format!(
            "/api/v3/repos/o/r/actions/workflows/{}/runs?per_page=20&exclude_pull_requests=true",
            workflow_id("new.yml")
        ),
    )
    .await;
    assert_eq!(code, 200, "{page}");
    assert_eq!(names(&page), ["new.yml"]);
    assert!(
        queries
            .lock()
            .unwrap()
            .iter()
            .any(|q| q.contains("workflow_id=new.yml")),
        "{queries:?}"
    );
}

/// GitHub answers 404 for a file name not on the default branch and an ID it doesn't know; gh
/// then says the workflow wasn't found. A selector with a directory, which gh accepts as a file
/// and sends as one escaped segment, names no workflow: GitHub's `{workflow_id}` is the file name.
#[tokio::test]
async fn an_unknown_workflow_is_not_found() {
    let (addr, _) = gateway().await;
    for path in [
        "/api/v3/repos/o/r/actions/workflows/new.yml",
        "/api/v3/repos/o/r/actions/workflows/nope.yml",
        "/api/v3/repos/o/r/actions/workflows/.forgejo%2Fworkflows%2Fci.yml",
        "/api/v3/repos/o/r/actions/workflows/7/runs?per_page=20&exclude_pull_requests=true",
    ] {
        let (code, _, body) = get_json(addr, path).await;
        assert_eq!(
            (code, &body["message"]),
            (404, &json!("Not Found")),
            "{path}"
        );
    }
}

/// T3 Code asks for a pull request's runs awaiting approval on every read of its checks, and shows
/// checks it couldn't read when refused. They are the blocked runs Forgejo holds for approval:
/// `need_approval` and not yet approved. With none, the workflow files go unread.
#[tokio::test]
async fn runs_awaiting_approval_are_those_forgejo_holds_for_it() {
    let (addr, queries, asked) = gateway_with_paths().await;
    let (code, _, page) = get_json(
        addr,
        "/api/v3/repos/o/r/actions/runs?head_sha=c4&branch=feat&event=pull_request&status=action_required&per_page=100&page=1",
    )
    .await;
    assert_eq!(code, 200, "{page}");
    assert_eq!(page, json!({"total_count":0,"workflow_runs":[]}));
    let query = queries.lock().unwrap()[0].clone();
    assert!(query.contains("status=blocked"), "{query}");
    assert!(
        !asked
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("/contents/")),
        "{:?}",
        asked.lock().unwrap()
    );

    let (code, _, page) = get_json(
        addr,
        "/api/v3/repos/o/r/actions/runs?head_sha=c6&status=action_required&per_page=100",
    )
    .await;
    assert_eq!(code, 200, "{page}");
    assert_eq!(page["total_count"], 1);
    assert_eq!(page["workflow_runs"][0]["id"], 6);

    // A repository the token can't read is still not found.
    let (code, _, _) = get_json(
        addr,
        "/api/v3/repos/nobody/nothing/actions/runs?status=action_required",
    )
    .await;
    assert_eq!(code, 404);
    // GitHub's other statuses the gateway doesn't map are refused, as before.
    for status in ["neutral", "timed_out", "sideways"] {
        let (code, _, _) = get_json(
            addr,
            &format!("/api/v3/repos/o/r/actions/runs?status={status}"),
        )
        .await;
        assert_eq!(code, 422, "{status}");
    }
}
