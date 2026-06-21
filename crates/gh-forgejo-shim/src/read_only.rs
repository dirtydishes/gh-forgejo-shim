//! Read-only Forgejo command handlers for managed `gh` invocations.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::config::EnvMap;
use crate::external::{git_output, git_run, open_web_url, GitRunResult};
use crate::forgejo::{
    CreateIssueRequest, CreatePullRequest, ForgejoClient, ForgejoResult, ListIssuesOptions,
    RepoRef as ForgejoRepoRef,
};
use crate::normalize::{
    filter_check_fields, filter_fields, filter_issue_fields, filter_repo_fields, normalize_issue,
    normalize_pr_checks, normalize_pull, normalize_repo, render_json_or_jq, render_json_or_jq_list,
    status_for_current_branch, with_status_check_rollup,
};
use crate::repo::{parse_repo_spec, RepoRef as DetectedRepoRef};
use crate::routing::RouteDecision;
use crate::{auth, create};
use crate::{Result, ShimError};

trait ForgejoApi {
    fn get_current_user(&self, host: &str) -> ForgejoResult<Value>;
    fn get_repo(&self, repo: &ForgejoRepoRef) -> ForgejoResult<Value>;
    fn create_pull(
        &self,
        repo: &ForgejoRepoRef,
        request: &CreatePullRequest,
    ) -> ForgejoResult<Value>;
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
    fn list_labels(&self, repo: &ForgejoRepoRef) -> ForgejoResult<Vec<Value>>;
    fn create_issue(
        &self,
        repo: &ForgejoRepoRef,
        request: &CreateIssueRequest,
    ) -> ForgejoResult<Value>;
    fn create_issue_comment(
        &self,
        repo: &ForgejoRepoRef,
        number: u64,
        body: &str,
    ) -> ForgejoResult<Value>;
}

struct RepoCommandContext<'a> {
    repo: &'a ForgejoRepoRef,
    client: &'a dyn ForgejoApi,
    token_present: bool,
    cwd: Option<&'a Path>,
}

impl ForgejoApi for ForgejoClient {
    fn get_current_user(&self, host: &str) -> ForgejoResult<Value> {
        ForgejoClient::get_current_user(self, host)
    }

    fn get_repo(&self, repo: &ForgejoRepoRef) -> ForgejoResult<Value> {
        ForgejoClient::get_repo(self, repo)
    }

