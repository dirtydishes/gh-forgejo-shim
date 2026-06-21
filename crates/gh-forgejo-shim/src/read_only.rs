//! Read-only Forgejo command handlers for managed `gh` invocations.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::auth;
use crate::config::EnvMap;
use crate::external::git_output;
use crate::forgejo::{ForgejoClient, ForgejoResult, ListIssuesOptions, RepoRef as ForgejoRepoRef};
use crate::normalize::{
    filter_check_fields, filter_fields, filter_issue_fields, filter_repo_fields, normalize_issue,
    normalize_pr_checks, normalize_pull, normalize_repo, render_json_or_jq, render_json_or_jq_list,
    status_for_current_branch, with_status_check_rollup,
};
use crate::repo::{parse_repo_spec, RepoRef as DetectedRepoRef};
use crate::routing::RouteDecision;
use crate::{Result, ShimError};

trait ForgejoApi {
    fn get_current_user(&self, host: &str) -> ForgejoResult<Value>;
    fn get_repo(&self, repo: &ForgejoRepoRef) -> ForgejoResult<Value>;
    fn list_pulls(
        &self,
        repo: &ForgejoRepoRef,
        state: &str,
        head: Option<&str>,
    ) -> ForgejoResult<Vec<Value>>;
    fn get_pull(&self, repo: &ForgejoRepoRef, number: u64) -> ForgejoResult<Value>;
    fn list_commit_statuses(&self, repo: &ForgejoRepoRef, sha: &str) -> ForgejoResult<Vec<Value>>;
    fn get_pull_diff(
        &self,
        repo: &ForgejoRepoRef,
        number: u64,
        patch: bool,
    ) -> ForgejoResult<String>;
    fn list_pull_files(&self, repo: &ForgejoRepoRef, number: u64) -> ForgejoResult<Vec<Value>>;
    fn list_issues(
        &self,
        repo: &ForgejoRepoRef,
        options: &ListIssuesOptions,
    ) -> ForgejoResult<Vec<Value>>;
    fn get_issue(&self, repo: &ForgejoRepoRef, number: u64) -> ForgejoResult<Value>;
}

impl ForgejoApi for ForgejoClient {
    fn get_current_user(&self, host: &str) -> ForgejoResult<Value> {
        ForgejoClient::get_current_user(self, host)
    }

    fn get_repo(&self, repo: &ForgejoRepoRef) -> ForgejoResult<Value> {
        ForgejoClient::get_repo(self, repo)
    }

    fn list_pulls(
        &self,
        repo: &ForgejoRepoRef,
        state: &str,
        head: Option<&str>,
    ) -> ForgejoResult<Vec<Value>> {
        ForgejoClient::list_pulls(self, repo, state, head)
    }

    fn get_pull(&self, repo: &ForgejoRepoRef, number: u64) -> ForgejoResult<Value> {
        ForgejoClient::get_pull(self, repo, number)
    }

    fn list_commit_statuses(&self, repo: &ForgejoRepoRef, sha: &str) -> ForgejoResult<Vec<Value>> {
        ForgejoClient::list_commit_statuses(self, repo, sha)
    }

    fn get_pull_diff(
        &self,
        repo: &ForgejoRepoRef,
        number: u64,
        patch: bool,
    ) -> ForgejoResult<String> {
        ForgejoClient::get_pull_diff(self, repo, number, patch)
    }

    fn list_pull_files(&self, repo: &ForgejoRepoRef, number: u64) -> ForgejoResult<Vec<Value>> {
        ForgejoClient::list_pull_files(self, repo, number)
    }

    fn list_issues(
        &self,
        repo: &ForgejoRepoRef,
        options: &ListIssuesOptions,
    ) -> ForgejoResult<Vec<Value>> {
        ForgejoClient::list_issues(self, repo, options)
    }

