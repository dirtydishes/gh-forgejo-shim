//! The GraphQL endpoint writes GitHub's response shape: data as the schema built it, and each
//! error with its `type` at the top and a `path`, without `extensions`. The report's document
//! (an issue body under 1,000 aliases) is refused before anything reaches Forgejo but the viewer.
use axum::{Json, Router, routing::get};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

async fn gateway() -> SocketAddr {
    let upstream = Router::new().route(
        "/api/v1/user",
        get(|| async {
            Json(json!({"id":1,"login":"e2e","full_name":"","avatar_url":"https://forge.test/a"}))
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
    addr
}

/// POST `query` to the gateway's GraphQL endpoint, returning the status and the JSON body.
async fn graphql(addr: SocketAddr, query: &str) -> (u16, Value) {
    let body = json!({ "query": query }).to_string();
    let request = format!(
        "POST /api/graphql HTTP/1.1\r\nHost: forge.test\r\nAuthorization: token disposable\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
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
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap())
}

#[tokio::test]
async fn data_and_errors_keep_githubs_shape() {
    let addr = gateway().await;

    let (status, body) = graphql(addr, "{viewer{login}}").await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"data": {"viewer": {"login": "e2e"}}}));

    let (status, body) = graphql(addr, "{viewer{unsupported}}").await;
    assert_eq!(status, 200);
    let error = &body["errors"][0];
    assert_eq!(error["type"], "GRAPHQL_VALIDATION_FAILED");
    assert_eq!(error["path"], json!([]));
    assert!(error.get("extensions").is_none(), "{error}");
}

#[tokio::test]
async fn the_reports_aliased_document_is_refused() {
    let addr = gateway().await;
    let fields: Vec<String> = (0..1000).map(|i| format!("a{i}:body")).collect();
    let query = format!(
        "{{repository(owner:\"o\",name:\"r\"){{issue(number:1){{{}}}}}}}",
        fields.join(" ")
    );

    let (status, body) = graphql(addr, &query).await;

    assert_eq!(status, 200);
    assert_eq!(body["data"], Value::Null);
    assert_eq!(body["errors"][0]["message"], "Query is too complex.");
    assert_eq!(body["errors"][0]["type"], "GRAPHQL_VALIDATION_FAILED");
}
