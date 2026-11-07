//! The gateway counts what it can't serve by GitHub's own name for it, from the fixed vocabulary
//! (docs/telemetry.md). A count never holds an owner, repository, path or anything a client typed.
use axum::{Json, Router, routing::get};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app, usage};
use serde_json::json;
use std::collections::BTreeMap;

#[tokio::test]
async fn unsupported_requests_are_counted_by_github_s_names_only() {
    let upstream = Router::new().route(
        "/api/v1/user",
        get(|| async {
            Json(json!({"id":1,"login":"e2e","full_name":"","avatar_url":"https://forge.test/a"}))
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
    let gateway = format!("http://{}", listener.local_addr().unwrap());
    let state = AppState::new(vec![host], "3.19.0".into()).unwrap();
    tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });

    let client = reqwest::Client::new();
    let send = |method: reqwest::Method, path: &str, body: Option<serde_json::Value>| {
        let mut request = client
            .request(method, format!("{gateway}{path}"))
            .header("host", "forge.test")
            .header("authorization", "token disposable");
        if let Some(body) = body {
            request = request.json(&body);
        }
        async move { request.send().await.unwrap().status().as_u16() }
    };
    // Nothing is counted until reports are on.
    assert_eq!(
        send(
            reqwest::Method::GET,
            "/api/v3/repos/acme/secret-repo/releases/7",
            None
        )
        .await,
        404
    );
    assert!(usage::take_unsupported().is_empty());
    usage::count_unsupported();
    // No route: a GitHub resource, and a path that names nothing GitHub has.
    assert_eq!(
        send(
            reqwest::Method::GET,
            "/api/v3/repos/acme/secret-repo/releases/7",
            None
        )
        .await,
        404
    );
    assert_eq!(
        send(
            reqwest::Method::GET,
            "/api/v3/repos/acme/secret-repo/acme-private",
            None
        )
        .await,
        404
    );
    // A route without that method.
    assert_eq!(
        send(
            reqwest::Method::PATCH,
            "/api/v3/repos/acme/secret-repo",
            None
        )
        .await,
        405
    );
    // GraphQL fields the gateway lacks: one GitHub has, one it doesn't.
    for field in ["projectsV2(first: 1) { totalCount }", "acmeSecretField"] {
        assert_eq!(
            send(
                reqwest::Method::POST,
                "/api/graphql",
                Some(json!({"query": format!("{{ repository(owner: \"acme\", name: \"secret-repo\") {{ {field} }} }}")})),
            )
            .await,
            200
        );
    }
    let counts = usage::take_unsupported();
    assert_eq!(
        counts,
        BTreeMap::from([
            ("graphql:Repository.other".to_string(), 1),
            ("graphql:Repository.projectsV2".to_string(), 1),
            ("rest:other".to_string(), 1),
            ("rest:repos".to_string(), 1),
            ("rest:repos.releases".to_string(), 1),
        ]),
    );
    let text = format!("{counts:?}");
    assert!(!text.contains("acme") && !text.contains("secret"), "{text}");
    assert!(
        usage::take_unsupported().is_empty(),
        "taking empties the count"
    );
}

/// Turned off, fga-server deletes the dates it kept for reports, and sends nothing.
#[test]
fn an_opted_out_server_keeps_no_report_dates() {
    let dir = std::env::temp_dir().join(format!("fga-usage-off-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (switch, value) in [("FGA_TELEMETRY", "0"), ("DO_NOT_TRACK", "1"), ("CI", "")] {
        for name in ["telemetry-first-week", "telemetry-sent"] {
            std::fs::write(dir.join(name), "2026-W45").unwrap();
        }
        let mut server = std::process::Command::new(env!("CARGO_BIN_EXE_fga-server"))
            .env_clear()
            .env("FGA_LISTEN", "127.0.0.1:0")
            .env("FGA_FORGEJO_URL", "http://127.0.0.1:9")
            .env("FGA_PUBLIC_URL", "https://forge.test")
            .env("FGA_STATE_DIR", &dir)
            .env("FGA_TELEMETRY_URL", "http://127.0.0.1:9/v1/report")
            .env(switch, value)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while dir.join("telemetry-sent").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = server.kill();
        let _ = server.wait();
        assert!(!dir.join("telemetry-sent").exists(), "{switch}");
        assert!(!dir.join("telemetry-first-week").exists(), "{switch}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