    fn get_issue(&self, repo: &ForgejoRepoRef, number: u64) -> ForgejoResult<Value> {
        ForgejoClient::get_issue(self, repo, number)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OutputArgs {
    json_fields: Vec<String>,
    jq: Option<String>,
    template: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ViewStatusArgs {
    output: OutputArgs,
    repo: Option<String>,
    web: bool,
    number: Option<u64>,
    branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListArgs {
    output: OutputArgs,
    repo: Option<String>,
    state: String,
    limit: Option<usize>,
    head: Option<String>,
    base: Option<String>,
}

impl Default for ListArgs {
    fn default() -> Self {
        Self {
            output: OutputArgs::default(),
            repo: None,
            state: "open".to_string(),
            limit: None,
            head: None,
            base: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RepoViewArgs {
    output: OutputArgs,
    repo: Option<String>,
    web: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ChecksArgs {
    json_fields: Vec<String>,
    repo: Option<String>,
    jq: Option<String>,
    number: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DiffArgs {
    repo: Option<String>,
    number: Option<u64>,
    branch: Option<String>,
    web: bool,
    name_only: bool,
    patch: bool,
    excludes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IssueListArgs {
    output: OutputArgs,
    repo: Option<String>,
    state: String,
    limit: Option<usize>,
    labels: Vec<String>,
    query: Option<String>,
    author: Option<String>,
    assignee: Option<String>,
    mentioned: Option<String>,
    milestone: Option<String>,
    web: bool,
}

impl Default for IssueListArgs {
    fn default() -> Self {
        Self {
            output: OutputArgs::default(),
            repo: None,
            state: "open".to_string(),
            limit: Some(30),
            labels: Vec::new(),
            query: None,
            author: None,
            assignee: None,
            mentioned: None,
            milestone: None,
            web: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct IssueViewArgs {
    output: OutputArgs,
    repo: Option<String>,
    number: Option<u64>,
    web: bool,
    comments: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AuthStatusArgs {
    output: OutputArgs,
    show_token: bool,
    active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ApiArgs {
    endpoint: String,
    jq: Option<String>,
    template: Option<String>,
    silent: bool,
}

pub fn run(
    argv: &[String],
    decision: &RouteDecision,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let host = decision.trace_host().map(ToOwned::to_owned);
    let repo = decision.repo.as_ref().map(to_forgejo_repo);
    let env_map = env_map(env);
    let home = home_path(env);

    let result = run_with_context(
        argv,
        host.as_deref(),
        repo.as_ref(),
        &env_map,
        home.as_deref(),
        cwd,
        stdout,
        stderr,
    );

    match result {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(stderr, "gh-forgejo-shim: {error}");
            1
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_with_context(
    argv: &[String],
    host: Option<&str>,
    repo: Option<&ForgejoRepoRef>,
    env: &EnvMap,
    home: Option<&Path>,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let Some(command) = argv.first().map(String::as_str) else {
        return Err(ShimError::new("unsupported empty Forgejo command"));
    };
    let host = host.ok_or_else(|| ShimError::new("could not determine Forgejo host"))?;
    let token = auth::discover_token(Some(host), env, home, Some(current_platform()), true)
        .map(|discovery| discovery.token);

    if command == "auth" {
        return run_auth(argv, host, token.as_deref(), stdout, stderr);
    }

    if command == "api" {
        if token.is_none() {
            writeln!(stderr, "{}", missing_auth_message(host))?;
            return Ok(1);
        }
        let client = ForgejoClient::new(token);
        return run_api(argv, host, &client, stdout);
    }

    let client = ForgejoClient::new(token);
    let repo = repo.ok_or_else(|| ShimError::new("could not determine Forgejo repository"))?;
    run_repo_command(argv, repo, &client, cwd, stdout, stderr)
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn run_with_client(
    argv: &[String],
    host: Option<&str>,
    repo: Option<&ForgejoRepoRef>,
    token: Option<&str>,
    client: &dyn ForgejoApi,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let Some(command) = argv.first().map(String::as_str) else {
        return Err(ShimError::new("unsupported empty Forgejo command"));
    };
    let host = host.ok_or_else(|| ShimError::new("could not determine Forgejo host"))?;

    if command == "auth" {
        return run_auth(argv, host, token, stdout, stderr);
    }

    if command == "api" {
        if token.is_none() {
            writeln!(stderr, "{}", missing_auth_message(host))?;
            return Ok(1);
        }
        return run_api(argv, host, client, stdout);
    }

    let repo = repo.ok_or_else(|| ShimError::new("could not determine Forgejo repository"))?;
    run_repo_command(argv, repo, client, cwd, stdout, stderr)
}

fn run_auth(
    argv: &[String],
    host: &str,
    token: Option<&str>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let Some(command) = argv.get(1).map(String::as_str) else {
        return Err(ShimError::new("missing Forgejo auth command"));
    };
    match command {
        "status" => {
            let parsed = parse_auth_status_args(&argv[2..])?;
            let Some(token) = token else {
                print_missing_auth_status(host, stderr)?;
                return Ok(1);
            };
            let data = auth_status_data(host, token, parsed.show_token);
            if parsed.output.json_fields.is_empty() {
                print_auth_status_text(host, parsed.show_token.then_some(token), stdout)?;
            } else {
                let filtered = filter_selected_fields(&data, &parsed.output.json_fields);
                print_json(&filtered, &parsed.output, stdout)?;
            }
            Ok(0)
        }
        "token" => {
            parse_auth_token_args(&argv[2..])?;
            if let Some(token) = token {
                writeln!(stdout, "{token}")?;
                Ok(0)
            } else {
                writeln!(stderr, "{}", missing_auth_message(host))?;
                Ok(1)
            }
        }
        other => Err(ShimError::new(format!(
            "unsupported Forgejo auth command: {other}"
        ))),
    }
}

fn run_api(
    argv: &[String],
    host: &str,
    client: &dyn ForgejoApi,
    stdout: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_api_args(&argv[1..])?;
    if !matches!(parsed.endpoint.as_str(), "user" | "/user") {
        return Err(ShimError::new(format!(
            "unsupported Forgejo api endpoint: {}",
            parsed.endpoint
        )));
    }
    let user = client.get_current_user(host)?;
    if parsed.silent {
        return Ok(0);
    }
    let output = OutputArgs {
        json_fields: Vec::new(),
        jq: parsed.jq,
        template: parsed.template,
    };
    print_json(&user, &output, stdout)?;
    Ok(0)
}

fn run_repo_command(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    match argv.first().map(String::as_str) {
        Some("repo") => run_repo_view_command(argv, repo, client, stdout, stderr),
        Some("issue") => run_issue_command(argv, repo, client, stdout, stderr),
        Some("pr") => run_pr_command(argv, repo, client, cwd, stdout, stderr),
        Some(other) => Err(ShimError::new(format!(
            "unsupported Forgejo command: {other}"
        ))),
        None => Err(ShimError::new("unsupported empty Forgejo command")),
    }
}

fn run_pr_command(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let Some(command) = argv.get(1).map(String::as_str) else {
        return Err(ShimError::new("missing Forgejo PR command"));
    };
    match command {
        "checks" => run_checks(&argv[2..], repo, client, cwd, stdout),
        "diff" => run_diff(&argv[2..], repo, client, cwd, stdout),
        "list" => run_list(&argv[2..], repo, client, stdout),
        "status" => run_status(&argv[2..], repo, client, cwd, stdout),
        "view" => run_view(&argv[2..], repo, client, cwd, stdout),
        other => {
            writeln!(
                stderr,
                "gh-forgejo-shim: unsupported Forgejo PR command: {other}"
            )?;
            Ok(1)
        }
    }
}

fn run_repo_view_command(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    match argv.get(1).map(String::as_str) {
        Some("view") => run_repo_view(&argv[2..], repo, client, stdout),
        Some(other) => {
            writeln!(
                stderr,
                "gh-forgejo-shim: unsupported Forgejo repo command: {other}"
            )?;
            Ok(1)
        }
        None => Err(ShimError::new("missing Forgejo repo command")),
    }
}

fn run_issue_command(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let Some(command) = argv.get(1).map(String::as_str) else {
        return Err(ShimError::new("missing Forgejo issue command"));
    };
    match command {
        "list" | "ls" => run_issue_list(&argv[2..], repo, client, stdout),
        "view" => run_issue_view(&argv[2..], repo, client, stdout),
        other => {
            writeln!(
                stderr,
                "gh-forgejo-shim: unsupported Forgejo issue command: {other}"
            )?;
            Ok(1)
        }
    }
}

fn run_list(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    stdout: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_list_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    let api_state = if parsed.state == "merged" {
        "closed"
    } else {
        parsed.state.as_str()
    };
    let mut pulls = client.list_pulls(&target_repo, api_state, parsed.head.as_deref())?;
    if parsed.state == "merged" {
        pulls.retain(|pull| normalize_pull(pull).get("state") == Some(&json!("MERGED")));
    }
    if let Some(base) = parsed.base.as_deref() {
        pulls.retain(|pull| {
            pull.get("base")
                .and_then(Value::as_object)
                .and_then(|base| base.get("ref"))
                .and_then(Value::as_str)
                == Some(base)
        });
    }
    truncate_to_limit(&mut pulls, parsed.limit);

    let normalized = pulls
        .iter()
        .map(|pull| {
            let enriched = enrich_pull_statuses(
                &target_repo,
                client,
                pull,
                parsed.output.json_fields.as_slice(),
            )?;
            Ok(filter_fields(
                &normalize_pull(&enriched),
                &parsed.output.json_fields,
            ))
        })
        .collect::<Result<Vec<_>>>()?;

    if parsed.output.json_fields.is_empty() {
        for pull in normalized {
            writeln!(stdout, "{}", format_pull_list_item(&pull))?;
        }
    } else {
        print_json_list(&normalized, &parsed.output, stdout)?;
    }
    Ok(0)
}

fn run_checks(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_checks_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    let pull = resolve_pull(&target_repo, client, parsed.number, None, cwd)?;
    let statuses = pull
        .as_ref()
        .map(|pull| pull_statuses(&target_repo, client, pull))
        .transpose()?
        .unwrap_or_default();
    let checks = normalize_pr_checks(&statuses)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|check| filter_check_fields(check, &parsed.json_fields))
        .collect::<Vec<_>>();

    if parsed.json_fields.is_empty() {
        if checks.is_empty() {
            writeln!(stdout, "no checks reported")?;
        }
        for check in checks {
            writeln!(stdout, "{}", format_check_item(&check))?;
        }
    } else {
        let output = OutputArgs {
            json_fields: parsed.json_fields,
            jq: parsed.jq,
            template: None,
        };
        print_json_list(&checks, &output, stdout)?;
    }
    Ok(0)
}

fn run_diff(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_diff_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    let pull = resolve_pull(
        &target_repo,
        client,
        parsed.number,
        parsed.branch.as_deref(),
        cwd,
    )?;
    let Some(pull) = pull else {
        return Ok(0);
    };
    let Some(number) = pull_number(&pull) else {
        return Ok(0);
    };

    if parsed.web {
        let normalized = normalize_pull(&pull);
        let url = normalized.get("url").and_then(Value::as_str).map_or_else(
            || format!("{}/pulls/{number}", target_repo.web_base_url()),
            str::to_string,
        );
        writeln!(stdout, "{url}/files")?;
        return Ok(0);
    }

    if parsed.name_only {
        for name in filtered_file_names(
            &client.list_pull_files(&target_repo, number)?,
            parsed.excludes.as_slice(),
        ) {
            writeln!(stdout, "{name}")?;
        }
        return Ok(0);
    }

    let text = client.get_pull_diff(&target_repo, number, parsed.patch)?;
    if text.ends_with('\n') {
        write!(stdout, "{text}")?;
    } else {
        writeln!(stdout, "{text}")?;
    }
    Ok(0)
}

fn run_view(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_view_status_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    let pull = if let Some(number) = parsed.number {
        Some(client.get_pull(&target_repo, number)?)
    } else {
        let branch = parsed.branch.or_else(|| current_branch(cwd));
        match branch {
            Some(branch) => client
                .list_pulls(&target_repo, "open", Some(&branch))?
                .into_iter()
                .next(),
            None => None,
        }
    };

    let Some(pull) = pull else {
        if !parsed.output.json_fields.is_empty() {
            print_json(&json!({}), &parsed.output, stdout)?;
        }
        return Ok(0);
    };

    let enriched = enrich_pull_statuses(
        &target_repo,
        client,
        &pull,
        parsed.output.json_fields.as_slice(),
    )?;
    let normalized = normalize_pull(&enriched);
    if parsed.web {
        if let Some(url) = normalized.get("url").and_then(Value::as_str) {
            if !url.is_empty() {
                writeln!(stdout, "{url}")?;
            }
        }
        return Ok(0);
    }
    if parsed.output.json_fields.is_empty() {
        writeln!(stdout, "{}", format_pull_text(&normalized))?;
    } else {
        print_json(
            &filter_fields(&normalized, &parsed.output.json_fields),
            &parsed.output,
            stdout,
        )?;
    }
    Ok(0)
}

fn run_status(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_view_status_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    let branch = parsed.branch.or_else(|| current_branch(cwd));
    let pull = match branch {
        Some(branch) => client
            .list_pulls(&target_repo, "open", Some(&branch))?
            .into_iter()
            .next(),
        None => None,
    };

    if parsed.output.json_fields.is_empty() {
        if let Some(pull) = pull {
            writeln!(stdout, "{}", format_pull_text(&normalize_pull(&pull)))?;
        }
        return Ok(0);
    }

    let enriched = pull
        .as_ref()
        .map(|pull| {
            enrich_pull_statuses(
                &target_repo,
                client,
                pull,
                parsed.output.json_fields.as_slice(),
            )
        })
        .transpose()?;
    let status = status_for_current_branch(enriched.as_ref(), &parsed.output.json_fields);
    print_json(&status, &parsed.output, stdout)?;
    Ok(0)
}

fn run_repo_view(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    stdout: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_repo_view_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    let data = normalize_repo(&client.get_repo(&target_repo)?, &target_repo);
    if parsed.web {
        let url = data.get("url").and_then(Value::as_str).unwrap_or("");
        if !url.is_empty() {
            writeln!(stdout, "{url}")?;
        }
        return Ok(0);
    }
    if parsed.output.json_fields.is_empty() {
        let url = data
            .get("url")
            .and_then(Value::as_str)
            .map_or_else(|| target_repo.web_base_url(), str::to_string);
        writeln!(stdout, "{url}")?;
    } else {
        print_json(
            &filter_repo_fields(&data, &parsed.output.json_fields),
            &parsed.output,
            stdout,
        )?;
    }
    Ok(0)
}

fn run_issue_list(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    stdout: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_issue_list_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if parsed.web {
        writeln!(stdout, "{}/issues", target_repo.web_base_url())?;
        return Ok(0);
    }

    let options = ListIssuesOptions {
        state: Some(parsed.state),
        labels: parsed.labels,
        query: parsed.query,
        milestone: parsed.milestone,
        author: parsed.author,
        assignee: parsed.assignee,
        mentioned: parsed.mentioned,
        limit: parsed.limit,
    };
    let mut issues = client.list_issues(&target_repo, &options)?;
    truncate_to_limit(&mut issues, options.limit);
    let normalized = issues
        .iter()
        .map(|issue| filter_issue_fields(&normalize_issue(issue), &parsed.output.json_fields))
        .collect::<Vec<_>>();

    if parsed.output.json_fields.is_empty() {
        for issue in normalized {
            writeln!(stdout, "{}", format_issue_list_item(&issue))?;
        }
    } else {
        print_json_list(&normalized, &parsed.output, stdout)?;
    }
    Ok(0)
}

fn run_issue_view(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    stdout: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_issue_view_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    let number = parsed
        .number
        .ok_or_else(|| ShimError::new("Forgejo issue view requires an issue number or URL"))?;
    let normalized = normalize_issue(&client.get_issue(&target_repo, number)?);

    if parsed.web {
        let url = normalized.get("url").and_then(Value::as_str).map_or_else(
            || format!("{}/issues/{number}", target_repo.web_base_url()),
            str::to_string,
        );
        writeln!(stdout, "{url}")?;
        return Ok(0);
    }
    if parsed.output.json_fields.is_empty() {
        writeln!(stdout, "{}", format_issue_text(&normalized))?;
    } else {
        print_json(
            &filter_issue_fields(&normalized, &parsed.output.json_fields),
            &parsed.output,
            stdout,
        )?;
    }
    Ok(0)
}

fn parse_auth_status_args(argv: &[String]) -> Result<AuthStatusArgs> {
    let mut parsed = AuthStatusArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if matches!(arg, "-h" | "--hostname") {
            index = skip_value(argv, index, arg)?;
        } else if has_value_prefix(arg, &["--hostname", "-h"]) {
            index += 1;
        } else if arg == "--json" {
            parsed.output.json_fields = split_fields(required_value(argv, index, arg)?);
            index += 2;
        } else if let Some(value) = flag_value(arg, "--json") {
            parsed.output.json_fields = split_fields(value);
            index += 1;
        } else if matches!(arg, "--jq" | "-q") {
            parsed.output.jq = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--jq", "-q"]) {
            parsed.output.jq = Some(value.to_string());
            index += 1;
        } else if arg == "--template" {
            parsed.output.template = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--template") {
            parsed.output.template = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "-t" | "--show-token") {
            parsed.show_token = true;
            index += 1;
        } else if matches!(arg, "-a" | "--active") {
            parsed.active = true;
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo auth status flag: {arg}"
            )));
        } else {
            return Err(ShimError::new(format!(
                "unexpected positional argument for Forgejo auth status: {arg}"
            )));
        }
    }
    Ok(parsed)
}

fn parse_auth_token_args(argv: &[String]) -> Result<()> {
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if matches!(arg, "-h" | "--hostname" | "-u" | "--user") {
            index = skip_value(argv, index, arg)?;
        } else if has_value_prefix(arg, &["--hostname", "-h", "--user", "-u"]) {
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo auth token flag: {arg}"
            )));
        } else {
            return Err(ShimError::new(format!(
                "unexpected positional argument for Forgejo auth token: {arg}"
            )));
        }
    }
    Ok(())
}

fn parse_api_args(argv: &[String]) -> Result<ApiArgs> {
    let mut parsed = ApiArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if matches!(arg, "--jq" | "-q") {
            parsed.jq = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--jq", "-q"]) {
            parsed.jq = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--template" | "-t") {
            parsed.template = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--template", "-t"]) {
            parsed.template = Some(value.to_string());
            index += 1;
        } else if arg == "--silent" {
            parsed.silent = true;
            index += 1;
        } else if api_flag_takes_value(arg) {
            index = skip_value(argv, index, arg)?;
        } else if api_flag_has_value(arg) || arg.starts_with('-') {
            index += 1;
        } else if parsed.endpoint.is_empty() {
            parsed.endpoint = arg.to_string();
            index += 1;
        } else {
            return Err(ShimError::new(format!(
                "unexpected positional argument for Forgejo api: {arg}"
            )));
        }
    }
    if parsed.endpoint.is_empty() {
        return Err(ShimError::new("missing Forgejo api endpoint"));
    }
    Ok(parsed)
}

fn parse_list_args(argv: &[String]) -> Result<ListArgs> {
    let mut parsed = ListArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if parse_output_arg(argv, &mut index, &mut parsed.output)? {
            continue;
        }
        if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--state" | "-s") {
            parsed.state = normalize_pr_state(required_value(argv, index, arg)?)?;
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--state", "-s"]) {
            parsed.state = normalize_pr_state(value)?;
            index += 1;
        } else if matches!(arg, "--limit" | "-L") {
            parsed.limit = Some(parse_limit(required_value(argv, index, arg)?)?);
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--limit", "-L"]) {
            parsed.limit = Some(parse_limit(value)?);
            index += 1;
        } else if matches!(arg, "--head" | "-H") {
            parsed.head = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--head", "-H"]) {
            parsed.head = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--base" | "-B") {
            parsed.base = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--base", "-B"]) {
            parsed.base = Some(value.to_string());
            index += 1;
        } else if matches!(
            arg,
            "--author" | "--app" | "--assignee" | "--label" | "--search"
        ) {
            index = skip_value(argv, index, arg)?;
        } else if has_value_prefix(
            arg,
            &["--author", "--app", "--assignee", "--label", "--search"],
        ) {
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo PR list flag: {arg}"
            )));
        } else {
            index += 1;
        }
    }
    Ok(parsed)
}

fn parse_checks_args(argv: &[String]) -> Result<ChecksArgs> {
    let mut parsed = ChecksArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--json" {
            parsed.json_fields = split_fields(required_value(argv, index, arg)?);
            index += 2;
        } else if let Some(value) = flag_value(arg, "--json") {
            parsed.json_fields = split_fields(value);
            index += 1;
        } else if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--jq" | "-q") {
            parsed.jq = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--jq", "-q"]) {
            parsed.jq = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--template" | "-t" | "--interval" | "-i") {
            index = skip_value(argv, index, arg)?;
        } else if has_value_prefix(arg, &["--template", "-t", "--interval", "-i"])
            || matches!(
                arg,
                "--web" | "-w" | "--watch" | "--fail-fast" | "--required"
            )
        {
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo PR checks flag: {arg}"
            )));
        } else {
            if let (Some(number), _) = parse_number_or_branch(arg, "pull") {
                parsed.number = Some(number);
            }
            index += 1;
        }
    }
    Ok(parsed)
}

