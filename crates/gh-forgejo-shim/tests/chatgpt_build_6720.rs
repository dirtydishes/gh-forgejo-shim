#[allow(dead_code)]
mod support;

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use gh_forgejo_shim::normalize::{
    filter_check_fields, filter_fields, normalize_pr_checks, normalize_pull,
};
use serde::Deserialize;
use serde_json::{json, Value};
use support::{start_json_server, CliFixture, ScriptedGh, TestResult};

type ContractResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessContract {
    schema: String,
    client: ClientIdentity,
    process: ProcessBoundary,
    commands: Vec<CommandContract>,
    github_2_96_control: GithubControl,
    forgejo_known_gaps: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientIdentity {
    application: String,
    version: String,
    build: String,
    bundle_identifier: String,
    bundled_cli: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessBoundary {
    input: Vec<String>,
    output: Vec<String>,
    deadline_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandContract {
    id: String,
    argv: Vec<String>,
    provenance: String,
    failure_effect: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GithubControl {
    binary_version: String,
    release_archive: String,
    release_archive_sha256: String,
    captured_at: String,
    provenance: String,
    repo: String,
    pull_request: u64,
    view_stdout_fixture: String,
    view_exit_code: i32,
    view_stderr: String,
    no_checks: ProcessOutput,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessOutput {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

#[test]
fn fixture_preserves_the_accepted_process_boundary() -> TestResult {
    let contract = load_contract()?;

    assert_eq!(contract.schema, "chatgpt-build-6720-process-contract/v1");
    assert_eq!(
        contract
            .commands
            .iter()
            .map(|command| command.id.as_str())
            .collect::<Vec<_>>(),
        ["version", "auth_status", "pr_list", "pr_view", "pr_checks"]
    );
    assert_eq!(contract.client.application, "ChatGPT.app");
    assert_eq!(contract.client.version, "26.814.41407");
    assert_eq!(contract.client.build, "6720");
    assert_eq!(contract.client.bundle_identifier, "com.openai.codex");
    assert_eq!(contract.client.bundled_cli, "0.148.0-alpha.15");
    assert_eq!(contract.process.input, ["cwd", "env", "stdin", "gh argv"]);
    assert_eq!(contract.process.output, ["exit status", "stdout", "stderr"]);
    assert_eq!(contract.process.deadline_ms, 5000);

    assert_command(
        &contract,
        "version",
        &["--version"],
        "accepted report template; exact ordering confirmed in installed build 6962 source",
        "fatal",
    )?;
    assert_command(
        &contract,
        "auth_status",
        &["auth", "status", "--active", "--hostname", "{host}"],
        "accepted report template; exact ordering confirmed in installed build 6962 source",
        "fatal",
    )?;
    assert_command(
        &contract,
        "pr_list",
        &[
            "pr",
            "list",
            "--head",
            "main",
            "--author",
            "@me",
            "--state",
            "all",
            "--json",
            "number,url,state,headRefName",
            "--repo",
            "127.0.0.1/dirtydishes/dirtypages",
        ],
        "exact build 6720 desktop capture",
        "fatal",
    )?;
    assert_command(
        &contract,
        "pr_view",
        &[
            "pr",
            "view",
            "{number}",
            "--json",
            "additions,author,autoMergeRequest,baseRefName,comments,createdAt,deletions,headRefName,headRefOid,isDraft,latestReviews,mergedAt,mergedBy,mergeStateStatus,mergeable,number,reviews,reviewDecision,reviewRequests,state,title,updatedAt,url,body,commits",
            "--repo",
            "{repo}",
        ],
        "accepted 25-field report contract; exact ordering confirmed in installed build 6962 source",
        "fatal",
    )?;
    assert_command(
        &contract,
        "pr_checks",
        &[
            "pr",
            "checks",
            "{number}",
            "--json",
            "bucket,completedAt,description,event,link,name,startedAt,state,workflow",
            "--repo",
            "{repo}",
        ],
        "accepted checks semantics; exact fields and ordering confirmed in installed build 6962 source",
        "nonfatal when no checks are reported",
    )?;
    assert_eq!(
        contract.forgejo_known_gaps,
        ["alias identity", "@me", "missing fields", "checks"]
    );

    let control = &contract.github_2_96_control;
    assert_eq!(control.binary_version, "gh version 2.96.0 (2026-07-02)");
    assert_eq!(control.release_archive, "gh_2.96.0_linux_amd64.tar.gz");
    assert_eq!(
        control.release_archive_sha256,
        "83d5c2ccad5498f58bf6368acb1ab32588cf43ab3a4b1c301bf36328b1c8bd60"
    );
    assert_eq!(control.captured_at, "2026-08-22");
    assert_eq!(
        control.provenance,
        "official GitHub CLI 2.96.0 release binary against github.com; email values redacted after capture"
    );
    assert_eq!(control.view_stdout_fixture, "github-pr-31.json");
    assert_eq!(control.view_exit_code, 0);
    assert_eq!(control.view_stderr, "");
    assert_eq!(control.repo, "dirtydishes/lyricslab");
    assert_eq!(control.pull_request, 31);
    assert_eq!(control.no_checks.exit_code, 1);
    assert_eq!(control.no_checks.stdout, "");
    assert_eq!(
        control.no_checks.stderr,
        "no checks reported on the 'lavender/production-rhyme-loop-bootstrap' branch\n"
    );
    Ok(())
}

#[test]
fn forgejo_alias_identity_routes_to_canonical_api_root() -> TestResult {
    let (host, handle) = start_json_server(vec![r#"{"login":"alice"}"#, r#"[]"#])?;
    let fixture = CliFixture::new()?;
    fixture.write_executable("gh", "#!/bin/sh\nprintf 'delegated-to-github\\n'\n")?;
    let api_root = format!("http://{host}/api/v1");
    let configured = fixture
        .command("gfj")?
        .args([
            "config",
            "add-host",
            "git.dirtydishes.dev",
            "--alias",
            "127.0.0.1",
            "--api-root",
            &api_root,
        ])
        .output()?;
    assert_eq!(
        configured.status.code(),
        Some(0),
        "host-profile setup failed: {}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let contract = load_contract()?;
    let pr_list = command(&contract, "pr_list")?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_TOKEN", "test-token")
        .env_remove("FJ_SHIM_REAL_GH")
        .env_remove("FJ_SHIM_HOSTS")
        .arg("gh")
        .args(&pr_list.argv)
        .output()?;
    let request_lines = handle
        .join()
        .map_err(|_| io::Error::other("fake server thread panicked"))??;

    assert_eq!(
        output.status.code(),
        Some(0),
        "alias command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout)?, "[]\n");
    assert_eq!(String::from_utf8(output.stderr)?, "");
    assert_eq!(request_lines.len(), 2);
    assert_request(&request_lines[0], "/api/v1/user")?;
    assert_request(
        &request_lines[1],
        "/api/v1/repos/dirtydishes/dirtypages/pulls",
    )?;
    Ok(())
}

#[test]
fn forgejo_alias_uses_full_configured_api_root() -> TestResult {
    let (host, handle) = start_json_server(vec![r#"{"login":"alice"}"#, r#"[]"#])?;
    let fixture = CliFixture::new()?;
    fixture.write_executable("gh", "#!/bin/sh\nprintf 'delegated-to-github\\n'\n")?;
    let api_root = format!("http://{host}/forgejo/api/v1");
    let configured = fixture
        .command("gfj")?
        .args([
            "config",
            "add-host",
            "git.dirtydishes.dev",
            "--alias",
            "127.0.0.1",
            "--api-root",
            &api_root,
        ])
        .output()?;
    assert_eq!(configured.status.code(), Some(0));
    let contract = load_contract()?;
    let pr_list = command(&contract, "pr_list")?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_TOKEN", "test-token")
        .env_remove("FJ_SHIM_REAL_GH")
        .env_remove("FJ_SHIM_HOSTS")
        .arg("gh")
        .args(&pr_list.argv)
        .output()?;
    let request_lines = handle
        .join()
        .map_err(|_| io::Error::other("fake server thread panicked"))??;

    assert_eq!(
        output.status.code(),
        Some(0),
        "custom API-root command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout)?, "[]\n");
    assert_eq!(String::from_utf8(output.stderr)?, "");
    assert_eq!(request_lines.len(), 2);
    assert_request(&request_lines[0], "/forgejo/api/v1/user")?;
    assert_request(
        &request_lines[1],
        "/forgejo/api/v1/repos/dirtydishes/dirtypages/pulls",
    )?;
    Ok(())
}

#[test]
fn forgejo_alias_auth_status_uses_credential_host_without_leaking_tokens() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.write_executable(
        "gh",
        "#!/bin/sh\nprintf 'delegated-to-github\\n'\nexit 23\n",
    )?;
    let configured = fixture
        .command("gfj")?
        .args([
            "config",
            "add-host",
            "git.dirtydishes.dev",
            "--alias",
            "127.0.0.1",
            "--credential-host",
            "auth.dirtydishes.dev",
        ])
        .output()?;
    assert_eq!(
        configured.status.code(),
        Some(0),
        "host-profile setup failed: {}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let other_configured = fixture
        .command("gfj")?
        .args([
            "config",
            "add-host",
            "git.other.test",
            "--credential-host",
            "auth.other.test",
        ])
        .output()?;
    assert_eq!(other_configured.status.code(), Some(0));

    let keys_dir = fixture
        .home()
        .join(".local")
        .join("share")
        .join("forgejo-cli");
    fs::create_dir_all(&keys_dir)?;
    fs::write(
        keys_dir.join("keys.json"),
        r#"{"hosts":{"auth.dirtydishes.dev":{"token":"linux-secret"},"unrelated.invalid":{"token":"unrelated-secret"}}}"#,
    )?;

    for requested_host in ["127.0.0.1", "git.dirtydishes.dev"] {
        let output = fixture
            .command("gh-forgejo-shim")?
            .env_remove("FJ_SHIM_HOSTS")
            .env_remove("FJ_SHIM_REAL_GH")
            .arg("gh")
            .args(["auth", "status", "--active", "--hostname", requested_host])
            .output()?;
        let stdout = String::from_utf8(output.stdout)?;
        let stderr = String::from_utf8(output.stderr)?;

        assert_eq!(output.status.code(), Some(0), "{requested_host}: {stderr}");
        assert!(
            stdout.contains("Logged in to git.dirtydishes.dev"),
            "{stdout}"
        );
        assert!(!stdout.contains("linux-secret"), "{stdout}");
        assert!(!stdout.contains("unrelated-secret"), "{stdout}");
        assert!(!stdout.contains("delegated-to-github"), "{stdout}");
        assert!(!stderr.contains("linux-secret"), "{stderr}");
        assert!(!stderr.contains("unrelated-secret"), "{stderr}");
    }

    let unrelated = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_HOSTS")
        .env_remove("FJ_SHIM_REAL_GH")
        .arg("gh")
        .args(["auth", "status", "--active", "--hostname", "git.other.test"])
        .output()?;
    let stdout = String::from_utf8(unrelated.stdout)?;
    let stderr = String::from_utf8(unrelated.stderr)?;
    assert_eq!(unrelated.status.code(), Some(1), "{stderr}");
    assert!(!stdout.contains("unrelated-secret"), "{stdout}");
    assert!(!stderr.contains("unrelated-secret"), "{stderr}");
    assert!(!stdout.contains("delegated-to-github"), "{stdout}");
    Ok(())
}

#[test]
fn duplicate_json_host_keys_fail_closed_before_auth_status() -> TestResult {
    let fixture = CliFixture::new()?;
    let keys_dir = fixture
        .home()
        .join(".local")
        .join("share")
        .join("forgejo-cli");
    fs::create_dir_all(&keys_dir)?;
    fs::write(
        keys_dir.join("keys.json"),
        r#"{"servers":[{"host":"auth.other.test","host":"auth.target.test","token":"duplicate-key-secret"}]}"#,
    )?;

    let output = fixture
        .command("gfj")?
        .args(["auth", "status", "auth.target.test"])
        .output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;

    assert_eq!(output.status.code(), Some(1), "{stdout}{stderr}");
    assert_eq!(stdout, "auth.target.test: not logged in\n");
    assert_eq!(stderr, "");
    assert!(!stdout.contains("duplicate-key-secret"));
    Ok(())
}

#[test]
fn sibling_json_host_fields_fail_closed_before_keyed_lookup() -> TestResult {
    let fixture = CliFixture::new()?;
    let keys_dir = fixture
        .home()
        .join(".local")
        .join("share")
        .join("forgejo-cli");
    fs::create_dir_all(&keys_dir)?;
    fs::write(
        keys_dir.join("keys.json"),
        r#"{"servers":[{"host":"auth.other.test","auth.target.test":{"token":"cross-host-secret"}}]}"#,
    )?;

    let output = fixture
        .command("gfj")?
        .args(["auth", "status", "auth.target.test"])
        .output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;

    assert_eq!(output.status.code(), Some(1), "{stdout}{stderr}");
    assert_eq!(stdout, "auth.target.test: not logged in\n");
    assert_eq!(stderr, "");
    assert!(!stdout.contains("cross-host-secret"));
    Ok(())
}

#[test]
fn forgejo_pr_list_resolves_author_me() -> TestResult {
    let (host, handle) = start_json_server(vec![
        r#"{"login":"alice","full_name":"Alice Example"}"#,
        r#"[{"number":1,"state":"open","html_url":"https://git.dirtydishes.dev/dirtydishes/dirtypages/pulls/1","head":{"ref":"main"},"base":{"ref":"main"},"user":{"login":"alice"}},{"number":2,"state":"open","html_url":"https://git.dirtydishes.dev/dirtydishes/dirtypages/pulls/2","head":{"ref":"main"},"base":{"ref":"main"},"user":{"login":"bob"}}]"#,
    ])?;
    let fixture = CliFixture::new()?;
    let api_root = format!("http://{host}/api/v1");
    let configured = fixture
        .command("gfj")?
        .args([
            "config",
            "add-host",
            "git.dirtydishes.dev",
            "--alias",
            "127.0.0.1",
            "--api-root",
            &api_root,
            "--credential-host",
            "git.dirtydishes.dev",
        ])
        .output()?;
    assert_eq!(
        configured.status.code(),
        Some(0),
        "future host-profile setup failed: {}",
        String::from_utf8_lossy(&configured.stderr)
    );

    let contract = load_contract()?;
    let argv = &command(&contract, "pr_list")?.argv;
    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_TOKEN", "test-token")
        .env_remove("FJ_SHIM_HOSTS")
        .arg("gh")
        .args(argv)
        .output()?;
    let request_lines = handle
        .join()
        .map_err(|_| io::Error::other("fake server thread panicked"))??;

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout)?,
        json!([{
            "headRefName": "main",
            "number": 1,
            "state": "OPEN",
            "url": "https://git.dirtydishes.dev/dirtydishes/dirtypages/pulls/1"
        }])
    );
    assert_eq!(request_lines.len(), 2);
    assert_request(&request_lines[0], "/api/v1/user")?;
    assert_request(
        &request_lines[1],
        "/api/v1/repos/dirtydishes/dirtypages/pulls",
    )?;
    Ok(())
}

#[test]
fn forgejo_pr_list_rejects_a_non_array_page_without_partial_output() -> TestResult {
    assert_malformed_pr_page_rejected(r#"{}"#)
}

#[test]
fn forgejo_pr_list_rejects_a_mixed_page_without_partial_output() -> TestResult {
    assert_malformed_pr_page_rejected(
        r#"[{"number":1,"state":"open","html_url":"https://git.dirtydishes.dev/dirtydishes/dirtypages/pulls/1","head":{"ref":"main"},"base":{"ref":"main"},"user":{"login":"alice"}},"malformed"]"#,
    )
}

fn assert_malformed_pr_page_rejected(page_body: &'static str) -> TestResult {
    let contract = load_contract()?;
    let argv = &command(&contract, "pr_list")?.argv;
    let (host, handle) = start_json_server(vec![r#"{"login":"alice"}"#, page_body])?;
    let fixture = CliFixture::new()?;
    let api_root = format!("http://{host}/api/v1");
    let configured = fixture
        .command("gfj")?
        .args([
            "config",
            "add-host",
            "git.dirtydishes.dev",
            "--alias",
            "127.0.0.1",
            "--api-root",
            &api_root,
            "--credential-host",
            "git.dirtydishes.dev",
        ])
        .output()?;
    assert_eq!(configured.status.code(), Some(0));

    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_TOKEN", "test-token")
        .env_remove("FJ_SHIM_HOSTS")
        .arg("gh")
        .args(argv)
        .output()?;
    let request_lines = handle
        .join()
        .map_err(|_| io::Error::other("fake server thread panicked"))??;

    assert_eq!(output.status.code(), Some(1), "{page_body}");
    assert_eq!(String::from_utf8(output.stdout)?, "", "{page_body}");
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "gh-forgejo-shim: Forgejo API returned an invalid pull request page\n",
        "{page_body}"
    );
    assert_eq!(request_lines.len(), 2);
    Ok(())
}

#[test]
#[ignore = "red contract probe: the missing pr view fields belong to S7 and S8"]
fn forgejo_pr_view_does_not_yet_emit_all_25_fields() -> TestResult {
    let contract = load_contract()?;
    let fields = json_fields(command(&contract, "pr_view")?)?;
    let pull = json!({
        "number": 31,
        "title": "Control",
        "state": "open",
        "html_url": "http://git.example.com/owner/repo/pulls/31",
        "head": {"ref": "feature", "sha": "abc123"},
        "base": {"ref": "main"},
        "user": {"login": "alice"}
    });
    let rendered = filter_fields(&normalize_pull(&pull), &fields);

    assert_eq!(object_keys(&rendered)?, string_set(&fields));
    Ok(())
}

#[test]
#[ignore = "red contract probe: complete checks output belongs to S9"]
fn forgejo_pr_checks_do_not_yet_emit_all_9_fields() -> TestResult {
    let contract = load_contract()?;
    let fields = json_fields(command(&contract, "pr_checks")?)?;
    let statuses = vec![json!({
        "context": "ci/test",
        "description": "tests passed",
        "state": "success",
        "target_url": "http://git.example.com/owner/repo/actions/runs/1",
        "created_at": "2026-08-22T12:00:00Z",
        "updated_at": "2026-08-22T12:01:00Z"
    })];
    let checks = normalize_pr_checks(&statuses);
    let first = checks
        .as_array()
        .and_then(|values| values.first())
        .ok_or_else(|| io::Error::other("checks normalization returned no rows"))?;
    let rendered = filter_check_fields(first, &fields);

    assert_eq!(object_keys(&rendered)?, string_set(&fields));
    Ok(())
}

#[test]
fn github_2_96_view_control_preserves_argv_status_and_literal_output() -> TestResult {
    let contract = load_contract()?;
    let control = &contract.github_2_96_control;
    let view = command(&contract, "pr_view")?;
    let argv = render_argv(
        &view.argv,
        control.pull_request,
        control.repo.as_str(),
        "github.com",
    );
    let stdout = load_fixture(&control.view_stdout_fixture)?;
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(object_keys(&parsed)?, string_set(&json_fields(view)?));
    let fixture = CliFixture::new()?;
    let scripted = ScriptedGh::install(
        &fixture,
        control.view_exit_code,
        &stdout,
        &control.view_stderr,
    )?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .env("FJ_SHIM_HOSTS", "github.com,git.example.com")
        .arg("gh")
        .args(&argv)
        .output()?;

    assert_eq!(output.status.code(), Some(control.view_exit_code));
    assert_eq!(String::from_utf8(output.stdout)?, stdout);
    assert_eq!(String::from_utf8(output.stderr)?, control.view_stderr);
    assert_eq!(scripted.argv()?, argv);
    Ok(())
}

#[test]
fn github_2_96_no_checks_control_preserves_nonzero_output() -> TestResult {
    let contract = load_contract()?;
    let control = &contract.github_2_96_control;
    let checks = command(&contract, "pr_checks")?;
    let argv = render_argv(
        &checks.argv,
        control.pull_request,
        control.repo.as_str(),
        "github.com",
    );
    let expected = &control.no_checks;
    let fixture = CliFixture::new()?;
    let scripted = ScriptedGh::install(
        &fixture,
        expected.exit_code,
        &expected.stdout,
        &expected.stderr,
    )?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .env("FJ_SHIM_HOSTS", "github.com,git.example.com")
        .arg("gh")
        .args(&argv)
        .output()?;

    assert_eq!(output.status.code(), Some(expected.exit_code));
    assert_eq!(String::from_utf8(output.stdout)?, expected.stdout);
    assert_eq!(String::from_utf8(output.stderr)?, expected.stderr);
    assert_eq!(scripted.argv()?, argv);
    Ok(())
}

fn assert_command(
    contract: &ProcessContract,
    id: &str,
    argv: &[&str],
    provenance: &str,
    failure_effect: &str,
) -> ContractResult<()> {
    let actual = command(contract, id)?;
    assert_eq!(actual.argv, argv);
    assert_eq!(actual.provenance, provenance);
    assert_eq!(actual.failure_effect, failure_effect);
    Ok(())
}

fn command<'a>(contract: &'a ProcessContract, id: &str) -> ContractResult<&'a CommandContract> {
    contract
        .commands
        .iter()
        .find(|command| command.id == id)
        .ok_or_else(|| format!("missing command fixture {id}").into())
}

fn json_fields(command: &CommandContract) -> ContractResult<Vec<String>> {
    command
        .argv
        .windows(2)
        .find(|pair| pair[0] == "--json")
        .map(|pair| pair[1].split(',').map(ToOwned::to_owned).collect())
        .ok_or_else(|| format!("{} has no --json fields", command.id).into())
}

fn object_keys(value: &Value) -> ContractResult<BTreeSet<String>> {
    value
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .ok_or_else(|| io::Error::other("expected a JSON object").into())
}

fn string_set(values: &[String]) -> BTreeSet<String> {
    values.iter().cloned().collect()
}

fn assert_request(request_line: &str, expected_path: &str) -> ContractResult<()> {
    let mut parts = request_line.split_whitespace();
    assert_eq!(parts.next(), Some("GET"));
    let path_and_query = parts
        .next()
        .ok_or_else(|| io::Error::other("request line has no path"))?;
    assert_eq!(path_and_query.split('?').next(), Some(expected_path));
    assert_eq!(parts.next(), Some("HTTP/1.1"));
    assert_eq!(parts.next(), None);
    Ok(())
}

fn render_argv(argv: &[String], number: u64, repo: &str, host: &str) -> Vec<String> {
    argv.iter()
        .map(|arg| {
            arg.replace("{number}", &number.to_string())
                .replace("{repo}", repo)
                .replace("{host}", host)
        })
        .collect()
}

fn load_contract() -> ContractResult<ProcessContract> {
    Ok(serde_json::from_str(&load_fixture(
        "process-contract.json",
    )?)?)
}

fn load_fixture(name: &str) -> ContractResult<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/chatgpt/build-6720")
        .join(name);
    Ok(fs::read_to_string(path)?)
}
