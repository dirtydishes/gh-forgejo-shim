//! `gh run view` follows a run's `jobs_url` and reads its artifacts. Forgejo 15's API has neither
//! per run, so the gateway links both under its own API and answers empty lists for a run that
//! exists.
use axum::{Json, Router, http::StatusCode, routing::get};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
};

async fn gateway() -> SocketAddr {
    let run = json!({"id":5,"workflow_id":"ci.yml","index_in_repo":1,"title":"t",
        "prettyref":"refs/heads/main","commit_sha":"abc","status":"success","event":"push",
        "html_url":"https://forge.test/o/r/actions/runs/1","created":"2026-11-03T00:00:00Z",
        "updated":"2026-11-03T00:00:09Z","started":"2026-11-03T00:00:01Z"});
    let listed = json!({"total_count":1,"workflow_runs":[run.clone()]});
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
            "/api/v1/repos/o/r/actions/runs/5",
            get(move || async move { Json(run) }),
        )
        .route(
            "/api/v1/repos/o/r/actions/runs/6",
            get(|| async { (StatusCode::NOT_FOUND, Json(json!({"message":"not found"}))) }),
        )
        .route(
            "/api/v1/repos/o/r/contents/.forgejo/workflows",
            get(|| async { Json(json!([{"name":"ci.yml","type":"file"}])) }),
        )
        .route(
            "/api/v1/repos/o/r/contents/.forgejo/workflows/ci.yml",
            get(|| async {
                Json(
                    json!({"content":"bmFtZTogY2kKb246IHB1c2gK"}), /* "name: ci\non: push\n" */
                )
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
async fn run_view_finds_jobs_and_artifacts_under_the_gateway() {
    let addr = gateway().await;
    let base = "https://forge.test/api/v3/repos/o/r/actions/runs/5";
    let (code, run) = get_json(addr, "/api/v3/repos/o/r/actions/runs/5").await;
    assert_eq!(code, 200, "{run}");
    assert_eq!(run["jobs_url"], format!("{base}/jobs"));
    assert_eq!(run["artifacts_url"], format!("{base}/artifacts"));
    let (code, list) = get_json(addr, "/api/v3/repos/o/r/actions/runs").await;
    assert_eq!(code, 200, "{list}");
    assert_eq!(list["workflow_runs"][0]["jobs_url"], format!("{base}/jobs"));
    assert_eq!(
        list["workflow_runs"][0]["artifacts_url"],
        format!("{base}/artifacts")
    );
    let (code, jobs) = get_json(addr, "/api/v3/repos/o/r/actions/runs/5/jobs").await;
    assert_eq!((code, jobs), (200, json!({"total_count":0,"jobs":[]})));
    let (code, artifacts) = get_json(addr, "/api/v3/repos/o/r/actions/runs/5/artifacts").await;
    assert_eq!(
        (code, artifacts),
        (200, json!({"total_count":0,"artifacts":[]}))
    );
    for path in ["jobs", "artifacts"] {
        let (code, _) = get_json(addr, &format!("/api/v3/repos/o/r/actions/runs/6/{path}")).await;
        assert_eq!(code, 404, "{path}");
    }
}