fn parse_diff_args(argv: &[String]) -> Result<DiffArgs> {
    let mut parsed = DiffArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--web" | "-w") {
            parsed.web = true;
            index += 1;
        } else if arg == "--name-only" {
            parsed.name_only = true;
            index += 1;
        } else if arg == "--patch" {
            parsed.patch = true;
            index += 1;
        } else if arg == "--color" {
            index = skip_value(argv, index, arg)?;
        } else if has_value_prefix(arg, &["--color"]) {
            index += 1;
        } else if matches!(arg, "--exclude" | "-e") {
            parsed
                .excludes
                .push(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--exclude", "-e"]) {
            parsed.excludes.push(value.to_string());
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo PR diff flag: {arg}"
            )));
        } else {
            let (number, branch) = parse_number_or_branch(arg, "pull");
            parsed.number = number;
            parsed.branch = branch;
            index += 1;
        }
    }
    Ok(parsed)
}

fn parse_view_status_args(argv: &[String]) -> Result<ViewStatusArgs> {
    let mut parsed = ViewStatusArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if parse_output_arg(argv, &mut index, &mut parsed.output)? {
            continue;
        }
        if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--web" | "-w") {
            parsed.web = true;
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo PR flag: {arg}"
            )));
        } else {
            let (number, branch) = parse_number_or_branch(arg, "pull");
            parsed.number = number;
            parsed.branch = branch;
            index += 1;
        }
    }
    Ok(parsed)
}

