//! The gh calls coding agents make on a pull request, and those ChatGPT desktop's PR-code tab
//! makes, against Forgejo 15's own replies (`fixtures/forgejo15-agent-pr.json`): PR #1 of
//! `e2e/demo`, three commits across four files, with an inline review comment on
//! `src/cli/commands.ts` line 45 and one on the old side of `README.md` line 3.
use axum::{
    Router,
    http::{StatusCode, Uri},
    response::IntoResponse,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::LazyLock,
};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("fixtures/forgejo15-agent-pr.json")).unwrap()
});

fn base() -> &'static str {
    FIXTURE["base"].as_str().unwrap()
}
fn head() -> &'static str {
    FIXTURE["head"].as_str().unwrap()
}

/// Forgejo, answering each captured path with its captured reply and anything else with 404.
async fn upstream(uri: Uri) -> axum::response::Response {
    match FIXTURE["routes"].get(uri.path()) {
        Some(Value::String(text)) => text.clone().into_response(),
        Some(value) => axum::Json(value.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"message": "not found"})),
        )
            .into_response(),
    }
}

async fn gateway() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(upstream))
            .await
            .unwrap()
    });
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

struct Reply {
    status: u16,
    headers: String,
    body: String,
}
impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("JSON: {}", self.body))
    }
}

async fn send(
    addr: SocketAddr,
    method: &str,
    path: &str,
    accept: &str,
    body: Option<Value>,
) -> Reply {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: forge.test\r\nAuthorization: token t\r\nAccept: {accept}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        String::from_utf8(response).unwrap()
    })
    .await
    .unwrap();
    let (head, rest) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(rest)
    } else {
        rest.to_owned()
    };
    Reply {
        status,
        headers: head.to_owned(),
        body,
    }
}

fn dechunk(mut rest: &str) -> String {
    let mut out = String::new();
    while let Some((size, tail)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap();
        if size == 0 {
            break;
        }
        out.push_str(&tail[..size]);
        rest = &tail[size + 2..];
    }
    out
}

async fn get(addr: SocketAddr, path: &str) -> Reply {
    send(addr, "GET", path, "application/vnd.github+json", None).await
}

