use std::fs;
use std::io;
use std::path::Path;

use gh_forgejo_shim::forgejo::RepoRef;
use gh_forgejo_shim::normalize::{
    filter_check_fields, filter_fields, filter_repo_fields, normalize_pr_checks, normalize_pull,
    normalize_repo, render_json_or_jq, status_for_current_branch, with_status_check_rollup,
};
use serde_json::{json, Value};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn simple_jq_matches_api_user_contract_probe() -> TestResult {
    let rendered = render_json_or_jq(
        &json!({"login": "alice", "full_name": "Alice Example"}),
        Some(".login"),
        None,
    )
    .ok_or_else(|| io::Error::other("jq unexpectedly produced no output"))?;

    assert_eq!(rendered, expected_stdout("gh_api_user_jq_login")?);
    Ok(())
}

#[test]
fn repo_view_normalization_matches_contract_probe() -> TestResult {
    let repo = json!({
        "id": 99,
        "name": "repo",
        "full_name": "owner/repo",
        "html_url": "https://git.example.com/owner/repo",
        "ssh_url": "git@git.example.com:owner/repo.git",
        "default_branch": "main",
        "private": false,
        "has_issues": true,
        "has_projects": true,
        "has_wiki": false,
        "allow_merge_commits": true,
        "allow_rebase": true,
        "allow_squash_merge": true,
        "forks_count": 2,
        "stars_count": 3,
        "watchers_count": 4,
        "open_issues_count": 5,
        "open_pr_counter": 6,
        "language": "Python",
        "permissions": {"pull": true, "push": true, "admin": false},
        "owner": {"login": "owner"}
    });
    let fields = ["nameWithOwner", "url", "defaultBranchRef", "sshUrl"];

    let normalized = normalize_repo(&repo, &RepoRef::new("git.example.com", "owner", "repo"));

    assert_eq!(
        filter_repo_fields(&normalized, &fields),
        expected_stdout_json("gh_repo_view_json_codex")?
    );
    Ok(())
}

#[test]
fn pr_status_normalization_matches_contract_probe() -> TestResult {
    let pull = json!({
        "number": 12,
        "title": "Ship it",
        "state": "open",
        "html_url": "https://git.example.com/owner/repo/pulls/12",
        "head": {"ref": "feature"},
        "base": {"ref": "main"},
        "user": {"login": "alice"}
    });
    let fields = ["number", "title", "url", "headRefName"];

    assert_eq!(
        status_for_current_branch(Some(&pull), &fields),
        expected_stdout_json("gh_pr_status_current_branch_json")?
    );
    Ok(())
}

#[test]
fn pr_list_board_normalization_matches_contract_probe() -> TestResult {
    let pull = json!({
        "number": 8,
        "title": "Make Codex happy",
        "state": "open",
        "html_url": "https://git.example.com/owner/repo/pulls/8",
        "head": {"ref": "feature", "sha": "abc123"},
        "base": {"ref": "main"},
        "user": {"login": "alice"},
        "created_at": "2026-05-31T00:00:00Z",
        "updated_at": "2026-05-31T01:00:00Z",
        "mergeable": false
    });
    let statuses = vec![json!({
        "context": "ci/test",
        "description": "tests passed",
        "state": "success",
        "target_url": "https://git.example.com/owner/repo/actions/runs/1",
        "created_at": "2026-06-01T00:00:00Z",
        "updated_at": "2026-06-01T00:02:00Z"
    })];
    let fields = [
        "additions",
        "baseRefName",
        "createdAt",
        "deletions",
        "headRefName",
        "isDraft",
        "number",
        "state",
        "title",
        "updatedAt",
        "url",
        "mergeStateStatus",
        "mergeable",
        "statusCheckRollup",
    ];

    let enriched = with_status_check_rollup(&pull, &statuses);
    let normalized = filter_fields(&normalize_pull(&enriched), &fields);

    assert_eq!(
        Value::Array(vec![normalized]),
        expected_stdout_json("gh_pr_list_codex_board_json")?
    );
    Ok(())
}

#[test]
fn pr_view_normalization_matches_contract_probe() -> TestResult {
    let pull = json!({
        "number": 12,
        "title": "Ship it",
        "state": "open",
        "html_url": "https://git.example.com/owner/repo/pulls/12",
        "head": {"ref": "feature"},
        "base": {"ref": "main"},
        "user": {"login": "alice"}
    });
    let fields = ["number", "title", "url", "headRefName"];

    assert_eq!(
        filter_fields(&normalize_pull(&pull), &fields),
        expected_stdout_json("gh_pr_view_json")?
    );
    Ok(())
}

#[test]
fn pr_checks_normalization_matches_contract_probe() -> TestResult {
    let statuses = vec![
        json!({
            "context": "ci/test",
            "description": "tests passed",
            "state": "success",
            "target_url": "https://git.example.com/owner/repo/actions/runs/1",
            "created_at": "2026-06-01T00:00:00Z",
            "updated_at": "2026-06-01T00:02:00Z"
        }),
        json!({
            "context": "lint",
            "description": "lint is running",
            "state": "pending",
            "created_at": "2026-06-01T00:01:00Z"
        }),
    ];
    let fields = ["bucket", "description", "link", "name", "workflow"];
    let checks = normalize_pr_checks(&statuses)
        .as_array()
        .ok_or_else(|| io::Error::other("checks normalization did not produce an array"))?
        .iter()
        .map(|check| filter_check_fields(check, &fields))
        .collect::<Vec<_>>();

    assert_eq!(
        Value::Array(checks),
        expected_stdout_json("gh_pr_checks_json")?
    );
    Ok(())
}

fn expected_stdout(id: &str) -> TestResult<String> {
    let expected = expected(id)?;
    expected
        .get("stdout")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| io::Error::other(format!("{id} has no stdout string")).into())
}

fn expected_stdout_json(id: &str) -> TestResult<Value> {
    let expected = expected(id)?;
    expected
        .get("stdout_json")
        .cloned()
        .ok_or_else(|| io::Error::other(format!("{id} has no stdout_json")).into())
}

fn expected(id: &str) -> TestResult<Value> {
    let contract = load_contract()?;
    contract
        .get("golden_probes")
        .and_then(Value::as_array)
        .and_then(|probes| {
            probes
                .iter()
                .find(|probe| probe.get("id").and_then(Value::as_str) == Some(id))
        })
        .and_then(|probe| probe.get("expected"))
        .cloned()
        .ok_or_else(|| io::Error::other(format!("missing golden probe {id}")).into())
}

fn load_contract() -> TestResult<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/implementation/rust-rewrite/contracts/compatibility-contract.v1.json");
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}
