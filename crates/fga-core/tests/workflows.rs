use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use fga_core::{actions::workflow_id, forgejo::Forgejo};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

#[tokio::test]
async fn workflow_listing_keeps_bad_files_with_filename_names() {
    let fixtures = [
        ("plain.yml", "name: ci\non: [push]\n".into(), "ci"),
        ("quoted.yml", "name: 'ci: checks'\n".into(), "ci: checks"),
        (
            "multiline.yml",
            "name: >-\n  ci\n  checks\n".into(),
            "ci checks",
        ),
        (
            "anchored.yaml",
            "title: &title ci\nname: *title\n".into(),
            "ci",
        ),
        (
            "missing.yml",
            "on: [push]\n".into(),
            ".forgejo/workflows/missing.yml",
        ),
        (
            "oversized.yml",
            format!("name: ci\n#{}", "x".repeat(256 * 1024)),
            ".forgejo/workflows/oversized.yml",
        ),
        (
            "malformed.yml",
            "name: ci\njobs: [broken".into(),
            ".forgejo/workflows/malformed.yml",
        ),
    ];
    let files: BTreeMap<_, _> = fixtures
        .iter()
        .map(|(filename, content, _)| (filename.to_string(), content.clone()))
        .collect();
    let upstream =
        Router::new()
            .route(
                "/api/v1/user",
                get(|| async {
                    Json(json!({"id":1,"login":"e2e","avatar_url":"https://forge.test/avatar"}))
                }),
            )
            .route(
                "/api/v1/repos/o/r/contents/{*path}",
                get(
                    |Path(path): Path<String>,
                     State(files): State<Arc<BTreeMap<String, String>>>| async move {
                        let value = if path == ".forgejo/workflows" {
                            Value::Array(
                                files
                                    .keys()
                                    .map(|name| json!({"name":name,"type":"file"}))
                                    .collect(),
                            )
                        } else {
                            let filename = path.strip_prefix(".forgejo/workflows/").unwrap();
                            json!({"content":STANDARD.encode(&files[filename])})
                        };
                        Json(value)
                    },
                ),
            )
            .with_state(Arc::new(files));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let session = Forgejo::new(url)
        .unwrap()
        .authenticate("token disposable")
        .await
        .unwrap();
    let workflows = session.workflows("o", "r").await.unwrap();
    assert_eq!(workflows.len(), fixtures.len());
    for (filename, _, name) in fixtures {
        let workflow = &workflows[filename];
        assert_eq!(workflow.name, name);
        assert_eq!(workflow.path, format!(".forgejo/workflows/{filename}"));
        assert_eq!(workflow.id, workflow_id(filename));
        assert_eq!(workflow.state, "active");
    }
    task.abort();
}

/// Forgejo reads the first workflow directory that exists, even an empty one, and knows
/// `.gitea/workflows` (`modules/actions/workflows.go:62-102`).
#[tokio::test]
async fn workflow_discovery_stops_at_the_first_existing_directory() {
    for (directories, expected) in [
        (
            vec![".gitea/workflows", ".github/workflows"],
            Some(".gitea/workflows/ci.yml"),
        ),
        (vec![".forgejo/workflows", ".github/workflows"], None),
    ] {
        let upstream = Router::new()
            .route(
                "/api/v1/user",
                get(|| async { Json(json!({"id":1,"login":"e2e","avatar_url":""})) }),
            )
            .route(
                "/api/v1/repos/o/r/contents/{*path}",
                get(move |Path(path): Path<String>| {
                    let directories = directories.clone();
                    async move {
                        if path == ".forgejo/workflows" && directories.contains(&path.as_str()) {
                            Json(json!([{"name":"README.md","type":"file"}])).into_response()
                        } else if directories.contains(&path.as_str()) {
                            Json(json!([{"name":"ci.yml","type":"file"}])).into_response()
                        } else if path.ends_with("/ci.yml") {
                            Json(json!({"content":STANDARD.encode("name: ci\n")})).into_response()
                        } else {
                            StatusCode::NOT_FOUND.into_response()
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let session = Forgejo::new(url)
            .unwrap()
            .authenticate("token disposable")
            .await
            .unwrap();
        let workflows = session.workflows("o", "r").await.unwrap();
        assert_eq!(workflows.values().map(|w| w.path.as_str()).next(), expected);
        task.abort();
    }
}