fn parse_repo_view_args(argv: &[String]) -> Result<RepoViewArgs> {
    let mut parsed = RepoViewArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if parse_output_arg(argv, &mut index, &mut parsed.output)? {
            continue;
        }
        if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--web" | "-w") {
            parsed.web = true;
            index += 1;
        } else if matches!(arg, "--branch" | "-b") {
            index = skip_value(argv, index, arg)?;
        } else if has_value_prefix(arg, &["--branch", "-b"]) {
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo repo view flag: {arg}"
            )));
        } else {
            parsed.repo = Some(arg.to_string());
            index += 1;
        }
    }
    Ok(parsed)
}

fn parse_issue_list_args(argv: &[String]) -> Result<IssueListArgs> {
    let mut parsed = IssueListArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if parse_output_arg(argv, &mut index, &mut parsed.output)? {
            continue;
        }
        if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--state" | "-s") {
            parsed.state = normalize_issue_state(required_value(argv, index, arg)?)?;
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--state", "-s"]) {
            parsed.state = normalize_issue_state(value)?;
            index += 1;
        } else if matches!(arg, "--limit" | "-L") {
            parsed.limit = Some(parse_limit(required_value(argv, index, arg)?)?);
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--limit", "-L"]) {
            parsed.limit = Some(parse_limit(value)?);
            index += 1;
        } else if matches!(arg, "--label" | "-l") {
            parsed
                .labels
                .extend(split_label_values(required_value(argv, index, arg)?));
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--label", "-l"]) {
            parsed.labels.extend(split_label_values(value));
            index += 1;
        } else if matches!(arg, "--search" | "-S") {
            parsed.query = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--search", "-S"]) {
            parsed.query = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--author" | "-A") {
            parsed.author = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--author", "-A"]) {
            parsed.author = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--assignee" | "-a") {
            parsed.assignee = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--assignee", "-a"]) {
            parsed.assignee = Some(value.to_string());
            index += 1;
        } else if arg == "--mention" {
            parsed.mentioned = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--mention") {
            parsed.mentioned = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--milestone" | "-m") {
            parsed.milestone = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--milestone", "-m"]) {
            parsed.milestone = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--web" | "-w") {
            parsed.web = true;
            index += 1;
        } else if arg == "--app" {
            index = skip_value(argv, index, arg)?;
        } else if has_value_prefix(arg, &["--app"]) {
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo issue list flag: {arg}"
            )));
        } else {
            index += 1;
        }
    }
    Ok(parsed)
}

