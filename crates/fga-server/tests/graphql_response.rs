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
    let (status, body) = graphql_text(addr, query).await;
    (status, serde_json::from_str(&body).unwrap())
}

/// [`graphql`], with the body as the gateway wrote it.
async fn graphql_text(addr: SocketAddr, query: &str) -> (u16, String) {
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
    (status, body.to_owned())
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

/// Every object's keys come sorted, whatever order the document asks for them in: the bytes are
/// those of the response re-encoded through serde_json's sorted map.
#[tokio::test]
async fn object_keys_are_written_sorted() {
    let addr = gateway().await;

    let (status, body) =
        graphql_text(addr, "{viewer{name login id} __typename unknownField}").await;

    assert_eq!(status, 200);
    let sorted = serde_json::to_string(&serde_json::from_str::<Value>(&body).unwrap()).unwrap();
    assert_eq!(body, sorted);
    let (_, body) = graphql_text(addr, "{viewer{name login id} __typename}").await;
    assert!(body.contains(r#""viewer":{"id":"#), "{body}");
    assert_eq!(
        body,
        serde_json::to_string(&serde_json::from_str::<Value>(&body).unwrap()).unwrap()
    );
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

/// Validation errors arise before any field resolves, so the output budget never sees them. A
/// document that defines many variables of an unknown type, within its cost, raises more than one
/// error for each: the response lists 100 and says how many more there were.
#[tokio::test]
async fn validation_errors_are_capped() {
    let addr = gateway().await;
    let variables: Vec<String> = (0..1_500).map(|i| format!("$a{i}: Nope")).collect();
    let query = format!("query ({}) {{ viewer {{ login }} }}", variables.join(", "));
    assert!(query.len() < 512 * 1024);

    let (status, body) = graphql(addr, &query).await;

    assert_eq!(status, 200);
    let errors = body["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 101);
    assert_eq!(errors[100]["type"], "RESOURCE_LIMITS_EXCEEDED");
    assert!(
        errors[100]["message"]
            .as_str()
            .unwrap()
            .ends_with("more errors left out"),
        "{}",
        errors[100]
    );
    assert!(body.to_string().len() < 64 * 1024);
}

/// A document past 512 KiB is refused before it is parsed.
#[tokio::test]
async fn an_oversized_document_is_refused() {
    let addr = gateway().await;
    let body = json!({ "query": format!("{{ viewer {{ login }} }} #{}", "x".repeat(600 * 1024)) })
        .to_string();
    let head = format!(
        "POST /api/graphql HTTP/1.1\r\nHost: forge.test\r\nAuthorization: token disposable\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let status = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(head.as_bytes()).unwrap();
        // The gateway may answer before it has read the whole body; a failed write is fine.
        let mut writer = stream.try_clone().unwrap();
        let sender = std::thread::spawn(move || {
            let _ = writer.write_all(body.as_bytes());
        });
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        let _ = sender.join();
        String::from_utf8_lossy(&response).into_owned()
    })
    .await
    .unwrap();

    assert!(status.starts_with("HTTP/1.1 413"), "{status:.200}");
}
