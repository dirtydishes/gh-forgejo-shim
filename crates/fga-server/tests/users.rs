//! `gh repo create OWNER/NAME` reads the owner with `GET users/{owner}` (`resolveOwner` in
//! `pkg/cmd/repo/create/http.go`, gh 2.103.3) and sends its `node_id` as `createRepository`'s
//! `ownerId`, creating in an organization when `type` is `Organization`. Forgejo answers
//! `users/{name}` for organizations too, so only `orgs/{name}` tells them apart.
use axum::{Json, Router, http::StatusCode, routing::get};
use fga_core::{forgejo::Forgejo, ids::node_id, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
};

async fn gateway() -> SocketAddr {
    let account = |id: i64, login: &str| {
        json!({"id":id,"login":login,"full_name":"","avatar_url":"https://forge.test/avatar",
            "html_url":format!("https://forge.test/{login}")})
    };
    let (viewer, acme) = (account(1, "o"), account(5, "acme"));
    let user = viewer.clone();
    let upstream = Router::new()
        .route("/api/v1/user", get(move || async move { Json(user) }))
        .route("/api/v1/users/o", get(move || async move { Json(viewer) }))
        .route("/api/v1/users/acme", get(move || async move { Json(acme) }))
        .route(
            "/api/v1/users/ghost",
            get(|| async {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"message":"user does not exist"})),
                )
            }),
        )
        .route(
            "/api/v1/orgs/acme",
            get(|| async { Json(json!({"id":5,"name":"acme","username":"acme"})) }),
        )
        .route("/api/v1/orgs/o", get(|| async { StatusCode::NOT_FOUND }));
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
async fn a_user_reads_as_github_does_with_the_id_graphql_gives_it() {
    let addr = gateway().await;
    let (code, user) = get_json(addr, "/api/v3/users/o").await;
    assert_eq!(code, 200, "{user}");
    assert_eq!(user["login"], "o");
    assert_eq!(user["type"], "User");
    assert_eq!(user["node_id"], node_id("User", "", 1));
    assert_eq!(user["html_url"], "https://forge.test/o");
}

#[tokio::test]
async fn an_organization_reads_with_its_type_and_organization_id() {
    let addr = gateway().await;
    let (code, org) = get_json(addr, "/api/v3/users/acme").await;
    assert_eq!(code, 200, "{org}");
    assert_eq!(org["login"], "acme");
    assert_eq!(org["type"], "Organization");
    assert_eq!(org["node_id"], node_id("Organization", "", 5));
}

#[tokio::test]
async fn a_missing_account_is_githubs_not_found() {
    let addr = gateway().await;
    let (code, body) = get_json(addr, "/api/v3/users/ghost").await;
    assert_eq!((code, &body["message"]), (404, &json!("Not Found")));
}
