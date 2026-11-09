//! The gateway counts what it lacks and what it fails by GitHub's own name for it, from the fixed
//! vocabulary (docs/telemetry.md). A count never holds an owner, repository, path, error or
//! anything a client typed.
use axum::{Json, Router, http::StatusCode, routing::get};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app, usage};
use serde_json::json;
use std::collections::BTreeMap;

fn user() -> serde_json::Value {
    json!({"id":1,"login":"e2e","full_name":"","avatar_url":"https://forge.test/a"})
}

#[tokio::test]
async fn requests_are_counted_by_github_s_names_only() {
    let upstream = Router::new()
        .route("/api/v1/user", get(|| async { Json(user()) }))
        .route(
            "/api/v1/repos/acme/secret-repo",
            get(|| async {
                Json(
                    json!({"id":1,"name":"secret-repo","full_name":"acme/secret-repo",
                    "owner":{"id":2,"login":"acme","full_name":"","avatar_url":""},
                    "private":false,"html_url":"","default_branch":"main","fork":false,
                    "archived":false}),
                )
            }),
        )
        // Forgejo fails, then sends what isn't JSON: both are the gateway's failures.
        .route(
            "/api/v1/repos/acme/secret-repo/pulls/1",
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        )
        .route(
            "/api/v1/repos/acme/secret-repo/pulls/4",
            get(|| async { "<html>acme-secret-error</html>" }),
        )
        // Ordinary answers: no access, and (by the fallback) no such PR.
        .route(
            "/api/v1/repos/acme/secret-repo/pulls/3",
            get(|| async { StatusCode::FORBIDDEN }),
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
    assert!(usage::take_counts().is_empty());
    usage::start_counting();
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
    // Failed on the gateway's side: Forgejo's 500, and an answer it can't read.
    for number in [1, 4] {
        let path = format!("/api/v3/repos/acme/secret-repo/pulls/{number}");
        assert_eq!(send(reqwest::Method::GET, &path, None).await, 502);
    }
    // Ordinary answers, which aren't failures: no access, no such PR, and a bad query.
    for (path, status) in [
        ("/api/v3/repos/acme/secret-repo/pulls/3", 403),
        ("/api/v3/repos/acme/secret-repo/pulls/2", 404),
        ("/api/v3/repos/acme/secret-repo/pulls?state=acme", 422),
    ] {
        assert_eq!(
            send(reqwest::Method::GET, path, None).await,
            status,
            "{path}"
        );
    }
    // A field that failed, under an alias, and one whose PR doesn't exist.
    assert_eq!(
        send(
            reqwest::Method::POST,
            "/api/graphql",
            Some(json!({"query": "{ acmeRepo: repository(owner: \"acme\", name: \"secret-repo\") { acmeBroken: pullRequest(number: 1) { title } acmeMissing: pullRequest(number: 2) { title } } }"})),
        )
        .await,
        200
    );
    let counts = usage::take_counts();
    assert_eq!(
        counts.failed,
        BTreeMap::from([
            ("graphql:Repository.pullRequest".to_string(), 1),
            ("rest:repos.pulls".to_string(), 2),
        ]),
    );
    assert_eq!(
        counts.unsupported,
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
    assert!(usage::take_counts().is_empty(), "taking empties the counts");
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
