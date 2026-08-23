#[allow(dead_code)]
mod support;

use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use support::{CliFixture, ScriptedGh, TestResult};

type ContractResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Deserialize)]
struct ProcessContract {
    schema: String,
    commands: Vec<CommandContract>,
    github_2_96_control: GithubControl,
    forgejo_known_gaps: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CommandContract {
    id: String,
    argv: Vec<String>,
    provenance: String,
}

#[derive(Debug, Deserialize)]
struct GithubControl {
    repo: String,
    pull_request: u64,
    view_empty_values: Value,
    no_checks: ProcessOutput,
}

#[derive(Debug, Deserialize)]
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
    assert!(contract
        .commands
        .iter()
        .all(|command| !command.argv.is_empty() && !command.provenance.is_empty()));
    assert_eq!(
        contract.forgejo_known_gaps,
        ["alias identity", "@me", "missing fields", "checks"]
    );
    assert_eq!(contract.github_2_96_control.no_checks.exit_code, 1);
    assert_eq!(contract.github_2_96_control.no_checks.stdout, "");
    assert_eq!(
        contract.github_2_96_control.no_checks.stderr,
        "no checks reported\n"
    );
    assert_eq!(
        contract.github_2_96_control.view_empty_values["reviewDecision"],
        ""
    );
    assert!(contract.github_2_96_control.view_empty_values["comments"].is_array());
    assert!(contract.github_2_96_control.view_empty_values["reviews"].is_array());
    assert!(contract.github_2_96_control.view_empty_values["mergedBy"].is_null());
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
fn github_2_96_view_control_preserves_argv_status_and_output() -> TestResult {
    let contract = load_contract()?;
    let control = &contract.github_2_96_control;
    let view = command(&contract, "pr_view")?;
    let argv = render_argv(
        &view.argv,
        control.pull_request,
        control.repo.as_str(),
        "github.com",
    );
    let stdout = format!("{}\n", control.view_empty_values);
    let fixture = CliFixture::new()?;
    let scripted = ScriptedGh::install(&fixture, 0, &stdout, "")?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .env("FJ_SHIM_HOSTS", "github.com,git.example.com")
        .arg("gh")
        .args(&argv)
        .output()?;

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stdout)?, stdout);
    assert_eq!(String::from_utf8(output.stderr)?, "");
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

fn command<'a>(contract: &'a ProcessContract, id: &str) -> ContractResult<&'a CommandContract> {
    contract
        .commands
        .iter()
        .find(|command| command.id == id)
        .ok_or_else(|| format!("missing command fixture {id}").into())
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
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/chatgpt/build-6720/process-contract.json");
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}
