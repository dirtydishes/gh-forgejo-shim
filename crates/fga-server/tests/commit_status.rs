//! The commit status routes Claude desktop reads once it has found a branch's pull request:
//! combined status, check runs, and workflow runs by head SHA.
use axum::{
    Json, Router,
    extract::{Query, RawQuery},
    routing::get,
};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
};

/// A fake Forgejo with one commit `abc`: a passing Actions job and a failing external status.
/// It records the query of every run list it serves.
async fn gateway() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let runs = Arc::new(Mutex::new(Vec::new()));
    let record = runs.clone();
    let status = |id: i64, context: &str, state: &str, target: &str| {
        json!({"id":id,"context":context,"status":state,"target_url":target,"description":"",
            "created_at":"2026-11-02T00:00:00Z","updated_at":"2026-11-02T00:00:05Z"})
    };
    let statuses = json!([
        status(
            1,
            "ci / check (push)",
            "success",
            "/o/r/actions/runs/5/jobs/0"
        ),
        status(2, "lint", "failure", "https://ci.example/7"),
    ]);
    let upstream = Router::new()
        .route(
            "/api/v1/user",
            get(|| async { Json(json!({"id":1,"login":"e2e","full_name":"","avatar_url":""})) }),
        )
        .route(
            "/api/v1/repos/o/r/commits/abc/statuses",
            get(move |Query(_): Query<HashMap<String, String>>| {
                let statuses = statuses.clone();
                async move { Json(statuses) }
            }),
        )
        .route(
            "/api/v1/repos/o/r/actions/runs",
            get(move |RawQuery(query): RawQuery| {
                record.lock().unwrap().push(query.unwrap_or_default());
                async { Json(json!({"total_count":0,"workflow_runs":[]})) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let forgejo = Forgejo::new(upstream_url.parse().unwrap()).unwrap();
    let host = Host::new(
        "https://forge.test".parse().unwrap(),
        forgejo,
        WipPrefixes::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(vec![host], "3.19.0".into()).unwrap();
    tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
    (addr, runs)
}

/// GET `path` from the gateway as forge.test, returning the status code and the JSON body.
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
    // The gateway answers in one piece or chunked; take the JSON between the outer braces.
    let json = &body[body.find('{').unwrap()..=body.rfind('}').unwrap()];
    (code, serde_json::from_str(json).unwrap())
}

#[tokio::test]
async fn commit_status_and_check_runs_answer_from_forgejo_statuses() {
    let (addr, _) = gateway().await;
    let (code, combined) =
        get_json(addr, "/api/v3/repos/o/r/commits/abc/status?per_page=100").await;
    assert_eq!(code, 200, "{combined}");
    assert_eq!(combined["state"], "failure");
    assert_eq!(combined["sha"], "abc");
    assert_eq!(
        combined["statuses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| json!([s["context"], s["state"], s["target_url"]]))
            .collect::<Vec<_>>(),
        [json!(["lint", "failure", "https://ci.example/7"])]
    );
    let (code, runs) = get_json(
        addr,
        "/api/v3/repos/o/r/commits/abc/check-runs?per_page=100",
    )
    .await;
    assert_eq!(code, 200, "{runs}");
    assert_eq!(runs["total_count"], 1);
    let run = &runs["check_runs"][0];
    assert_eq!(
        json!([
            run["name"],
            run["status"],
            run["conclusion"],
            run["head_sha"],
            run["details_url"]
        ]),
        json!([
            "check",
            "completed",
            "success",
            "abc",
            "https://forge.test/o/r/actions/runs/5/jobs/0"
        ])
    );
}

/// Claude desktop repeats `per_page`; GitHub takes the last value instead of refusing the query.
#[tokio::test]
async fn workflow_runs_by_head_sha_accept_a_repeated_parameter() {
    let (addr, runs) = gateway().await;
    let (code, body) = get_json(
        addr,
        "/api/v3/repos/o/r/actions/runs?head_sha=abc&per_page=100&per_page=100",
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["total_count"], 0);
    let seen = runs.lock().unwrap().clone();
    assert!(
        seen.iter().any(|q| q.contains("head_sha=abc")),
        "Forgejo got {seen:?}"
    );
    // An unknown parameter is still refused, as before.
    let (code, _) = get_json(addr, "/api/v3/repos/o/r/actions/runs?bogus=1").await;
    assert_eq!(code, 422);
}
