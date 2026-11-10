//! Read-only Forgejo command handlers for managed `gh` invocations.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fga_core::forgejo::{named_like_a_ref, CommitStatus, Protection};
use serde_json::{json, Value};

use crate::config::{self, EnvMap};
use crate::external::{git_output, git_run, open_web_url, GitRunResult};
use crate::forgejo::{
    CreateIssueRequest, CreatePullRequest, ForgejoClient, ForgejoResult, ListIssuesOptions,
    RepoRef as ForgejoRepoRef,
};
use crate::normalize::{
    filter_fields, filter_issue_fields, filter_repo_fields, normalize_issue, normalize_pull,
    normalize_repo, render_json_or_jq, render_json_or_jq_list, status_for_current_branch,
    with_status_check_rollup_for_repo,
};
use crate::repo::{parse_repo_spec, Checkout, RepoRef as DetectedRepoRef};
use crate::routing::RouteDecision;
use crate::{auth, checks, create};
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
    fn list_check_statuses(
        &self,
        repo: &ForgejoRepoRef,
        sha: &str,
    ) -> ForgejoResult<Vec<CommitStatus>>;
    fn get_branch_protection(
        &self,
        repo: &ForgejoRepoRef,
        branch: &str,
    ) -> ForgejoResult<Option<Protection>>;
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

    fn list_check_statuses(
        &self,
        repo: &ForgejoRepoRef,
        sha: &str,
    ) -> ForgejoResult<Vec<CommitStatus>> {
        ForgejoClient::list_check_statuses(self, repo, sha)
    }

    fn get_branch_protection(
        &self,
        repo: &ForgejoRepoRef,
        branch: &str,
    ) -> ForgejoResult<Option<Protection>> {
        ForgejoClient::get_branch_protection(self, repo, branch)
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
    /// A pull request or issue URL given as the selector, which names the repository.
    url: Option<String>,
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
    output: OutputArgs,
    repo: Option<String>,
    /// A pull request or issue URL given as the selector, which names the repository.
    url: Option<String>,
    number: Option<u64>,
    branch: Option<String>,
    required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DiffArgs {
    repo: Option<String>,
    /// A pull request or issue URL given as the selector, which names the repository.
    url: Option<String>,
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
    /// A pull request or issue URL given as the selector, which names the repository.
    url: Option<String>,
    number: Option<u64>,
    branch: Option<String>,
    body: Option<String>,
    web: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CheckoutArgs {
    repo: Option<String>,
    /// Whether the selector is a pull request URL, which names the repository.
    url: bool,
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
    /// A pull request or issue URL given as the selector, which names the repository.
    url: Option<String>,
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
    let config = config::load_config_with_env(Some(&config::config_path(home)), env)?;
    let token = auth::resolve_token(host, &config, env, home, Some(current_platform()))
        .map(|discovery| discovery.token);

    if command == "auth" {
        return run_auth(argv, host, token.as_deref(), stdout, stderr);
    }

    let make_client =
        |token| ForgejoClient::for_host(host, token, &config, env).map_err(ShimError::from);

    if command == "api" {
        if token.is_none() {
            writeln!(stderr, "{}", missing_auth_message(host))?;
            return Ok(1);
        }
        let client = make_client(token)?;
        return run_api(argv, host, &client, stdout);
    }

    let token_present = token.is_some();
    let client = make_client(token)?;
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
    // A URL's repository, which routing has already chosen, wins over `-R`, as in gh's finder.
    let target_repo = target_repo(parsed.repo.as_deref().filter(|_| !parsed.url), repo)?;
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
    let head = PullHead::of(&pull);
    if let Some(name) = head.name.as_deref().filter(|name| name.starts_with('-')) {
        writeln!(stderr, "invalid branch name: {name:?}")?;
        return Ok(1);
    }
    // `None` checks out detached: for `--detach`, and for a head no local branch may be named
    // after.
    let branch = if parsed.detach {
        None
    } else if let Some(branch) = parsed.local_branch.clone() {
        Some(branch)
    } else {
        local_branch_for(&head, number, || {
            default_branch(&pull, client, &target_repo)
        })?
    };

    // A filesystem that ignores case, as macOS's does, stores `Develop` in the user's `develop`, so
    // git would read or replace that branch. git records such a filesystem at clone or init as
    // `core.ignorecase`. A branch of the exact name is the one meant. gh doesn't check this.
    let local_branches =
        git_output(&["for-each-ref", "--format=%(refname)", "refs/heads"], cwd).unwrap_or_default();
    let local_branches = local_branches
        .lines()
        .filter_map(|name| name.strip_prefix("refs/heads/"))
        .collect::<Vec<_>>();
    let ignores_case =
        || git_output(&["config", "--bool", "core.ignorecase"], cwd).as_deref() == Some("true");
    if let Some(branch) = branch
        .as_deref()
        .filter(|branch| !local_branches.contains(branch))
    {
        let lower = branch.to_lowercase();
        if let Some(other) = local_branches
            .iter()
            .find(|name| name.to_lowercase() == lower)
            .filter(|_| ignores_case())
        {
            writeln!(
                stderr,
                "the local branch {other:?} differs from {branch:?} only in case; name another \
                 with --branch"
            )?;
            return Ok(1);
        }
    }

    // As gh: a head branch by name only while it is the PR's head in the base repository, and
    // otherwise the PR's own ref there. A fork's branch name, fetched from the base, would be the
    // base's branch of that name.
    let mut fetch_refs = Vec::new();
    if let Some(name) = head.name.as_deref().filter(|_| head.is_base_branch) {
        fetch_refs.push(format!("refs/heads/{name}"));
    }
    if let Some(number) = number {
        fetch_refs.push(format!("refs/pull/{number}/head"));
    }
    // As gh: from the base repository's remote, or its clone URL when no remote holds it. In a
    // fork's clone, `origin` is the fork.
    let remote = Checkout::new(cwd)
        .remote_for(&target_repo.host, &target_repo.owner, &target_repo.repo)
        .unwrap_or_else(|| clone_url(&pull, &target_repo));

    // An empty `--refmap` keeps git from also applying the remote's configured mapping to what it
    // fetches. One into a local branch (`+refs/heads/x:refs/heads/x`, or a mirror's
    // `+refs/heads/*:refs/heads/*`) would reset that branch before the fast-forward check, unpushed
    // commits and all. gh's fetch has no `--refmap`, so this goes further than gh.
    let mut fetch_error = String::new();
    let mut fetched = false;
    for fetch_ref in fetch_refs {
        let output = git_run(
            &[
                "fetch",
                "--refmap=",
                "--",
                remote.as_str(),
                fetch_ref.as_str(),
            ],
            cwd,
        );
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

    let output = match branch.as_deref() {
        None => git_run(&["checkout", "--detach", "FETCH_HEAD"], cwd),
        Some(branch) if parsed.force => git_run(&["checkout", "-B", branch, "FETCH_HEAD"], cwd),
        Some(branch) => {
            if local_branches.contains(&branch) {
                // As gh: an existing branch moves only by fast-forward, so its own commits stay.
                // One that can't is refused before the switch, so the user stays where they were,
                // as gh's fetch into the branch is refused.
                let local = format!("refs/heads/{branch}");
                let ancestor = git_run(&["merge-base", "--is-ancestor", &local, "FETCH_HEAD"], cwd);
                if ancestor.code == 1 {
                    writeln!(
                        stderr,
                        "the local branch {branch:?} has commits the pull request doesn't, so it \
                         can't fast-forward; reset it with --force, or name another with --branch"
                    )?;
                    return Ok(1);
                }
                let output = git_run(&["checkout", branch], cwd);
                if output.code == 0 {
                    git_run(&["merge", "--ff-only", "FETCH_HEAD"], cwd)
                } else {
                    output
                }
            } else {
                git_run(&["checkout", "-b", branch, "FETCH_HEAD"], cwd)
            }
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

    writeln!(
        stdout,
        "checked out {}",
        branch.as_deref().unwrap_or("FETCH_HEAD")
    )?;
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
    let target_repo = selected_repo(parsed.url.as_deref(), parsed.repo.as_deref(), repo)?;
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
    .ok_or_else(|| ShimError::new("could not find pull request"))?;
    let Some(sha) = pull_head_sha(&pull) else {
        writeln!(stderr, "no commit found on the pull request")?;
        return Ok(1);
    };
    let statuses = client.list_check_statuses(&target_repo, &sha)?;
    let head = pull
        .pointer("/head/ref")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if statuses.is_empty() {
        writeln!(stderr, "no checks reported on the '{head}' branch")?;
        return Ok(1);
    }
    let protection = if parsed.required {
        let base = pull
            .pointer("/base/ref")
            .and_then(Value::as_str)
            .ok_or_else(|| ShimError::new("could not determine pull request base branch"))?;
        client.get_branch_protection(&target_repo, base)?
    } else {
        None
    };
    let rows = checks::aggregate(
        &statuses,
        &target_repo,
        parsed.required,
        protection.as_ref(),
    )?;
    if rows.is_empty() && parsed.required {
        writeln!(stderr, "no required checks reported on the '{head}' branch")?;
        return Ok(1);
    }
    if parsed.output.json_fields.is_empty() {
        checks::print(rows, stdout)
    } else {
        let values = rows
            .iter()
            .map(|row| row.export(&parsed.output.json_fields))
            .collect::<Result<Vec<_>>>()?;
        print_json_list(&values, &parsed.output, stdout)?;
        Ok(0)
    }
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
    let target_repo = selected_repo(parsed.url.as_deref(), parsed.repo.as_deref(), repo)?;
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
        let url = pull_page_url(&pull, &target_repo, number);
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
    let target_repo = selected_repo(parsed.url.as_deref(), parsed.repo.as_deref(), repo)?;
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
        let url = pull_page_url(&pull, &target_repo, number);
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
    let target_repo = selected_repo(parsed.url.as_deref(), parsed.repo.as_deref(), repo)?;
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
                writeln!(stdout, "{}", fga_core::map::forgejo_pull_web_url(url))?;
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
    let target_repo = selected_repo(parsed.url.as_deref(), parsed.repo.as_deref(), repo)?;
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
            .map(fga_core::map::pull_web_url)
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
    let target_repo = selected_repo(parsed.url.as_deref(), parsed.repo.as_deref(), repo)?;
    if let Some(code) = missing_token_exit(token_present, &target_repo, stderr)? {
        return Ok(code);
    }
    let number = parsed
        .number
        .ok_or_else(|| ShimError::new("Forgejo issue view requires an issue number or URL"))?;
    let normalized = normalize_issue(&client.get_issue(&target_repo, number)?);

    if parsed.web {
        // An issue number can name a pull request; --web gives Forgejo's own page for it.
        let url = normalized.get("url").and_then(Value::as_str).map_or_else(
            || format!("{}/issues/{number}", target_repo.web_base_url()),
            fga_core::map::forgejo_pull_web_url,
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
            parsed.url = number_from_url(arg, "pull").is_some();
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
            parsed.output.json_fields = split_fields(required_value(argv, index, arg)?);
            index += 2;
        } else if let Some(value) = flag_value(arg, "--json") {
            parsed.output.json_fields = split_fields(value);
            index += 1;
        } else if matches!(arg, "-R" | "--repo") {
            parsed.repo = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value(arg, "--repo") {
            parsed.repo = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--jq" | "-q") {
            parsed.output.jq = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--jq", "-q"]) {
            parsed.output.jq = Some(value.to_string());
            index += 1;
        } else if matches!(arg, "--template" | "-t") {
            parsed.output.template = Some(required_value(argv, index, arg)?.to_string());
            index += 2;
        } else if let Some(value) = flag_value_any(arg, &["--template", "-t"]) {
            parsed.output.template = Some(value.to_string());
            index += 1;
        } else if arg == "--required" {
            parsed.required = true;
            index += 1;
        } else if matches!(arg, "--web" | "-w") {
            index += 1;
        } else if arg.starts_with('-') {
            return Err(ShimError::new(format!(
                "unsupported Forgejo PR checks flag: {arg}"
            )));
        } else {
            (parsed.number, parsed.branch) = parse_number_or_branch(arg, "pull");
            parsed.url = selector_url(arg, "pull");
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
            parsed.url = selector_url(arg, "pull");
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
            parsed.url = selector_url(arg, "pull");
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
            parsed.url = selector_url(arg, "pull");
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
            parsed.url = selector_url(arg, "issue");
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
        "gh-forgejo-shim: Forgejo auth is not configured for {host}; run gh-forgejo-shim auth login {host} or gh-forgejo-shim auth import --from fj --hostname {host}"
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

/// The repository a command selects: a pull request or issue URL's own, which gh's finder takes
/// whatever `-R` says, then `-R`, then the routed repository.
fn selected_repo(
    url: Option<&str>,
    value: Option<&str>,
    fallback: &ForgejoRepoRef,
) -> Result<ForgejoRepoRef> {
    target_repo(url.or(value), fallback)
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

/// A PR's head as `gh pr checkout` reads it.
struct PullHead {
    /// The head branch's name, as `fga_core::forgejo::Branch::name` reads it: `label` once Forgejo
    /// has put `refs/pull/N/head` in `ref` for a deleted branch or fork, and `ref` for an AGit PR,
    /// whose `label` is empty.
    name: Option<String>,
    /// The head repository's owner, which gh puts before a head branch named as the base's
    /// default branch.
    owner: Option<String>,
    /// Whether the head is a branch of the base repository that is still the open PR's head, the
    /// only head gh fetches by name (`headRepository` in `docs/gateway/MAPPING.md` §10). A fork,
    /// a deleted fork, an AGit PR, a closed PR and a branch named like a ref are fetched from the
    /// PR's own ref.
    is_base_branch: bool,
}

impl PullHead {
    fn of(pull: &Value) -> Self {
        let text = |pointer: &str| {
            pull.pointer(pointer)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        };
        let label = text("/head/label");
        let ref_name = text("/head/ref");
        let name = match (label, ref_name) {
            (Some(label), Some(ref_name)) if ref_name.starts_with("refs/pull/") => Some(label),
            (label, ref_name) => ref_name.or(label),
        };
        let repo =
            |side: &str| text(&format!("/{side}/repo/full_name")).map(str::to_ascii_lowercase);
        let open = text("/state") == Some("open")
            && !pull.get("merged").and_then(Value::as_bool).unwrap_or(false);
        let is_base_branch = open
            && repo("head").is_some()
            && repo("head") == repo("base")
            && label.is_some()
            && label == ref_name
            && !ref_name.is_some_and(named_like_a_ref);
        Self {
            name: name.map(ToOwned::to_owned),
            owner: text("/head/repo/owner/login").map(ToOwned::to_owned),
            is_base_branch,
        }
    }
}

/// The local branch gh 2.103.3 checks a PR out to without `--branch` (`checkout.go`'s
/// `cmdsForExistingRemote` and `cmdsForMissingRemote`), or `None` to check out detached. A head it
/// fetches from the PR's own ref and named as the base's default branch gets the head owner's name
/// in front, so a fork's `main` lands on `fork/main`, never the user's `main`. This compares
/// without case, unlike gh, so that a fork's `Main` can't write the user's `main` on a
/// filesystem that ignores case. A head named like a ref (`refs/heads/main`, `heads/main`), or
/// `@`, names no branch: git would read it as another ref, as in #302, and an AGit PR's
/// `refs/pull/N/head` lands detached as it does through gh.
fn local_branch_for(
    head: &PullHead,
    number: Option<u64>,
    default_branch: impl FnOnce() -> Result<Option<String>>,
) -> Result<Option<String>> {
    let pull_branch = || {
        number
            .map(|number| format!("pull-{number}"))
            .ok_or_else(|| ShimError::new("could not determine local branch name"))
    };
    let Some(name) = head.name.as_deref() else {
        return pull_branch().map(Some);
    };
    if named_like_a_ref(name) || name == "@" {
        return Ok(None);
    }
    if head.is_base_branch {
        return Ok(Some(name.to_owned()));
    }
    let default_branch = default_branch()?
        .ok_or_else(|| ShimError::new("could not read the base repository's default branch"))?;
    if !name.eq_ignore_ascii_case(&default_branch) {
        return Ok(Some(name.to_owned()));
    }
    match head.owner.as_deref() {
        Some(owner) => Ok(Some(format!("{owner}/{name}"))),
        None => pull_branch().map(Some),
    }
}

/// The base repository's clone URL: from the PR, as Forgejo sends it, or else its HTTPS URL.
fn clone_url(pull: &Value, repo: &ForgejoRepoRef) -> String {
    pull.pointer("/base/repo/clone_url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .map_or_else(|| format!("{}.git", repo.web_base_url()), ToOwned::to_owned)
}

/// The base repository's default branch: from the PR, as Forgejo sends it, or else the repository.
fn default_branch(
    pull: &Value,
    client: &dyn ForgejoApi,
    repo: &ForgejoRepoRef,
) -> Result<Option<String>> {
    let read = |value: &Value| {
        value
            .get("default_branch")
            .and_then(Value::as_str)
            .filter(|branch| !branch.is_empty())
            .map(ToOwned::to_owned)
    };
    match pull.pointer("/base/repo").and_then(read) {
        Some(branch) => Ok(Some(branch)),
        None => Ok(read(&client.get_repo(repo)?)),
    }
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
    Ok(with_status_check_rollup_for_repo(pull, &statuses, repo))
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

/// `value` when it's a URL naming a pull request or issue of `kind`, which then selects its
/// repository too.
fn selector_url(value: &str, kind: &str) -> Option<String> {
    number_from_url(value, kind).map(|_| value.to_string())
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

/// The page a `--web` command gives the user: Forgejo's own `/pulls/{n}`, which opens without a
/// redirect in a client-only install. Printed PR URLs elsewhere use GitHub's `/pull/{n}`.
fn pull_page_url(pull: &Value, target_repo: &crate::forgejo::RepoRef, number: u64) -> String {
    normalize_pull(pull)
        .get("url")
        .and_then(Value::as_str)
        .map_or_else(
            || format!("{}/pulls/{number}", target_repo.web_base_url()),
            fga_core::map::forgejo_pull_web_url,
        )
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
        check_statuses: Vec<CommitStatus>,
        protection: Option<Protection>,
        protection_branches: RefCell<Vec<String>>,
        files: Vec<Value>,
        repo: Value,
        user: Value,
        diff: String,
        created_pulls: RefCell<Vec<(ForgejoRepoRef, CreatePullRequest)>>,
        created_issues: RefCell<Vec<(ForgejoRepoRef, CreateIssueRequest)>>,
        comments: RefCell<Vec<(ForgejoRepoRef, u64, String)>>,
        /// The `owner/repo` each pull request, issue, diff and check read went to.
        read_repos: RefCell<Vec<String>>,
        /// The `owner/repo` of each pull request read by number.
        pull_repos: RefCell<Vec<String>>,
    }

    impl FakeApi {
        fn read(&self, repo: &ForgejoRepoRef) {
            self.read_repos
                .borrow_mut()
                .push(format!("{}/{}", repo.owner, repo.repo));
        }
    }

    impl Default for FakeApi {
        fn default() -> Self {
            Self {
                pulls: Vec::new(),
                issues: Vec::new(),
                labels: Vec::new(),
                statuses: Vec::new(),
                check_statuses: Vec::new(),
                protection: None,
                protection_branches: RefCell::new(Vec::new()),
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
                read_repos: RefCell::new(Vec::new()),
                pull_repos: RefCell::new(Vec::new()),
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

        fn get_pull(&self, repo: &ForgejoRepoRef, number: u64) -> ForgejoResult<Value> {
            self.read(repo);
            self.pull_repos
                .borrow_mut()
                .push(format!("{}/{}", repo.owner, repo.repo));
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

        fn list_check_statuses(
            &self,
            repo: &ForgejoRepoRef,
            sha: &str,
        ) -> ForgejoResult<Vec<CommitStatus>> {
            self.read(repo);
            assert_eq!(sha, "abc123");
            Ok(self.check_statuses.clone())
        }

        fn get_branch_protection(
            &self,
            _repo: &ForgejoRepoRef,
            branch: &str,
        ) -> ForgejoResult<Option<Protection>> {
            self.protection_branches
                .borrow_mut()
                .push(branch.to_string());
            Ok(self.protection.clone())
        }

        fn get_pull_diff(
            &self,
            repo: &ForgejoRepoRef,
            _number: u64,
            _patch: bool,
        ) -> ForgejoResult<String> {
            self.read(repo);
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

        fn get_issue(&self, repo: &ForgejoRepoRef, number: u64) -> ForgejoResult<Value> {
            self.read(repo);
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

    /// Throwaway repositories for `pr checkout`. The forge's repositories are bare ones under
    /// `forge/`, which the work clone reaches as `ssh://git@git.example.com/<owner>/<repo>.git`
    /// through a stand-in for ssh, so its remotes name the repositories they hold. The stand-in
    /// refuses any other host, such as the one in [`pull_from`]'s clone URLs.
    struct CheckoutGitFixture {
        root: PathBuf,
        work: PathBuf,
    }

    impl CheckoutGitFixture {
        /// A clone of `owner/repo`, with `origin` for it.
        fn new() -> Self {
            let fixture = Self::forge();
            fixture.clone_from("owner/repo");
            fixture
        }

        /// A clone of the user's fork `alice/repo` as `origin`, with `upstream` for `owner/repo`.
        fn fork_clone() -> Self {
            let fixture = Self::forge();
            fixture.forge_repo("alice/repo");
            fixture.clone_from("alice/repo");
            let url = fixture.url("owner/repo");
            git(&fixture.work, ["remote", "add", "upstream", url.as_str()]);
            git(&fixture.work, ["fetch", "-q", "upstream"]);
            fixture
        }

        /// The forge with `owner/repo`: `main`, and `feature-pr` one commit ahead as PR 12's
        /// head.
        fn forge() -> Self {
            let id = NEXT_GIT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gh-forgejo-shim-checkout-{}-{id}",
                std::process::id()
            ));
            let seed = root.join("seed");
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

            // git runs it as `ssh git@git.example.com "git-upload-pack '/owner/repo.git'"`.
            let forge = root.join("forge");
            fs::create_dir_all(&forge).expect("create forge directory");
            fs::write(
                root.join("ssh"),
                format!(
                    "[ \"$1\" = git@git.example.com ] || {{ echo \"no such host: $1\" >&2; exit 255; }}\n\
                     for command; do :; done\n\
                     program=${{command%% *}}\n\
                     path=${{command#*\\'}}\n\
                     path=${{path%\\'}}\n\
                     exec git \"${{program#git-}}\" '{}'/\"${{path#/}}\"\n",
                    forge.display()
                ),
            )
            .expect("write ssh stand-in");

            let fixture = Self { root, work };
            let base = fixture.forge_repo("owner/repo");
            git(
                &base,
                ["update-ref", "refs/pull/12/head", "refs/heads/feature-pr"],
            );
            fixture
        }

        /// A bare `full_name` on the forge, cloned from the seed.
        fn forge_repo(&self, full_name: &str) -> PathBuf {
            let path = self.bare(full_name);
            git(
                &self.root,
                [
                    OsStr::new("clone"),
                    OsStr::new("-q"),
                    OsStr::new("--bare"),
                    self.root.join("seed").as_os_str(),
                    path.as_os_str(),
                ],
            );
            path
        }

        fn bare(&self, full_name: &str) -> PathBuf {
            self.root.join("forge").join(format!("{full_name}.git"))
        }

        fn url(&self, full_name: &str) -> String {
            format!("ssh://git@git.example.com/{full_name}.git")
        }

        fn clone_from(&self, full_name: &str) {
            let ssh = format!("core.sshCommand=sh '{}'", self.root.join("ssh").display());
            git(
                &self.root,
                [
                    OsStr::new("clone"),
                    OsStr::new("-q"),
                    OsStr::new("--config"),
                    OsStr::new(&ssh),
                    OsStr::new("--config"),
                    OsStr::new("ssh.variant=simple"),
                    OsStr::new(&self.url(full_name)),
                    self.work.as_os_str(),
                ],
            );
            git(&self.work, ["config", "user.email", "test@example.com"]);
            git(&self.work, ["config", "user.name", "Test User"]);
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

        fn rev(&self, name: &str) -> String {
            git_output(
                &["rev-parse", "--verify", "--quiet", name],
                Some(&self.work),
            )
            .unwrap_or_default()
        }

        /// Puts a commit with `content` on top of `main` at `refs/pull/{number}/head` in
        /// `owner/repo`, as a fork's PR has it, and returns the commit.
        fn fork_pull_head(&self, number: u64, content: &str) -> String {
            self.push_commit(
                "owner/repo",
                "main",
                &format!("refs/pull/{number}/head"),
                content,
            )
        }

        /// Commits `content` on top of `parent` in the seed and pushes it to `dest` in
        /// `full_name`, returning the commit.
        fn push_commit(&self, full_name: &str, parent: &str, dest: &str, content: &str) -> String {
            let seed = self.root.join("seed");
            git(&seed, ["checkout", "-q", "--detach", parent]);
            fs::write(seed.join("README.md"), content).expect("write seed file");
            git(&seed, ["commit", "-qam", content.trim()]);
            let refspec = format!("HEAD:{dest}");
            git(
                &seed,
                [
                    OsStr::new("push"),
                    OsStr::new("-q"),
                    self.bare(full_name).as_os_str(),
                    OsStr::new(&refspec),
                ],
            );
            let head = git_output(&["rev-parse", "HEAD"], Some(&seed)).unwrap_or_default();
            git(&seed, ["checkout", "-q", "main"]);
            head
        }

        /// Makes branch names that differ only in case two refs, as they are on a filesystem that
        /// keeps case: on one that ignores case, by moving the refs to reftable. `false` when this
        /// git can't.
        fn case_sensitive_refs(&self) -> bool {
            let probe = self.work.join("CASE-PROBE");
            fs::write(&probe, "").expect("write case probe");
            let ignores_case = self.work.join("case-probe").exists();
            fs::remove_file(&probe).expect("remove case probe");
            !ignores_case
                || git_run(
                    &["refs", "migrate", "--ref-format=reftable"],
                    Some(&self.work),
                )
                .code
                    == 0
        }

        /// Commits on the local `main` without pushing, and returns the commit.
        fn unpushed_commit_on_main(&self) -> String {
            fs::write(self.work.join("NOTES.md"), "unpushed\n").expect("write unpushed file");
            git(&self.work, ["add", "NOTES.md"]);
            git(&self.work, ["commit", "-qm", "unpushed"]);
            self.rev("main")
        }
    }

    /// A Forgejo PR into `owner/repo`'s `main` from `head_owner/repo`'s `branch`. Its clone URLs
    /// name a host the stand-in for ssh refuses, so the fetch succeeds only from a remote.
    fn pull_from(number: u64, head_owner: &str, branch: &str) -> Value {
        let repo = |id: u64, owner: &str| {
            json!({
                "id": id,
                "full_name": format!("{owner}/repo"),
                "default_branch": "main",
                "clone_url": format!("ssh://git@clone-url.example.com/{owner}/repo.git"),
                "owner": {"login": owner},
            })
        };
        json!({
            "number": number,
            "state": "open",
            "merged": false,
            "head": {
                "label": branch,
                "ref": branch,
                "sha": "abc123",
                "repo": repo(if head_owner == "owner" { 99 } else { 100 }, head_owner),
            },
            "base": {"label": "main", "ref": "main", "sha": "def456", "repo": repo(99, "owner")},
        })
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
                "url": "https://git.example.com/owner/repo/pull/7",
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
    fn pr_create_default_output_uses_githubs_pull_url() {
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
        assert_eq!(stdout.trim(), "https://git.example.com/owner/repo/pull/7");
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
    fn pr_create_web_keeps_a_branch_named_like_a_pull_request() {
        let client = FakeApi::default();
        crate::external::take_opened_web_urls();

        let (code, stdout, stderr) = run_fake(
            &[
                "pr",
                "create",
                "--title",
                "Ship it",
                "--base",
                "main",
                "--head",
                "feat/pull/3",
                "--web",
            ],
            &client,
            Some("token"),
            None,
        );

        let url = "https://git.example.com/owner/repo/compare/main...feat/pull/3";
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, format!("{url}\n"));
        assert_eq!(crate::external::take_opened_web_urls(), [url]);
    }

    #[test]
    fn pr_web_commands_give_forgejos_page_and_other_output_githubs_url() {
        let pull = json!({
            "number": 8,
            "title": "Make Codex happy",
            "state": "open",
            "html_url": "https://git.example.com/owner/repo/pulls/8",
            "head": {"ref": "feature", "sha": "abc123"},
            "base": {"ref": "main"},
            "user": {"login": "alice"}
        });
        let client = FakeApi {
            pulls: vec![pull.clone()],
            issues: vec![
                json!({"number": 8, "title": "Make Codex happy", "state": "open",
                "html_url": "https://git.example.com/owner/repo/pulls/8", "user": {"login": "alice"},
                "pull_request": {"merged": false}}),
            ],
            ..FakeApi::default()
        };
        for (argv, expected) in [
            (
                &["pr", "view", "8", "--web"][..],
                "https://git.example.com/owner/repo/pulls/8\n",
            ),
            (
                &["pr", "diff", "8", "--web"][..],
                "https://git.example.com/owner/repo/pulls/8/files\n",
            ),
            (
                &["issue", "view", "8", "--web"][..],
                "https://git.example.com/owner/repo/pulls/8\n",
            ),
        ] {
            let (code, stdout, stderr) = run_fake(argv, &client, Some("token"), None);
            assert_eq!(code, 0, "{argv:?}: {stderr}");
            assert_eq!(stdout, expected, "{argv:?}");
        }
        // Forgejo's issue route answers for a pull request; its URL takes GitHub's layout, as the
        // gateway gives it.
        let (code, stdout, stderr) = run_fake(
            &["issue", "view", "8", "--json", "url"],
            &client,
            Some("token"),
            None,
        );
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).ok(),
            Some(json!({"url": "https://git.example.com/owner/repo/pull/8"}))
        );
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
    fn pr_status_outputs_absolute_status_rollup_urls() {
        let client = FakeApi {
            pulls: vec![json!({
                "number": 8,
                "title": "Make Codex happy",
                "state": "open",
                "html_url": "https://git.example.com/owner/repo/pulls/8",
                "head": {"ref": "feature", "sha": "abc123"},
                "base": {"ref": "main"},
                "user": {"login": "alice"},
            })],
            statuses: vec![json!({
                "context": "ci/test",
                "description": "tests passed",
                "state": "success",
                "target_url": "/owner/repo/actions/runs/1",
                "created_at": "2026-06-01T00:00:00Z",
                "updated_at": "2026-06-01T00:02:00Z"
            })],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &[
                "pr",
                "status",
                "feature",
                "--json",
                "number,title,url,headRefName,statusCheckRollup",
            ],
            &client,
            Some("token"),
            None,
        );

        assert_eq!(code, 0, "{stderr}");
        let value = serde_json::from_str::<Value>(&stdout).expect("status json");
        assert_eq!(
            value["currentBranch"]["statusCheckRollup"][0]["targetUrl"],
            "https://git.example.com/owner/repo/actions/runs/1"
        );
        assert_eq!(
            value["currentBranch"]["statusCheckRollup"][0]["detailsUrl"],
            "https://git.example.com/owner/repo/actions/runs/1"
        );
    }

    #[test]
    fn pr_checks_pending_exit_is_eight_but_json_exits_zero() {
        // gh v2.103.3 checks.go:73-74,189-190,248-252. PR #3's real
        // checks have finished, so an oracle case cannot exercise pending.
        let mut client = checks_api();
        let pending = check_status("pending");
        client.check_statuses = vec![pending];
        let (code, stdout, stderr) = run_fake(&["pr", "checks", "8"], &client, Some("token"), None);
        assert_eq!(code, 8, "{stderr}");
        assert_eq!(
            stdout,
            "test\tpending\t0\thttps://git.example.com/owner/repo/actions/runs/1/jobs/1\t\n"
        );
        assert_eq!(stderr, "");

        let (code, stdout, stderr) = run_fake(
            &["pr", "checks", "8", "--json", "bucket,name"],
            &client,
            Some("token"),
            None,
        );
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).expect("json"),
            json!([{"bucket":"pending","name":"test"}])
        );

        client.check_statuses.push(CommitStatus {
            context: "external".into(),
            ..check_status("failure")
        });
        let (code, _, stderr) = run_fake(&["pr", "checks", "8"], &client, Some("token"), None);
        assert_eq!(code, 1, "failure wins over pending: {stderr}");
        let (code, _, stderr) = run_fake(
            &["pr", "checks", "8", "--json", "bucket"],
            &client,
            Some("token"),
            None,
        );
        assert_eq!(code, 0, "failure JSON also exits zero: {stderr}");
    }

    #[test]
    fn pr_checks_required_uses_base_branch_protection_and_core_globs() {
        let mut client = checks_api();
        client.check_statuses = vec![
            check_status("success"),
            CommitStatus {
                context: "external".into(),
                ..check_status("failure")
            },
        ];
        client.protection = Some(Protection {
            enable_status_check: true,
            status_check_contexts: vec!["ci / * (pull_request)".into()],
            ..Protection::default()
        });
        let (code, stdout, stderr) = run_fake(
            &["pr", "checks", "8", "--required", "--json", "name,bucket"],
            &client,
            Some("token"),
            None,
        );
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).expect("json"),
            json!([{"bucket":"pass","name":"test"}])
        );
        assert_eq!(*client.protection_branches.borrow(), ["main"]);

        client
            .protection
            .as_mut()
            .expect("protection")
            .enable_status_check = false;
        for args in [
            vec!["pr", "checks", "8", "--required"],
            vec!["pr", "checks", "8", "--required", "--json", "name"],
        ] {
            let (code, stdout, stderr) = run_fake(&args, &client, Some("token"), None);
            assert_eq!(code, 1);
            assert_eq!(stdout, "");
            assert_eq!(
                stderr,
                "no required checks reported on the 'feature' branch\n"
            );
        }
    }

    #[test]
    fn pr_checks_without_statuses_names_the_branch_even_in_json_mode() {
        let client = checks_api();
        for args in [
            vec!["pr", "checks", "8"],
            vec!["pr", "checks", "8", "--json", "name"],
            vec!["pr", "checks", "8", "--required"],
        ] {
            let (code, stdout, stderr) = run_fake(&args, &client, Some("token"), None);
            assert_eq!(code, 1);
            assert_eq!(stdout, "");
            assert_eq!(stderr, "no checks reported on the 'feature' branch\n");
        }
        assert!(client.protection_branches.borrow().is_empty());
    }

    fn checks_api() -> FakeApi {
        FakeApi {
            pulls: vec![
                json!({"number":8,"head":{"ref":"feature","sha":"abc123"},"base":{"ref":"main"}}),
            ],
            ..FakeApi::default()
        }
    }

    fn check_status(state: &str) -> CommitStatus {
        serde_json::from_value(json!({
            "id": 1, "context": "ci / test (pull_request)", "status": state,
            "target_url": "/owner/repo/actions/runs/1/jobs/1", "description":"Has started running",
            "created_at":"2026-06-01T00:00:00Z", "updated_at":"2026-06-01T00:01:00Z"
        }))
        .expect("valid status")
    }

    #[test]
    fn pr_checks_and_diff_use_statuses_and_files() {
        let client = FakeApi {
            pulls: vec![json!({"number": 8, "head": {"ref": "feature", "sha": "abc123"}})],
            check_statuses: vec![serde_json::from_value(json!({
                "id": 1,
                "context": "ci/test",
                "description": "tests passed",
                "status": "success",
                "target_url": "https://git.example.com/owner/repo/actions/runs/1",
                "created_at": "2026-06-01T00:00:00Z",
                "updated_at": "2026-06-01T00:00:02Z",
            }))
            .expect("valid status")],
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
                "workflow": "",
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
    fn pr_checkout_of_a_fork_main_never_replaces_the_local_main() {
        for args in [
            &[
                "pr",
                "checkout",
                "https://git.example.com/owner/repo/pulls/7",
                "--force",
            ][..],
            &[
                "pr",
                "checkout",
                "https://git.example.com/owner/repo/pulls/7",
            ][..],
            &["pr", "checkout", "7", "--recurse-submodules", "--force"][..],
        ] {
            let git_repo = CheckoutGitFixture::new();
            let fork_head = git_repo.fork_pull_head(7, "fork\n");
            let unpushed = git_repo.unpushed_commit_on_main();
            let client = FakeApi {
                pulls: vec![pull_from(7, "fork", "main")],
                ..FakeApi::default()
            };

            let (code, stdout, stderr) =
                run_fake(args, &client, Some("token"), Some(git_repo.work()));

            assert_eq!(git_repo.rev("main"), unpushed, "{args:?}: {stderr}");
            assert_eq!(code, 0, "{args:?}: {stderr}");
            assert_eq!(stdout, "checked out fork/main\n", "{args:?}");
            assert_eq!(git_repo.branch().as_deref(), Some("fork/main"), "{args:?}");
            assert_eq!(git_repo.rev("HEAD"), fork_head, "{args:?}");
        }
    }

    #[test]
    fn pr_checkout_of_a_same_repository_branch_keeps_its_name() {
        // No local branch, a stale one under `--force`, and a stale one without it, which is
        // fast-forwarded.
        for (stale, force, tip) in [
            (false, false, "feature-pr"),
            (true, true, "feature-pr"),
            (true, false, "feature-pr"),
        ] {
            let git_repo = CheckoutGitFixture::new();
            if stale {
                git(git_repo.work(), ["branch", "feature-pr", "main"]);
            }
            let client = FakeApi {
                pulls: vec![pull_from(12, "owner", "feature-pr")],
                ..FakeApi::default()
            };
            let mut args = vec!["pr", "checkout", "12"];
            if force {
                args.push("--force");
            }

            let (code, stdout, stderr) =
                run_fake(&args, &client, Some("token"), Some(git_repo.work()));

            assert_eq!(code, 0, "{args:?}: {stderr}");
            assert_eq!(stdout, "checked out feature-pr\n", "{args:?}");
            assert_eq!(git_repo.branch().as_deref(), Some("feature-pr"), "{args:?}");
            assert_eq!(
                git_repo.rev("HEAD"),
                git_repo.rev(&format!("origin/{tip}")),
                "{args:?}"
            );
        }
    }

    #[test]
    fn pr_checkout_in_a_fork_clone_fetches_from_the_base_remote() {
        let git_repo = CheckoutGitFixture::fork_clone();
        // The base's `feature-pr` moved on after the fork was made, so the fork's is older.
        let pull_head = git_repo.push_commit(
            "owner/repo",
            "feature-pr",
            "refs/heads/feature-pr",
            "moved\n",
        );
        git(
            &git_repo.bare("owner/repo"),
            ["update-ref", "refs/pull/12/head", "refs/heads/feature-pr"],
        );
        let fork_head = git_repo.fork_pull_head(7, "fork\n");
        assert_ne!(git_repo.rev("origin/feature-pr"), pull_head);

        for (number, head_owner, branch, head) in [
            (12, "owner", "feature-pr", pull_head),
            (7, "fork", "topic", fork_head),
        ] {
            let client = FakeApi {
                pulls: vec![pull_from(number, head_owner, branch)],
                ..FakeApi::default()
            };
            let number = number.to_string();

            let (code, stdout, stderr) = run_fake(
                &["pr", "checkout", number.as_str()],
                &client,
                Some("token"),
                Some(git_repo.work()),
            );

            assert_eq!(code, 0, "{branch}: {stderr}");
            assert_eq!(stdout, format!("checked out {branch}\n"));
            assert_eq!(git_repo.rev("HEAD"), head, "{branch}");
            git(git_repo.work(), ["checkout", "-q", "main"]);
        }
    }

    #[test]
    fn pr_checkout_never_fetches_from_a_mirror_that_pushes_to_the_forge() {
        // `origin` fetches from a local mirror taken before the PR's branch moved, and pushes to
        // the forge by push URL or by a second `url`. git fetches from the first `url` alone.
        for key in ["remote.origin.pushurl", "remote.origin.url"] {
            let git_repo = CheckoutGitFixture::new();
            let mirror = git_repo.root.join("mirror.git");
            git(
                &git_repo.root,
                [
                    OsStr::new("clone"),
                    OsStr::new("-q"),
                    OsStr::new("--mirror"),
                    git_repo.bare("owner/repo").as_os_str(),
                    mirror.as_os_str(),
                ],
            );
            let pull_head = git_repo.push_commit(
                "owner/repo",
                "feature-pr",
                "refs/heads/feature-pr",
                "moved\n",
            );
            git(
                &git_repo.bare("owner/repo"),
                ["update-ref", "refs/pull/12/head", "refs/heads/feature-pr"],
            );
            let forge = git_repo.url("owner/repo");
            git(
                git_repo.work(),
                [
                    OsStr::new("remote"),
                    OsStr::new("set-url"),
                    OsStr::new("origin"),
                    mirror.as_os_str(),
                ],
            );
            git(git_repo.work(), ["config", "--add", key, forge.as_str()]);
            let mut pull = pull_from(12, "owner", "feature-pr");
            pull["base"]["repo"]["clone_url"] = json!(forge);
            let client = FakeApi {
                pulls: vec![pull],
                ..FakeApi::default()
            };

            let (code, stdout, stderr) = run_fake(
                &["pr", "checkout", "12"],
                &client,
                Some("token"),
                Some(git_repo.work()),
            );

            assert_eq!(code, 0, "{key}: {stderr}");
            assert_eq!(stdout, "checked out feature-pr\n", "{key}");
            assert_eq!(git_repo.rev("HEAD"), pull_head, "{key}");
        }
    }

    #[test]
    fn pr_checkout_with_repo_fetches_that_repository_s_pull_request() {
        // Without a remote for `other/repo`, so from its clone URL, and with one under a name of
        // the user's, so from that remote.
        for remote in [false, true] {
            let git_repo = CheckoutGitFixture::new();
            git_repo.forge_repo("other/repo");
            let pull_head =
                git_repo.push_commit("other/repo", "main", "refs/pull/12/head", "other\n");
            if remote {
                let url = git_repo.url("other/repo");
                git(git_repo.work(), ["remote", "add", "theirs", url.as_str()]);
            }
            let mut pull = pull_from(12, "someone", "topic");
            pull["base"]["repo"] = json!({
                "id": 101,
                "full_name": "other/repo",
                "default_branch": "main",
                "clone_url": if remote {
                    "ssh://git@clone-url.example.com/other/repo.git".to_string()
                } else {
                    git_repo.url("other/repo")
                },
                "owner": {"login": "other"},
            });
            let client = FakeApi {
                pulls: vec![pull],
                ..FakeApi::default()
            };

            let (code, stdout, stderr) = run_fake(
                &["pr", "checkout", "12", "-R", "other/repo"],
                &client,
                Some("token"),
                Some(git_repo.work()),
            );

            assert_eq!(*client.pull_repos.borrow(), ["other/repo"]);
            assert_eq!(code, 0, "{remote}: {stderr}");
            assert_eq!(stdout, "checked out topic\n");
            assert_eq!(git_repo.rev("HEAD"), pull_head, "{remote}");
        }
    }

    #[test]
    fn pr_checkout_of_a_url_ignores_repo() {
        let git_repo = CheckoutGitFixture::new();
        let client = FakeApi {
            pulls: vec![pull_from(12, "owner", "feature-pr")],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &[
                "pr",
                "checkout",
                "https://git.example.com/owner/repo/pulls/12",
                "-R",
                "other/repo",
            ],
            &client,
            Some("token"),
            Some(git_repo.work()),
        );

        assert_eq!(*client.pull_repos.borrow(), ["owner/repo"]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "checked out feature-pr\n");
        assert_eq!(git_repo.rev("HEAD"), git_repo.rev("origin/feature-pr"));
    }

    #[test]
    fn pr_checkout_fast_forwards_a_branch_checked_out_before() {
        let git_repo = CheckoutGitFixture::new();
        let first = git_repo.fork_pull_head(7, "fork\n");
        let client = FakeApi {
            pulls: vec![pull_from(7, "fork", "main")],
            ..FakeApi::default()
        };
        let checkout = || {
            run_fake(
                &["pr", "checkout", "7"],
                &client,
                Some("token"),
                Some(git_repo.work()),
            )
        };
        let (code, _, stderr) = checkout();
        assert_eq!(code, 0, "{stderr}");
        git(git_repo.work(), ["checkout", "-q", "main"]);
        // The fork pushes again.
        let second = git_repo.push_commit("owner/repo", &first, "refs/pull/7/head", "again\n");

        let (code, stdout, stderr) = checkout();

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "checked out fork/main\n");
        assert_eq!(git_repo.branch().as_deref(), Some("fork/main"));
        assert_eq!(git_repo.rev("fork/main"), second);
    }

    #[test]
    fn pr_checkout_refuses_a_branch_it_cannot_fast_forward() {
        // A fork's `develop` against the user's own, and a same-repository branch with a local
        // commit.
        for (number, head_owner, branch) in [(7, "fork", "develop"), (12, "owner", "feature-pr")] {
            let git_repo = CheckoutGitFixture::new();
            git_repo.fork_pull_head(7, "fork\n");
            git(git_repo.work(), ["checkout", "-q", "-b", branch, "main"]);
            fs::write(git_repo.work().join("NOTES.md"), "mine\n").expect("write local file");
            git(git_repo.work(), ["add", "NOTES.md"]);
            git(git_repo.work(), ["commit", "-qm", "mine"]);
            let mine = git_repo.rev(branch);
            // The user works on `topic`, with a file git doesn't track.
            git(git_repo.work(), ["checkout", "-q", "-b", "topic", "main"]);
            fs::write(git_repo.work().join("SCRATCH.md"), "draft\n").expect("write untracked file");
            let client = FakeApi {
                pulls: vec![pull_from(number, head_owner, branch)],
                ..FakeApi::default()
            };
            let number = number.to_string();

            let (code, stdout, stderr) = run_fake(
                &["pr", "checkout", number.as_str()],
                &client,
                Some("token"),
                Some(git_repo.work()),
            );

            assert_ne!(code, 0, "{branch}");
            assert_eq!(stdout, "", "{branch}");
            assert!(stderr.contains("fast-forward"), "{branch}: {stderr}");
            for word in [branch, "--force", "--branch"] {
                assert!(stderr.contains(word), "{branch}: {stderr}");
            }
            assert_eq!(git_repo.rev(branch), mine, "{branch}");
            assert_eq!(git_repo.branch().as_deref(), Some("topic"), "{branch}");
        }
    }

    #[test]
    fn pr_checkout_keeps_a_branch_a_fetch_mapping_names() {
        // git applies the configured mapping to a fetch of the branch as well as the command
        // line's, which would reset the branch before the fast-forward check.
        for mapping in [
            "+refs/heads/feature-pr:refs/heads/feature-pr",
            "+refs/heads/*:refs/heads/*",
        ] {
            let git_repo = CheckoutGitFixture::new();
            git(
                git_repo.work(),
                ["checkout", "-q", "-b", "feature-pr", "main"],
            );
            fs::write(git_repo.work().join("NOTES.md"), "mine\n").expect("write local file");
            git(git_repo.work(), ["add", "NOTES.md"]);
            git(git_repo.work(), ["commit", "-qm", "mine"]);
            let mine = git_repo.rev("feature-pr");
            git(git_repo.work(), ["checkout", "-q", "main"]);
            git(
                git_repo.work(),
                ["config", "--add", "remote.origin.fetch", mapping],
            );
            let client = FakeApi {
                pulls: vec![pull_from(12, "owner", "feature-pr")],
                ..FakeApi::default()
            };

            let (code, stdout, stderr) = run_fake(
                &["pr", "checkout", "12"],
                &client,
                Some("token"),
                Some(git_repo.work()),
            );

            assert_eq!(code, 1, "{mapping}: {stderr}");
            assert_eq!(stdout, "", "{mapping}");
            assert!(stderr.contains("fast-forward"), "{mapping}: {stderr}");
            assert_eq!(git_repo.rev("feature-pr"), mine, "{mapping}");
            assert_eq!(git_repo.branch().as_deref(), Some("main"), "{mapping}");
        }
    }

    #[test]
    fn pr_checkout_refuses_a_branch_named_as_another_but_for_case() {
        // On a filesystem that ignores case, as macOS's does, `Develop` is the user's `develop`.
        // git records that at clone as `core.ignorecase`.
        for (branch, extra, local) in [
            ("Develop", &[][..], "Develop"),
            ("Develop", &["--force"][..], "Develop"),
            ("topic", &["--branch", "DEVELOP"][..], "DEVELOP"),
        ] {
            let git_repo = CheckoutGitFixture::new();
            git(git_repo.work(), ["config", "core.ignorecase", "true"]);
            git_repo.fork_pull_head(7, "fork\n");
            git(git_repo.work(), ["branch", "develop", "main"]);
            let develop = git_repo.rev("develop");
            let client = FakeApi {
                pulls: vec![pull_from(7, "fork", branch)],
                ..FakeApi::default()
            };
            let mut args = vec!["pr", "checkout", "7"];
            args.extend(extra);

            let (code, stdout, stderr) =
                run_fake(&args, &client, Some("token"), Some(git_repo.work()));

            assert_eq!(code, 1, "{args:?}");
            assert_eq!(stdout, "", "{args:?}");
            assert_eq!(
                stderr,
                format!(
                    "the local branch \"develop\" differs from {local:?} only in case; \
                     name another with --branch\n"
                ),
                "{args:?}"
            );
            assert_eq!(git_repo.rev("develop"), develop, "{args:?}");
            assert_eq!(git_repo.branch().as_deref(), Some("main"), "{args:?}");
        }
    }

    #[test]
    fn pr_checkout_takes_a_branch_named_as_another_but_for_case_where_git_keeps_case() {
        for (branch, extra, local) in [
            ("Develop", &[][..], "Develop"),
            ("Develop", &["--force"][..], "Develop"),
            ("topic", &["--branch", "DEVELOP"][..], "DEVELOP"),
        ] {
            let git_repo = CheckoutGitFixture::new();
            if !git_repo.case_sensitive_refs() {
                eprintln!("skipped: this git can't keep branch names apart by case here");
                return;
            }
            git(git_repo.work(), ["config", "core.ignorecase", "false"]);
            let fork_head = git_repo.fork_pull_head(7, "fork\n");
            git(git_repo.work(), ["branch", "develop", "main"]);
            let develop = git_repo.rev("develop");
            let client = FakeApi {
                pulls: vec![pull_from(7, "fork", branch)],
                ..FakeApi::default()
            };
            let mut args = vec!["pr", "checkout", "7"];
            args.extend(extra);

            let (code, stdout, stderr) =
                run_fake(&args, &client, Some("token"), Some(git_repo.work()));

            assert_eq!(code, 0, "{args:?}: {stderr}");
            assert_eq!(stdout, format!("checked out {local}\n"), "{args:?}");
            assert_eq!(git_repo.branch().as_deref(), Some(local), "{args:?}");
            assert_eq!(git_repo.rev("HEAD"), fork_head, "{args:?}");
            assert_eq!(git_repo.rev("develop"), develop, "{args:?}");
        }
    }

    #[test]
    fn pr_checkout_of_a_branch_that_exists_ignores_its_case_sibling() {
        let git_repo = CheckoutGitFixture::new();
        if !git_repo.case_sensitive_refs() {
            eprintln!("skipped: this git can't keep branch names apart by case here");
            return;
        }
        git(git_repo.work(), ["config", "core.ignorecase", "true"]);
        let fork_head = git_repo.fork_pull_head(7, "fork\n");
        git(git_repo.work(), ["branch", "feature", "main"]);
        git(git_repo.work(), ["branch", "Feature", "main"]);
        let sibling = git_repo.rev("Feature");
        let client = FakeApi {
            pulls: vec![pull_from(7, "fork", "feature")],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &["pr", "checkout", "7"],
            &client,
            Some("token"),
            Some(git_repo.work()),
        );

        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "checked out feature\n");
        assert_eq!(git_repo.branch().as_deref(), Some("feature"));
        assert_eq!(git_repo.rev("feature"), fork_head);
        assert_eq!(git_repo.rev("Feature"), sibling);
    }

    #[test]
    fn pr_checkout_of_a_closed_pull_request_fetches_its_own_ref() {
        let git_repo = CheckoutGitFixture::new();
        let pull_head = git_repo.rev("origin/feature-pr");
        // The branch moves on after the PR closes.
        git(git_repo.work(), ["checkout", "-q", "origin/feature-pr"]);
        git(
            git_repo.work(),
            ["commit", "-q", "--allow-empty", "-m", "later"],
        );
        git(git_repo.work(), ["push", "-q", "origin", "HEAD:feature-pr"]);
        git(git_repo.work(), ["checkout", "-q", "main"]);
        let mut pull = pull_from(12, "owner", "feature-pr");
        pull["state"] = json!("closed");
        let client = FakeApi {
            pulls: vec![pull],
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
        assert_eq!(git_repo.rev("HEAD"), pull_head);
    }

    #[test]
    fn pr_checkout_of_a_fork_main_in_another_case_gets_the_owner_in_front() {
        let git_repo = CheckoutGitFixture::new();
        let fork_head = git_repo.fork_pull_head(7, "fork\n");
        let unpushed = git_repo.unpushed_commit_on_main();
        let client = FakeApi {
            pulls: vec![pull_from(7, "fork", "Main")],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &["pr", "checkout", "7", "--force"],
            &client,
            Some("token"),
            Some(git_repo.work()),
        );

        assert_eq!(git_repo.rev("main"), unpushed, "{stderr}");
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "checked out fork/Main\n");
        assert_eq!(git_repo.rev("HEAD"), fork_head);
    }

    #[test]
    fn pr_checkout_of_a_deleted_fork_main_never_replaces_the_local_main() {
        let git_repo = CheckoutGitFixture::new();
        let fork_head = git_repo.fork_pull_head(7, "fork\n");
        let unpushed = git_repo.unpushed_commit_on_main();
        let mut pull = pull_from(7, "fork", "main");
        pull["head"]["ref"] = json!("refs/pull/7/head");
        pull["head"]["repo"] = Value::Null;
        let client = FakeApi {
            pulls: vec![pull],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &["pr", "checkout", "7", "--force"],
            &client,
            Some("token"),
            Some(git_repo.work()),
        );

        assert_eq!(git_repo.rev("main"), unpushed, "{stderr}");
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "checked out pull-7\n");
        assert_eq!(git_repo.rev("HEAD"), fork_head);
    }

    #[test]
    fn pr_checkout_of_a_head_named_like_a_ref_lands_detached() {
        // An AGit PR, branches git would read as other refs, and `@`, from the base repository.
        for (label, ref_name) in [
            ("", "refs/pull/7/head"),
            ("refs/heads/main", "refs/heads/main"),
            ("heads/main", "heads/main"),
            ("@", "@"),
        ] {
            let git_repo = CheckoutGitFixture::new();
            let pull_head = git_repo.fork_pull_head(7, "pr\n");
            let unpushed = git_repo.unpushed_commit_on_main();
            let mut pull = pull_from(7, "owner", ref_name);
            pull["head"]["label"] = json!(label);
            let client = FakeApi {
                pulls: vec![pull],
                ..FakeApi::default()
            };

            let (code, stdout, stderr) = run_fake(
                &["pr", "checkout", "7", "--force"],
                &client,
                Some("token"),
                Some(git_repo.work()),
            );

            assert_eq!(git_repo.rev("main"), unpushed, "{ref_name}: {stderr}");
            assert_eq!(code, 0, "{ref_name}: {stderr}");
            assert_eq!(stdout, "checked out FETCH_HEAD\n", "{ref_name}");
            assert_eq!(git_repo.branch(), None, "{ref_name}");
            assert_eq!(git_repo.rev("HEAD"), pull_head, "{ref_name}");
        }
    }

    #[test]
    fn pr_checkout_refuses_a_head_named_like_an_option() {
        let git_repo = CheckoutGitFixture::new();
        let client = FakeApi {
            pulls: vec![pull_from(12, "fork", "-f")],
            ..FakeApi::default()
        };

        let (code, stdout, stderr) = run_fake(
            &["pr", "checkout", "12"],
            &client,
            Some("token"),
            Some(git_repo.work()),
        );

        assert_eq!(code, 1);
        assert_eq!(stdout, "");
        assert_eq!(stderr, "invalid branch name: \"-f\"\n");
        assert_eq!(git_repo.branch().as_deref(), Some("main"));
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

        // A --web command gives Forgejo's own /pulls/8, which opens without a redirect.
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            stdout,
            "https://git.example.com/owner/repo/pulls/8#new-comment\n"
        );
        assert!(
            crate::external::take_opened_web_urls()
                .contains(&"https://git.example.com/owner/repo/pulls/8#new-comment".to_string()),
            "Forgejo's page should be opened"
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

    /// gh takes a pull request or issue URL's repository and doesn't read `-R` then
    /// (`pkg/cmd/pr/shared/finder.go`, `pkg/cmd/issue/shared/lookup.go` in gh 2.103.3), so a
    /// comment on a URL's pull request never goes to `-R`'s pull request of the same number.
    #[test]
    fn a_url_selector_names_the_repository_even_with_a_repo_flag() {
        let client = FakeApi {
            pulls: vec![json!({
                "number": 1,
                "html_url": "https://git.example.com/target/project/pulls/1",
                "head": {"ref": "feature", "sha": "abc123"},
                "base": {"ref": "main"},
            })],
            issues: vec![json!({
                "number": 1,
                "title": "Bug",
                "html_url": "https://git.example.com/target/project/issues/1",
            })],
            check_statuses: vec![serde_json::from_value(json!({
                "id": 1,
                "context": "ci/test",
                "description": "tests passed",
                "status": "success",
                "target_url": "https://git.example.com/target/project/actions/runs/1",
                "created_at": "2026-06-01T00:00:00Z",
                "updated_at": "2026-06-01T00:00:02Z",
            }))
            .expect("valid status")],
            ..FakeApi::default()
        };
        let pull = "https://git.example.com/target/project/pulls/1";
        let issue = "https://git.example.com/target/project/issues/1";
        let other = "git.example.com/other/project";

        for args in [
            &["pr", "comment", pull, "-R", other, "--body", "hi"][..],
            &["pr", "comment", "--repo", other, pull, "--body", "hi"],
            &[
                "pr",
                "view",
                pull,
                &format!("--repo={other}"),
                "--json",
                "number",
            ],
            &["pr", "diff", pull, "-R", other],
            &["pr", "checks", pull, "-R", other, "--json", "name"],
            &["issue", "view", issue, "-R", other, "--json", "number"],
        ] {
            let (code, _, stderr) = run_fake(args, &client, Some("token"), None);
            assert_eq!(code, 0, "{args:?}: {stderr}");
        }

        let comments = client.comments.borrow();
        assert_eq!(comments.len(), 2);
        for (repo, number, _) in comments.iter() {
            assert_eq!(
                (repo.owner.as_str(), repo.repo.as_str(), *number),
                ("target", "project", 1)
            );
        }
        let reads = client.read_repos.borrow();
        assert!(!reads.is_empty());
        assert!(
            reads.iter().all(|repo| repo == "target/project"),
            "{reads:?}"
        );
    }
}