async fn graphql(addr: SocketAddr, request: Value) -> Value {
    let reply = send(
        addr,
        "POST",
        "/api/graphql",
        "application/json",
        Some(request),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let value = reply.json();
    assert!(value.get("errors").is_none(), "{value}");
    value
}

/// Codex's `gh pr view 1 --json number,title,url,body,reviews,comments,files` and
/// `--json title,body,baseRefName,headRefName,headRefOid,files,url`, as gh 2.103.3 sends them,
/// failed on `files`.
#[tokio::test]
async fn pr_view_json_files_lists_each_file_with_its_counts() {
    let addr = gateway().await;
    let expected = json!([
        {"path": "README.md", "additions": 1, "deletions": 1, "changeType": "MODIFIED"},
        {"path": "docs/guide.md", "additions": 0, "deletions": 3, "changeType": "DELETED"},
        {"path": "src/cli/commands.ts", "additions": 11, "deletions": 11, "changeType": "MODIFIED"},
        {"path": "src/util.ts", "additions": 1, "deletions": 0, "changeType": "ADDED"},
    ]);
    for (_, request) in FIXTURE["gh_requests"].as_object().unwrap() {
        let pr = &graphql(addr, request.clone()).await["data"]["repository"]["pullRequest"];
        assert_eq!(pr["files"]["nodes"], expected);
    }
    let pr = &graphql(
        addr,
        FIXTURE["gh_requests"]["pr view 1 --json number,title,url,body,reviews,comments,files"]
            .clone(),
    )
    .await["data"]["repository"]["pullRequest"];
    assert_eq!(pr["reviews"]["nodes"][0]["state"], "COMMENTED");
    assert_eq!(pr["comments"]["nodes"][0]["body"], "Thanks for the review.");
}

/// Codex's `gh api repos/O/R/pulls/1/comments` was a 404.
#[tokio::test]
async fn rest_review_comments_carry_their_path_and_line() {
    let addr = gateway().await;
    let reply = get(addr, "/api/v3/repos/e2e/demo/pulls/1/comments").await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let comments = reply.json();
    let comments = comments.as_array().unwrap();
    assert_eq!(comments.len(), 2);
    let inline = &comments[0];
    for (field, value) in [
        ("path", json!("src/cli/commands.ts")),
        ("line", json!(45)),
        ("original_line", json!(45)),
        ("side", json!("RIGHT")),
        ("commit_id", json!(head())),
        ("original_commit_id", json!(head())),
        ("body", json!("Why scale line 45?")),
        (
            "html_url",
            json!("https://forge.test/e2e/demo/pull/1#issuecomment-2"),
        ),
        ("pull_request_review_id", json!(1)),
        ("subject_type", json!("line")),
    ] {
        assert_eq!(inline[field], value, "{field}");
    }
    assert_eq!(inline["user"]["login"], "reviewer");
    assert!(
        inline["diff_hunk"]
            .as_str()
            .unwrap()
            .ends_with("+export const line45 = 450;")
    );
    assert!(inline["created_at"].is_string());
    // The first comment of a conversation replies to nothing.
    assert!(inline.get("in_reply_to_id").is_none());
    let old_side = &comments[1];
    assert_eq!(old_side["path"], "README.md");
    assert_eq!(old_side["line"], 3);
    assert_eq!(old_side["side"], "LEFT");
}

/// Codex's review-thread query failed with `Unknown field "path" / "line" on type
/// "PullRequestReviewComment"`.
#[tokio::test]
async fn review_thread_comments_carry_their_path_line_and_url() {
    let addr = gateway().await;
    let query = r#"query{repository(owner:"e2e",name:"demo"){pullRequest(number:1){ reviewThreads(first:100){ nodes{ isResolved comments(first:100){ nodes{ body path line url } } } } }}}"#;
    let data = graphql(addr, json!({"query": query})).await;
    assert_eq!(
        data["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"],
        json!([
            {"isResolved": false, "comments": {"nodes": [{"body": "Why scale line 45?",
                "path": "src/cli/commands.ts", "line": 45,
                "url": "https://forge.test/e2e/demo/pull/1#issuecomment-2"}]}},
            {"isResolved": false, "comments": {"nodes": [{"body": "The old wording was clearer.",
                "path": "README.md", "line": 3,
                "url": "https://forge.test/e2e/demo/pull/1#issuecomment-3"}]}},
        ])
    );
    let query = r#"query{repository(owner:"e2e",name:"demo"){pullRequest(number:1){ reviewThreads(first:1){ nodes{ path line originalLine startLine diffSide isOutdated subjectType comments(first:1){ nodes{ originalLine startLine originalStartLine outdated subjectType diffHunk commit{oid} originalCommit{oid} fullDatabaseId } } } } }}}"#;
    let thread = &graphql(addr, json!({"query": query})).await["data"]["repository"]["pullRequest"]
        ["reviewThreads"]["nodes"][0];
    assert_eq!(thread["diffSide"], "RIGHT");
    assert_eq!(thread["isOutdated"], false);
    assert_eq!(thread["startLine"], Value::Null);
    let comment = &thread["comments"]["nodes"][0];
    assert_eq!(comment["outdated"], false);
    assert_eq!(comment["commit"]["oid"], head());
    assert_eq!(comment["originalCommit"]["oid"], head());
    assert_eq!(comment["fullDatabaseId"], "2");
    assert_eq!(comment["subjectType"], "LINE");
}

/// ChatGPT's PR-code tab reads the pull request's `base.sha` and `head.sha`.
#[tokio::test]
async fn rest_pull_names_its_base_and_head_commits() {
    let addr = gateway().await;
    let pr = get(addr, "/api/v3/repos/e2e/demo/pulls/1").await.json();
    assert_eq!(pr["base"]["sha"], base());
    assert_eq!(pr["head"]["sha"], head());
}

/// The PR-code tab's `gh api repos/O/R/compare/BASE...HEAD?per_page=100 --paginate --slurp`.
#[tokio::test]
async fn compare_names_the_merge_base_commits_and_files() {
    let addr = gateway().await;
    let reply = get(
        addr,
        &format!(
            "/api/v3/repos/e2e/demo/compare/{}...{}?per_page=100",
            base(),
            head()
        ),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    // One page, so `--paginate` stops after it.
    assert!(!reply.headers.to_ascii_lowercase().contains("\nlink:"));
    let compare = reply.json();
    assert_eq!(compare["merge_base_commit"]["sha"], base());
    assert_eq!(compare["base_commit"]["sha"], base());
    assert_eq!(compare["status"], "ahead");
    assert_eq!(compare["ahead_by"], 3);
    assert_eq!(compare["behind_by"], 0);
    assert_eq!(compare["total_commits"], 3);
    let commits = compare["commits"].as_array().unwrap();
    assert_eq!(commits[0]["parents"][0]["sha"], base());
    assert_eq!(
        commits[0]["commit"]["message"],
        "commands: scale lines 40 to 50"
    );
    assert_eq!(commits[2]["sha"], head());
    assert_eq!(
        commits[2]["commit"]["message"],
        "docs: drop the old guide\n\nThe README covers it now."
    );
    for pair in commits.windows(2) {
        assert_eq!(pair[1]["parents"][0]["sha"], pair[0]["sha"]);
    }
    let files = compare["files"].as_array().unwrap();
    let names: Vec<_> = files
        .iter()
        .map(|f| f["filename"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "README.md",
            "docs/guide.md",
            "src/cli/commands.ts",
            "src/util.ts"
        ]
    );
    assert_eq!(files[1]["status"], "removed");
    assert_eq!(files[3]["status"], "added");
    assert_eq!(files[2]["additions"], 11);
    assert!(
        files[2]["patch"]
            .as_str()
            .unwrap()
            .contains("+export const line45 = 450;")
    );
}

/// The same comparison with `Accept: application/vnd.github.diff` is the pull request's diff.
#[tokio::test]
async fn compare_as_a_diff_is_the_pull_requests_diff() {
    let addr = gateway().await;
    let path = format!(
        "/api/v3/repos/e2e/demo/compare/{}...{}?per_page=100",
        base(),
        head()
    );
    let reply = send(addr, "GET", &path, "application/vnd.github.diff", None).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(
        reply.body,
        FIXTURE["routes"]["/api/v1/repos/e2e/demo/pulls/1.diff"]
    );
    // A comparison that is no pull request's has no diff Forgejo will give.
    let path = format!("/api/v3/repos/e2e/demo/compare/{}...{}", head(), base());
    let reply = send(addr, "GET", &path, "application/vnd.github.diff", None).await;
    assert_eq!(reply.status, 422, "{}", reply.body);
}

/// The PR-code tab's `gh api repos/O/R/contents/PATH?ref=SHA -H "Accept:
/// application/vnd.github.object+json"`.
#[tokio::test]
async fn contents_give_a_files_base64_at_a_commit() {
    let addr = gateway().await;
    let path = format!(
        "/api/v3/repos/e2e/demo/contents/src/cli/commands.ts?ref={}",
        head()
    );
    let reply = send(
        addr,
        "GET",
        &path,
        "application/vnd.github.object+json",
        None,
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let file = reply.json();
    assert_eq!(file["type"], "file");
    assert_eq!(file["encoding"], "base64");
    assert_eq!(file["path"], "src/cli/commands.ts");
    let content: String = file["content"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .collect();
    let text = String::from_utf8(STANDARD.decode(content).unwrap()).unwrap();
    assert!(text.contains("export const line45 = 450;\n"));
    assert_eq!(file["size"], text.len());
    let missing = format!("/api/v3/repos/e2e/demo/contents/nope.txt?ref={}", head());
    assert_eq!(get(addr, &missing).await.status, 404);
    // A directory with `object+json` is one object holding its entries.
    let dir = format!("/api/v3/repos/e2e/demo/contents/src?ref={}", head());
    let dir = send(
        addr,
        "GET",
        &dir,
        "application/vnd.github.object+json",
        None,
    )
    .await
    .json();
    assert_eq!(dir["type"], "dir");
    assert!(
        dir["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "cli")
    );
}

/// The PR-code tab's `object(expression: "SHA:dir/.gitattributes") { ... on Blob { text
/// isTruncated } }`: a missing path is null, as on GitHub.
#[tokio::test]
async fn object_expressions_read_blobs_and_missing_paths_are_null() {
    let addr = gateway().await;
    let query = format!(
        r#"query{{repository(owner:"e2e",name:"demo"){{f0: object(expression:"{h}:.gitattributes"){{... on Blob{{text isTruncated}}}} f1: object(expression:"{h}:src/.gitattributes"){{... on Blob{{text isTruncated}}}} f2: object(expression:"{h}:src/cli/.gitattributes"){{... on Blob{{text isTruncated}}}} f3: object(expression:"{h}"){{oid}}}}}}"#,
        h = head()
    );
    let data = graphql(addr, json!({"query": query})).await;
    assert_eq!(
        data["data"]["repository"],
        json!({
            "f0": {"text": "*.ts text eol=lf\n", "isTruncated": false},
            "f1": {"text": "*.snap binary\n", "isTruncated": false},
            "f2": null,
            "f3": {"oid": head()},
        })
    );
}

/// Other REST reads agents run on a pull request.
#[tokio::test]
async fn pull_files_reviews_commits_and_conversation_read_as_on_github() {
    let addr = gateway().await;
    let files = get(addr, "/api/v3/repos/e2e/demo/pulls/1/files")
        .await
        .json();
    assert_eq!(files[0]["filename"], "README.md");
    assert_eq!(files[0]["status"], "modified");
    assert_eq!(files[0]["changes"], 2);
    assert_eq!(
        files[0]["patch"],
        "@@ -1,3 +1,3 @@\n # demo\n \n-A small CLI.\n+A small CLI with helpers."
    );
    assert_eq!(
        files[0]["contents_url"],
        format!(
            "https://forge.test/api/v3/repos/e2e/demo/contents/README.md?ref={}",
            head()
        )
    );
    let reviews = get(addr, "/api/v3/repos/e2e/demo/pulls/1/reviews")
        .await
        .json();
    assert_eq!(reviews[0]["state"], "COMMENTED");
    assert_eq!(reviews[0]["user"]["login"], "reviewer");
    assert_eq!(reviews[0]["commit_id"], head());
    let commits = get(addr, "/api/v3/repos/e2e/demo/pulls/1/commits")
        .await
        .json();
    assert_eq!(
        commits[0]["commit"]["message"],
        "commands: scale lines 40 to 50"
    );
    assert_eq!(commits[2]["sha"], head());
    let comments = get(addr, "/api/v3/repos/e2e/demo/issues/1/comments")
        .await
        .json();
    assert_eq!(comments[0]["body"], "Thanks for the review.");
    assert_eq!(comments[0]["user"]["login"], "e2e");
    // Paging follows GitHub's: `per_page`, `page` and a `Link` header.
    let page = get(
        addr,
        "/api/v3/repos/e2e/demo/pulls/1/comments?per_page=1&page=2",
    )
    .await;
    assert_eq!(page.json()[0]["path"], "README.md");
    assert!(page.headers.contains("rel=\"prev\""), "{}", page.headers);
}
