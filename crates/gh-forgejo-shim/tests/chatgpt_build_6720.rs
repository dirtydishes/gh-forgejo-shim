mod support;

use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use support::{CliFixture, TestResult};

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

fn command<'a>(contract: &'a ProcessContract, id: &str) -> ContractResult<&'a CommandContract> {
    contract
        .commands
        .iter()
        .find(|command| command.id == id)
        .ok_or_else(|| format!("missing command fixture {id}").into())
}

fn load_contract() -> ContractResult<ProcessContract> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/chatgpt/build-6720/process-contract.json");
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}
