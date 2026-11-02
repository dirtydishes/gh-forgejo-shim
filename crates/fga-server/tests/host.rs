//! gh's socket setting sends every host's requests to the gateway, each carrying that host's
//! token (plan review F2). Only requests addressed to the public host may reach Forgejo.
use axum::{Json, Router, http::HeaderMap, routing::get};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
};

/// Start a fake Forgejo that records every Authorization it receives, and a gateway whose public
/// URL is `public` in front of it.
async fn gateway(public: &str) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let (upstream, seen) = forgejo().await;
    let host = Host::new(public.parse().unwrap(), upstream, WipPrefixes::default());
    (serve(vec![host]).await, seen)
}

async fn serve(hosts: Vec<Host>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(hosts, "3.19.0".into());
    tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
    addr
}

/// A fake Forgejo that records every Authorization it receives.
async fn forgejo() -> (Forgejo, Arc<Mutex<Vec<String>>>) {
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
    (Forgejo::new(upstream_url.parse().unwrap()).unwrap(), seen)
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
    let (addr, seen) = gateway("https://forge.test").await;
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
    let (addr, seen) = gateway("https://forge.test").await;
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

/// A missing port is the scheme's default, not the configured port; an explicit port must be a
/// real one; and equivalent IP spellings are the same host (parallax r1#1-#3).
#[tokio::test]
async fn ports_and_addresses_compare_as_values() {
    for (public, head, expected) in [
        (
            "https://forge.test:8443",
            "GET /api/v3/user HTTP/1.1\r\nHost: forge.test",
            421,
        ),
        (
            "https://forge.test:8443",
            "GET https://forge.test/api/v3/user HTTP/1.1\r\nHost: forge.test:8443",
            421,
        ),
        (
            "https://forge.test:8443",
            "GET /api/v3/user HTTP/1.1\r\nHost: forge.test:8443",
            200,
        ),
        (
            "https://forge.test",
            "GET /api/v3/user HTTP/1.1\r\nHost: forge.test:99999",
            421,
        ),
        (
            "https://forge.test",
            "GET /api/v3/user HTTP/1.1\r\nHost: forge.test:abc",
            421,
        ),
        (
            "https://forge.test",
            "GET /api/v3/user HTTP/1.1\r\nHost: user@forge.test",
            421,
        ),
        (
            "https://forge.test",
            "GET /api/v3/user HTTP/1.1\r\nHost: forge.test.",
            421,
        ),
        (
            "https://[0:0:0:0:0:0:0:1]",
            "GET /api/v3/user HTTP/1.1\r\nHost: [::1]",
            200,
        ),
        (
            "https://[::1]",
            "GET /api/v3/user HTTP/1.1\r\nHost: [0:0:0:0:0:0:0:1]:443",
            200,
        ),
        (
            "https://[::1]",
            "GET /api/v3/user HTTP/1.1\r\nHost: [::2]",
            421,
        ),
        // A missing port follows the request's own scheme (Opus o1#2), and a port is digits only (o1#3).
        (
            "https://forge.test",
            "GET http://forge.test/api/v3/user HTTP/1.1\r\nHost: forge.test",
            421,
        ),
        (
            "https://forge.test",
            "GET /api/v3/user HTTP/1.1\r\nHost: forge.test:+443",
            421,
        ),
        // Only an address's canonical spelling names it: gh keys tokens by host text (Opus o1#1).
        (
            "https://127.0.0.1",
            "GET /api/v3/user HTTP/1.1\r\nHost: 127.0.0.1",
            200,
        ),
        (
            "https://127.0.0.1",
            "GET /api/v3/user HTTP/1.1\r\nHost: 127.1",
            421,
        ),
        (
            "https://127.0.0.1",
            "GET /api/v3/user HTTP/1.1\r\nHost: 2130706433",
            421,
        ),
        (
            "https://127.0.0.1",
            "GET /api/v3/user HTTP/1.1\r\nHost: 0x7f.0.0.1",
            421,
        ),
    ] {
        let (addr, seen) = gateway(public).await;
        let status = send(addr, &format!("{head}\r\nAuthorization: token t")).await;
        assert_eq!(status, expected, "{public} {head:?}");
        let reached = seen.lock().unwrap().len();
        assert_eq!(reached, usize::from(expected == 200), "{public} {head:?}");
    }
}

/// A launcher serves every host of one gh call, each with its own upstream and the one token it
/// handed over; anything else is refused before any upstream sees it.
#[tokio::test]
async fn each_host_admits_only_its_own_token_and_reaches_only_its_forge() {
    let (a_forge, a_seen) = forgejo().await;
    let (b_forge, b_seen) = forgejo().await;
    let addr = serve(vec![
        Host::new(
            "https://a.test".parse().unwrap(),
            a_forge,
            WipPrefixes::default(),
        )
        .expecting("a-token".into()),
        Host::new(
            "https://b.test".parse().unwrap(),
            b_forge,
            WipPrefixes::default(),
        )
        .expecting("b-token".into()),
    ])
    .await;
    for (host, auth, expected) in [
        ("a.test", "token a-token", 200),
        ("b.test", "Bearer b-token", 200),
        ("a.test", "token b-token", 401),
        ("b.test", "token a-token", 401),
        ("a.test", "token github-token", 401),
        ("a.test", "token a-token-and-more", 401),
        ("a.test", "basic a-token", 401),
        ("b.test", "", 401),
        ("api.github.com", "token a-token", 421),
    ] {
        let head = format!("GET /api/v3/user HTTP/1.1\r\nHost: {host}\r\nAuthorization: {auth}");
        assert_eq!(send(addr, &head).await, expected, "{head:?}");
    }
    assert_eq!(*a_seen.lock().unwrap(), ["token a-token"]);
    assert_eq!(*b_seen.lock().unwrap(), ["Bearer b-token"]);
}

/// Each host's links use its own public URL, `https://<host>`.
#[tokio::test]
async fn each_host_links_to_its_own_web_ui() {
    let (a_forge, _) = forgejo().await;
    let (b_forge, _) = forgejo().await;
    let addr = serve(vec![
        Host::new(
            "https://a.test".parse().unwrap(),
            a_forge,
            WipPrefixes::default(),
        ),
        Host::new(
            "https://b.test".parse().unwrap(),
            b_forge,
            WipPrefixes::default(),
        ),
    ])
    .await;
    let request = "GET /api/v3/ HTTP/1.1\r\nHost: b.test\r\nAuthorization: token t\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let body = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
    .await
    .unwrap();
    assert!(
        body.contains("\"current_user_url\":\"https://b.test/api/v3/user\""),
        "{body}"
    );
    assert!(!body.contains("a.test"), "{body}");
}
