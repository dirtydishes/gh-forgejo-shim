#[allow(dead_code)]
mod support;

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use gh_forgejo_shim::forgejo::{ForgejoClient, RepoRef};
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
    assert_eq!(control.no_checks.exit_code, 1);
    assert_eq!(control.no_checks.stdout, "");
    assert_eq!(
        control.no_checks.stderr,
        "no checks reported on the 'lavender/production-rhyme-loop-bootstrap' branch\n"
    );
    Ok(())
}

#[test]
#[ignore = "red contract probe: alias identity belongs to S2 and S3"]
fn forgejo_alias_identity_is_not_yet_canonicalized() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.write_executable("gh", "#!/bin/sh\nprintf 'delegated-to-github\\n'\n")?;
    let contract = load_contract()?;
    let pr_list = command(&contract, "pr_list")?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .env("FJ_SHIM_HOSTS", "git.dirtydishes.dev")
        .arg("gh")
        .args(&pr_list.argv)
        .output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout)?, "[]\n");
    assert_eq!(String::from_utf8(output.stderr)?, "");
    Ok(())
}

#[test]
#[ignore = "red contract probe: @me filtering belongs to S3"]
fn forgejo_pr_list_does_not_yet_resolve_author_me() -> TestResult {
    let (host, handle) = start_json_server(
        r#"[{"number":1,"user":{"login":"alice"}},{"number":2,"user":{"login":"bob"}}]"#,
    )?;
    let client = ForgejoClient::new(None).with_scheme("http");
    let pulls = client.list_pulls(&RepoRef::new(host, "owner", "repo"), "all", None)?;
    let request_line = handle
        .join()
        .map_err(|_| io::Error::other("fake server thread panicked"))??;

    assert_eq!(
        request_line,
        "GET /api/v1/repos/owner/repo/pulls?state=all HTTP/1.1"
    );
    assert_eq!(
        pulls.len(),
        1,
        "@me must retain only the current user's pull"
    );
    assert_eq!(pulls[0]["user"]["login"], "alice");
    Ok(())
}

#[test]
#[ignore = "red contract probe: the missing pr view fields belong to S5"]
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
#[ignore = "red contract probe: complete checks output belongs to S6"]
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
    assert_eq!(object_keys(&parsed)?.len(), 25);
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
