//! A run whose workflow file exists only on its own branch, such as a PR's run of the workflow it
//! adds. The gateway names workflows from the default branch, so such a run used to make the whole
//! `GET actions/runs` page, and `GET actions/runs/{id}`, answer 404. GitHub lists them.
use axum::{
    Json, Router, extract::RawQuery, http::StatusCode, response::IntoResponse, routing::get,
};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
};

fn run(id: i64, workflow: &str, sha: &str) -> Value {
    json!({"id":id,"workflow_id":workflow,"index_in_repo":id,"title":"t",
        "prettyref":"refs/heads/topic","commit_sha":sha,"status":"success","event":"pull_request",
        "html_url":format!("https://forge.test/o/r/actions/runs/{id}"),
        "created":"2026-11-03T00:00:00Z","updated":"2026-11-03T00:00:09Z",
        "started":"2026-11-03T00:00:01Z"})
}

async fn gateway() -> SocketAddr {
    let (normal, branch_only, unreadable) = (
        run(1, "ci.yml", "c1"),
        run(2, "new.yml", "c2"),
        run(3, "gone.yml", "c3"),
    );
    let listed = json!({"total_count":3,"workflow_runs":[normal, branch_only.clone(), unreadable]});
    let upstream = Router::new()
        .route(
            "/api/v1/user",
            get(|| async { Json(json!({"id":1,"login":"e2e","full_name":"","avatar_url":""})) }),
        )
        .route(
            "/api/v1/repos/o/r/actions/runs",
            get(move || async move { Json(listed) }),
        )
        .route(
            "/api/v1/repos/o/r/actions/runs/2",
            get(move || async move { Json(branch_only) }),
        )
        // The default branch has only ci.yml.
        .route(
            "/api/v1/repos/o/r/contents/.forgejo/workflows",
            get(|| async { Json(json!([{"name":"ci.yml","type":"file"}])) }),
        )
        .route(
            "/api/v1/repos/o/r/contents/.forgejo/workflows/ci.yml",
            // "name: ci\non: push\n"
            get(|| async { Json(json!({"content":"bmFtZTogY2kKb246IHB1c2gK"})) }),
        )
        // new.yml exists only at the run's own commit.
        .route(
            "/api/v1/repos/o/r/contents/.forgejo/workflows/new.yml",
            get(|RawQuery(query): RawQuery| async move {
                if query.as_deref() == Some("ref=c2") {
                    // "name: new ci\non: pull_request\n"
                    Json(json!({"content":"bmFtZTogbmV3IGNpCm9uOiBwdWxsX3JlcXVlc3QK"}))
                        .into_response()
                } else {
                    StatusCode::NOT_FOUND.into_response()
                }
            }),
        );
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
    addr
}

async fn get_json(addr: SocketAddr, path: &str) -> (u16, Value) {
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
    let body = response.split_once("\r\n\r\n").unwrap().1;
    let json = &body[body.find('{').unwrap()..=body.rfind('}').unwrap()];
    (code, serde_json::from_str(json).unwrap())
}

#[tokio::test]
async fn a_run_of_a_workflow_only_on_its_branch_is_listed_and_viewed() {
    let addr = gateway().await;
    let (code, page) = get_json(addr, "/api/v3/repos/o/r/actions/runs").await;
    assert_eq!(code, 200, "{page}");
    let runs: Vec<(Value, Value)> = page["workflow_runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].clone(), r["path"].clone()))
        .collect();
    assert_eq!(
        runs,
        [
            (json!("ci"), json!(".forgejo/workflows/ci.yml")),
            // Named from the file at the run's own commit.
            (json!("new ci"), json!(".forgejo/workflows/new.yml")),
            // Not readable anywhere: the file name stands in, and the page still answers.
            (json!("gone.yml"), json!(".forgejo/workflows/gone.yml")),
        ]
    );
    let (code, viewed) = get_json(addr, "/api/v3/repos/o/r/actions/runs/2").await;
    assert_eq!(code, 200, "{viewed}");
    assert_eq!(viewed["name"], "new ci");
    // gh's `run view` then looks the run's workflow up by ID, and stops on a 404.
    let (code, workflow) = get_json(
        addr,
        &format!(
            "/api/v3/repos/o/r/actions/workflows/{}",
            viewed["workflow_id"]
        ),
    )
    .await;
    assert_eq!(code, 200, "{workflow}");
    assert_eq!(
        (&workflow["name"], &workflow["path"]),
        (&json!("new ci"), &json!(".forgejo/workflows/new.yml"))
    );
}

/// A repository that keeps its workflows in `.github/workflows`: a branch-only workflow is looked
/// for there first, and an error other than "not found" stops the search with the file name
/// instead of reading the next directory.
#[tokio::test]
async fn branch_only_workflows_are_read_from_the_directory_in_use() {
    use std::sync::{Arc, Mutex};
    let reads = Arc::new(Mutex::new(Vec::new()));
    let record = reads.clone();
    let listed =
        json!({"total_count":2,"workflow_runs":[run(4, "new.yml", "c9"), run(5, "x.yml", "c9")]});
    let missing = || StatusCode::NOT_FOUND.into_response();
    let upstream = Router::new()
        .route(
            "/api/v1/user",
            get(|| async { Json(json!({"id":1,"login":"e2e","full_name":"","avatar_url":""})) }),
        )
        .route(
            "/api/v1/repos/o/g/actions/runs",
            get(move || async move { Json(listed) }),
        )
        .route(
            "/api/v1/repos/o/g/contents/.forgejo/workflows",
            get(move || async move { missing() }),
        )
        .route(
            "/api/v1/repos/o/g/contents/.gitea/workflows",
            get(move || async move { missing() }),
        )
        .route(
            "/api/v1/repos/o/g/contents/.github/workflows",
            get(|| async { Json(json!([{"name":"ci.yml","type":"file"}])) }),
        )
        .route(
            "/api/v1/repos/o/g/contents/.github/workflows/ci.yml",
            get(|| async { Json(json!({"content":"bmFtZTogY2kKb246IHB1c2gK"})) }),
        )
        .route(
            "/api/v1/repos/o/g/contents/{*path}",
            get(
                move |axum::extract::Path(path): axum::extract::Path<String>| {
                    record.lock().unwrap().push(path.clone());
                    async move {
                        match path.as_str() {
                            ".github/workflows/new.yml" => {
                                Json(json!({"content":"bmFtZTogbmV3IGNpCm9uOiBwdWxsX3JlcXVlc3QK"}))
                                    .into_response()
                            }
                            // Anything else this test reaches is a server fault, not a missing file.
                            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                        }
                    }
                },
            ),
        );
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
    let (code, page) = get_json(addr, "/api/v3/repos/o/g/actions/runs").await;
    assert_eq!(code, 200, "{page}");
    let runs: Vec<(Value, Value)> = page["workflow_runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].clone(), r["path"].clone()))
        .collect();
    assert_eq!(
        runs,
        [
            (json!("new ci"), json!(".github/workflows/new.yml")),
            (json!("x.yml"), json!(".github/workflows/x.yml")),
        ]
    );
    // Only the directory in use was read for each run's file.
    let mut reads = reads.lock().unwrap().clone();
    reads.sort();
    assert_eq!(
        reads,
        [".github/workflows/new.yml", ".github/workflows/x.yml"]
    );
}