fn parse_issue_view_args(argv: &[String]) -> Result<IssueViewArgs> {
    let mut parsed = IssueViewArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if parse_output_arg(argv, &mut index, &mut parsed.output)? {
            continue;
        }
        if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--web" | "-w") {
            parsed.web = true;
            index += 1;
        } else if matches!(arg, "--comments" | "-c") {
            parsed.comments = true;
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo issue view flag: {arg}"
            )));
        } else {
            let (number, _) = parse_number_or_branch(arg, "issue");
            parsed.number = number;
            index += 1;
        }
    }
    Ok(parsed)
}

fn parse_output_arg(argv: &[String], index: &mut usize, output: &mut OutputArgs) -> Result<bool> {
    let arg = argv[*index].as_str();
    if arg == "--json" {
        output.json_fields = split_fields(required_value(argv, *index, arg)?);
        *index += 2;
        return Ok(true);
    }
    if let Some(value) = flag_value(arg, "--json") {
        output.json_fields = split_fields(value);
        *index += 1;
        return Ok(true);
    }
    if matches!(arg, "--jq" | "-q") {
        output.jq = Some(required_value(argv, *index, arg)?.to_string());
        *index += 2;
        return Ok(true);
    }
    if let Some(value) = flag_value_any(arg, &["--jq", "-q"]) {
        output.jq = Some(value.to_string());
        *index += 1;
        return Ok(true);
    }
    if matches!(arg, "--template" | "-t") {
        output.template = Some(required_value(argv, *index, arg)?.to_string());
        *index += 2;
        return Ok(true);
    }
    if let Some(value) = flag_value_any(arg, &["--template", "-t"]) {
        output.template = Some(value.to_string());
        *index += 1;
        return Ok(true);
    }
    Ok(false)
}

fn print_json(value: &Value, output: &OutputArgs, stdout: &mut dyn Write) -> Result<()> {
    if let Some(rendered) =
        render_json_or_jq(value, output.jq.as_deref(), output.template.as_deref())
    {
        write!(stdout, "{rendered}")?;
    }
    Ok(())
}

fn print_json_list(values: &[Value], output: &OutputArgs, stdout: &mut dyn Write) -> Result<()> {
    if let Some(rendered) =
        render_json_or_jq_list(values, output.jq.as_deref(), output.template.as_deref())
    {
        write!(stdout, "{rendered}")?;
    }
    Ok(())
}

fn auth_status_data(host: &str, token: &str, show_token: bool) -> Value {
    let mut entry = json!({
        "state": "success",
        "active": true,
        "host": host,
        "login": "",
        "tokenSource": "gh-forgejo-shim",
        "scopes": "",
        "gitProtocol": "https",
    });
    if show_token {
        if let Some(object) = entry.as_object_mut() {
            object.insert("token".to_string(), Value::String(token.to_string()));
        }
    }
    json!({ "hosts": { host: [entry] } })
}

