//! ChatGPT desktop finds a branch's pull request with real gh 2.103.3:
//! `gh pr list --head <branch> --author @me --state all --json number,url,state,headRefName --repo
//! <forge>/<o>/<r>`. `--author` sends gh down its search path (`searchPullRequests` in
//! `pkg/cmd/pr/list/http.go`). Here the forge refuses the issue list, as Forgejo does for a token
//! without `read:issue`, which a plain `pr list` never needs. gh reaches the gateway over its
//! `http_unix_socket` setting.
use axum::{
    Json, Router, extract::RawQuery, http::StatusCode, response::IntoResponse, routing::get,
};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn real_gh() -> Option<PathBuf> {
    let gh = std::env::var_os("GFJS_REAL_GH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/gh"));
    let version = Command::new(&gh).arg("--version").output().ok()?;
    String::from_utf8_lossy(&version.stdout)
        .contains("gh version 2.103.3")
        .then_some(gh)
}

/// HOME and the XDG directories for a program the test runs, so its state stays in the test's temp
/// directory and never lands in the repository or the user's home.
fn isolated<'a>(command: &'a mut Command, home: &Path) -> &'a mut Command {
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
}

fn pull(number: i64, head: &str, author: &str, state: &str, merged: bool) -> Value {
    let user = |login: &str| json!({"id":1,"login":login,"full_name":"","avatar_url":""});
    let repo = json!({"name":"r","owner":user("o")});
    json!({"number":number,"title":format!("pr {number}"),"state":state,"merged":merged,
        "draft":false,"html_url":format!("https://forge.test/o/r/pulls/{number}"),
        "user":user(author),"merged_at":if merged {json!("2026-11-03T00:00:00Z")} else {Value::Null},
        "allow_maintainer_edit":false,
        "head":{"label":head,"ref":head,"sha":format!("{number:040}"),"repo":repo},
        "base":{"label":"main","ref":"main","sha":"0".repeat(40),"repo":repo}})
}

#[tokio::test]
#[ignore = "needs real gh 2.103.3"]
async fn chatgpt_finds_its_branch_pr_without_issue_scope() {
    let Some(gh) = real_gh() else {
        eprintln!("skipped: no gh 2.103.3 (set GFJS_REAL_GH)");
        return;
    };
    let root = std::env::temp_dir().join(format!("fga-chatgpt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let user = json!({"id":1,"login":"o","full_name":"","avatar_url":""});
    let repository = json!({"id":1,"name":"r","full_name":"o/r","private":false,"owner":user,
        "html_url":"https://forge.test/o/r","default_branch":"main","fork":false,"archived":false,
        "permissions":{"admin":false,"push":true,"pull":true}});
    // The viewer's PRs from s/pass, open and merged; another branch; someone else's from s/pass.
    let pulls = [
        pull(4, "s/pass", "o", "open", false),
        pull(3, "other", "o", "open", false),
        pull(2, "s/pass", "someone", "open", false),
        pull(1, "s/pass", "o", "closed", true),
    ];
    let upstream_api = Router::new()
        .route("/api/v1/user", get(move || async move { Json(user) }))
        .route(
            "/api/v1/repos/o/r",
            get(move || async move { Json(repository) }),
        )
        // Forgejo's answer to a token without issue scope.
        .route(
            "/api/v1/repos/o/r/issues",
            get(|| async {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({"message":"token does not have at least one of required scope(s): [read:issue]"})),
                )
            }),
        )
        .route(
            "/api/v1/repos/o/r/pulls",
            get(move |RawQuery(query): RawQuery| {
                let query = query.unwrap_or_default();
                let poster = url::form_urlencoded::parse(query.as_bytes())
                    .find(|(k, _)| k == "poster")
                    .map(|(_, v)| v.into_owned());
                let page = url::form_urlencoded::parse(query.as_bytes())
                    .find(|(k, _)| k == "page")
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_else(|| "1".into());
                let listed: Vec<Value> = if page == "1" {
                    pulls
                        .iter()
                        .filter(|p| poster.as_ref().is_none_or(|who| p["user"]["login"] == *who))
                        .cloned()
                        .collect()
                } else {
                    Vec::new()
                };
                async move { Json(listed).into_response() }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let forgejo = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, upstream_api).await.unwrap() });
    let host = Host::new(
        "https://forge.test".parse().unwrap(),
        Forgejo::new(forgejo.parse().unwrap()).unwrap(),
        WipPrefixes::default(),
    );
    let socket = root.join("gw.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let state = AppState::new(vec![host], "3.19.0".into()).unwrap();
    tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });

    let config = root.join("gh");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.yml"),
        format!("http_unix_socket: {}\n", socket.display()),
    )
    .unwrap();
    std::fs::write(
        config.join("hosts.yml"),
        "forge.test:\n    oauth_token: t\n    user: o\n",
    )
    .unwrap();
    let output = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(gh);
        command.env_clear();
        isolated(&mut command, &root)
            .args([
                "pr",
                "list",
                "--head",
                "s/pass",
                "--author",
                "@me",
                "--state",
                "all",
                "--json",
                "number,url,state,headRefName",
                "--repo",
                "forge.test/o/r",
            ])
            .current_dir(&root)
            .env("PATH", std::env::var("PATH").unwrap())
            .env("GH_CONFIG_DIR", &config)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("GH_DEBUG", "api")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    // gh's own request, as GH_DEBUG logs it.
    for line in stderr
        .lines()
        .filter(|l| l.contains("PullRequestSearch") || l.contains("\"q\""))
    {
        eprintln!("REQUEST {line}");
    }
    assert!(output.status.success(), "{stderr}");
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        listed,
        json!([
            {"headRefName":"s/pass","number":4,"state":"OPEN","url":"https://forge.test/o/r/pull/4"},
            {"headRefName":"s/pass","number":1,"state":"MERGED","url":"https://forge.test/o/r/pull/1"}
        ])
    );
}
