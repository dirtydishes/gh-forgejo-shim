use reqwest::{Client, Method, StatusCode, header::AUTHORIZATION};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::OnceCell;
use url::Url;

mod cache;
mod reads;
mod types;
mod writes;
use cache::SessionCache;
pub use types::*;
pub use writes::*;

#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("Bad credentials")]
    Unauthorized,
    #[error("Not Found")]
    NotFound,
    #[error("Resource not accessible by token")]
    Forbidden,
    #[error("Forgejo request timed out")]
    Timeout,
    #[error("Forgejo returned an invalid response")]
    Upstream,
}

#[derive(Clone)]
pub struct Forgejo {
    client: Client,
    base: Url,
    sessions: Arc<Mutex<SessionCache>>,
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Error::Timeout
        } else {
            Error::Upstream
        }
    }
}

/// One upstream response body plus whether Forgejo's `Link` header announced another page.
type Page = (serde_json::Value, bool);
type Memo = Arc<Mutex<HashMap<String, Arc<OnceCell<Result<Page, Error>>>>>>;

/// Credentials and memoized upstream data live only for the incoming request.
#[derive(Clone)]
pub struct Session {
    pub forgejo: Forgejo,
    authorization: String,
    pub viewer: User,
    memo: Memo,
    deadline: tokio::time::Instant,
}

/// Paged reads ask Forgejo for this many items per page.
pub(crate) const PAGE_SIZE: usize = 50;
/// A list longer than this many pages is treated as an upstream fault, not read forever.
const MAX_PAGES: usize = 200;

/// Map Forgejo's error statuses onto the gateway's errors; success passes through.
fn ok(response: reqwest::Response) -> Result<reqwest::Response, Error> {
    match response.status() {
        StatusCode::UNAUTHORIZED => Err(Error::Unauthorized),
        StatusCode::FORBIDDEN => Err(Error::Forbidden),
        StatusCode::NOT_FOUND => Err(Error::NotFound),
        status if !status.is_success() => Err(Error::Upstream),
        _ => Ok(response),
    }
}

impl Forgejo {
    pub fn new(base: Url) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base,
            sessions: Arc::default(),
        })
    }

    pub async fn authenticate(&self, authorization: &str) -> Result<Session, Error> {
        let (scheme, token) = authorization.split_once(' ').ok_or(Error::Unauthorized)?;
        if !(scheme.eq_ignore_ascii_case("token") || scheme.eq_ignore_ascii_case("bearer"))
            || token.is_empty()
            || token.chars().any(char::is_whitespace)
        {
            return Err(Error::Unauthorized);
        }
        let key: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let cached = self
            .sessions
            .lock()
            .expect("session cache lock")
            .get(&key, Instant::now());
        let viewer = match cached {
            Some(viewer) => viewer,
            None => {
                let response = self
                    .send::<()>(Method::GET, &["user"], &[], None, authorization)
                    .await?;
                let viewer: User = ok(response)?.json().await?;
                self.sessions.lock().expect("session cache lock").insert(
                    key,
                    viewer.clone(),
                    Instant::now(),
                );
                viewer
            }
        };
        Ok(Session {
            forgejo: self.clone(),
            authorization: authorization.to_owned(),
            viewer,
            memo: Arc::default(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(10),
        })
    }

    /// The one upstream request path: URL, credentials and transport errors. Callers map statuses
    /// with [`ok`], or first claim the statuses whose bodies they interpret.
    async fn send<B: serde::Serialize + ?Sized>(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&B>,
        authorization: &str,
    ) -> Result<reqwest::Response, Error> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| Error::Upstream)?
            .pop_if_empty()
            .extend(["api", "v1"])
            .extend(segments);
        let mut request = self
            .client
            .request(method, url)
            .query(query)
            .header(AUTHORIZATION, authorization);
        if let Some(body) = body {
            request = request.json(body);
        }
        Ok(request.send().await?)
    }
}