fn print_auth_status_text(host: &str, token: Option<&str>, stdout: &mut dyn Write) -> Result<()> {
    writeln!(stdout, "{host}")?;
    writeln!(stdout, "  [ok] Logged in to {host} using gh-forgejo-shim")?;
    writeln!(stdout, "  - Active account: true")?;
    writeln!(stdout, "  - Token source: gh-forgejo-shim")?;
    if let Some(token) = token {
        writeln!(stdout, "  - Token: {token}")?;
    }
    Ok(())
}

fn print_missing_auth_status(host: &str, stderr: &mut dyn Write) -> Result<()> {
    writeln!(stderr, "{host}")?;
    writeln!(stderr, "  X Not logged in to {host}")?;
    writeln!(stderr, "  - Run: gh-forgejo-shim auth login {host}")?;
    Ok(())
}

fn filter_selected_fields(data: &Value, fields: &[String]) -> Value {
    if fields.is_empty() {
        return data.clone();
    }
    let mut object = serde_json::Map::new();
    for field in fields {
        if let Some(value) = data.get(field) {
            object.insert(field.clone(), value.clone());
        }
    }
    Value::Object(object)
}

fn missing_auth_message(host: &str) -> String {
    format!(
        "gh-forgejo-shim: Forgejo auth is not configured for {host}; run gh-forgejo-shim auth login {host} or gh-forgejo-shim auth import {host}"
    )
}

fn target_repo(value: Option<&str>, fallback: &ForgejoRepoRef) -> Result<ForgejoRepoRef> {
    match value {
        Some(value) => parse_repo_spec(value, Some(&fallback.host))
            .map(|repo| ForgejoRepoRef::new(repo.host, repo.owner, repo.name))
            .ok_or_else(|| ShimError::new("could not parse --repo value")),
        None => Ok(fallback.clone()),
    }
}

fn resolve_pull(
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    number: Option<u64>,
    branch: Option<&str>,
    cwd: Option<&Path>,
) -> Result<Option<Value>> {
    if let Some(number) = number {
        return Ok(Some(client.get_pull(repo, number)?));
    }
    let Some(target_branch) = branch.map(str::to_string).or_else(|| current_branch(cwd)) else {
        return Ok(None);
    };
    Ok(client
        .list_pulls(repo, "open", Some(&target_branch))?
        .into_iter()
        .next())
}

fn enrich_pull_statuses(
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    pull: &Value,
    fields: &[String],
) -> Result<Value> {
    if !fields.iter().any(|field| field == "statusCheckRollup") {
        return Ok(pull.clone());
    }
    let statuses = pull_statuses(repo, client, pull)?;
    Ok(with_status_check_rollup(pull, &statuses))
}

fn pull_statuses(
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    pull: &Value,
) -> Result<Vec<Value>> {
    let Some(sha) = pull_head_sha(pull) else {
        return Ok(Vec::new());
    };
    Ok(client.list_commit_statuses(repo, &sha)?)
}

fn pull_number(pull: &Value) -> Option<u64> {
    let raw = pull.get("number").or_else(|| pull.get("id"))?;
    if raw.is_boolean() {
        return None;
    }
    raw.as_u64().or_else(|| raw.as_str()?.parse().ok())
}

fn pull_head_sha(pull: &Value) -> Option<String> {
    pull.get("head")
        .and_then(Value::as_object)
        .and_then(|head| head.get("sha"))
        .and_then(Value::as_str)
        .filter(|sha| !sha.is_empty())
        .map(ToOwned::to_owned)
}

fn current_branch(cwd: Option<&Path>) -> Option<String> {
    git_output(&["branch", "--show-current"], cwd)
}

fn format_pull_text(data: &Value) -> String {
    format!(
        "#{} {}\nstate: {}\nurl: {}",
        python_text(data.get("number")),
        truthy_text(data.get("title")).unwrap_or_default(),
        truthy_text(data.get("state")).unwrap_or_else(|| "UNKNOWN".to_string()),
        truthy_text(data.get("url")).unwrap_or_default(),
    )
    .trim_end()
    .to_string()
}

fn format_pull_list_item(data: &Value) -> String {
    format!(
        "{}\t{}\t{}",
        python_text(data.get("number")),
        truthy_text(data.get("title")).unwrap_or_default(),
        truthy_text(data.get("headRefName")).unwrap_or_default(),
    )
    .trim_end()
    .to_string()
}

fn format_issue_text(data: &Value) -> String {
    format!(
        "#{} {}\nstate: {}\nurl: {}\n\n{}",
        python_text(data.get("number")),
        truthy_text(data.get("title")).unwrap_or_default(),
        truthy_text(data.get("state")).unwrap_or_else(|| "UNKNOWN".to_string()),
        truthy_text(data.get("url")).unwrap_or_default(),
        truthy_text(data.get("body")).unwrap_or_default(),
    )
    .trim_end()
    .to_string()
}

fn format_issue_list_item(data: &Value) -> String {
    format!(
        "{}\t{}\t{}",
        python_text(data.get("number")),
        truthy_text(data.get("title")).unwrap_or_default(),
        truthy_text(data.get("state")).unwrap_or_default(),
    )
    .trim_end()
    .to_string()
}

fn format_check_item(data: &Value) -> String {
    format!(
        "{}\t{}\t{}",
        truthy_text(data.get("bucket")).unwrap_or_else(|| "pending".to_string()),
        truthy_text(data.get("name")).unwrap_or_else(|| "status".to_string()),
        truthy_text(data.get("description")).unwrap_or_default(),
    )
    .trim_end()
    .to_string()
}

fn python_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Null) | None => "None".to_string(),
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Bool(false)) => "False".to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(value @ (Value::Array(_) | Value::Object(_))) => value.to_string(),
    }
}

fn truthy_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::Null => None,
        Value::Bool(false) => None,
        Value::Bool(true) => Some("True".to_string()),
        Value::Number(number) if number.as_i64() == Some(0) || number.as_u64() == Some(0) => None,
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) if text.is_empty() => None,
        Value::String(text) => Some(text.clone()),
        Value::Array(items) if items.is_empty() => None,
        Value::Object(object) if object.is_empty() => None,
        value @ (Value::Array(_) | Value::Object(_)) => Some(value.to_string()),
    }
}

fn filtered_file_names(files: &[Value], excludes: &[String]) -> Vec<String> {
    files
        .iter()
        .filter_map(|item| {
            item.get("filename")
                .or_else(|| item.get("name"))
                .or_else(|| item.get("path"))
                .and_then(Value::as_str)
        })
        .filter(|name| !excludes.iter().any(|pattern| wildcard_match(pattern, name)))
        .map(ToOwned::to_owned)
        .collect()
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    wildcard_match_bytes(pattern.as_bytes(), value.as_bytes())
}

