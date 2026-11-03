//! `gh pr merge --delete-branch` deletes the head branch with GitHub's
//! `DELETE git/refs/heads/{branch}`. gh counts 404 and 422 as "already deleted", so the gateway must
//! really delete the branch, and must not answer either code for a refusal.
use axum::{
    Json, Router,
    http::{StatusCode, Uri},
    response::IntoResponse,
    routing::get,
};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
};

/// A fake Forgejo that answers as Forgejo 15 does and records every branch DELETE. `o/r` has
/// `feature/x` and `topic` (deletable), `main` (default), `kept` (protected), `busy` (Forgejo
/// answers 409) and `lazy` (answers 202); deleting a branch that doesn't exist is a 500 there
/// (`GetBranchCommit` returns `ErrNotExist`), and reading one is a 404 naming it in `errors`.
/// `o/hidden` is a private repository the token can't see, so every read of it is a bare 404.
/// `o/nocode` is readable, but its Code unit is off, so reading its existing `topic` is a bare 404.
async fn gateway() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    const BRANCHES: [&str; 6] = ["feature/x", "topic", "main", "kept", "busy", "lazy"];
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let record = deleted.clone();
    let not_found = |errors: Value| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"message":"The target couldn't be found.",
                "url":"https://forge.test/api/swagger","errors":errors})),
        )
            .into_response()
    };
    let upstream = Router::new()
        .route(
            "/api/v1/user",
            get(|| async { Json(json!({"id":1,"login":"e2e","full_name":"","avatar_url":""})) }),
        )
        .route(
            "/api/v1/repos/o/r",
            get(|| async {
                Json(json!({"id":1,"name":"r","full_name":"o/r","private":false,
                    "owner":{"id":1,"login":"o","avatar_url":""},"html_url":"https://forge.test/o/r",
                    "default_branch":"main","fork":false,"archived":false,
                    "permissions":{"admin":false,"push":true,"pull":true}}))
            }),
        )
        .route(
            "/api/v1/repos/o/nocode",
            get(|| async {
                Json(json!({"id":2,"name":"nocode","full_name":"o/nocode","private":false,
                    "owner":{"id":1,"login":"o","avatar_url":""},"html_url":"https://forge.test/o/nocode",
                    "default_branch":"main","fork":false,"archived":false,
                    "permissions":{"admin":false,"push":true,"pull":true}}))
            }),
        )
        .route(
            "/api/v1/repos/o/nocode/branches/{*branch}",
            get(move || async move { not_found(json!([])) }),
        )
        .route(
            "/api/v1/repos/o/r/branches/{*branch}",
            get(move |uri: Uri| async move {
                let branch = uri.path().trim_start_matches("/api/v1/repos/o/r/branches/");
                if BRANCHES.contains(&branch) {
                    Json(json!({"name":branch})).into_response()
                } else {
                    not_found(json!([format!("branch does not exist [name: {branch}]")]))
                }
            })
            .delete(move |uri: Uri| {
                let path = uri.path().to_owned();
                record.lock().unwrap().push(path.clone());
                async move {
                    let error = |code, message: &str| (code, Json(json!({"message":message})));
                    match path.trim_start_matches("/api/v1/repos/o/r/branches/") {
                        "feature/x" | "topic" => StatusCode::NO_CONTENT.into_response(),
                        "main" => error(StatusCode::FORBIDDEN, "can not delete default branch")
                            .into_response(),
                        "kept" => error(StatusCode::FORBIDDEN, "branch protected").into_response(),
                        "busy" => error(StatusCode::CONFLICT, "conflict").into_response(),
                        "lazy" => StatusCode::ACCEPTED.into_response(),
                        _ => error(StatusCode::INTERNAL_SERVER_ERROR, "DeleteBranch").into_response(),
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
    let heads = |path: &str| format!("/api/v3/repos/o/r/git/refs/heads/{path}");
    // gh escapes the branch name as one path segment.
    assert_eq!(delete_ref(addr, &heads("feature%2Fx")).await.0, 204);
    assert_eq!(delete_ref(addr, &heads("topic")).await.0, 204);
    // A branch Forgejo says doesn't exist gets GitHub's answer, which gh reads as already
    // deleted; nothing is sent.
    let (code, body) = delete_ref(addr, &heads("gone")).await;
    assert_eq!(
        (code, message(&body)),
        (422, json!("Reference does not exist"))
    );
    // A repository the token can't see proves nothing about the branch: a refusal, not "gone".
    // A 404 that doesn't name the branch proves nothing about it: a hidden repository, or a
    // readable one whose Code unit is off. A refusal, not "gone".
    for repo in ["hidden", "nocode"] {
        let path = format!("/api/v3/repos/o/{repo}/git/refs/heads/topic");
        assert_eq!(delete_ref(addr, &path).await.0, 403, "{repo}");
    }
    // A refusal stays a refusal, so gh reports it instead of claiming the branch is gone.
    for branch in ["main", "kept"] {
        assert_eq!(delete_ref(addr, &heads(branch)).await.0, 403, "{branch}");
    }
    // Anything but Forgejo's 204 is a delete that didn't happen.
    for branch in ["busy", "lazy"] {
        assert_eq!(delete_ref(addr, &heads(branch)).await.0, 502, "{branch}");
    }
    // No branch has an empty part; Forgejo would merge the slashes and delete another branch.
    for path in ["a//b", "feature%2Fx%2F", "feature/x/", "%2Ffeature%2Fx"] {
        let (code, body) = delete_ref(addr, &heads(path)).await;
        assert_eq!(
            (code, message(&body)),
            (422, json!("Reference does not exist")),
            "{path}"
        );
    }
    // Valid git names the path guard won't send: refused as a failure, never "already deleted".
    for path in ["a%252Fb", "%252e%252e"] {
        assert_eq!(delete_ref(addr, &heads(path)).await.0, 400, "{path}");
    }
    assert_eq!(
        *deleted.lock().unwrap(),
        ["feature/x", "topic", "main", "kept", "busy", "lazy"]
            .map(|branch| format!("/api/v1/repos/o/r/branches/{branch}"))
    );
}