    fn create_pull(
        &self,
        repo: &ForgejoRepoRef,
        request: &CreatePullRequest,
    ) -> ForgejoResult<Value> {
        ForgejoClient::create_pull(self, repo, request)
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

    fn list_labels(&self, repo: &ForgejoRepoRef) -> ForgejoResult<Vec<Value>> {
        ForgejoClient::list_labels(self, repo)
    }

    fn create_issue(
        &self,
        repo: &ForgejoRepoRef,
        request: &CreateIssueRequest,
    ) -> ForgejoResult<Value> {
        ForgejoClient::create_issue(self, repo, request)
    }

    fn create_issue_comment(
        &self,
        repo: &ForgejoRepoRef,
        number: u64,
        body: &str,
    ) -> ForgejoResult<Value> {
        ForgejoClient::create_issue_comment(self, repo, number, body)
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CommentArgs {
    repo: Option<String>,
    number: Option<u64>,
    branch: Option<String>,
    body: Option<String>,
    web: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CheckoutArgs {
    repo: Option<String>,
    number: Option<u64>,
    branch: Option<String>,
    local_branch: Option<String>,
    detach: bool,
    force: bool,
    recurse_submodules: bool,
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
struct IssueCreateArgs {
    title: Option<String>,
    body: Option<String>,
    repo: Option<String>,
    web: bool,
    assignees: Vec<String>,
    labels: Vec<String>,
    milestone: Option<i64>,
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
    stdin: &mut dyn Read,
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
        stdin,
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
    stdin: &mut dyn Read,
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

    let token_present = token.is_some();
    let client = ForgejoClient::new(token);
    let repo = repo.ok_or_else(|| ShimError::new("could not determine Forgejo repository"))?;
    let context = RepoCommandContext {
        repo,
        client: &client,
        token_present,
        cwd,
    };
    run_repo_command(argv, &context, stdout, stderr, stdin)
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
    stdin: &mut dyn Read,
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
    let context = RepoCommandContext {
        repo,
        client,
        token_present: token.is_some(),
        cwd,
    };
    run_repo_command(argv, &context, stdout, stderr, stdin)
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
    context: &RepoCommandContext<'_>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    stdin: &mut dyn Read,
) -> Result<i32> {
    match argv.first().map(String::as_str) {
        Some("repo") => run_repo_view_command(
            argv,
            context.repo,
            context.client,
            context.token_present,
            stdout,
            stderr,
        ),
        Some("issue") => run_issue_command(
            argv,
            context.repo,
            context.client,
            context.token_present,
            stdout,
            stderr,
            stdin,
        ),
        Some("pr") => run_pr_command(argv, context, stdout, stderr, stdin),
        Some(other) => Err(ShimError::new(format!(
            "unsupported Forgejo command: {other}"
        ))),
        None => Err(ShimError::new("unsupported empty Forgejo command")),
    }
}

fn run_pr_command(
    argv: &[String],
    context: &RepoCommandContext<'_>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    stdin: &mut dyn Read,
) -> Result<i32> {
    let Some(command) = argv.get(1).map(String::as_str) else {
        return Err(ShimError::new("missing Forgejo PR command"));
    };
    match command {
        "checks" => run_checks(
            &argv[2..],
            context.repo,
            context.client,
            context.token_present,
            context.cwd,
            stdout,
            stderr,
        ),
        "checkout" | "co" => run_checkout(
            &argv[2..],
            context.repo,
            context.client,
            context.token_present,
            context.cwd,
            stdout,
            stderr,
        ),
        "comment" => run_comment(
            &argv[2..],
            context.repo,
            context.client,
            context.token_present,
            context.cwd,
            stdout,
            stderr,
            stdin,
        ),
        "create" | "new" => run_create(
            &argv[2..],
            context.repo,
            context.client,
            context.token_present,
            context.cwd,
            stdout,
            stderr,
            stdin,
        ),
        "diff" => run_diff(
            &argv[2..],
            context.repo,
            context.client,
            context.token_present,
            context.cwd,
            stdout,
            stderr,
        ),
        "list" => run_list(
            &argv[2..],
            context.repo,
            context.client,
            context.token_present,
            stdout,
            stderr,
        ),
        "status" => run_status(
            &argv[2..],
            context.repo,
            context.client,
            context.token_present,
            context.cwd,
            stdout,
            stderr,
        ),
        "view" => run_view(
            &argv[2..],
            context.repo,
            context.client,
            context.token_present,
            context.cwd,
            stdout,
            stderr,
        ),
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
    token_present: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    match argv.get(1).map(String::as_str) {
        Some("view") => run_repo_view(&argv[2..], repo, client, token_present, stdout, stderr),
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
    token_present: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    stdin: &mut dyn Read,
) -> Result<i32> {
    let Some(command) = argv.get(1).map(String::as_str) else {
        return Err(ShimError::new("missing Forgejo issue command"));
    };
    match command {
        "create" | "new" => run_issue_create(
            &argv[2..],
            repo,
            client,
            token_present,
            stdout,
            stderr,
            stdin,
        ),
        "list" | "ls" => run_issue_list(&argv[2..], repo, client, token_present, stdout, stderr),
        "view" => run_issue_view(&argv[2..], repo, client, token_present, stdout, stderr),
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
    token_present: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_list_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }
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

fn run_checkout(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    token_present: bool,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_checkout_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }
    let pull = resolve_pull(
        &target_repo,
        client,
        parsed.number,
        parsed.branch.as_deref(),
        cwd,
    )?
    .ok_or_else(|| ShimError::new("could not find pull request to check out"))?;
    let number = pull_number(&pull);
    let head_ref = pull_head_ref(&pull);

    let mut fetch_refs = Vec::new();
    if let Some(head_ref) = head_ref.as_deref() {
        fetch_refs.push(head_ref.to_string());
    }
    if let Some(number) = number {
        fetch_refs.push(format!("refs/pull/{number}/head"));
    }

    let mut fetch_error = String::new();
    let mut fetched = false;
    for fetch_ref in dedupe_strings(fetch_refs) {
        let output = git_run(&["fetch", "origin", fetch_ref.as_str()], cwd);
        if output.code == 0 {
            fetched = true;
            break;
        }
        fetch_error = output.message;
    }
    if !fetched {
        writeln!(
            stderr,
            "{}",
            if fetch_error.is_empty() {
                "git fetch failed"
            } else {
                fetch_error.as_str()
            }
        )?;
        return Ok(1);
    }

    if parsed.detach {
        let output = git_run(&["checkout", "--detach", "FETCH_HEAD"], cwd);
        if output.code != 0 {
            writeln!(
                stderr,
                "{}",
                git_error_message(&output, "git checkout failed")
            )?;
            return Ok(output.code);
        }
        writeln!(stdout, "checked out FETCH_HEAD")?;
        return Ok(0);
    }

    let branch = parsed
        .local_branch
        .clone()
        .or(head_ref)
        .or_else(|| number.map(|number| format!("pull-{number}")))
        .ok_or_else(|| ShimError::new("could not determine local branch name"))?;
    let output = if parsed.force {
        git_run(&["checkout", "-B", branch.as_str(), "FETCH_HEAD"], cwd)
    } else {
        let ref_name = format!("refs/heads/{branch}");
        let exists = git_run(&["show-ref", "--verify", "--quiet", ref_name.as_str()], cwd);
        if exists.code == 0 {
            git_run(&["checkout", branch.as_str()], cwd)
        } else {
            git_run(&["checkout", "-b", branch.as_str(), "FETCH_HEAD"], cwd)
        }
    };
    if output.code != 0 {
        writeln!(
            stderr,
            "{}",
            git_error_message(&output, "git checkout failed")
        )?;
        return Ok(output.code);
    }

    if parsed.recurse_submodules {
        let output = git_run(&["submodule", "update", "--init", "--recursive"], cwd);
        if output.code != 0 {
            writeln!(
                stderr,
                "{}",
                git_error_message(&output, "git submodule update failed")
            )?;
            return Ok(output.code);
        }
    }

    writeln!(stdout, "checked out {branch}")?;
    Ok(0)
}

fn run_checks(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    token_present: bool,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_checks_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }
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

#[allow(clippy::too_many_arguments)]
fn run_comment(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    token_present: bool,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    stdin: &mut dyn Read,
) -> Result<i32> {
    let parsed = parse_comment_args(argv, stdin)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }

    let pull = resolve_pull(
        &target_repo,
        client,
        parsed.number,
        parsed.branch.as_deref(),
        cwd,
    )?
    .ok_or_else(|| ShimError::new("could not find pull request to comment on"))?;
    let number = pull_number(&pull)
        .ok_or_else(|| ShimError::new("could not determine pull request number"))?;

    if parsed.web {
        let normalized = normalize_pull(&pull);
        let url = normalized.get("url").and_then(Value::as_str).map_or_else(
            || format!("{}/pulls/{number}", target_repo.web_base_url()),
            str::to_string,
        );
        open_web_url(&format!("{url}#new-comment"));
        writeln!(stdout, "{url}#new-comment")?;
        return Ok(0);
    }

    let body = parsed
        .body
        .ok_or_else(|| ShimError::new("Forgejo PR comment requires --body or --body-file"))?;
    client.create_issue_comment(&target_repo, number, &body)?;
    Ok(0)
}

fn run_diff(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    token_present: bool,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_diff_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }
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
    token_present: bool,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_view_status_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }
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
    token_present: bool,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_view_status_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }
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

#[allow(clippy::too_many_arguments)]
fn run_create(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    token_present: bool,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    stdin: &mut dyn Read,
) -> Result<i32> {
    let parsed = create::parse_create_args_with_reader(argv, stdin)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }

    let base = parsed
        .base
        .unwrap_or_else(|| create::default_base_branch(cwd));
    let head = parsed
        .head
        .or_else(|| current_branch(cwd))
        .ok_or_else(|| ShimError::new("could not determine head branch; pass --head"))?;
    let title = parsed
        .title
        .or_else(|| {
            (parsed.fill || parsed.fill_first || parsed.fill_verbose)
                .then(|| create::fill_title(cwd))
                .flatten()
        })
        .ok_or_else(|| {
            ShimError::new(
                "Forgejo PR create requires --title or one of --fill/--fill-first/--fill-verbose",
            )
        })?;
    let body = parsed
        .body
        .or_else(|| {
            (parsed.fill || parsed.fill_first || parsed.fill_verbose)
                .then(|| create::fill_body(parsed.fill_verbose, cwd))
        })
        .unwrap_or_default();

    if parsed.web {
        let url = format!("{}/compare/{base}...{head}", target_repo.web_base_url());
        open_web_url(&url);
        writeln!(stdout, "{url}")?;
        return Ok(0);
    }

    let pull = client.create_pull(
        &target_repo,
        &CreatePullRequest::new(title, body, base, head).draft(parsed.draft),
    )?;
    let normalized = normalize_pull(&pull);
    if parsed.json_fields.is_empty() {
        let url = normalized
            .get("url")
            .or_else(|| pull.get("html_url"))
            .or_else(|| pull.get("url"))
            .and_then(Value::as_str)
            .map(create::format_created_pull_url)
            .unwrap_or_default();
        writeln!(stdout, "{url}")?;
    } else {
        print_json(
            &filter_fields(&normalized, &parsed.json_fields),
            &OutputArgs {
                json_fields: parsed.json_fields,
                jq: None,
                template: None,
            },
            stdout,
        )?;
    }
    Ok(0)
}

fn run_repo_view(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    token_present: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_repo_view_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }
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

fn run_issue_create(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    token_present: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    stdin: &mut dyn Read,
) -> Result<i32> {
    let parsed = parse_issue_create_args(argv, stdin)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }

    if parsed.web {
        let url = format!("{}/issues/new", target_repo.web_base_url());
        open_web_url(&url);
        writeln!(stdout, "{url}")?;
        return Ok(0);
    }

    let title = parsed
        .title
        .filter(|title| !title.is_empty())
        .ok_or_else(|| ShimError::new("Forgejo issue create requires --title"))?;
    let labels = resolve_label_ids(client, &target_repo, &parsed.labels)?;
    let request = CreateIssueRequest::new(title)
        .body(parsed.body.unwrap_or_default())
        .assignees(parsed.assignees)
        .labels(labels);
    let request = if let Some(milestone) = parsed.milestone {
        request.milestone(milestone)
    } else {
        request
    };
    let issue = client.create_issue(&target_repo, &request)?;
    let normalized = normalize_issue(&issue);
    let url = normalized
        .get("url")
        .or_else(|| issue.get("html_url"))
        .or_else(|| issue.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("");
    writeln!(stdout, "{url}")?;
    Ok(0)
}

fn run_issue_list(
    argv: &[String],
    repo: &ForgejoRepoRef,
    client: &dyn ForgejoApi,
    token_present: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_issue_list_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }
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
    token_present: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<i32> {
    let parsed = parse_issue_view_args(argv)?;
    let target_repo = target_repo(parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }
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

fn parse_checkout_args(argv: &[String]) -> Result<CheckoutArgs> {
    let mut parsed = CheckoutArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--branch" | "-b") {
            parsed.local_branch = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--branch", "-b"]) {
            parsed.local_branch = Some(value.to_string());
            index += 1;
        } else if arg == "--detach" {
            parsed.detach = true;
            index += 1;
        } else if matches!(arg, "--force" | "-f") {
            parsed.force = true;
            index += 1;
        } else if arg == "--recurse-submodules" {
            parsed.recurse_submodules = true;
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo PR checkout flag: {arg}"
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

fn parse_comment_args(argv: &[String], stdin: &mut dyn Read) -> Result<CommentArgs> {
    let mut parsed = CommentArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--body" | "-b") {
            parsed.body = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--body", "-b"]) {
            parsed.body = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--body-file" | "-F") {
            parsed.body = Some(body_from_file(required_value(argv, index, arg)?, stdin)?);
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--body-file", "-F"]) {
            parsed.body = Some(body_from_file(value, stdin)?);
            index += 1;
        } else if matches!(arg, "--web" | "-w") {
            parsed.web = true;
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo PR comment flag: {arg}"
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

fn parse_issue_create_args(argv: &[String], stdin: &mut dyn Read) -> Result<IssueCreateArgs> {
    let mut parsed = IssueCreateArgs::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if matches!(arg, "--title" | "-t") {
            parsed.title = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--title", "-t"]) {
            parsed.title = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--body" | "-b") {
            parsed.body = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--body", "-b"]) {
            parsed.body = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--body-file" | "-F") {
            parsed.body = Some(body_from_file(required_value(argv, index, arg)?, stdin)?);
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--body-file", "-F"]) {
            parsed.body = Some(body_from_file(value, stdin)?);
            index += 1;
        } else if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--web" | "-w") {
            parsed.web = true;
            index += 1;
        } else if matches!(arg, "--assignee" | "-a") {
            parsed
                .assignees
                .extend(split_label_values(required_value(argv, index, arg)?));
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--assignee", "-a"]) {
            parsed.assignees.extend(split_label_values(value));
            index += 1;
        } else if matches!(arg, "--label" | "-l") {
            parsed
                .labels
                .extend(split_label_values(required_value(argv, index, arg)?));
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--label", "-l"]) {
            parsed.labels.extend(split_label_values(value));
            index += 1;
        } else if matches!(arg, "--milestone" | "-m") {
            parsed.milestone = optional_i64(required_value(argv, index, arg)?);
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--milestone", "-m"]) {
            parsed.milestone = optional_i64(value);
            index += 1;
        } else if matches!(
            arg,
            "--editor" | "-e" | "--project" | "-p" | "--recover" | "--template" | "-T"
        ) {
            return Err(ShimError::new(format!(
                "unsupported Forgejo issue create flag: {arg}"
            )));
        } else if has_value_prefix(arg, &["--project", "-p", "--recover", "--template", "-T"]) {
            let flag = arg.split_once('=').map_or(arg, |(flag, _)| flag);
            return Err(ShimError::new(format!(
                "unsupported Forgejo issue create flag: {flag}"
            )));
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo issue create flag: {arg}"
            )));
        } else {
            return Err(ShimError::new(format!(
                "unexpected positional argument for Forgejo issue create: {arg}"
            )));
        }
    }
    parsed.assignees.retain(|assignee| assignee != "@me");
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

fn missing_token_exit(
    token_present: bool,
    repo: &ForgejoRepoRef,
    stderr: &mut dyn Write,
) -> Result<Option<i32>> {
    if token_present {
        return Ok(None);
    }
    writeln!(stderr, "{}", missing_auth_message(&repo.host))?;
    Ok(Some(1))
}

fn target_repo(value: Option<&str>, fallback: &ForgejoRepoRef) -> Result<ForgejoRepoRef> {
    match value {
        Some(value) => parse_repo_spec(value, Some(&fallback.host))
            .map(|repo| ForgejoRepoRef::new(repo.host, repo.owner, repo.name))
            .ok_or_else(|| ShimError::new("could not parse --repo value")),
        None => Ok(fallback.clone()),
    }
}

fn body_from_file(path_value: &str, stdin: &mut dyn Read) -> Result<String> {
    if path_value == "-" {
        let mut body = String::new();
        stdin
            .read_to_string(&mut body)
            .map_err(|error| ShimError::new(format!("could not read stdin body: {error}")))?;
        return Ok(body);
    }
    fs::read_to_string(path_value)
        .map_err(|error| ShimError::new(format!("could not read body file {path_value}: {error}")))
}

fn pull_head_ref(pull: &Value) -> Option<String> {
    pull.get("head")
        .and_then(Value::as_object)
        .and_then(|head| head.get("ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_label_ids(
    client: &dyn ForgejoApi,
    repo: &ForgejoRepoRef,
    labels: &[String],
) -> Result<Vec<i64>> {
    if labels.is_empty() {
        return Ok(Vec::new());
    }
    let unresolved = labels
        .iter()
        .filter(|label| optional_i64(label).is_none())
        .collect::<Vec<_>>();
    let mut label_map = HashMap::new();
    if !unresolved.is_empty() {
        for label in client.list_labels(repo)? {
            if let (Some(name), Some(id)) = (
                label.get("name").and_then(Value::as_str),
                label.get("id").and_then(Value::as_i64),
            ) {
                label_map.insert(name.to_string(), id);
            }
        }
    }

    let mut result = Vec::new();
    for label in labels {
        if let Some(id) = optional_i64(label) {
            result.push(id);
        } else if let Some(id) = label_map.get(label) {
            result.push(*id);
        } else {
            return Err(ShimError::new(format!(
                "could not resolve Forgejo issue label: {label}"
            )));
        }
    }
    Ok(result)
}

fn optional_i64(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}

fn git_error_message<'a>(output: &'a GitRunResult, fallback: &'a str) -> &'a str {
    if output.message.is_empty() {
        fallback
    } else {
        output.message.as_str()
    }
}

fn dedupe_strings(values: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        if !result.contains(&value) {
            result.push(value);
        }
    }
    result
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
    use std::cell::RefCell;
    use std::ffi::OsStr;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_GIT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone)]
    struct FakeApi {
        pulls: Vec<Value>,
        issues: Vec<Value>,
        labels: Vec<Value>,
        statuses: Vec<Value>,
        files: Vec<Value>,
        repo: Value,
        user: Value,
        diff: String,
        created_pulls: RefCell<Vec<(ForgejoRepoRef, CreatePullRequest)>>,
        created_issues: RefCell<Vec<(ForgejoRepoRef, CreateIssueRequest)>>,
        comments: RefCell<Vec<(ForgejoRepoRef, u64, String)>>,
    }

    impl Default for FakeApi {
        fn default() -> Self {
            Self {
                pulls: Vec::new(),
                issues: Vec::new(),
                labels: Vec::new(),
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
                created_pulls: RefCell::new(Vec::new()),
                created_issues: RefCell::new(Vec::new()),
                comments: RefCell::new(Vec::new()),
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

        fn create_pull(
            &self,
            repo: &ForgejoRepoRef,
            request: &CreatePullRequest,
        ) -> ForgejoResult<Value> {
            self.created_pulls
                .borrow_mut()
                .push((repo.clone(), request.clone()));
            Ok(json!({
                "number": 7,
                "title": request.title,
                "body": request.body,
                "state": "open",
                "html_url": "https://git.example.com/owner/repo/pulls/7",
                "head": {"ref": request.head},
                "base": {"ref": request.base},
                "draft": request.draft,
            }))
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

        fn list_labels(&self, _repo: &ForgejoRepoRef) -> ForgejoResult<Vec<Value>> {
            Ok(self.labels.clone())
        }

        fn create_issue(
            &self,
            repo: &ForgejoRepoRef,
            request: &CreateIssueRequest,
        ) -> ForgejoResult<Value> {
            self.created_issues
                .borrow_mut()
                .push((repo.clone(), request.clone()));
            Ok(json!({
                "number": 13,
                "title": request.title,
                "body": request.body,
                "state": "open",
                "html_url": "https://git.example.com/owner/repo/issues/13",
                "user": {"login": "alice"},
            }))
        }

        fn create_issue_comment(
            &self,
            repo: &ForgejoRepoRef,
            number: u64,
            body: &str,
        ) -> ForgejoResult<Value> {
            self.comments
                .borrow_mut()
                .push((repo.clone(), number, body.to_string()));
            Ok(json!({"body": body}))
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
        run_fake_with_stdin(args, client, token, cwd, "")
    }

    fn run_fake_with_stdin(
        args: &[&str],
        client: &FakeApi,
        token: Option<&str>,
        cwd: Option<&Path>,
        stdin_text: &str,
    ) -> (i32, String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut stdin = std::io::Cursor::new(stdin_text.as_bytes());
        let code = run_with_client(
            &argv(args),
            Some("git.example.com"),
            Some(&repo()),
            token,
            client,
            cwd,
            &mut stdout,
            &mut stderr,
            &mut stdin,
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

    struct CheckoutGitFixture {
        root: PathBuf,
        work: PathBuf,
    }

    impl CheckoutGitFixture {
        fn new() -> Self {
            let id = NEXT_GIT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gh-forgejo-shim-checkout-{}-{id}",
                std::process::id()
            ));
            let seed = root.join("seed");
            let origin = root.join("origin.git");
            let work = root.join("work");
            fs::create_dir_all(&seed).expect("create seed repo directory");

            git(&seed, ["init", "-b", "main"]);
            git(&seed, ["config", "user.email", "test@example.com"]);
            git(&seed, ["config", "user.name", "Test User"]);
            fs::write(seed.join("README.md"), "main\n").expect("write main file");
            git(&seed, ["add", "README.md"]);
            git(&seed, ["commit", "-m", "initial"]);
            git(&seed, ["checkout", "-b", "feature-pr"]);
            fs::write(seed.join("README.md"), "feature\n").expect("write feature file");
            git(&seed, ["commit", "-am", "feature"]);
            git(&seed, ["checkout", "main"]);

            git(
                &root,
                [
                    OsStr::new("clone"),
                    OsStr::new("--bare"),
                    seed.as_os_str(),
                    origin.as_os_str(),
                ],
            );
            git(
                &origin,
                [
                    OsStr::new("update-ref"),
                    OsStr::new("refs/pull/12/head"),
                    OsStr::new("refs/heads/feature-pr"),
                ],
            );
            git(
                &root,
                [OsStr::new("clone"), origin.as_os_str(), work.as_os_str()],
            );
            git(&work, ["config", "user.email", "test@example.com"]);
            git(&work, ["config", "user.name", "Test User"]);

            Self { root, work }
        }

        fn work(&self) -> &Path {
            &self.work
        }

        fn readme(&self) -> String {
            fs::read_to_string(self.work.join("README.md")).expect("read worktree file")
        }

        fn branch(&self) -> Option<String> {
            current_branch(Some(&self.work))
        }
    }

    impl Drop for CheckoutGitFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct FillGitFixture {
        root: PathBuf,
    }

    impl FillGitFixture {
        fn new() -> Self {
            let id = NEXT_GIT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gh-forgejo-shim-create-fill-{}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create fill repo directory");
            git(&root, ["init", "-b", "feature"]);
            git(&root, ["config", "user.email", "test@example.com"]);
            git(&root, ["config", "user.name", "Test User"]);
            fs::write(root.join("README.md"), "fill\n").expect("write fill file");
            git(&root, ["add", "README.md"]);
            git(&root, ["commit", "-m", "Ship fill", "-m", "Fill body"]);
            Self { root }
        }

        fn path(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for FillGitFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn git<I, S>(cwd: &Path, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = args
            .into_iter()
            .map(|arg| arg.as_ref().to_owned())
            .collect::<Vec<_>>();
        let output = Command::new("git")
            .args(&args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
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
    fn pr_create_posts_payload_and_outputs_json() {
        let client = FakeApi::default();

        let (code, stdout, stderr) = run_fake_with_stdin(
            &[
                "pr",
                "create",
                "--title",
                "Ship it",
                "--body-file",
                "-",
                "--base",
                "main",
                "--head",
                "feature",
                "--draft",
                "--json",
                "number,title,url,isDraft,baseRefName,headRefName",
            ],
            &client,
            Some("token"),
            None,
            "body from stdin",
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).ok(),
            Some(json!({
                "baseRefName": "main",
                "headRefName": "feature",
                "isDraft": true,
                "number": 7,
                "title": "Ship it",
                "url": "https://git.example.com/owner/repo/pulls/7",
            }))
        );
        let created = client.created_pulls.borrow();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].0, repo());
        assert_eq!(created[0].1.title, "Ship it");
        assert_eq!(created[0].1.body, "body from stdin");
        assert_eq!(created[0].1.base, "main");
        assert_eq!(created[0].1.head, "feature");
        assert!(created[0].1.draft);
    }

    #[test]
    fn pr_create_default_output_adds_codex_url_marker() {
        let client = FakeApi::default();

        let (code, stdout, stderr) = run_fake(
            &[
                "pr", "new", "--title", "Ship it", "--base", "main", "--head", "feature",
            ],
            &client,
            Some("token"),
            None,
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            stdout.trim(),
            "https://git.example.com/owner/repo/pulls/7#codex-pr=/pull/7"
        );
    }

    #[test]
    fn pr_create_fill_verbose_uses_temporary_git_repo() {
        let fixture = FillGitFixture::new();
        let client = FakeApi::default();

        let (code, _stdout, stderr) = run_fake(
            &["pr", "create", "--fill-verbose"],
            &client,
            Some("token"),
            Some(fixture.path()),
        );

        assert_eq!(code, 0, "{stderr}");
        let created = client.created_pulls.borrow();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].1.title, "Ship fill");
        assert_eq!(created[0].1.body, "Fill body");
        assert_eq!(created[0].1.base, "main");
        assert_eq!(created[0].1.head, "feature");
    }

    #[test]
    fn pr_create_web_opens_and_prints_compare_url() {
        let client = FakeApi::default();
        crate::external::take_opened_web_urls();

        let (code, stdout, stderr) = run_fake(
            &[
                "pr", "create", "--title", "Ship it", "--base", "main", "--head", "feature",
                "--web",
            ],
            &client,
            Some("token"),
            None,
        );

        let url = "https://git.example.com/owner/repo/compare/main...feature";
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, format!("{url}\n"));
        assert!(
            crate::external::take_opened_web_urls().contains(&url.to_string()),
            "web url should be opened"
        );
        assert!(client.created_pulls.borrow().is_empty());
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
            Some("token"),
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
            Some("token"),
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
            Some("token"),
            None,
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "README.md\n");
    }

    #[test]
    fn pr_checkout_fetches_head_ref_and_creates_local_branch() {
        let git_repo = CheckoutGitFixture::new();
        let client = FakeApi {
            pulls: vec![json!({
                "number": 12,
                "head": {"ref": "feature-pr", "sha": "abc123"},
            })],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &["pr", "checkout", "12"],
            &client,
            Some("token"),
            Some(git_repo.work()),
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "checked out feature-pr\n");
        assert_eq!(stderr, "");
        assert_eq!(git_repo.branch().as_deref(), Some("feature-pr"));
        assert_eq!(git_repo.readme(), "feature\n");
    }

    #[test]
    fn pr_co_falls_back_to_pull_ref_when_head_ref_is_absent() {
        let git_repo = CheckoutGitFixture::new();
        let client = FakeApi {
            pulls: vec![json!({
                "number": 12,
                "head": {"sha": "abc123"},
            })],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &["pr", "co", "12"],
            &client,
            Some("token"),
            Some(git_repo.work()),
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "checked out pull-12\n");
        assert_eq!(stderr, "");
        assert_eq!(git_repo.branch().as_deref(), Some("pull-12"));
        assert_eq!(git_repo.readme(), "feature\n");
    }

    #[test]
    fn pr_checkout_preserves_git_dirty_worktree_error() {
        let git_repo = CheckoutGitFixture::new();
        fs::write(git_repo.work().join("README.md"), "dirty\n").expect("dirty worktree file");
        let client = FakeApi {
            pulls: vec![json!({
                "number": 12,
                "head": {"ref": "feature-pr", "sha": "abc123"},
            })],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &["pr", "checkout", "12"],
            &client,
            Some("token"),
            Some(git_repo.work()),
        );

        assert_ne!(code, 0);
        assert_eq!(stdout, "");
        assert!(
            stderr.contains("Your local changes") || stderr.contains("would be overwritten"),
            "{stderr}"
        );
        assert_eq!(git_repo.branch().as_deref(), Some("main"));
        assert_eq!(git_repo.readme(), "dirty\n");
    }

    #[test]
    fn no_current_pr_view_and_status_return_empty_success_shapes() {
        let client = FakeApi::default();

        let (code, stdout, stderr) = run_fake(
            &["pr", "view", "feature", "--json", "number,title"],
            &client,
            Some("token"),
            None,
        );
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "{}");

        let (code, stdout, stderr) = run_fake(
            &["pr", "status", "feature", "--json", "number,title"],
            &client,
            Some("token"),
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
            Some("token"),
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
            Some("token"),
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

    #[test]
    fn read_only_repo_commands_require_configured_auth() {
        let client = FakeApi::default();

        let (code, stdout, stderr) =
            run_fake(&["repo", "view", "--json", "url"], &client, None, None);

        assert_eq!(code, 1);
        assert_eq!(stdout, "");
        assert!(stderr.contains("Forgejo auth is not configured for git.example.com"));
        assert!(stderr.contains("gh-forgejo-shim auth login git.example.com"));
    }

    #[test]
    fn mutating_commands_require_configured_auth() {
        let client = FakeApi::default();

        let (code, stdout, stderr) =
            run_fake(&["issue", "create", "--title", "Bug"], &client, None, None);

        assert_eq!(code, 1);
        assert_eq!(stdout, "");
        assert!(stderr.contains("Forgejo auth is not configured for git.example.com"));
        assert!(client.created_issues.borrow().is_empty());
    }

    #[test]
    fn pr_comment_posts_stdin_body_to_issue_comments() {
        let client = FakeApi {
            pulls: vec![json!({
                "number": 8,
                "title": "Make Codex happy",
                "state": "open",
                "html_url": "https://git.example.com/owner/repo/pulls/8",
                "head": {"ref": "feature"},
            })],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake_with_stdin(
            &["pr", "comment", "8", "--body-file", "-"],
            &client,
            Some("token"),
            None,
            "from stdin\n",
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "");
        let comments = client.comments.borrow();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].1, 8);
        assert_eq!(comments[0].2, "from stdin\n");
    }

    #[test]
    fn pr_comment_requires_body_unless_opening_web_url() {
        let client = FakeApi {
            pulls: vec![json!({
                "number": 8,
                "html_url": "https://git.example.com/owner/repo/pulls/8",
                "head": {"ref": "feature"},
            })],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) =
            run_fake(&["pr", "comment", "8"], &client, Some("token"), None);

        assert_eq!(code, 1);
        assert_eq!(stdout, "");
        assert!(stderr.contains("Forgejo PR comment requires --body or --body-file"));

        let (code, stdout, stderr) = run_fake(
            &["pr", "comment", "8", "--web"],
            &client,
            Some("token"),
            None,
        );

        let url = "https://git.example.com/owner/repo/pulls/8#new-comment";
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, format!("{url}\n"));
        assert!(
            crate::external::take_opened_web_urls().contains(&url.to_string()),
            "web url should be opened"
        );
        assert!(client.comments.borrow().is_empty());
    }

    #[test]
    fn issue_create_posts_payload_with_body_file_labels_and_repo_override() {
        let client = FakeApi {
            labels: vec![json!({"id": 5, "name": "bug"})],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake_with_stdin(
            &[
                "issue",
                "new",
                "--title",
                "Bug",
                "--body-file",
                "-",
                "--label",
                "bug,7",
                "--assignee",
                "@me,bob",
                "--milestone",
                "3",
                "-R",
                "other/project",
            ],
            &client,
            Some("token"),
            None,
            "details\n",
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "https://git.example.com/owner/repo/issues/13\n");
        let created = client.created_issues.borrow();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].0.owner, "other");
        assert_eq!(created[0].0.repo, "project");
        assert_eq!(created[0].1.title, "Bug");
        assert_eq!(created[0].1.body, "details\n");
        assert_eq!(created[0].1.assignees, vec!["bob".to_string()]);
        assert_eq!(created[0].1.labels, vec![5, 7]);
        assert_eq!(created[0].1.milestone, Some(3));
    }

    #[test]
    fn issue_create_requires_title_and_supports_web_url() {
        let client = FakeApi::default();

        let (code, stdout, stderr) = run_fake(
            &["issue", "create", "--body", "missing"],
            &client,
            Some("token"),
            None,
        );

        assert_eq!(code, 1);
        assert_eq!(stdout, "");
        assert!(stderr.contains("Forgejo issue create requires --title"));

        let (code, stdout, stderr) =
            run_fake(&["issue", "create", "--web"], &client, Some("token"), None);

        let url = "https://git.example.com/owner/repo/issues/new";
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, format!("{url}\n"));
        assert!(
            crate::external::take_opened_web_urls().contains(&url.to_string()),
            "web url should be opened"
        );
        assert!(client.created_issues.borrow().is_empty());
    }
}
