//! gh's socket setting sends every host's requests to the gateway, each carrying that host's
//! token (plan review F2). Only requests addressed to the public host may reach Forgejo.
use axum::{Json, Router, http::HeaderMap, routing::get};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, app};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
};

/// Start a fake Forgejo that records every Authorization it receives, and a gateway for
/// `https://forge.test` in front of it.
async fn gateway() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    let upstream = Router::new()
        .route(
            "/api/v1/user",
            get(|| async {
                Json(json!({"id":1,"login":"e2e","full_name":"","avatar_url":"https://forge.test/a"}))
            }),
        )
        .layer(axum::middleware::from_fn(
            move |headers: HeaderMap, req, next: axum::middleware::Next| {
                let record = record.clone();
                async move {
                    let auth = headers.get("authorization").map(|v| v.to_str().unwrap().to_owned());
                    record.lock().unwrap().push(auth.unwrap_or_default());
                    next.run(req).await
                }
            },
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let state = AppState::new(
        Forgejo::new(upstream_url.parse().unwrap()).unwrap(),
        "https://forge.test".parse().unwrap(),
        "3.19.0".into(),
        WipPrefixes::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
    (addr, seen)
}

/// Send one raw HTTP/1.1 request, so the test controls the request target and Host lines exactly.
async fn send(addr: SocketAddr, head: &str) -> u16 {
    let request = format!("{head}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response.split(' ').nth(1).unwrap().parse().unwrap()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn other_hosts_tokens_never_reach_forgejo() {
    let (addr, seen) = gateway().await;
    for head in [
        // `gh api https://api.github.com/user` and gh's github.com GraphQL.
        "GET /user HTTP/1.1\r\nHost: api.github.com\r\nAuthorization: token github-token",
        "POST /graphql HTTP/1.1\r\nHost: api.github.com\r\nAuthorization: token github-token",
        "GET /api/v3/user HTTP/1.1\r\nHost: github.com\r\nAuthorization: token github-token",
        // A second Forgejo host, and the right name on another port.
        "GET /api/v3/user HTTP/1.1\r\nHost: other.forge.test\r\nAuthorization: token other-token",
        "POST /api/graphql HTTP/1.1\r\nHost: other.forge.test\r\nAuthorization: token other-token",
        "GET /api/v3/user HTTP/1.1\r\nHost: forge.test:8443\r\nAuthorization: token other-token",
        // The request target's authority wins over a matching Host.
        "GET https://api.github.com/user HTTP/1.1\r\nHost: forge.test\r\nAuthorization: token github-token",
        // Ambiguous or missing hosts.
        "GET /api/v3/user HTTP/1.1\r\nHost: forge.test\r\nHost: api.github.com\r\nAuthorization: token github-token",
        "GET /api/v3/user HTTP/1.1\r\nAuthorization: token github-token",
    ] {
        let status = send(addr, head).await;
        let leaked = seen.lock().unwrap().clone();
        assert!(leaked.is_empty(), "{head:?} sent {leaked:?} to Forgejo");
        assert_eq!(status, 421, "{head:?}");
    }
}

#[tokio::test]
async fn the_public_host_still_reaches_forgejo() {
    let (addr, seen) = gateway().await;
    for (head, token) in [
        ("GET /api/v3/user HTTP/1.1\r\nHost: forge.test", "token one"),
        (
            "GET /api/v3/user HTTP/1.1\r\nHost: FORGE.test:443",
            "token two",
        ),
        (
            "GET https://forge.test/api/v3/user HTTP/1.1\r\nHost: forge.test",
            "token three",
        ),
    ] {
        let status = send(addr, &format!("{head}\r\nAuthorization: {token}")).await;
        assert_eq!(status, 200, "{head:?}");
    }
    assert_eq!(
        *seen.lock().unwrap(),
        ["token one", "token two", "token three"]
    );
}