impl Session {
    /// [`Forgejo::send`] under this request's credentials and shared deadline.
    async fn send<B: serde::Serialize + ?Sized>(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&B>,
    ) -> Result<reqwest::Response, Error> {
        tokio::time::timeout_at(
            self.deadline,
            self.forgejo
                .send(method, segments, query, body, &self.authorization),
        )
        .await
        .map_err(|_| Error::Timeout)?
    }

    /// Run `fetch` once per key for this request. The `OnceCell` also coalesces concurrent
    /// GraphQL aliases; the map lock is never held across an await.
    async fn memoize<F: Future<Output = Result<Page, Error>>>(
        &self,
        key: String,
        fetch: impl FnOnce() -> F,
    ) -> Result<Page, Error> {
        let cell = self
            .memo
            .lock()
            .expect("request memo lock")
            .entry(key)
            .or_default()
            .clone();
        cell.get_or_init(fetch).await.clone()
    }

    /// Drop every memoized read; writes call this so later reads see their effect.
    fn forget_reads(&self) {
        self.memo.lock().expect("request memo lock").clear();
    }

    /// Drop one memoized unqueried GET so the next [`Session::get`] refetches it.
    fn forget(&self, segments: &[&str]) {
        let key = serde_json::to_string(&(segments, &[] as &[(&str, String)]))
            .expect("request key serializes");
        self.memo.lock().expect("request memo lock").remove(&key);
    }

    /// One memoized GET, with whether Forgejo announced a next page. Reads use only local page
    /// numbers, never an upstream-provided URL, so credentials stay on this host.
    pub(crate) async fn page<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<(T, bool), Error> {
        let key = serde_json::to_string(&(segments, query)).expect("request key serializes");
        let (body, next) = self
            .memoize(key, || async {
                let response = ok(self.send::<()>(Method::GET, segments, query, None).await?)?;
                let next = response
                    .headers()
                    .get(reqwest::header::LINK)
                    .and_then(|h| h.to_str().ok())
                    .is_some_and(|h| h.split(',').any(|link| link.contains("rel=\"next\"")));
                Ok((response.json().await?, next))
            })
            .await?;
        Ok((
            serde_json::from_value(body).map_err(|_| Error::Upstream)?,
            next,
        ))
    }

    /// A single read. Use it for every endpoint whose swagger has no `page` parameter: such
    /// endpoints return the whole list on every request, so paging them would never end.
    pub async fn get<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<T, Error> {
        Ok(self.page(segments, query).await?.0)
    }

    /// Every item of a list endpoint whose swagger has `page` and `limit`.
    pub async fn pages<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<Vec<T>, Error> {
        let mut items = Vec::new();
        for page in 1..=MAX_PAGES {
            let mut paged = query.to_vec();
            paged.extend([("limit", PAGE_SIZE.to_string()), ("page", page.to_string())]);
            let (batch, next): (Vec<T>, _) = self.page(segments, &paged).await?;
            let count = batch.len();
            items.extend(batch);
            if !next && count < PAGE_SIZE {
                return Ok(items);
            }
        }
        Err(Error::Upstream)
    }

    /// An unmemoized read of a raw body, such as a diff.
    pub async fn text(&self, segments: &[&str], query: &[(&str, String)]) -> Result<String, Error> {
        Ok(
            ok(self.send::<()>(Method::GET, segments, query, None).await?)?
                .text()
                .await?,
        )
    }

