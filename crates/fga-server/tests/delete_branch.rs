//! `gh pr merge --delete-branch` deletes the head branch with GitHub's
//! `DELETE git/refs/heads/{branch}`. gh counts 404 and 422 as "already deleted", so the gateway must
//! really delete the branch, and must not answer either code for a refusal.
use axum::{
    Json, Router,
    http::{StatusCode, Uri},
    response::IntoResponse,
    routing::{delete, get},
};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
};

/// A fake Forgejo that records every branch DELETE: `feature/x` and `topic` delete, `main` is the
/// default branch, `kept` is protected, and anything else doesn't exist.
async fn gateway() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let record = deleted.clone();
    let upstream = Router::new()
        .route(
            "/api/v1/user",
            get(|| async { Json(json!({"id":1,"login":"e2e","full_name":"","avatar_url":""})) }),
        )
        .route(
            "/api/v1/repos/o/r/branches/{*branch}",
            delete(move |uri: Uri| {
                let path = uri.path().to_owned();
                record.lock().unwrap().push(path.clone());
                async move {
                    let branch = path.trim_start_matches("/api/v1/repos/o/r/branches/");
                    match branch {
                        "feature/x" | "topic" => StatusCode::NO_CONTENT.into_response(),
                        "main" => (
                            StatusCode::FORBIDDEN,
                            Json(json!({"message":"can not delete default branch"})),
                        )
                            .into_response(),
                        "kept" => (
                            StatusCode::FORBIDDEN,
                            Json(json!({"message":"branch protected"})),
                        )
                            .into_response(),
                        "busy" => (StatusCode::CONFLICT, Json(json!({"message":"conflict"})))
                            .into_response(),
                        _ => (StatusCode::NOT_FOUND, Json(json!({"message":"not found"})))
                            .into_response(),
                    }
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
    (addr, deleted)
}

/// Send `DELETE path` as gh does and return the status code and body.
async fn delete_ref(addr: SocketAddr, path: &str) -> (u16, String) {
    let request = format!(
        "DELETE {path} HTTP/1.1\r\nHost: forge.test\r\nAuthorization: token t\r\nConnection: close\r\n\r\n"
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
    (code, response.split_once("\r\n\r\n").unwrap().1.to_owned())
}

fn message(body: &str) -> Value {
    serde_json::from_str::<Value>(&body[body.find('{').unwrap()..=body.rfind('}').unwrap()])
        .unwrap()["message"]
        .clone()
}

#[tokio::test]
async fn deleting_a_head_ref_deletes_the_forgejo_branch() {
    let (addr, deleted) = gateway().await;
    // gh escapes the branch name as one path segment.
    let (code, _) = delete_ref(addr, "/api/v3/repos/o/r/git/refs/heads/feature%2Fx").await;
    assert_eq!(code, 204);
    let (code, _) = delete_ref(addr, "/api/v3/repos/o/r/git/refs/heads/topic").await;
    assert_eq!(code, 204);
    // GitHub's answer for a missing ref, which gh reads as already deleted.
    let (code, body) = delete_ref(addr, "/api/v3/repos/o/r/git/refs/heads/gone").await;
    assert_eq!(
        (code, message(&body)),
        (422, json!("Reference does not exist"))
    );
    // A refusal stays a refusal, so gh reports it instead of claiming the branch is gone.
    for branch in ["main", "kept"] {
        let (code, _) =
            delete_ref(addr, &format!("/api/v3/repos/o/r/git/refs/heads/{branch}")).await;
        assert_eq!(code, 403, "{branch}");
    }
    // Forgejo saying something this route shouldn't is a failure gh reports.
    let (code, _) = delete_ref(addr, "/api/v3/repos/o/r/git/refs/heads/busy").await;
    assert_eq!(code, 502);
    // No branch has an empty part; Forgejo would merge the slashes and delete another branch.
    for path in ["a//b", "feature%2Fx%2F", "feature/x/", "%2Ffeature%2Fx"] {
        let (code, body) =
            delete_ref(addr, &format!("/api/v3/repos/o/r/git/refs/heads/{path}")).await;
        assert_eq!(
            (code, message(&body)),
            (422, json!("Reference does not exist")),
            "{path}"
        );
    }
    // Valid git names the path guard won't send: refused as a failure, never "already deleted".
    for path in ["a%252Fb", "%252e%252e"] {
        let (code, _) = delete_ref(addr, &format!("/api/v3/repos/o/r/git/refs/heads/{path}")).await;
        assert_eq!(code, 400, "{path}");
    }
    assert_eq!(
        *deleted.lock().unwrap(),
        [
            "/api/v1/repos/o/r/branches/feature/x",
            "/api/v1/repos/o/r/branches/topic",
            "/api/v1/repos/o/r/branches/gone",
            "/api/v1/repos/o/r/branches/main",
            "/api/v1/repos/o/r/branches/kept",
            "/api/v1/repos/o/r/branches/busy",
        ]
    );
}