fn wildcard_match_bytes(pattern: &[u8], value: &[u8]) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }
    if pattern[0] == b'*' {
        return wildcard_match_bytes(&pattern[1..], value)
            || (!value.is_empty() && wildcard_match_bytes(pattern, &value[1..]));
    }
    !value.is_empty()
        && (pattern[0] == b'?' || pattern[0] == value[0])
        && wildcard_match_bytes(&pattern[1..], &value[1..])
}

fn truncate_to_limit(values: &mut Vec<Value>, limit: Option<usize>) {
    if let Some(limit) = limit {
        values.truncate(limit);
    }
}

fn split_fields(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn split_label_values(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_limit(value: &str) -> Result<usize> {
    let limit = value
        .parse::<isize>()
        .map_err(|_| ShimError::new(format!("invalid --limit value: {value}")))?;
    if limit < 0 {
        return Err(ShimError::new("--limit must be non-negative"));
    }
    usize::try_from(limit).map_err(|_| ShimError::new(format!("invalid --limit value: {value}")))
}

fn normalize_pr_state(value: &str) -> Result<String> {
    let normalized = value.to_ascii_lowercase();
    if matches!(normalized.as_str(), "open" | "closed" | "merged" | "all") {
        Ok(normalized)
    } else {
        Err(ShimError::new(
            "--state must be one of: open, closed, merged, all",
        ))
    }
}

fn normalize_issue_state(value: &str) -> Result<String> {
    let normalized = value.to_ascii_lowercase();
    if matches!(normalized.as_str(), "open" | "closed" | "all") {
        Ok(normalized)
    } else {
        Err(ShimError::new("--state must be one of: open, closed, all"))
    }
}

fn parse_number_or_branch(value: &str, kind: &str) -> (Option<u64>, Option<String>) {
    if let Some(number) = number_from_url(value, kind) {
        return (Some(number), None);
    }
    if let Ok(number) = value.parse::<u64>() {
        return (Some(number), None);
    }
    (None, Some(value.to_string()))
}

fn number_from_url(value: &str, kind: &str) -> Option<u64> {
    let scheme_end = value.find("://")?;
    let scheme = &value[..scheme_end];
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let rest = &value[scheme_end + 3..];
    let path_start = rest.find('/')?;
    let path = &rest[path_start + 1..];
    let markers = if kind == "pull" {
        ["pulls", "pull"]
    } else {
        ["issues", "issue"]
    };
    let parts = path
        .split(['/', '?', '#'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    parts.windows(2).find_map(|window| {
        markers
            .contains(&window[0])
            .then(|| window[1].parse::<u64>().ok())
            .flatten()
    })
}

fn required_value<'a>(argv: &'a [String], index: usize, flag: &str) -> Result<&'a str> {
    argv.get(index + 1)
        .map(String::as_str)
        .ok_or_else(|| ShimError::new(format!("missing value for {flag}")))
}

fn skip_value(argv: &[String], index: usize, flag: &str) -> Result<usize> {
    required_value(argv, index, flag)?;
    Ok(index + 2)
}

fn flag_value<'a>(arg: &'a str, flag: &str) -> Option<&'a str> {
    arg.strip_prefix(flag)?.strip_prefix('=')
}

fn flag_value_any<'a>(arg: &'a str, flags: &[&str]) -> Option<&'a str> {
    flags.iter().find_map(|flag| flag_value(arg, flag))
}

fn has_value_prefix(arg: &str, flags: &[&str]) -> bool {
    flag_value_any(arg, flags).is_some()
}

fn api_flag_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "--cache"
            | "-F"
            | "--field"
            | "-H"
            | "--header"
            | "--hostname"
            | "--input"
            | "--method"
            | "-X"
            | "--preview"
            | "-p"
            | "-f"
            | "--raw-field"
    )
}

fn api_flag_has_value(arg: &str) -> bool {
    has_value_prefix(
        arg,
        &[
            "--cache",
            "-F",
            "--field",
            "-H",
            "--header",
            "--hostname",
            "--input",
            "--method",
            "-X",
            "--preview",
            "-p",
            "-f",
            "--raw-field",
        ],
    )
}

fn env_map(env: &HashMap<String, String>) -> EnvMap {
    env.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn home_path(env: &HashMap<String, String>) -> Option<PathBuf> {
    env.get("HOME")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn current_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::consts::OS
    }
}