    pub async fn statuses(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<CommitStatus>, Error> {
        self.pages(&["repos", owner, repo, "commits", sha, "statuses"], &[])
            .await
    }

    pub async fn reviews(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Vec<Review>, Error> {
        self.pages(
            &[
                "repos",
                owner,
                repo,
                "pulls",
                &number.to_string(),
                "reviews",
            ],
            &[],
        )
        .await
    }

    pub async fn commits(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Vec<PullCommit>, Error> {
        self.pages(
            &[
                "repos",
                owner,
                repo,
                "pulls",
                &number.to_string(),
                "commits",
            ],
            &[],
        )
        .await
    }

    pub async fn comments(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Vec<Comment>, Error> {
        // No `page` parameter in swagger: one request returns every comment.
        self.get(
            &[
                "repos",
                owner,
                repo,
                "issues",
                &number.to_string(),
                "comments",
            ],
            &[],
        )
        .await
    }

    pub async fn protection(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Option<Protection>, Error> {
        // No query parameters in swagger: one request returns every rule.
        let rules = self
            .get(&["repos", owner, repo, "branch_protections"], &[])
            .await?;
        Ok(Protection::for_branch(rules, branch))
    }

    pub async fn behind(
        &self,
        owner: &str,
        repo: &str,
        base: &str,
        head: &str,
    ) -> Result<bool, Error> {
        match self
            .get::<serde_json::Value>(
                &["repos", owner, repo, "compare", &format!("{base}...{head}")],
                &[],
            )
            .await
        {
            Ok(value) => {
                if let Some(behind) = value["behind_by"].as_u64() {
                    return Ok(behind > 0);
                }
                // Forgejo 15 omits behind_by. Reverse the comparison to count base
                // commits absent from the head, using the same merge-base semantics.
                let reverse: serde_json::Value = self
                    .get(
                        &["repos", owner, repo, "compare", &format!("{head}...{base}")],
                        &[],
                    )
                    .await?;
                reverse["total_commits"]
                    .as_u64()
                    .map(|n| n > 0)
                    .ok_or(Error::Upstream)
            }
            Err(Error::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub async fn repository(&self, owner: &str, name: &str) -> Result<Repository, Error> {
        self.get(&["repos", owner, name], &[]).await
    }

    /// Forgejo's repository owner is a User-shaped object even for organizations.
    pub async fn owner_type(&self, owner: &str) -> Result<&'static str, Error> {
        match self.get::<serde_json::Value>(&["orgs", owner], &[]).await {
            Ok(_) => Ok("Organization"),
            Err(Error::NotFound) => Ok("User"),
            Err(error) => Err(error),
        }
    }

    pub async fn pull_request(
        &self,
        owner: &str,
        name: &str,
        number: i32,
    ) -> Result<PullRequest, Error> {
        let segments = ["repos", owner, name, "pulls", &number.to_string()];
        let key = format!("pull:{owner}/{name}/{number}");
        let (value, _) = self
            .memoize(key, || async {
                let mut value: serde_json::Value = self.get(&segments, &[]).await?;
                let pr: PullRequest =
                    serde_json::from_value(value.clone()).map_err(|_| Error::Upstream)?;
                // Forgejo 15's false also means checking or error. Retry once; a persistent
                // false remains UNKNOWN, because it does not prove a conflict. Drafts always
                // report false and map to UNKNOWN anyway, so they skip the wait.
                if pr.state == "open" && !pr.merged && !pr.mergeable && !pr.draft {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    self.forget(&segments);
                    value = self.get(&segments, &[]).await?;
                }
                Ok((value, false))
            })
            .await?;
        serde_json::from_value(value).map_err(|_| Error::Upstream)
    }

    /// Filter after traversing upstream pages: a match may occur past the first page.
    /// GitHub pagination applies to the filtered set, not Forgejo's unfiltered pages.
    pub async fn pull_requests(
        &self,
        owner: &str,
        name: &str,
        state: &str,
        head: Option<&str>,
        poster: Option<&str>,
    ) -> Result<Vec<PullRequest>, Error> {
        let mut query = vec![("state", state.to_owned())];
        if let Some(poster) = poster {
            query.push(("poster", poster.into()));
        }
        // Forgejo's converter returns nil for a PR whose issue or repos fail to load
        // (`services/convert/pull.go:31-50`), so list entries can be null.
        let pulls: Vec<Option<PullRequest>> =
            self.pages(&["repos", owner, name, "pulls"], &query).await?;
        let mut pulls: Vec<PullRequest> = pulls.into_iter().flatten().collect();
        pulls.retain(|pr| {
            let state_matches =
                state == "all" || (if pr.merged { "closed" } else { &pr.state }) == state;
            state_matches && head.is_none_or(|filter| pr.head.matches(filter))
        });
        pulls.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.number.cmp(&a.number))
        });
        Ok(pulls)
    }
}
