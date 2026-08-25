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

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use serde_json::json;

    use super::*;

    struct FakeSource {
        login: String,
        pages: Vec<Vec<Value>>,
        fail_on_page: Option<usize>,
        current_user_calls: Cell<usize>,
        requests: RefCell<Vec<(String, usize, usize)>>,
    }

    impl FakeSource {
        fn new(pages: Vec<Vec<Value>>) -> Self {
            Self {
                login: "alice".to_string(),
                pages,
                fail_on_page: None,
                current_user_calls: Cell::new(0),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn failing_on(mut self, page: usize) -> Self {
            self.fail_on_page = Some(page);
            self
        }
    }

    impl PullRequestSource for FakeSource {
        fn current_user_login(&self, _host: &str) -> ForgejoResult<String> {
            self.current_user_calls
                .set(self.current_user_calls.get() + 1);
            Ok(self.login.clone())
        }

        fn pull_request_page(
            &self,
            _repo: &RepoRef,
            state: &str,
            page: usize,
            page_size: usize,
        ) -> ForgejoResult<Vec<Value>> {
            self.requests
                .borrow_mut()
                .push((state.to_string(), page, page_size));
            if self.fail_on_page == Some(page) {
                return Err(ForgejoError::new("simulated deadline exceeded"));
            }
            Ok(self.pages.get(page - 1).cloned().unwrap_or_default())
        }
    }

    fn repo() -> RepoRef {
        RepoRef::new("git.example.com", "owner", "repo")
    }

    fn query(author: Option<&'static str>, limit: Option<usize>) -> PullRequestQuery<'static> {
        PullRequestQuery {
            state: "all",
            head: Some("main"),
            author,
            base: None,
            draft_only: false,
            limit,
        }
    }

    fn pull(number: usize, login: &str, head: &str) -> Value {
        json!({
            "number": number,
            "head": {"ref": head},
            "base": {"ref": "main"},
            "user": {"login": login, "full_name": "Alice Example"}
        })
    }

    fn page(start: usize, count: usize, login: &str) -> Vec<Value> {
        (start..start + count)
            .map(|number| pull(number, login, "main"))
            .collect()
    }

    #[test]
    fn finds_a_filtered_match_on_page_two() {
        let source = FakeSource::new(vec![page(1, PAGE_SIZE, "bob"), page(51, 1, "alice")]);

        let pulls = discover(&source, &repo(), &query(Some("@me"), None)).unwrap();

        assert_eq!(pulls, vec![pull(51, "alice", "main")]);
        assert_eq!(source.current_user_calls.get(), 1);
        assert_eq!(
            *source.requests.borrow(),
            vec![("all".to_string(), 1, 50), ("all".to_string(), 2, 50)]
        );
    }

    #[test]
    fn stops_after_an_empty_terminal_page() {
        let source = FakeSource::new(vec![page(1, PAGE_SIZE, "bob"), Vec::new()]);

        let pulls = discover(&source, &repo(), &query(Some("@me"), None)).unwrap();

        assert!(pulls.is_empty());
        assert_eq!(source.requests.borrow().len(), 2);
    }

    #[test]
    fn applies_the_real_gh_default_limit_of_thirty() {
        let source = FakeSource::new(vec![page(1, PAGE_SIZE, "alice")]);

        let pulls = discover(&source, &repo(), &query(Some("alice"), None)).unwrap();

        assert_eq!(pulls.len(), 30);
        assert_eq!(source.current_user_calls.get(), 0);
        assert_eq!(source.requests.borrow().len(), 1);
    }

    #[test]
    fn explicit_limit_can_reach_page_two() {
        let source = FakeSource::new(vec![page(1, PAGE_SIZE, "alice"), page(51, 1, "alice")]);

        let pulls = discover(&source, &repo(), &query(Some("alice"), Some(51))).unwrap();

        assert_eq!(pulls.len(), 51);
        assert_eq!(source.requests.borrow().len(), 2);
    }

    #[test]
    fn rejects_more_than_one_thousand_candidates_without_partial_data() {
        let mut pages = (0..20)
            .map(|page_number| page(page_number * PAGE_SIZE + 1, PAGE_SIZE, "bob"))
            .collect::<Vec<_>>();
        pages.push(page(1001, 1, "alice"));
        let source = FakeSource::new(pages);

        let error = discover(&source, &repo(), &query(Some("@me"), None)).unwrap_err();

        assert_eq!(
            error.message(),
            "Forgejo pull request discovery exceeded 1,000 candidates"
        );
        assert_eq!(source.requests.borrow().len(), 21);
    }

    #[test]
    fn returns_an_error_instead_of_partial_data_when_a_later_page_fails() {
        let source = FakeSource::new(vec![page(1, PAGE_SIZE, "bob")]).failing_on(2);

        let error = discover(&source, &repo(), &query(Some("@me"), None)).unwrap_err();

        assert_eq!(error.message(), "simulated deadline exceeded");
        assert_eq!(source.requests.borrow().len(), 2);
    }

    #[test]
    fn filters_head_refs_and_labels_without_using_display_names() {
        let source = FakeSource::new(vec![vec![
            pull(1, "alice", "main"),
            json!({
                "number": 2,
                "head": {"label": "alice:main"},
                "base": {"ref": "main"},
                "user": {"login": "alice"}
            }),
            pull(3, "bob", "main"),
            pull(4, "alice", "other"),
        ]]);

        let pulls = discover(&source, &repo(), &query(Some("@me"), Some(30))).unwrap();

        let numbers = pulls
            .iter()
            .filter_map(|pull| pull.get("number").and_then(Value::as_u64))
            .collect::<Vec<_>>();
        assert_eq!(numbers, vec![1, 2]);
    }
}
