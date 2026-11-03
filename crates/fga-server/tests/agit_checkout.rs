//! What real gh 2.103.3 does when it checks out an AGit PR through the gateway, end to end. An AGit
//! PR has no branch, so the gateway reports Forgejo's `refs/pull/N/head` as `headRefName` and no
//! head repository, and gh takes its missing-remote path (`cmdsForMissingRemote` in
//! `pkg/cmd/pr/checkout/checkout.go`). gh reaches the gateway over its `http_unix_socket` setting;
//! git fetches the PR from a local bare repository standing in for the forge.
use axum::{Json, Router, routing::get};
use fga_core::{forgejo::Forgejo, map::WipPrefixes};
use fga_server::{AppState, Host, app};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
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

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[tokio::test]
#[ignore = "needs real gh 2.103.3 and git"]
async fn an_agit_checkout_lands_detached_on_the_pr_head() {
    let Some(gh) = real_gh() else {
        eprintln!("skipped: no gh 2.103.3 (set GFJS_REAL_GH)");
        return;
    };
    let root = std::env::temp_dir().join(format!("fga-agit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // The forge's repository: main at A, and the AGit PR's ref refs/pull/3/head at C.
    let upstream = root.join("up.git");
    let seed = root.join("seed");
    assert!(
        git(
            &root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                upstream.to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["commit", "-q", "--allow-empty", "-m", "A"]);
    let a = text(&git(&seed, &["rev-parse", "HEAD"]));
    git(&seed, &["commit", "-q", "--allow-empty", "-m", "C"]);
    let c = text(&git(&seed, &["rev-parse", "HEAD"]));
    git(
        &seed,
        &[
            "push",
            "-q",
            upstream.to_str().unwrap(),
            &format!("{a}:refs/heads/main"),
            &format!("{c}:refs/pull/3/head"),
        ],
    );
    // The user's clone, as if cloned from the forge over SSH: gh matches the remote to the host
    // and fetches through it, and a stand-in for ssh serves the bare repository.
    let work = root.join("work");
    let clone = git(
        &root,
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    assert!(clone.status.success());
    git(
        &work,
        &[
            "remote",
            "set-url",
            "origin",
            "ssh://git@forge.test/o/r.git",
        ],
    );
    let ssh = root.join("ssh");
    std::fs::write(
        &ssh,
        format!("#!/bin/sh\nexec git-upload-pack '{}'\n", upstream.display()),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let user = json!({"id":1,"login":"o","full_name":"","avatar_url":""});
    let repository = json!({"id":1,"name":"r","full_name":"o/r","private":false,"owner":user,
        "html_url":"https://forge.test/o/r","default_branch":"main","fork":false,"archived":false,
        "permissions":{"admin":false,"push":true,"pull":true}});
    let pull = json!({"number":3,"title":"agit","state":"open","merged":false,"draft":false,
        "html_url":"https://forge.test/o/r/pulls/3","user":user,"merged_at":null,
        "allow_maintainer_edit":false,
        "head":{"label":"","ref":"refs/pull/3/head","sha":c,"repo":{"name":"r","owner":user}},
        "base":{"label":"main","ref":"main","sha":a,"repo":{"name":"r","owner":user}}});
    let upstream_api = Router::new()
        .route("/api/v1/user", get(move || async move { Json(user) }))
        .route(
            "/api/v1/repos/o/r",
            get(move || async move { Json(repository) }),
        )
        .route(
            "/api/v1/repos/o/r/pulls/3",
            get(move || async move { Json(pull) }),
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
        format!(
            "http_unix_socket: {}\ngit_protocol: ssh\n",
            socket.display()
        ),
    )
    .unwrap();
    // A disposable token for the fake forge; the gateway sends it to Forgejo as is.
    std::fs::write(
        config.join("hosts.yml"),
        "forge.test:\n    oauth_token: t\n    git_protocol: ssh\n    user: o\n",
    )
    .unwrap();
    let checkout = {
        let (gh, work, config, root, ssh) = (
            gh.clone(),
            work.clone(),
            config.clone(),
            root.clone(),
            ssh.clone(),
        );
        tokio::task::spawn_blocking(move || {
            Command::new(gh)
                .args(["pr", "checkout", "3", "-R", "forge.test/o/r"])
                .current_dir(&work)
                .env_clear()
                .env("PATH", std::env::var("PATH").unwrap())
                .env("HOME", &root)
                .env("GH_CONFIG_DIR", &config)
                .env("GH_PROMPT_DISABLED", "1")
                .env("GH_NO_UPDATE_NOTIFIER", "1")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_SSH_COMMAND", &ssh)
                .output()
                .unwrap()
        })
        .await
        .unwrap()
    };
    let head = text(&git(&work, &["rev-parse", "HEAD"]));
    let branch = git(&work, &["symbolic-ref", "-q", "HEAD"]);
    let refs = text(&git(&work, &["for-each-ref", "--format=%(refname)"]));
    let branches = text(&git(&work, &["config", "--get-regexp", "^branch\\."]));
    eprintln!(
        "AGIT exit={:?}\nstdout={}\nstderr={}\nHEAD={head} (C={c})\nsymbolic-ref: {} {}\nrefs:\n{refs}\nbranch config:\n{branches}",
        checkout.status.code(),
        String::from_utf8_lossy(&checkout.stdout),
        String::from_utf8_lossy(&checkout.stderr),
        branch.status.code().unwrap_or(-1),
        text(&branch),
    );
    let _ = std::fs::remove_dir_all(&root);
    assert!(checkout.status.success(), "gh pr checkout failed");
    // HEAD is the PR's head, detached: gh fetched the PR's ref into the raw ref
    // `refs/pull/3/head` and checked that out, so no local branch was made.
    assert_eq!(head, c, "HEAD is not the PR's head");
    assert!(
        !branch.status.success(),
        "HEAD is on a branch: {}",
        text(&branch)
    );
    let refs: Vec<&str> = refs.lines().collect();
    assert!(refs.contains(&"refs/pull/3/head"), "{refs:?}");
    assert!(
        !refs.iter().any(|r| r.starts_with("refs/heads/refs/")),
        "{refs:?}"
    );
    // gh still writes tracking config for that name, as for a branch.
    assert!(
        branches.contains("branch.refs/pull/3/head.merge refs/pull/3/head"),
        "{branches}"
    );
}
