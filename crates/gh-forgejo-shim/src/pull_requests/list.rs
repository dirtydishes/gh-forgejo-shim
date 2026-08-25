use serde_json::Value;

use crate::forgejo::{ForgejoClient, ForgejoError, ForgejoResult, RepoRef};
use crate::normalize::normalize_pull;

pub(crate) const PAGE_SIZE: usize = 50;

pub(crate) trait PullRequestSource {
    fn current_user_login(&self, host: &str) -> ForgejoResult<String>;

    fn pull_request_page(
        &self,
        repo: &RepoRef,
        state: &str,
        page: usize,
        page_size: usize,
    ) -> ForgejoResult<Vec<Value>>;
}

impl PullRequestSource for ForgejoClient {
    fn current_user_login(&self, host: &str) -> ForgejoResult<String> {
        self.get_current_user(host)?
            .get("login")
            .and_then(Value::as_str)
            .filter(|login| !login.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| ForgejoError::new("Forgejo API user response has no login"))
    }

    fn pull_request_page(
        &self,
        repo: &RepoRef,
        state: &str,
        page: usize,
        page_size: usize,
    ) -> ForgejoResult<Vec<Value>> {
        self.list_pull_page(repo, state, page, page_size)
    }
}

pub(crate) struct PullRequestQuery<'a> {
    pub(crate) state: &'a str,
    pub(crate) head: Option<&'a str>,
    pub(crate) author: Option<&'a str>,
    pub(crate) base: Option<&'a str>,
    pub(crate) draft_only: bool,
    pub(crate) limit: Option<usize>,
}

pub(crate) fn discover<S: PullRequestSource + ?Sized>(
    source: &S,
    repo: &RepoRef,
    query: &PullRequestQuery<'_>,
) -> ForgejoResult<Vec<Value>> {
    let author = match query.author {
        Some("@me") => Some(source.current_user_login(&repo.host)?),
        Some(author) => Some(author.to_string()),
        None => None,
    };
    let api_state = if query.state == "merged" {
        "closed"
    } else {
        query.state
    };
    let mut pulls = source.pull_request_page(repo, api_state, 1, PAGE_SIZE)?;
    pulls.retain(|pull| matches_query(pull, query, author.as_deref()));
    if let Some(limit) = query.limit {
        pulls.truncate(limit);
    }
    Ok(pulls)
}

fn matches_query(pull: &Value, query: &PullRequestQuery<'_>, author: Option<&str>) -> bool {
    matches_head(pull, query.head)
        && matches_author(pull, author)
        && matches_base(pull, query.base)
        && (!query.draft_only || pull.get("draft").and_then(Value::as_bool) == Some(true))
        && (query.state != "merged"
            || normalize_pull(pull).get("state") == Some(&Value::String("MERGED".to_string())))
}

fn matches_head(pull: &Value, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    let Some(head) = pull.get("head").and_then(Value::as_object) else {
        return false;
    };
    let reference = head.get("ref").and_then(Value::as_str);
    let label = head.get("label").and_then(Value::as_str);
    reference == Some(expected)
        || label == Some(expected)
        || label.is_some_and(|value| value.ends_with(&format!(":{expected}")))
}

fn matches_author(pull: &Value, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    pull.get("user")
        .and_then(Value::as_object)
        .and_then(|user| user.get("login"))
        .and_then(Value::as_str)
        .is_some_and(|login| login.eq_ignore_ascii_case(expected))
}

fn matches_base(pull: &Value, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    pull.get("base")
        .and_then(Value::as_object)
        .and_then(|base| base.get("ref"))
        .and_then(Value::as_str)
        == Some(expected)
}
