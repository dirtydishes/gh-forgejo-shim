use std::fmt;
use std::time::Duration;

use fga_core::forgejo::{CommitStatus, Protection};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, LOCATION, USER_AGENT};
use reqwest::redirect::Policy;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use url::Url;

use crate::ShimError;

/// Upper bound for paginated reads: 200 pages of 50 items.
const MAX_PAGES: usize = 200;

const USER_AGENT_VALUE: &str = "gh-forgejo-shim";

pub type ForgejoResult<T> = std::result::Result<T, ForgejoError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoError {
    message: String,
}

impl ForgejoError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ForgejoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ForgejoError {}

impl From<ForgejoError> for ShimError {
    fn from(error: ForgejoError) -> Self {
        Self::new(error.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub host: String,
    pub owner: String,
    pub repo: String,
}

impl RepoRef {
    pub fn new(host: impl Into<String>, owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            owner: owner.into(),
            repo: repo.into(),
        }
    }

    pub fn web_base_url(&self) -> String {
        format!("https://{}/{}/{}", self.host, self.owner, self.repo)
    }

    pub fn api_base_url(&self) -> String {
        self.api_base_url_with_scheme("https")
    }

    fn api_base_url_with_scheme(&self, scheme: &str) -> String {
        format!(
            "{scheme}://{}/api/v1/repos/{}/{}",
            host_for_url(&self.host),
            quote_path_segment(&self.owner),
            quote_path_segment(&self.repo)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePullRequest {
    pub title: String,
    pub body: String,
    pub base: String,
    pub head: String,
    pub draft: bool,
}

impl CreatePullRequest {
    pub fn new(
        title: impl Into<String>,
        body: impl Into<String>,
        base: impl Into<String>,
        head: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            base: base.into(),
            head: head.into(),
            draft: false,
        }
    }

    pub fn draft(mut self, draft: bool) -> Self {
        self.draft = draft;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateIssueRequest {
    pub title: String,
    pub body: String,
    pub assignees: Vec<String>,
    pub labels: Vec<i64>,
    pub milestone: Option<i64>,
}

impl CreateIssueRequest {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: String::new(),
            assignees: Vec::new(),
            labels: Vec::new(),
            milestone: None,
        }
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn assignees(mut self, assignees: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.assignees = assignees.into_iter().map(Into::into).collect();
        self
    }

    pub fn labels(mut self, labels: impl IntoIterator<Item = i64>) -> Self {
        self.labels = labels.into_iter().collect();
        self
    }

    pub fn milestone(mut self, milestone: i64) -> Self {
        self.milestone = Some(milestone);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListIssuesOptions {
    pub state: Option<String>,
    pub labels: Vec<String>,
    pub query: Option<String>,
    pub milestone: Option<String>,
    pub author: Option<String>,
    pub assignee: Option<String>,
    pub mentioned: Option<String>,
    pub limit: Option<usize>,
}

impl ListIssuesOptions {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> &str {
        self.state.as_deref().unwrap_or("open")
    }
}

#[derive(Clone)]
pub struct ForgejoClient {
    token: Option<String>,
    timeout: Duration,
    scheme: String,
    base_url: Option<String>,
    http: Client,
}

impl ForgejoClient {
    pub fn new(token: Option<String>) -> Self {
        Self {
            token,
            timeout: Duration::from_secs(30),
            scheme: "https".to_string(),
            base_url: None,
            http: Client::builder()
                .redirect(Policy::none())
                .build()
                .expect("HTTP client that follows no redirect"),
        }
    }

    /// Build a client for one configured host. Both CA sources add roots to
    /// the normal trust store; neither disables certificate verification.
    /// It follows only a renamed repository's redirect within the API.
    pub fn for_host(
        host: &str,
        token: Option<String>,
        config: &crate::config::Config,
        env: &crate::config::EnvMap,
    ) -> ForgejoResult<Self> {
        Self::for_host_following(host, token, config, env, true)
    }

    /// [`ForgejoClient::for_host`] for [`ForgejoClient::probe`]: it follows no redirect, so the
    /// token goes to the configured host only and a redirect reads as the answer it is.
    pub fn for_host_without_redirects(
        host: &str,
        token: Option<String>,
        config: &crate::config::Config,
        env: &crate::config::EnvMap,
    ) -> ForgejoResult<Self> {
        Self::for_host_following(host, token, config, env, false)
    }

    fn for_host_following(
        host: &str,
        token: Option<String>,
        config: &crate::config::Config,
        env: &crate::config::EnvMap,
        redirects: bool,
    ) -> ForgejoResult<Self> {
        let host = crate::config::normalize_host(host);
        let settings = config.host.get(&host);
        let mut paths = Vec::new();
        if let Some(path) = settings.and_then(|settings| settings.ca_file.as_deref()) {
            paths.push(path);
        }
        if let Some(path) = env.get("FJ_SHIM_CA_FILE").filter(|path| !path.is_empty()) {
            let path = std::path::Path::new(path);
            if !path.is_absolute() {
                return Err(ForgejoError::new(
                    "FJ_SHIM_CA_FILE must be an absolute path",
                ));
            }
            paths.push(path);
        }
        let base_url = settings
            .and_then(|settings| settings.api_url.clone())
            .unwrap_or_else(|| format!("https://{}", host_for_url(&host)));
        // `repo_api_base_url` builds repository calls the same way.
        let policy = match Url::parse(&format!("{base_url}/api/v1/repos")) {
            Ok(repos) if redirects => renamed_repository_redirects(&repos),
            _ => Policy::none(),
        };
        let mut client = Self::with_ca_files(token, &paths, policy)?;
        client.base_url = Some(base_url);
        Ok(client)
    }

    /// A client trusting `path`'s roots too, following no redirect.
    pub fn with_ca_file(token: Option<String>, path: &std::path::Path) -> ForgejoResult<Self> {
        Self::with_ca_files(token, &[path], Policy::none())
    }

    fn with_ca_files(
        token: Option<String>,
        paths: &[&std::path::Path],
        redirects: Policy,
    ) -> ForgejoResult<Self> {
        let mut builder = Client::builder().redirect(redirects);
        for path in paths {
            let pem = std::fs::read(path).map_err(|error| {
                ForgejoError::new(format!(
                    "could not read CA certificate {}: {error}",
                    path.display()
                ))
            })?;
            let certificates = reqwest::Certificate::from_pem_bundle(&pem).map_err(|error| {
                ForgejoError::new(format!(
                    "invalid CA certificate {}: {error}",
                    path.display()
                ))
            })?;
            if certificates.is_empty() {
                return Err(ForgejoError::new(format!(
                    "invalid CA certificate {}: no PEM certificates found",
                    path.display()
                )));
            }
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate);
            }
        }
        let http = builder.build().map_err(|error| {
            ForgejoError::new(format!("could not configure HTTP client: {error}"))
        })?;
        Ok(Self {
            token,
            timeout: Duration::from_secs(30),
            scheme: "https".to_string(),
            base_url: None,
            http,
        })
    }

    pub fn with_token(token: impl Into<String>) -> Self {
        Self::new(Some(token.into()))
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_scheme(mut self, scheme: impl Into<String>) -> Self {
        self.scheme = scheme.into();
        self
    }

    /// One request to `path` below the host's base, answered with its status and JSON body
    /// (`null` when it isn't JSON) whatever the status. Errors are transport failures only, and
    /// their text names no token.
    pub fn probe(&self, host: &str, method: Method, path: &str) -> ForgejoResult<(u16, Value)> {
        let mut request = self
            .http
            .request(
                method.clone(),
                format!("{}{path}", self.host_base_url(host)),
            )
            .timeout(self.timeout)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, USER_AGENT_VALUE);
        if let Some(token) = &self.token {
            request = request.header(AUTHORIZATION, format!("token {token}"));
        }
        if method != Method::GET {
            request = request.header(CONTENT_TYPE, "application/json").body("{}");
        }
        let response = request.send().map_err(|error| {
            ForgejoError::new(if error.is_timeout() {
                "it didn't answer in time".to_string()
            } else if error.is_connect() {
                "it couldn't be reached".to_string()
            } else {
                "the request failed".to_string()
            })
        })?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null);
        Ok((status, body))
    }

    pub fn get_current_user(&self, host: &str) -> ForgejoResult<Value> {
        self.request_json(
            Method::GET,
            format!("{}/api/v1/user", self.host_base_url(host)),
            None,
        )
    }

    pub fn create_pull(&self, repo: &RepoRef, request: &CreatePullRequest) -> ForgejoResult<Value> {
        self.request_json(
            Method::POST,
            format!("{}/pulls", self.repo_api_base_url(repo)?),
            Some(json!({
                "title": request.title,
                "body": request.body,
                "base": request.base,
                "head": request.head,
                "draft": request.draft,
            })),
        )
    }

    pub fn get_pull(&self, repo: &RepoRef, number: u64) -> ForgejoResult<Value> {
        self.request_json(
            Method::GET,
            format!("{}/pulls/{number}", self.repo_api_base_url(repo)?),
            None,
        )
    }

    pub fn get_pull_diff(&self, repo: &RepoRef, number: u64, patch: bool) -> ForgejoResult<String> {
        let diff_type = if patch { "patch" } else { "diff" };
        self.request_text(
            Method::GET,
            format!(
                "{}/pulls/{number}.{diff_type}",
                self.repo_api_base_url(repo)?
            ),
            None,
            "text/plain",
        )
    }

    pub fn list_pull_files(&self, repo: &RepoRef, number: u64) -> ForgejoResult<Vec<Value>> {
        let files = self.request_json(
            Method::GET,
            format!("{}/pulls/{number}/files", self.repo_api_base_url(repo)?),
            None,
        )?;
        Ok(list_of_objects(files))
    }

    pub fn list_commit_statuses(&self, repo: &RepoRef, sha: &str) -> ForgejoResult<Vec<Value>> {
        let statuses = self.request_json(
            Method::GET,
            format!(
                "{}/statuses/{}",
                self.repo_api_base_url(repo)?,
                path_segment(sha)?
            ),
            None,
        )?;
        if let Some(items) = statuses.get("statuses").and_then(Value::as_array) {
            return Ok(items
                .iter()
                .filter(|item| item.is_object())
                .cloned()
                .collect());
        }
        Ok(list_of_objects(statuses))
    }

    /// Check mapping needs the full history, including contexts beyond page one.
    pub fn list_check_statuses(
        &self,
        repo: &RepoRef,
        sha: &str,
    ) -> ForgejoResult<Vec<CommitStatus>> {
        self.get_pages(format!(
            "{}/commits/{}/statuses",
            self.repo_api_base_url(repo)?,
            path_segment(sha)?
        ))
    }

    pub fn get_branch_protection(
        &self,
        repo: &RepoRef,
        branch: &str,
    ) -> ForgejoResult<Option<Protection>> {
        // Forgejo's branch_protections list takes no page or limit parameters and
        // returns every rule on each request, so it is read once, not paginated.
        let value = self.request_json(
            Method::GET,
            format!("{}/branch_protections", self.repo_api_base_url(repo)?),
            None,
        )?;
        let rules: Vec<Protection> = serde_json::from_value(value)
            .map_err(|error| ForgejoError::new(format!("invalid Forgejo response: {error}")))?;
        // Match the gateway Session::protection rule selection, including globs.
        Ok(rules.into_iter().find(|rule| {
            glob::Pattern::new(&rule.rule_name).is_ok_and(|pattern| pattern.matches(branch))
        }))
    }

    fn get_pages<T: DeserializeOwned>(&self, url: String) -> ForgejoResult<Vec<T>> {
        self.get_pages_up_to(url, MAX_PAGES)
    }

    fn get_pages_up_to<T: DeserializeOwned>(
        &self,
        url: String,
        max_pages: usize,
    ) -> ForgejoResult<Vec<T>> {
        let mut items = Vec::new();
        for page in 1.. {
            // An endpoint that ignores `page` returns the same non-empty page forever.
            if page > max_pages {
                return Err(ForgejoError::new(format!(
                    "Forgejo returned more than {max_pages} pages for {url}"
                )));
            }
            let value =
                self.request_json(Method::GET, format!("{url}?page={page}&limit=50"), None)?;
            let batch: Vec<T> = serde_json::from_value(value)
                .map_err(|error| ForgejoError::new(format!("invalid Forgejo response: {error}")))?;
            // A server may cap the requested limit below 50. Only an empty page
            // proves that all history was read; a short page does not.
            if batch.is_empty() {
                break;
            }
            items.extend(batch);
        }
        Ok(items)
    }

    pub fn get_repo(&self, repo: &RepoRef) -> ForgejoResult<Value> {
        self.request_json(Method::GET, self.repo_api_base_url(repo)?, None)
    }

    pub fn list_issues(
        &self,
        repo: &RepoRef,
        options: &ListIssuesOptions,
    ) -> ForgejoResult<Vec<Value>> {
        let mut params = vec![
            ("state".to_string(), options.state().to_string()),
            ("type".to_string(), "issues".to_string()),
        ];
        if !options.labels.is_empty() {
            params.push(("labels".to_string(), options.labels.join(",")));
        }
        if let Some(query) = &options.query {
            params.push(("q".to_string(), query.clone()));
        }
        if let Some(milestone) = &options.milestone {
            params.push(("milestones".to_string(), milestone.clone()));
        }
        if let Some(author) = &options.author {
            if author != "@me" {
                params.push(("created_by".to_string(), author.clone()));
            }
        }
        if let Some(assignee) = &options.assignee {
            if assignee != "@me" {
                params.push(("assigned_by".to_string(), assignee.clone()));
            }
        }
        if let Some(mentioned) = &options.mentioned {
            if mentioned != "@me" {
                params.push(("mentioned_by".to_string(), mentioned.clone()));
            }
        }
        if let Some(limit) = options.limit {
            params.push(("limit".to_string(), limit.to_string()));
        }

        let issues = self.request_json(
            Method::GET,
            format!(
                "{}/issues?{}",
                self.repo_api_base_url(repo)?,
                form_urlencode(&params)
            ),
            None,
        )?;
        Ok(list_of_objects(issues))
    }

    pub fn get_issue(&self, repo: &RepoRef, number: u64) -> ForgejoResult<Value> {
        self.request_json(
            Method::GET,
            format!("{}/issues/{number}", self.repo_api_base_url(repo)?),
            None,
        )
    }

    pub fn list_labels(&self, repo: &RepoRef) -> ForgejoResult<Vec<Value>> {
        let labels = self.request_json(
            Method::GET,
            format!("{}/labels", self.repo_api_base_url(repo)?),
            None,
        )?;
        Ok(list_of_objects(labels))
    }

    pub fn create_issue(
        &self,
        repo: &RepoRef,
        request: &CreateIssueRequest,
    ) -> ForgejoResult<Value> {
        let mut payload = serde_json::Map::new();
        payload.insert("title".to_string(), Value::String(request.title.clone()));
        payload.insert("body".to_string(), Value::String(request.body.clone()));
        if !request.assignees.is_empty() {
            payload.insert("assignees".to_string(), json!(request.assignees));
        }
        if !request.labels.is_empty() {
            payload.insert("labels".to_string(), json!(request.labels));
        }
        if let Some(milestone) = request.milestone {
            payload.insert("milestone".to_string(), json!(milestone));
        }

        self.request_json(
            Method::POST,
            format!("{}/issues", self.repo_api_base_url(repo)?),
            Some(Value::Object(payload)),
        )
    }

    pub fn create_issue_comment(
        &self,
        repo: &RepoRef,
        number: u64,
        body: &str,
    ) -> ForgejoResult<Value> {
        self.request_json(
            Method::POST,
            format!("{}/issues/{number}/comments", self.repo_api_base_url(repo)?),
            Some(json!({ "body": body })),
        )
    }

    pub fn list_pulls(
        &self,
        repo: &RepoRef,
        state: &str,
        head: Option<&str>,
    ) -> ForgejoResult<Vec<Value>> {
        let params = [("state".to_string(), state.to_string())];
        let pulls = self.request_json(
            Method::GET,
            format!(
                "{}/pulls?{}",
                self.repo_api_base_url(repo)?,
                form_urlencode(&params)
            ),
            None,
        )?;
        let pulls = list_of_objects(pulls);
        let Some(head) = head else {
            return Ok(pulls);
        };
        Ok(pulls
            .into_iter()
            .filter(|pull| head_matches(pull, head))
            .collect())
    }

    fn repo_api_base_url(&self, repo: &RepoRef) -> ForgejoResult<String> {
        Ok(format!(
            "{}/api/v1/repos/{}/{}",
            self.host_base_url(&repo.host),
            path_segment(&repo.owner)?,
            path_segment(&repo.repo)?
        ))
    }

    fn host_base_url(&self, host: &str) -> String {
        self.base_url
            .clone()
            .unwrap_or_else(|| format!("{}://{}", self.scheme, host_for_url(host)))
    }

    fn request_json(
        &self,
        method: Method,
        url: String,
        payload: Option<Value>,
    ) -> ForgejoResult<Value> {
        let response_data = self.request(method, url, payload, "application/json")?;
        if response_data.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_slice(&response_data)
            .map_err(|_| ForgejoError::new("Forgejo API returned invalid JSON"))
    }

    fn request_text(
        &self,
        method: Method,
        url: String,
        payload: Option<Value>,
        accept: &str,
    ) -> ForgejoResult<String> {
        let response_data = self.request(method, url, payload, accept)?;
        Ok(String::from_utf8_lossy(&response_data).into_owned())
    }

    fn request(
        &self,
        method: Method,
        url: String,
        payload: Option<Value>,
        accept: &str,
    ) -> ForgejoResult<Vec<u8>> {
        let mut request = self
            .http
            .request(method, &url)
            .timeout(self.timeout)
            .header(ACCEPT, accept)
            .header(USER_AGENT, USER_AGENT_VALUE);

        if let Some(token) = &self.token {
            request = request.header(AUTHORIZATION, format!("token {token}"));
        }

        if let Some(payload) = payload {
            let body = serde_json::to_vec(&payload).map_err(|error| {
                ForgejoError::new(format!("Forgejo API request failed: {error}"))
            })?;
            request = request.header(CONTENT_TYPE, "application/json").body(body);
        }

        let response = request
            .send()
            .map_err(|error| ForgejoError::new(format!("Forgejo API request failed: {error}")))?;
        let status = response.status();
        if status.is_redirection() {
            // Only the origin it pointed to: its path, query and userinfo may each hold a secret.
            let target = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|location| response.url().join(location).ok())
                .map(|target| format!(" to {}", target.origin().ascii_serialization()))
                .unwrap_or_default();
            return Err(ForgejoError::new(format!(
                "Forgejo API answered HTTP {} with a redirect{target} that the shim didn't follow: it follows only a renamed repository's redirect within the API, up to 10 in a row",
                status.as_u16()
            )));
        }
        let response_data = response
            .bytes()
            .map_err(|error| ForgejoError::new(format!("Forgejo API request failed: {error}")))?;
        let response_data = response_data.to_vec();

        if !status.is_success() {
            let message = String::from_utf8_lossy(&response_data);
            return Err(ForgejoError::new(format!(
                "Forgejo API returned HTTP {}: {message}",
                status.as_u16()
            )));
        }

        Ok(response_data)
    }
}

fn list_of_objects(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items.into_iter().filter(Value::is_object).collect(),
        _ => Vec::new(),
    }
}

fn host_for_url(host: &str) -> String {
    let trimmed = host.trim().trim_matches('/');
    if let Some((_, rest)) = trimmed.split_once("://") {
        let end = rest.find('/').unwrap_or(rest.len());
        return rest[..end].to_string();
    }
    trimmed.to_string()
}

/// The redirects a client follows, given `repos`, the URL its repository calls start with:
/// `<base>/api/v1/repos`, parsed as its requests are, so a base path keeps every segment, empty
/// ones included. Forgejo answers an API call for a renamed or moved repository with a redirect
/// to the same path and query under the new owner and name: 301 for a renamed repository, 307
/// for a renamed owner. The client follows that, up to 10 times. It returns any other redirect as
/// the answer, so the token never goes to another path or service on the same origin.
fn renamed_repository_redirects(repos: &Url) -> Policy {
    let repos: Vec<String> = repos
        .path_segments()
        .into_iter()
        .flatten()
        .map(str::to_owned)
        .collect();
    Policy::custom(move |attempt| {
        // `previous` starts with the first request, so this allows 10 redirects.
        let follow = attempt.previous().len() <= 10
            && attempt
                .previous()
                .last()
                .is_some_and(|from| renames_repository(&repos, from, attempt.url()));
        if follow {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// Whether `to` is `from`, an API call below `repos` (the path segments of `<base>/api/v1/repos`),
/// on the same origin with only the owner and repository after `repos` changed, to plain names.
fn renames_repository(repos: &[String], from: &Url, to: &Url) -> bool {
    fn segments(url: &Url) -> Option<Vec<&str>> {
        url.path_segments().map(Iterator::collect)
    }
    let (Some(old), Some(new)) = (segments(from), segments(to)) else {
        return false;
    };
    let names = repos.len()..repos.len() + 2;
    let below_repos =
        |path: &[&str]| path.len() >= names.end && path[..repos.len()].iter().eq(repos.iter());
    from.origin() == to.origin()
        && to.username().is_empty()
        && to.password().is_none()
        && from.query() == to.query()
        && old.len() == new.len()
        && below_repos(&old)
        && below_repos(&new)
        && old[names.end..] == new[names.end..]
        && new[names].iter().all(|name| is_forgejo_name(name))
}

/// A name Forgejo allows for an owner or repository: ASCII letters, digits, `-`, `_` and `.`,
/// and never `.` or `..`. Anything else in a redirect, an escape included, isn't Forgejo's rename.
fn is_forgejo_name(name: &str) -> bool {
    !matches!(name, "" | "." | "..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn head_matches(item: &Value, head: &str) -> bool {
    let Some(head_data) = item.get("head").and_then(Value::as_object) else {
        return false;
    };
    let ref_name = head_data.get("ref").and_then(Value::as_str);
    let label = head_data.get("label").and_then(Value::as_str);
    ref_name == Some(head)
        || label == Some(head)
        || label.is_some_and(|value| value.ends_with(&format!(":{head}")))
}

fn quote_path_segment(value: &str) -> String {
    percent_encode(value.as_bytes(), PercentEncodeMode::Path)
}

/// `value` escaped as one segment of an API path, refused before any request when a URL parser
/// or proxy could resolve it out of the path: [`fga_core::forgejo::climbs`], the rule the gateway
/// applies. Escaping alone isn't enough, since `.` stays as written and a proxy may decode `%2F`.
fn path_segment(value: &str) -> ForgejoResult<String> {
    if fga_core::forgejo::climbs(value) {
        return Err(ForgejoError::new(format!(
            "{value:?} can't be part of a Forgejo API path: it could resolve outside the repository"
        )));
    }
    Ok(quote_path_segment(value))
}

fn form_urlencode(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                percent_encode(key.as_bytes(), PercentEncodeMode::Form),
                percent_encode(value.as_bytes(), PercentEncodeMode::Form)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

enum PercentEncodeMode {
    Path,
    Form,
}

fn percent_encode(bytes: &[u8], mode: PercentEncodeMode) -> String {
    let mut encoded = String::new();
    for byte in bytes {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                encoded.push(char::from(*byte));
            }
            b' ' if matches!(mode, PercentEncodeMode::Form) => encoded.push('+'),
            value => encoded.push_str(&format!("%{value:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::io::{self, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    #[derive(Debug)]
    struct CapturedRequest {
        request_line: String,
        headers: BTreeMap<String, String>,
        body: String,
    }

    struct FakeResponse {
        status: u16,
        reason: &'static str,
        content_type: &'static str,
        body: &'static str,
    }

    impl FakeResponse {
        fn json(body: &'static str) -> Self {
            Self {
                status: 200,
                reason: "OK",
                content_type: "application/json",
                body,
            }
        }

        fn text(body: &'static str) -> Self {
            Self {
                status: 200,
                reason: "OK",
                content_type: "text/plain",
                body,
            }
        }

        fn status(status: u16, reason: &'static str, body: &'static str) -> Self {
            Self {
                status,
                reason,
                content_type: "text/plain",
                body,
            }
        }
    }

    #[test]
    fn configured_ca_file_is_checked() {
        let missing = std::env::temp_dir().join("gh-forgejo-shim-missing-test-ca.pem");
        let error = ForgejoClient::with_ca_file(None, &missing)
            .err()
            .expect("missing CA must fail");
        assert!(error.to_string().contains("CA certificate"));
        let invalid = std::env::current_exe().expect("test executable");
        let error = ForgejoClient::with_ca_file(None, &invalid)
            .err()
            .expect("invalid CA must fail");
        assert!(error.to_string().contains("CA certificate"));
        let valid =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-ca.pem");
        assert!(ForgejoClient::with_ca_file(None, &valid).is_ok());
    }

    #[test]
    fn host_and_environment_ca_files_are_both_loaded() {
        let valid =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-ca.pem");
        let invalid = std::env::current_exe().expect("test executable");
        for (host_ca, env_ca) in [(&invalid, &valid), (&valid, &invalid)] {
            let config = configured_host(None, Some(host_ca.clone()));
            let env = crate::config::EnvMap::from([(
                "FJ_SHIM_CA_FILE".to_string(),
                env_ca.to_string_lossy().to_string(),
            )]);
            let error = ForgejoClient::for_host("git.example.com", None, &config, &env)
                .err()
                .expect("invalid CA must fail");
            assert!(error.to_string().contains("invalid CA certificate"));
        }
    }

    #[test]
    fn environment_ca_file_must_be_absolute_and_empty_means_unset() {
        let config = configured_host(None, None);
        let env = |value: &str| {
            crate::config::EnvMap::from([("FJ_SHIM_CA_FILE".to_string(), value.to_string())])
        };
        let error = ForgejoClient::for_host("git.example.com", None, &config, &env("ca.pem"))
            .err()
            .expect("relative CA path must fail");
        assert!(error.to_string().contains("absolute path"));
        assert!(ForgejoClient::for_host("git.example.com", None, &config, &env("")).is_ok());
    }

    #[test]
    fn configured_http_base_applies_to_user_and_repository_endpoints() -> TestResult {
        let (host, handle) = start_fake_server(vec![
            FakeResponse::json(r#"{"login":"test"}"#),
            FakeResponse::json(r#"{"name":"repo"}"#),
        ])?;
        let config = configured_host(Some(format!("http://{host}/forgejo")), None);
        let client = ForgejoClient::for_host(
            "git.example.com",
            Some("host-token".to_string()),
            &config,
            &crate::config::EnvMap::new(),
        )?;
        assert_eq!(client.get_current_user("git.example.com")?["login"], "test");
        assert_eq!(
            client.get_repo(&RepoRef::new("git.example.com", "own er", "r+e"))?["name"],
            "repo"
        );
        let requests = finish_fake_server(handle)?;
        assert_eq!(
            requests[0].request_line,
            "GET /forgejo/api/v1/user HTTP/1.1"
        );
        assert_eq!(
            requests[1].request_line,
            "GET /forgejo/api/v1/repos/own%20er/r%2Be HTTP/1.1"
        );
        for request in requests {
            assert_eq!(
                request.headers.get("authorization").map(String::as_str),
                Some("token host-token")
            );
        }
        Ok(())
    }

    fn configured_host(
        api_url: Option<String>,
        ca_file: Option<std::path::PathBuf>,
    ) -> crate::config::Config {
        crate::config::Config {
            hosts: vec!["git.example.com".to_string()],
            host: BTreeMap::from([(
                "git.example.com".to_string(),
                crate::config::HostConfig {
                    api_url,
                    ca_file,
                    token_env: None,
                    ..Default::default()
                },
            )]),
            paths: crate::config::PathsConfig { gh: None, fj: None },
            path: std::path::PathBuf::new(),
        }
    }

    #[test]
    fn private_ca_enables_tls_without_disabling_hostname_checks() -> TestResult {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        use std::sync::Arc;

        fn serve() -> io::Result<(u16, JoinHandle<()>)> {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let port = listener.local_addr()?.port();
            let cert =
                CertificateDer::from(include_bytes!("../tests/fixtures/test-server.der").to_vec());
            let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                include_bytes!("../tests/fixtures/test-server-key.der").to_vec(),
            ));
            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .map_err(io::Error::other)?;
            let handle = thread::spawn(move || {
                let (socket, _) = listener.accept().expect("accept");
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("timeout");
                let connection =
                    rustls::ServerConnection::new(Arc::new(config)).expect("connection");
                let mut stream = rustls::StreamOwned::new(connection, socket);
                if read_request(&mut stream).is_ok() {
                    let _ = write_response(&mut stream, &FakeResponse::json(r#"{"login":"test"}"#));
                }
            });
            Ok((port, handle))
        }
        let ca =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-ca.pem");
        let (port, handle) = serve()?;
        assert!(ForgejoClient::new(None)
            .get_current_user(&format!("localhost:{port}"))
            .is_err());
        handle.join().expect("server");
        let (port, handle) = serve()?;
        let client = ForgejoClient::with_ca_file(None, &ca)?;
        assert_eq!(
            client.get_current_user(&format!("localhost:{port}"))?["login"],
            "test"
        );
        handle.join().expect("server");
        let (port, handle) = serve()?;
        assert!(client
            .get_current_user(&format!("127.0.0.1:{port}"))
            .is_err());
        handle.join().expect("server");
        for (host_ca, env_ca) in [(true, false), (false, true), (true, true)] {
            let (port, handle) = serve()?;
            let config = configured_host(
                Some(format!("https://localhost:{port}")),
                host_ca.then(|| ca.clone()),
            );
            let env = if env_ca {
                crate::config::EnvMap::from([(
                    "FJ_SHIM_CA_FILE".to_string(),
                    ca.to_string_lossy().to_string(),
                )])
            } else {
                crate::config::EnvMap::new()
            };
            let client = ForgejoClient::for_host("git.example.com", None, &config, &env)?;
            assert_eq!(client.get_current_user("git.example.com")?["login"], "test");
            handle.join().expect("server");
        }
        Ok(())
    }

    #[test]
    fn repo_ref_builds_python_compatible_urls() {
        let repo = RepoRef::new("git.example.com", "own er", "r/e");

        assert_eq!(repo.web_base_url(), "https://git.example.com/own er/r/e");
        assert_eq!(
            repo.api_base_url(),
            "https://git.example.com/api/v1/repos/own%20er/r%2Fe"
        );
    }

    /// A segment a URL parser or proxy could resolve out of the repository's path is refused
    /// before any request, by the gateway's rule: dot segments, separators, and either escaped
    /// up to three times. The forge's listener must have no connection waiting afterwards.
    #[test]
    fn climbing_segments_are_refused_before_any_request() -> TestResult {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let config = configured_host(
            Some(format!("http://{}/forgejo", listener.local_addr()?)),
            None,
        );
        let client = ForgejoClient::for_host(
            "git.example.com",
            Some("host-token".to_string()),
            &config,
            &crate::config::EnvMap::new(),
        )?;
        for (owner, repo) in [
            ("..", ".."),
            (".", "r"),
            ("o", "."),
            ("o", ".."),
            ("%2e%2e", "r"),
            ("o", "%2E%2e"),
            ("%252e%252e", "r"),
            ("o", "%25252e%25252e"),
            ("o/..", "r"),
            ("o", "r%2F..%2F.."),
            ("o", "r%252Fx"),
            ("o", "..\\x"),
            ("o", "%5C"),
            ("o", ".\t."),
        ] {
            let error = client
                .get_repo(&RepoRef::new("git.example.com", owner, repo))
                .err()
                .ok_or_else(|| format!("{owner}/{repo} was sent"))?;
            assert!(
                error
                    .message()
                    .contains("can't be part of a Forgejo API path"),
                "{owner}/{repo}: {error}"
            );
        }
        let error = client
            .list_check_statuses(&RepoRef::new("git.example.com", "o", "r"), "..")
            .err()
            .ok_or("a `..` commit was sent")?;
        assert!(error.message().contains("\"..\" can't be part"), "{error}");

        match listener.accept() {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
            other => Err(format!("a request reached the forge: {other:?}").into()),
        }
    }

    /// Dots inside a name are ordinary: Forgejo has `.profile` repositories.
    #[test]
    fn dotted_names_are_sent_as_written() -> TestResult {
        let (host, handle) = start_fake_server(vec![FakeResponse::json(r#"{"name":".profile"}"#)])?;
        let client = ForgejoClient::new(None).with_scheme("http");

        client.get_repo(&RepoRef::new(host, "o.rg-1_", ".profile"))?;
        let requests = finish_fake_server(handle)?;

        assert_eq!(
            requests[0].request_line,
            "GET /api/v1/repos/o.rg-1_/.profile HTTP/1.1"
        );
        Ok(())
    }

    #[test]
    fn current_user_preserves_headers_and_strips_scheme_from_host() -> TestResult {
        let (host, handle) = start_fake_server(vec![FakeResponse::json(
            r#"{"login":"alice","full_name":"Alice"}"#,
        )])?;
        let client = ForgejoClient::with_token("secret-token").with_scheme("http");

        let user = client.get_current_user(&format!("https://{host}/extra/path"))?;
        let requests = finish_fake_server(handle)?;

        assert_eq!(user["login"], "alice");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].request_line, "GET /api/v1/user HTTP/1.1");
        assert_eq!(
            requests[0].headers.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            requests[0].headers.get("authorization").map(String::as_str),
            Some("token secret-token")
        );
        assert_eq!(
            requests[0].headers.get("user-agent").map(String::as_str),
            Some(USER_AGENT_VALUE)
        );
        Ok(())
    }

    #[test]
    fn create_pull_posts_python_compatible_json() -> TestResult {
        let (host, handle) = start_fake_server(vec![FakeResponse::json(
            r#"{"number":7,"title":"Ship it"}"#,
        )])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");

        let pull = client.create_pull(
            &repo,
            &CreatePullRequest::new("Ship it", "body", "main", "feature").draft(true),
        )?;
        let requests = finish_fake_server(handle)?;

        assert_eq!(pull["number"], 7);
        assert_eq!(
            requests[0].request_line,
            "POST /api/v1/repos/owner/repo/pulls HTTP/1.1"
        );
        assert_eq!(
            requests[0].headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        let body: Value = serde_json::from_str(&requests[0].body)?;
        assert_eq!(
            body,
            json!({
                "title": "Ship it",
                "body": "body",
                "base": "main",
                "head": "feature",
                "draft": true,
            })
        );
        Ok(())
    }

    #[test]
    fn list_issues_uses_forgejo_query_names_and_filters_non_objects() -> TestResult {
        let (host, handle) = start_fake_server(vec![FakeResponse::json(
            r#"[{"number":1}, [], {"number":2}]"#,
        )])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");
        let options = ListIssuesOptions {
            state: Some("closed".to_string()),
            labels: vec!["bug".to_string(), "help wanted".to_string()],
            query: Some("needs work".to_string()),
            milestone: Some("v1".to_string()),
            author: Some("alice".to_string()),
            assignee: Some("@me".to_string()),
            mentioned: Some("bob".to_string()),
            limit: Some(10),
        };

        let issues = client.list_issues(&repo, &options)?;
        let requests = finish_fake_server(handle)?;

        assert_eq!(issues, vec![json!({"number": 1}), json!({"number": 2})]);
        assert_eq!(
            requests[0].request_line,
            "GET /api/v1/repos/owner/repo/issues?state=closed&type=issues&labels=bug%2Chelp+wanted&q=needs+work&milestones=v1&created_by=alice&mentioned_by=bob&limit=10 HTTP/1.1"
        );
        Ok(())
    }

    #[test]
    fn list_pulls_filters_head_by_ref_or_label_suffix() -> TestResult {
        let (host, handle) = start_fake_server(vec![FakeResponse::json(
            r#"[
                {"number":1,"head":{"ref":"feature"}},
                {"number":2,"head":{"label":"alice:feature"}},
                {"number":3,"head":{"ref":"other"}}
            ]"#,
        )])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");

        let pulls = client.list_pulls(&repo, "open", Some("feature"))?;
        let requests = finish_fake_server(handle)?;

        let numbers = pulls
            .iter()
            .filter_map(|pull| pull.get("number").and_then(Value::as_i64))
            .collect::<Vec<_>>();
        assert_eq!(numbers, vec![1, 2]);
        assert_eq!(
            requests[0].request_line,
            "GET /api/v1/repos/owner/repo/pulls?state=open HTTP/1.1"
        );
        Ok(())
    }

    #[test]
    fn check_status_history_reads_past_short_pages_and_uses_status_field() -> TestResult {
        let (host, handle) = start_fake_server(vec![
            FakeResponse::json(
                r#"[{"id":2,"context":"ci","status":"success","target_url":"","description":null,"created_at":"2026-06-01T00:00:00Z","updated_at":"2026-06-01T00:00:01Z"}]"#,
            ),
            FakeResponse::json(
                r#"[{"id":1,"context":"other","status":"pending","target_url":"","description":null,"created_at":"2026-06-01T00:00:00Z","updated_at":"2026-06-01T00:00:00Z"}]"#,
            ),
            FakeResponse::json("[]"),
        ])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");
        let statuses = client.list_check_statuses(&repo, "abc 123")?;
        let requests = finish_fake_server(handle)?;
        assert_eq!(statuses.iter().map(|s| s.id).collect::<Vec<_>>(), [2, 1]);
        assert!(matches!(
            statuses[1].state,
            fga_core::forgejo::CommitStatusState::Pending
        ));
        for (index, request) in requests.iter().enumerate() {
            assert_eq!(request.request_line, format!("GET /api/v1/repos/owner/repo/commits/abc%20123/statuses?page={}&limit=50 HTTP/1.1", index + 1));
        }
        Ok(())
    }

    #[test]
    fn check_statuses_reject_invalid_dtos_instead_of_reporting_no_checks() -> TestResult {
        let (host, handle) = start_fake_server(vec![FakeResponse::json(
            r#"[{"context":"ci","state":"success"}]"#,
        )])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");
        let error = client
            .list_check_statuses(&repo, "abc")
            .expect_err("invalid DTO");
        finish_fake_server(handle)?;
        assert!(error.message().contains("invalid Forgejo response"));
        Ok(())
    }

    #[test]
    fn branch_protection_reads_once_and_selects_the_matching_glob() -> TestResult {
        // Forgejo 15 returns the full rule list for any `page`, so a paginated read
        // would never see an empty page.
        let (host, handle) = start_fake_server(vec![FakeResponse::json(
            r#"[{"rule_name":"other","enable_status_check":true},{"rule_name":"release/*","enable_status_check":true,"status_check_contexts":["ci/*"]}]"#,
        )])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");
        let protection = client
            .get_branch_protection(&repo, "release/next")?
            .expect("matched rule");
        let requests = finish_fake_server(handle)?;
        assert_eq!(protection.status_check_contexts, ["ci/*"]);
        assert!(protection.enable_status_check);
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].request_line,
            "GET /api/v1/repos/owner/repo/branch_protections HTTP/1.1"
        );
        Ok(())
    }

    #[test]
    fn paginated_reads_stop_at_the_page_cap() -> TestResult {
        let page = r#"[{"id":1,"context":"ci","status":"success","target_url":"","description":null,"created_at":"2026-06-01T00:00:00Z","updated_at":"2026-06-01T00:00:00Z"}]"#;
        let (host, handle) = start_fake_server(vec![
            FakeResponse::json(page),
            FakeResponse::json(page),
            FakeResponse::json(page),
        ])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");
        let url = format!("{}/commits/abc/statuses", client.repo_api_base_url(&repo)?);
        let error = client
            .get_pages_up_to::<CommitStatus>(url, 3)
            .expect_err("page cap");
        let requests = finish_fake_server(handle)?;
        assert_eq!(requests.len(), 3);
        assert!(error.message().contains("more than 3 pages"));
        Ok(())
    }

    #[test]
    fn statuses_accept_forgejo_statuses_envelope() -> TestResult {
        let (host, handle) = start_fake_server(vec![FakeResponse::json(
            r#"{"statuses":[{"context":"ci"}, "bad"]}"#,
        )])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");

        let statuses = client.list_commit_statuses(&repo, "abc 123")?;
        let requests = finish_fake_server(handle)?;

        assert_eq!(statuses, vec![json!({"context": "ci"})]);
        assert_eq!(
            requests[0].request_line,
            "GET /api/v1/repos/owner/repo/statuses/abc%20123 HTTP/1.1"
        );
        Ok(())
    }

    #[test]
    fn text_requests_use_text_accept_header() -> TestResult {
        let (host, handle) = start_fake_server(vec![FakeResponse::text("diff --git\n")])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");

        let diff = client.get_pull_diff(&repo, 12, false)?;
        let requests = finish_fake_server(handle)?;

        assert_eq!(diff, "diff --git\n");
        assert_eq!(
            requests[0].request_line,
            "GET /api/v1/repos/owner/repo/pulls/12.diff HTTP/1.1"
        );
        assert_eq!(
            requests[0].headers.get("accept").map(String::as_str),
            Some("text/plain")
        );
        Ok(())
    }

    #[test]
    fn http_errors_use_python_compatible_prefix() -> TestResult {
        let (host, handle) =
            start_fake_server(vec![FakeResponse::status(418, "Teapot", "short and stout")])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");

        let error = client
            .get_repo(&repo)
            .err()
            .ok_or_else(|| io::Error::other("expected Forgejo HTTP error from fake server"))?;
        let _requests = finish_fake_server(handle)?;

        assert_eq!(
            error.to_string(),
            "Forgejo API returned HTTP 418: short and stout"
        );
        Ok(())
    }

    #[test]
    fn invalid_json_uses_python_compatible_message() -> TestResult {
        let (host, handle) = start_fake_server(vec![FakeResponse::json("not-json")])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");

        let error = client
            .get_repo(&repo)
            .err()
            .ok_or_else(|| io::Error::other("expected invalid JSON error from fake server"))?;
        let _requests = finish_fake_server(handle)?;

        assert_eq!(error.to_string(), "Forgejo API returned invalid JSON");
        Ok(())
    }

    /// Forgejo 15's answers to API calls for a renamed repository (301) and a renamed owner
    /// (307), behind a proxy that serves the forge under `/forgejo`.
    #[test]
    fn a_renamed_repository_or_owner_redirect_is_followed_within_the_api() -> TestResult {
        let (host, seen) = start_recording_server(|path| match path {
            "/forgejo/api/v1/repos/old-owner/old.repo/issues/1" => (
                "307 Temporary Redirect",
                Some("/forgejo/api/v1/repos/new-owner/old.repo/issues/1".to_string()),
            ),
            "/forgejo/api/v1/repos/new-owner/old.repo/issues/1" => (
                "301 Moved Permanently",
                Some("/forgejo/api/v1/repos/new-owner/new_repo/issues/1".to_string()),
            ),
            "/forgejo/api/v1/repos/o/old/pulls?state=open" => (
                "301 Moved Permanently",
                Some("/forgejo/api/v1/repos/o/new/pulls?state=open".to_string()),
            ),
            _ => ("200 OK", None),
        })?;
        let client = subpath_client(&host)?;

        client.get_issue(&RepoRef::new("git.example.com", "old-owner", "old.repo"), 1)?;
        client.list_pulls(&RepoRef::new("git.example.com", "o", "old"), "open", None)?;

        assert_eq!(
            paths(&seen),
            [
                "/forgejo/api/v1/repos/old-owner/old.repo/issues/1",
                "/forgejo/api/v1/repos/new-owner/old.repo/issues/1",
                "/forgejo/api/v1/repos/new-owner/new_repo/issues/1",
                "/forgejo/api/v1/repos/o/old/pulls?state=open",
                "/forgejo/api/v1/repos/o/new/pulls?state=open",
            ]
        );
        Ok(())
    }

    /// Any other redirect is the answer, so the token makes no second request: not to another
    /// service or API route on the origin, nor out of the base path or to another origin.
    #[test]
    fn no_other_redirect_is_followed() -> TestResult {
        let (other, other_seen) = start_recording_server(|_| ("200 OK", None))?;
        let locations = [
            "/other-service/capture".to_string(),
            "/forgejo/api/v1/user".to_string(),
            "/api/v1/repos/o/new".to_string(),
            "/forgejo/api/v1/repos/o/new/../../../../other".to_string(),
            "/forgejo/api/v1/repos/o/%2e%2e".to_string(),
            "/forgejo/api/v1/repos/o/new%2F..%2F..%2Fother".to_string(),
            "/forgejo/api/v1/repos/o/new/extra".to_string(),
            "/forgejo/api/v1/repos/o/new?state=all".to_string(),
            format!("http://{other}/forgejo/api/v1/repos/o/new"),
        ];
        for status in [
            "301 Moved Permanently",
            "302 Found",
            "307 Temporary Redirect",
            "308 Permanent Redirect",
        ] {
            for location in &locations {
                let target = location.clone();
                let (host, seen) = start_recording_server(move |path| {
                    if path == "/forgejo/api/v1/repos/o/r" {
                        (status, Some(target.clone()))
                    } else {
                        ("200 OK", None)
                    }
                })?;
                let error = subpath_client(&host)?
                    .get_repo(&RepoRef::new("git.example.com", "o", "r"))
                    .err()
                    .ok_or_else(|| format!("{status} to {location} was followed"))?;

                assert!(
                    error.message().contains("that the shim didn't follow"),
                    "{status} to {location}: {error}"
                );
                assert_eq!(
                    paths(&seen),
                    ["/forgejo/api/v1/repos/o/r"],
                    "{status} to {location}"
                );
            }
        }
        assert_eq!(paths(&other_seen), Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn rename_redirects_stop_after_ten() -> TestResult {
        let (host, seen) = start_recording_server(|path| {
            let next = path
                .strip_prefix("/forgejo/api/v1/repos/o/r")
                .and_then(|n| n.parse::<u32>().ok())
                .map(|n| format!("/forgejo/api/v1/repos/o/r{}", n + 1));
            ("301 Moved Permanently", next)
        })?;

        let result = subpath_client(&host)?.get_repo(&RepoRef::new("git.example.com", "o", "r0"));

        assert!(result.is_err());
        assert_eq!(paths(&seen).len(), 11);
        Ok(())
    }

    /// Absolute Locations naming the forge's own host: a rename there is followed, but not one
    /// that changes the scheme or carries userinfo.
    #[test]
    fn absolute_locations_are_judged_by_origin_and_userinfo() -> TestResult {
        for (followed, location) in [
            (true, "http://{own}/forgejo/api/v1/repos/o/new"),
            (false, "https://{own}/forgejo/api/v1/repos/o/new"),
            (false, "http://alice:pw@{own}/forgejo/api/v1/repos/o/new"),
            (false, "http://alice@{own}/forgejo/api/v1/repos/o/new"),
        ] {
            let own = Arc::new(std::sync::OnceLock::<String>::new());
            let address = Arc::clone(&own);
            let (host, seen) = start_recording_server(move |path| {
                if path == "/forgejo/api/v1/repos/o/r" {
                    let target = location.replace("{own}", address.get().expect("address"));
                    ("301 Moved Permanently", Some(target))
                } else {
                    ("200 OK", None)
                }
            })?;
            own.set(host.clone()).map_err(|_| "address set twice")?;

            let result =
                subpath_client(&host)?.get_repo(&RepoRef::new("git.example.com", "o", "r"));

            if followed {
                result?;
                assert_eq!(
                    paths(&seen),
                    ["/forgejo/api/v1/repos/o/r", "/forgejo/api/v1/repos/o/new"]
                );
            } else {
                let error = result
                    .err()
                    .ok_or_else(|| format!("{location} was followed"))?;
                assert!(
                    error.message().contains("that the shim didn't follow"),
                    "{location}: {error}"
                );
                assert_eq!(paths(&seen), ["/forgejo/api/v1/repos/o/r"], "{location}");
            }
        }
        Ok(())
    }

    /// `doctor`'s probes follow no redirect at all, a rename's included: the answer is the 301.
    #[test]
    fn the_doctor_probe_client_follows_no_redirect() -> TestResult {
        let (host, seen) = start_recording_server(|path| {
            if path == "/forgejo/api/v1/repos/o/old" {
                (
                    "301 Moved Permanently",
                    Some("/forgejo/api/v1/repos/o/new".to_string()),
                )
            } else {
                ("200 OK", None)
            }
        })?;
        let client = ForgejoClient::for_host_without_redirects(
            "git.example.com",
            Some("secret-token".to_string()),
            &configured_host(Some(format!("http://{host}/forgejo")), None),
            &crate::config::EnvMap::new(),
        )?;

        let (status, _) = client.probe("git.example.com", Method::GET, "/api/v1/repos/o/old")?;

        assert_eq!(status, 301);
        assert_eq!(paths(&seen), ["/forgejo/api/v1/repos/o/old"]);
        Ok(())
    }

    /// A base path keeps its empty segments: requests go to `<base>/api/v1/...` as written.
    #[test]
    fn a_rename_is_followed_below_a_base_path_with_an_empty_segment() -> TestResult {
        let (host, seen) = start_recording_server(|path| match path {
            "/forgejo//nested/api/v1/repos/o/old" => (
                "301 Moved Permanently",
                Some("/forgejo//nested/api/v1/repos/o/new".to_string()),
            ),
            _ => ("200 OK", None),
        })?;
        let client = ForgejoClient::for_host(
            "git.example.com",
            Some("secret-token".to_string()),
            &configured_host(Some(format!("http://{host}/forgejo//nested")), None),
            &crate::config::EnvMap::new(),
        )?;

        client.get_repo(&RepoRef::new("git.example.com", "o", "old"))?;

        assert_eq!(
            paths(&seen),
            [
                "/forgejo//nested/api/v1/repos/o/old",
                "/forgejo//nested/api/v1/repos/o/new"
            ]
        );
        Ok(())
    }

    /// The error names only the origin a refused redirect pointed to: its userinfo, path, query
    /// and fragment may each carry a secret.
    #[test]
    fn a_refused_redirect_names_only_its_origin() -> TestResult {
        let (host, _seen) = start_recording_server(|path| {
            if path == "/forgejo/api/v1/repos/o/r" {
                (
                    "302 Found",
                    Some(
                        "http://alice:PRIVATE-PASSWORD@127.0.0.1:9/sso/continue/PATH-SECRET?access_token=SYNTHETIC#frag"
                            .to_string(),
                    ),
                )
            } else {
                ("200 OK", None)
            }
        })?;

        let error = subpath_client(&host)?
            .get_repo(&RepoRef::new("git.example.com", "o", "r"))
            .err()
            .ok_or("the redirect was followed")?;

        let message = error.message();
        assert!(
            message.contains("with a redirect to http://127.0.0.1:9 that"),
            "{message}"
        );
        for secret in [
            "alice",
            "PRIVATE-PASSWORD",
            "sso",
            "PATH-SECRET",
            "SYNTHETIC",
            "frag",
        ] {
            assert!(!message.contains(secret), "{message}");
        }
        Ok(())
    }

    /// `auth login` checks the token with `/user` through the same client.
    #[test]
    fn the_login_token_check_follows_no_redirect_off_the_api() -> TestResult {
        for status in ["302 Found", "307 Temporary Redirect"] {
            let (host, seen) = start_recording_server(move |path| {
                if path == "/forgejo/api/v1/user" {
                    (status, Some("/other-service/capture".to_string()))
                } else {
                    ("200 OK", None)
                }
            })?;
            let config = configured_host(Some(format!("http://{host}/forgejo")), None);

            let error = crate::auth::validate_token_with(
                "git.example.com",
                "secret-token",
                &config,
                &crate::config::EnvMap::new(),
            )
            .err()
            .ok_or_else(|| format!("{status} was followed"))?;

            assert!(
                error.to_string().contains("that the shim didn't follow"),
                "{status}: {error}"
            );
            assert_eq!(paths(&seen), ["/forgejo/api/v1/user"], "{status}");
        }
        Ok(())
    }

    /// A client for `git.example.com`, a forge served under `/forgejo` at `host`.
    fn subpath_client(host: &str) -> ForgejoResult<ForgejoClient> {
        ForgejoClient::for_host(
            "git.example.com",
            Some("secret-token".to_string()),
            &configured_host(Some(format!("http://{host}/forgejo")), None),
            &crate::config::EnvMap::new(),
        )
    }

    /// The path and query of each request `seen` holds, in order.
    fn paths(seen: &Mutex<Vec<CapturedRequest>>) -> Vec<String> {
        seen.lock()
            .expect("request log")
            .iter()
            .map(|request| {
                let path = request.request_line.split(' ').nth(1).unwrap_or_default();
                assert_eq!(
                    request.headers.get("authorization").map(String::as_str),
                    Some("token secret-token"),
                    "{path}"
                );
                path.to_string()
            })
            .collect()
    }

    /// A server answering each request with `respond(path)`, a status line and the `Location` to
    /// send if any, and a JSON body, for as long as the test runs. It keeps every request it reads
    /// before it answers, so a client call that returns has been recorded.
    fn start_recording_server(
        respond: impl Fn(&str) -> (&'static str, Option<String>) + Send + 'static,
    ) -> io::Result<(String, Arc<Mutex<Vec<CapturedRequest>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let host = listener.local_addr()?.to_string();
        let seen = Arc::<Mutex<Vec<CapturedRequest>>>::default();
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let Ok(request) = read_request(&mut stream) else {
                    continue;
                };
                let path = request.request_line.split(' ').nth(1).unwrap_or_default();
                let (status, location) = respond(path);
                log.lock().expect("request log").push(request);
                let location = location
                    .map(|location| format!("location: {location}\r\n"))
                    .unwrap_or_default();
                let body = r#"{"login":"alice","name":"r","full_name":"o/r"}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{location}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Ok((host, seen))
    }

    fn start_fake_server(
        responses: Vec<FakeResponse>,
    ) -> io::Result<(String, JoinHandle<io::Result<Vec<CapturedRequest>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let host = listener.local_addr()?.to_string();
        let handle = thread::spawn(move || {
            let mut captured = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept()?;
                let request = read_request(&mut stream)?;
                write_response(&mut stream, &response)?;
                captured.push(request);
            }
            Ok(captured)
        });
        Ok((host, handle))
    }

    fn finish_fake_server(
        handle: JoinHandle<io::Result<Vec<CapturedRequest>>>,
    ) -> io::Result<Vec<CapturedRequest>> {
        handle
            .join()
            .map_err(|_| io::Error::other("fake server thread panicked"))?
    }

    fn read_request(stream: &mut impl Read) -> io::Result<CapturedRequest> {
        let mut bytes = Vec::new();
        let mut scratch = [0_u8; 512];
        let header_end = loop {
            let count = stream.read(&mut scratch)?;
            if count == 0 {
                break find_header_end(&bytes).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::UnexpectedEof, "request headers ended early")
                })?;
            }
            bytes.extend_from_slice(&scratch[..count]);
            if let Some(header_end) = find_header_end(&bytes) {
                break header_end;
            }
        };

        let headers_text = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
        let mut lines = headers_text.split("\r\n");
        let request_line = lines.next().unwrap_or_default().to_string();
        let mut headers = BTreeMap::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
            }
        }

        let content_length = headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default();
        let body_start = header_end + 4;
        while bytes.len() < body_start + content_length {
            let count = stream.read(&mut scratch)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&scratch[..count]);
        }

        let body_end = bytes.len().min(body_start + content_length);
        let body = String::from_utf8_lossy(&bytes[body_start..body_end]).into_owned();
        Ok(CapturedRequest {
            request_line,
            headers,
            body,
        })
    }

    fn write_response(stream: &mut impl Write, response: &FakeResponse) -> io::Result<()> {
        write!(
            stream,
            "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            response.status,
            response.reason,
            response.content_type,
            response.body.len(),
            response.body
        )
    }

    fn find_header_end(bytes: &[u8]) -> Option<usize> {
        bytes.windows(4).position(|window| window == b"\r\n\r\n")
    }
}
