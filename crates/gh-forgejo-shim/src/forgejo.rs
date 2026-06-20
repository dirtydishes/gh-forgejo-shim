use std::fmt;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::Method;
use serde_json::{json, Value};

use crate::ShimError;

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
    http: Client,
}

impl ForgejoClient {
    pub fn new(token: Option<String>) -> Self {
        Self {
            token,
            timeout: Duration::from_secs(30),
            scheme: "https".to_string(),
            http: Client::new(),
        }
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

    pub fn get_current_user(&self, host: &str) -> ForgejoResult<Value> {
        self.request_json(
            Method::GET,
            format!("{}://{}/api/v1/user", self.scheme, host_for_url(host)),
            None,
        )
    }

    pub fn create_pull(&self, repo: &RepoRef, request: &CreatePullRequest) -> ForgejoResult<Value> {
        self.request_json(
            Method::POST,
            format!("{}/pulls", self.repo_api_base_url(repo)),
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
            format!("{}/pulls/{number}", self.repo_api_base_url(repo)),
            None,
        )
    }

    pub fn get_pull_diff(&self, repo: &RepoRef, number: u64, patch: bool) -> ForgejoResult<String> {
        let diff_type = if patch { "patch" } else { "diff" };
        self.request_text(
            Method::GET,
            format!(
                "{}/pulls/{number}.{diff_type}",
                self.repo_api_base_url(repo)
            ),
            None,
            "text/plain",
        )
    }

    pub fn list_pull_files(&self, repo: &RepoRef, number: u64) -> ForgejoResult<Vec<Value>> {
        let files = self.request_json(
            Method::GET,
            format!("{}/pulls/{number}/files", self.repo_api_base_url(repo)),
            None,
        )?;
        Ok(list_of_objects(files))
    }

    pub fn list_commit_statuses(&self, repo: &RepoRef, sha: &str) -> ForgejoResult<Vec<Value>> {
        let statuses = self.request_json(
            Method::GET,
            format!(
                "{}/statuses/{}",
                self.repo_api_base_url(repo),
                quote_path_segment(sha)
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

    pub fn get_repo(&self, repo: &RepoRef) -> ForgejoResult<Value> {
        self.request_json(Method::GET, self.repo_api_base_url(repo), None)
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
                self.repo_api_base_url(repo),
                form_urlencode(&params)
            ),
            None,
        )?;
        Ok(list_of_objects(issues))
    }

    pub fn get_issue(&self, repo: &RepoRef, number: u64) -> ForgejoResult<Value> {
        self.request_json(
            Method::GET,
            format!("{}/issues/{number}", self.repo_api_base_url(repo)),
            None,
        )
    }

    pub fn list_labels(&self, repo: &RepoRef) -> ForgejoResult<Vec<Value>> {
        let labels = self.request_json(
            Method::GET,
            format!("{}/labels", self.repo_api_base_url(repo)),
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
            format!("{}/issues", self.repo_api_base_url(repo)),
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
            format!("{}/issues/{number}/comments", self.repo_api_base_url(repo)),
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
                self.repo_api_base_url(repo),
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

    fn repo_api_base_url(&self, repo: &RepoRef) -> String {
        repo.api_base_url_with_scheme(&self.scheme)
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
    fn repo_ref_builds_python_compatible_urls() {
        let repo = RepoRef::new("git.example.com", "own er", "r/e");

        assert_eq!(repo.web_base_url(), "https://git.example.com/own er/r/e");
        assert_eq!(
            repo.api_base_url(),
            "https://git.example.com/api/v1/repos/own%20er/r%2Fe"
        );
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
    fn statuses_accept_forgejo_statuses_envelope() -> TestResult {
        let (host, handle) = start_fake_server(vec![FakeResponse::json(
            r#"{"statuses":[{"context":"ci"}, "bad"]}"#,
        )])?;
        let client = ForgejoClient::new(None).with_scheme("http");
        let repo = RepoRef::new(host, "owner", "repo");

        let statuses = client.list_commit_statuses(&repo, "abc/123")?;
        let requests = finish_fake_server(handle)?;

        assert_eq!(statuses, vec![json!({"context": "ci"})]);
        assert_eq!(
            requests[0].request_line,
            "GET /api/v1/repos/owner/repo/statuses/abc%2F123 HTTP/1.1"
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
