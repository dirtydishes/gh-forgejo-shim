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
        ("boolean cluster then value", "-cRgit.example.com/selected/repo"),
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
        assert_eq!(String::from_utf8(output.stdout)?, "{\"number\":13}\n");
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
        assert!(
            stderr.contains("provider is ambiguous"),
            "{name}: {stderr}"
        );
        assert!(!stderr.contains("delegated-to-github"), "{name}: {stderr}");
    }
    Ok(())
}