fn to_forgejo_repo(repo: &DetectedRepoRef) -> ForgejoRepoRef {
    ForgejoRepoRef::new(repo.host.clone(), repo.owner.clone(), repo.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forgejo::ForgejoError;

    #[derive(Clone)]
    struct FakeApi {
        pulls: Vec<Value>,
        issues: Vec<Value>,
        statuses: Vec<Value>,
        files: Vec<Value>,
        repo: Value,
        user: Value,
        diff: String,
    }

    impl Default for FakeApi {
        fn default() -> Self {
            Self {
                pulls: Vec::new(),
                issues: Vec::new(),
                statuses: Vec::new(),
                files: vec![
                    json!({"filename": "README.md"}),
                    json!({"filename": "generated/output.txt"}),
                ],
                repo: json!({
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
                }),
                user: json!({"login": "alice", "full_name": "Alice Example"}),
                diff: "diff --git a/README.md b/README.md\n".to_string(),
            }
        }
    }

    impl ForgejoApi for FakeApi {
        fn get_current_user(&self, _host: &str) -> ForgejoResult<Value> {
            Ok(self.user.clone())
        }

        fn get_repo(&self, _repo: &ForgejoRepoRef) -> ForgejoResult<Value> {
            Ok(self.repo.clone())
        }

        fn list_pulls(
            &self,
            _repo: &ForgejoRepoRef,
            _state: &str,
            head: Option<&str>,
        ) -> ForgejoResult<Vec<Value>> {
            Ok(self
                .pulls
                .iter()
                .filter(|pull| {
                    head.is_none_or(|head| {
                        pull.get("head")
                            .and_then(Value::as_object)
                            .and_then(|head_data| head_data.get("ref"))
                            .and_then(Value::as_str)
                            == Some(head)
                    })
                })
                .cloned()
                .collect())
        }

        fn get_pull(&self, _repo: &ForgejoRepoRef, number: u64) -> ForgejoResult<Value> {
            self.pulls
                .iter()
                .find(|pull| pull_number(pull) == Some(number))
                .cloned()
                .ok_or_else(|| ForgejoError::new("not found"))
        }

        fn list_commit_statuses(
            &self,
            _repo: &ForgejoRepoRef,
            _sha: &str,
        ) -> ForgejoResult<Vec<Value>> {
            Ok(self.statuses.clone())
        }

        fn get_pull_diff(
            &self,
            _repo: &ForgejoRepoRef,
            _number: u64,
            _patch: bool,
        ) -> ForgejoResult<String> {
            Ok(self.diff.clone())
        }

        fn list_pull_files(
            &self,
            _repo: &ForgejoRepoRef,
            _number: u64,
        ) -> ForgejoResult<Vec<Value>> {
            Ok(self.files.clone())
        }

        fn list_issues(
            &self,
            _repo: &ForgejoRepoRef,
            _options: &ListIssuesOptions,
        ) -> ForgejoResult<Vec<Value>> {
            Ok(self.issues.clone())
        }

        fn get_issue(&self, _repo: &ForgejoRepoRef, number: u64) -> ForgejoResult<Value> {
            self.issues
                .iter()
                .find(|issue| pull_number(issue) == Some(number))
                .cloned()
                .ok_or_else(|| ForgejoError::new("not found"))
        }
    }

    fn repo() -> ForgejoRepoRef {
        ForgejoRepoRef::new("git.example.com", "owner", "repo")
    }

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn run_fake(
        args: &[&str],
        client: &FakeApi,
        token: Option<&str>,
        cwd: Option<&Path>,
    ) -> (i32, String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_with_client(
            &argv(args),
            Some("git.example.com"),
            Some(&repo()),
            token,
            client,
            cwd,
            &mut stdout,
            &mut stderr,
        )
        .unwrap_or_else(|error| {
            let _ = writeln!(stderr, "gh-forgejo-shim: {error}");
            1
        });
        (
            code,
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
        )
    }

    #[test]
    fn auth_status_outputs_text_and_json_without_leaking_token_by_default() {
        let client = FakeApi::default();

        let (code, stdout, stderr) =
            run_fake(&["auth", "status"], &client, Some("secret-token"), None);

        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("Logged in to git.example.com"), "{stdout}");
        assert!(!stdout.contains("secret-token"), "{stdout}");

        let (code, stdout, stderr) = run_fake(
            &["auth", "status", "--json", "hosts"],
            &client,
            Some("secret-token"),
            None,
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).ok(),
            Some(json!({
                "hosts": {
                    "git.example.com": [{
                        "active": true,
                        "gitProtocol": "https",
                        "host": "git.example.com",
                        "login": "",
                        "scopes": "",
                        "state": "success",
                        "tokenSource": "gh-forgejo-shim",
                    }]
                }
            }))
        );
    }

    #[test]
    fn api_user_supports_simple_jq() {
        let client = FakeApi::default();

        let (code, stdout, stderr) = run_fake(
            &["api", "user", "--jq", ".login"],
            &client,
            Some("token"),
            None,
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "alice\n");
    }

    #[test]
    fn pr_list_outputs_codex_board_fields_with_status_rollup() {
        let client = FakeApi {
            pulls: vec![json!({
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
            })],
            statuses: vec![json!({
                "context": "ci/test",
                "description": "tests passed",
                "state": "success",
                "target_url": "https://git.example.com/owner/repo/actions/runs/1",
                "created_at": "2026-06-01T00:00:00Z",
                "updated_at": "2026-06-01T00:02:00Z"
            })],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &[
                "pr",
                "list",
                "--json",
                "number,title,url,headRefName,statusCheckRollup",
            ],
            &client,
            None,
            None,
        );

        assert_eq!(code, 0, "{stderr}");
        let value = serde_json::from_str::<Value>(&stdout).ok();
        assert_eq!(
            value.as_ref().and_then(|value| value[0]["number"].as_i64()),
            Some(8)
        );
        assert_eq!(
            value
                .as_ref()
                .and_then(|value| value[0]["statusCheckRollup"][0]["state"].as_str()),
            Some("SUCCESS")
        );
    }

    #[test]
    fn pr_checks_and_diff_use_statuses_and_files() {
        let client = FakeApi {
            pulls: vec![json!({"number": 8, "head": {"ref": "feature", "sha": "abc123"}})],
            statuses: vec![json!({
                "context": "ci/test",
                "description": "tests passed",
                "state": "success",
                "target_url": "https://git.example.com/owner/repo/actions/runs/1",
            })],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &[
                "pr",
                "checks",
                "8",
                "--json",
                "bucket,description,link,name,workflow",
            ],
            &client,
            None,
            None,
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).ok(),
            Some(json!([{
                "bucket": "pass",
                "description": "tests passed",
                "link": "https://git.example.com/owner/repo/actions/runs/1",
                "name": "ci/test",
                "workflow": "ci/test",
            }]))
        );

        let (code, stdout, stderr) = run_fake(
            &["pr", "diff", "8", "--name-only", "--exclude", "generated/*"],
            &client,
            None,
            None,
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "README.md\n");
    }

    #[test]
    fn no_current_pr_view_and_status_return_empty_success_shapes() {
        let client = FakeApi::default();

        let (code, stdout, stderr) = run_fake(
            &["pr", "view", "feature", "--json", "number,title"],
            &client,
            None,
            None,
        );
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "{}");

        let (code, stdout, stderr) = run_fake(
            &["pr", "status", "feature", "--json", "number,title"],
            &client,
            None,
            None,
        );
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).ok(),
            Some(json!({"createdBy": [], "currentBranch": null, "needsReview": []}))
        );
    }

    #[test]
    fn repo_and_issue_json_outputs_match_github_shapes() {
        let client = FakeApi {
            issues: vec![json!({
                "number": 13,
                "title": "Fix it",
                "body": "Details",
                "state": "closed",
                "html_url": "https://git.example.com/owner/repo/issues/13",
                "user": {"login": "alice"},
                "comments": 2
            })],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &[
                "repo",
                "view",
                "--json",
                "nameWithOwner,url,defaultBranchRef,sshUrl",
            ],
            &client,
            None,
            None,
        );
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).ok(),
            Some(json!({
                "defaultBranchRef": {"name": "main"},
                "nameWithOwner": "owner/repo",
                "sshUrl": "git@git.example.com:owner/repo.git",
                "url": "https://git.example.com/owner/repo",
            }))
        );

        let (code, stdout, stderr) = run_fake(
            &[
                "issue",
                "view",
                "13",
                "--json",
                "number,title,body,state,closed",
            ],
            &client,
            None,
            None,
        );
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).ok(),
            Some(json!({
                "body": "Details",
                "closed": true,
                "number": 13,
                "state": "CLOSED",
                "title": "Fix it",
            }))
        );
    }
}
