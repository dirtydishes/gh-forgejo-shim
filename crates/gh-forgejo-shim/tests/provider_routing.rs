#![allow(dead_code)]

mod support;

use std::io;
use std::process::Output;

use support::{start_json_server, CliFixture, TestResult};

struct RouteRunner {
    fixture: CliFixture,
}

impl RouteRunner {
    fn new() -> io::Result<Self> {
        let fixture = CliFixture::new()?;
        fixture.write_executable(
            "gh",
            "#!/bin/sh\nprintf 'delegated-to-github\\n'\nexit 23\n",
        )?;
        Ok(Self { fixture })
    }

    fn configure_forgejo(&self, api_root: &str) -> TestResult {
        let output = self
            .fixture
            .command("gfj")?
            .args([
                "config",
                "add-host",
                "git.example.com",
                "--api-root",
                api_root,
            ])
            .output()?;
        assert_eq!(
            output.status.code(),
            Some(0),
            "host setup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    fn run(&self, env_key: &str, env_value: &str, argv: &[&str]) -> io::Result<Output> {
        self.fixture
            .command("gh-forgejo-shim")?
            .env_remove("FJ_SHIM_REAL_GH")
            .env_remove("FJ_SHIM_HOSTS")
            .env("FJ_SHIM_TOKEN", "test-token")
            .env(env_key, env_value)
            .arg("gh")
            .args(argv)
            .output()
    }
}

#[test]
fn attached_and_clustered_repo_flags_run_locally_against_the_selected_repo() -> TestResult {
    const ISSUE: &str = r#"{"number":13,"title":"Grammar","state":"open","html_url":"https://git.example.com/selected/repo/issues/13","user":{"login":"alice"},"body":""}"#;

    for (name, repo_flag) in [
        ("attached value", "-Rgit.example.com/selected/repo"),
        (
            "boolean cluster then value",
            "-cRgit.example.com/selected/repo",
        ),
    ] {
        let (api_host, handle) = start_json_server(vec![ISSUE])?;
        let runner = RouteRunner::new()?;
        runner.configure_forgejo(&format!("http://{api_host}/api/v1"))?;

        let output = runner.run(
            "GH_REPO",
            "github.com/base/repo",
            &["issue", "view", repo_flag, "13", "--json", "number"],
        )?;
        let requests = handle
            .join()
            .map_err(|_| io::Error::other("fake server thread panicked"))??;

        assert_eq!(
            output.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, "{\"number\": 13}\n");
        assert_eq!(String::from_utf8(output.stderr)?, "");
        assert_eq!(
            requests,
            ["GET /api/v1/repos/selected/repo/issues/13 HTTP/1.1"],
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn clustered_pr_flags_run_locally_against_the_selected_repo() -> TestResult {
    let (api_host, handle) = start_json_server(vec![r#"[]"#])?;
    let runner = RouteRunner::new()?;
    runner.configure_forgejo(&format!("http://{api_host}/api/v1"))?;

    let output = runner.run(
        "GH_REPO",
        "github.com/base/repo",
        &[
            "pr",
            "list",
            "-dRgit.example.com/selected/repo",
            "--json",
            "number",
        ],
    )?;
    let requests = handle
        .join()
        .map_err(|_| io::Error::other("fake server thread panicked"))??;

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout)?, "[]\n");
    assert_eq!(String::from_utf8(output.stderr)?, "");
    assert_eq!(
        requests,
        ["GET /api/v1/repos/selected/repo/pulls?state=open HTTP/1.1"]
    );
    Ok(())
}

#[test]
fn repo_view_repo_flags_run_locally_against_the_selected_repo() -> TestResult {
    const REPO: &str = r#"{"name":"repo","full_name":"selected/repo","html_url":"https://git.example.com/selected/repo","owner":{"login":"selected"}}"#;

    for (name, repo_args) in [
        (
            "short repository selector",
            &["-R", "git.example.com/selected/repo"][..],
        ),
        (
            "long repository selector",
            &["--repo=git.example.com/selected/repo"][..],
        ),
    ] {
        let (api_host, handle) = start_json_server(vec![REPO])?;
        let runner = RouteRunner::new()?;
        runner.configure_forgejo(&format!("http://{api_host}/api/v1"))?;
        let mut argv = vec!["repo", "view"];
        argv.extend_from_slice(repo_args);
        argv.extend_from_slice(&["--json", "name"]);

        let output = runner.run("GH_REPO", "github.com/base/repo", &argv)?;
        let requests = handle
            .join()
            .map_err(|_| io::Error::other("fake server thread panicked"))??;

        assert_eq!(
            output.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, "{\"name\": \"repo\"}\n");
        assert_eq!(String::from_utf8(output.stderr)?, "");
        assert_eq!(
            requests,
            ["GET /api/v1/repos/selected/repo HTTP/1.1"],
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn malformed_explicit_repo_selectors_never_fall_back_to_context() -> TestResult {
    let runner = RouteRunner::new()?;
    let output = runner.run(
        "GH_REPO",
        "github.com/base/repo",
        &["issue", "view", "--repo=", "13"],
    )?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;

    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("invalid repository selector"), "{stderr}");
    assert!(!stderr.contains("delegated-to-github"), "{stderr}");

    const ISSUE: &str = r#"{"number":13,"title":"Wrong repository","state":"open","html_url":"https://git.example.com/owner/repo/issues/13","user":{"login":"alice"},"body":""}"#;
    let (api_host, handle) = start_json_server(vec![ISSUE])?;
    let runner = RouteRunner::new()?;
    runner.fixture.init_git_repo()?;
    runner.configure_forgejo(&format!("http://{api_host}/api/v1"))?;
    let output = runner
        .fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .env_remove("FJ_SHIM_HOSTS")
        .env("FJ_SHIM_TOKEN", "test-token")
        .arg("gh")
        .args(["issue", "view", "--repo", "not-a-repository", "13"])
        .output()?;
    let requests = handle
        .join()
        .map_err(|_| io::Error::other("fake server thread panicked"))??;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;

    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("invalid repository selector"), "{stderr}");
    assert_eq!(requests, Vec::<String>::new());
    Ok(())
}

#[test]
fn ambiguous_unsupported_options_fail_closed_before_provider_delegation() -> TestResult {
    let runner = RouteRunner::new()?;
    let cases = [
        (
            "Forgejo context with a later GitHub-shaped selector",
            "git.example.com/base/repo",
            &[
                "release",
                "create",
                "v1",
                "--notes",
                "--repo=github.com/other/repo",
                "--draft",
            ][..],
        ),
        (
            "GitHub context with a later Forgejo-shaped selector",
            "github.com/base/repo",
            &[
                "workflow",
                "run",
                "build.yml",
                "--raw-field",
                "--repo=git.example.com/other/repo",
            ][..],
        ),
        (
            "unknown short option arity",
            "git.example.com/base/repo",
            &[
                "workflow",
                "run",
                "build.yml",
                "-f",
                "--repo=github.com/other/repo",
            ][..],
        ),
    ];

    for (name, gh_repo, argv) in cases {
        let output = runner.run("GH_REPO", gh_repo, argv)?;
        let stdout = String::from_utf8(output.stdout)?;
        let stderr = String::from_utf8(output.stderr)?;

        assert_eq!(output.status.code(), Some(1), "{name}: {stderr}");
        assert_eq!(stdout, "", "{name}");
        assert!(stderr.contains("provider is ambiguous"), "{name}: {stderr}");
        assert!(!stderr.contains("delegated-to-github"), "{name}: {stderr}");
    }
    Ok(())
}

#[test]
fn managed_gh_command_targets_never_cross_provider() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.write_executable("gh", "#!/bin/sh\necho delegated\n")?;

    let cases = vec![
        (
            "attached -q keeps Forgejo issue URL authoritative",
            "GH_REPO",
            "github.com/base/repo",
            vec![
                "issue",
                "view",
                "-q.number",
                "https://git.example.com/owner/repo/issues/13",
            ],
            false,
        ),
        (
            "attached -q keeps GitHub issue URL authoritative",
            "GH_REPO",
            "git.example.com/base/repo",
            vec![
                "issue",
                "view",
                "-q.number",
                "https://github.com/owner/repo/issues/13",
            ],
            true,
        ),
        (
            "attached -t keeps Forgejo pull URL authoritative",
            "GH_REPO",
            "github.com/base/repo",
            vec![
                "pr",
                "view",
                "-t{{.number}}",
                "https://git.example.com/owner/repo/pulls/7",
            ],
            false,
        ),
        (
            "attached -i keeps GitHub pull URL authoritative",
            "GH_REPO",
            "git.example.com/base/repo",
            vec![
                "pr",
                "checks",
                "-i10",
                "https://github.com/owner/repo/pulls/7",
            ],
            true,
        ),
        (
            "attached -b keeps Forgejo pull URL authoritative",
            "GH_REPO",
            "github.com/base/repo",
            vec![
                "pr",
                "checkout",
                "-blocal",
                "https://git.example.com/owner/repo/pulls/7",
            ],
            false,
        ),
        (
            "attached -R selects Forgejo",
            "GH_REPO",
            "github.com/base/repo",
            vec!["issue", "view", "-Rgit.example.com/owner/repo", "13"],
            false,
        ),
        (
            "attached -R selects GitHub",
            "GH_REPO",
            "git.example.com/base/repo",
            vec!["issue", "view", "-Rgithub.com/owner/repo", "13"],
            true,
        ),
        (
            "attached auth -h selects Forgejo",
            "GH_HOST",
            "github.com",
            vec!["auth", "status", "-hgit.example.com"],
            false,
        ),
        (
            "attached auth -h selects GitHub",
            "GH_HOST",
            "git.example.com",
            vec!["auth", "status", "-hgithub.com"],
            true,
        ),
        (
            "API include flag preserves later Forgejo hostname",
            "GH_HOST",
            "github.com",
            vec!["api", "-i", "--hostname", "git.example.com", "user"],
            false,
        ),
        (
            "API include flag preserves later GitHub hostname",
            "GH_HOST",
            "git.example.com",
            vec!["api", "-i", "--hostname", "github.com", "user"],
            true,
        ),
        (
            "PR comments flag preserves Forgejo URL",
            "GH_REPO",
            "github.com/base/repo",
            vec![
                "pr",
                "view",
                "--comments",
                "https://git.example.com/owner/repo/pulls/7",
            ],
            false,
        ),
        (
            "PR editor flag preserves GitHub URL",
            "GH_REPO",
            "git.example.com/base/repo",
            vec![
                "pr",
                "comment",
                "--editor",
                "https://github.com/owner/repo/pulls/7",
            ],
            true,
        ),
        (
            "PR draft flag preserves later Forgejo repo",
            "GH_REPO",
            "github.com/base/repo",
            vec![
                "pr",
                "list",
                "--draft",
                "--repo",
                "git.example.com/owner/repo",
            ],
            false,
        ),
        (
            "PR dry-run flag preserves later GitHub repo",
            "GH_REPO",
            "git.example.com/base/repo",
            vec![
                "pr",
                "create",
                "--dry-run",
                "--repo",
                "github.com/owner/repo",
            ],
            true,
        ),
        (
            "unknown workflow option fails closed in GitHub context",
            "GH_REPO",
            "github.com/base/repo",
            vec![
                "workflow",
                "run",
                "build.yml",
                "--ref",
                "main",
                "--repo",
                "git.example.com/owner/repo",
            ],
            false,
        ),
        (
            "unknown workflow option fails closed in Forgejo context",
            "GH_REPO",
            "git.example.com/base/repo",
            vec![
                "workflow",
                "run",
                "build.yml",
                "--ref",
                "main",
                "--repo",
                "github.com/owner/repo",
            ],
            false,
        ),
        (
            "unknown release option fails closed in GitHub context",
            "GH_REPO",
            "github.com/base/repo",
            vec![
                "release",
                "view",
                "v1.0.0",
                "--json",
                "name",
                "--repo",
                "git.example.com/owner/repo",
            ],
            false,
        ),
        (
            "unknown release option fails closed in Forgejo context",
            "GH_REPO",
            "git.example.com/base/repo",
            vec![
                "release",
                "view",
                "v1.0.0",
                "--repo",
                "github.com/owner/repo",
                "--json",
                "name",
            ],
            false,
        ),
        (
            "Forgejo issue URL after -c",
            "GH_REPO",
            "github.com/base/repo",
            vec![
                "issue",
                "view",
                "-c",
                "https://git.example.com/owner/repo/issues/13",
            ],
            false,
        ),
        (
            "Forgejo pull URL after -i",
            "GH_REPO",
            "github.com/base/repo",
            vec![
                "pr",
                "checks",
                "-i",
                "10",
                "https://git.example.com/owner/repo/pulls/7",
            ],
            false,
        ),
        (
            "Forgejo pull URL after --exclude",
            "GH_REPO",
            "github.com/base/repo",
            vec![
                "pr",
                "diff",
                "--exclude",
                "generated",
                "https://git.example.com/owner/repo/pulls/7",
            ],
            false,
        ),
        (
            "plain Forgejo repo target",
            "GH_REPO",
            "github.com/base/repo",
            vec!["repo", "view", "git.example.com/owner/repo"],
            false,
        ),
        (
            "plain GitHub repo target",
            "GH_REPO",
            "git.example.com/base/repo",
            vec!["repo", "view", "github.com/owner/repo"],
            true,
        ),
        (
            "repo-shaped body value in Forgejo context",
            "GH_REPO",
            "git.example.com/owner/repo",
            vec![
                "pr",
                "create",
                "--title",
                "test",
                "--body",
                "--repo=https://github.com/other/project",
            ],
            false,
        ),
        (
            "repo-shaped body value in GitHub context",
            "GH_REPO",
            "github.com/owner/repo",
            vec![
                "pr",
                "create",
                "--title",
                "test",
                "--body",
                "--repo=https://git.example.com/other/project",
            ],
            true,
        ),
        (
            "hostname-shaped API field in Forgejo context",
            "GH_HOST",
            "git.example.com",
            vec!["api", "user", "-f", "--hostname=github.com"],
            false,
        ),
        (
            "hostname-shaped API field in GitHub context",
            "GH_HOST",
            "github.com",
            vec!["api", "user", "-f", "--hostname=git.example.com"],
            true,
        ),
    ];

    for (name, env_key, env_value, args, delegates) in cases {
        let output = fixture
            .command("gh-forgejo-shim")?
            .env_remove("FJ_SHIM_REAL_GH")
            .env(env_key, env_value)
            .arg("gh")
            .args(args)
            .output()?;
        let stdout = String::from_utf8(output.stdout)?;
        assert_eq!(output.status.success(), delegates, "{name}: {stdout}");
        assert_eq!(stdout.contains("delegated"), delegates, "{name}: {stdout}");
    }
    Ok(())
}
