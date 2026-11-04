//! `gh api rate_limit` reads GitHub's rate-limit overview. Forgejo has no rate-limit API, so the
//! gateway answers GitHub's shape with a full budget.
use axum::{Json, Router, routing::get};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpStream,
    time::{SystemTime, UNIX_EPOCH},
};

#[tokio::test]
async fn rate_limit_reports_a_full_budget_in_githubs_shape() {
    let upstream = Router::new().route(
        "/api/v1/user",
        get(|| async { Json(json!({"id":1,"login":"e2e","full_name":"","avatar_url":""})) }),
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

    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /api/v3/rate_limit HTTP/1.1\r\nHost: forge.test\r\nAuthorization: token t\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
    .await
    .unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = response.split_once("\r\n\r\n").unwrap().1;
    let body: Value =
        serde_json::from_str(&body[body.find('{').unwrap()..=body.rfind('}').unwrap()]).unwrap();
    // Hourly families renew an hour from the request, per-minute ones a minute from it.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let hour = body["rate"]["reset"].as_u64().unwrap();
    assert!(
        hour >= now + 3600 - 5 && hour <= now + 3600,
        "{hour} vs {now}"
    );
    let minute = hour - 3600 + 60;
    let budget =
        |limit: u64, reset: u64| json!({"limit":limit,"used":0,"remaining":limit,"reset":reset});
    assert_eq!(
        body,
        json!({"resources":{"core":budget(5000, hour),"graphql":budget(5000, hour),
            "search":budget(30, minute),"code_search":budget(10, minute)},"rate":budget(5000, hour)})
    );
}
