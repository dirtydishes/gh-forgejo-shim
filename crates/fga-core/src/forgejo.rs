use reqwest::{Client, Method, StatusCode, header::AUTHORIZATION};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::OnceCell;
use url::Url;

mod cache;
mod glob;
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
    #[error(
        "Forgejo's reply is too large: the gateway reads at most 16 MiB per response and 64 MiB per request"
    )]
    TooLarge,
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

/// A fully read upstream response, so a deadline around the request also covers its body.
struct Reply {
    status: StatusCode,
    /// Whether Forgejo's `Link` header names a next page; `None` when it sent no links.
    next: Option<bool>,
    body: Vec<u8>,
}

impl Reply {
    fn json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        serde_json::from_slice(&self.body).map_err(|_| Error::Upstream)
    }
}

/// One upstream JSON body plus its [`Reply::next`].
type Page = (serde_json::Value, Option<bool>);
type Memo = Arc<Mutex<HashMap<String, Arc<OnceCell<Result<Page, Error>>>>>>;

/// Credentials and memoized upstream data live only for the incoming request.
#[derive(Clone)]
pub struct Session {
    pub forgejo: Forgejo,
    authorization: String,
    pub viewer: User,
    memo: Memo,
    deadline: tokio::time::Instant,
    /// Bytes this request may still read from Forgejo, shared by its memoized and concurrent reads.
    budget: Arc<AtomicUsize>,
}

/// Paged reads ask Forgejo for this many items per page.
pub(crate) const PAGE_SIZE: usize = 50;
/// A list longer than this many pages is treated as an upstream fault, not read forever.
pub(crate) const MAX_PAGES: usize = 200;
/// Diffs, comments and timelines are read unpaged, so every body is capped, and so is the total one
/// incoming request reads; the messages in [`Error::TooLarge`] name both.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;

/// Map Forgejo's error statuses onto the gateway's errors; success passes through.
fn ok(reply: Reply) -> Result<Reply, Error> {
    match reply.status {
        StatusCode::UNAUTHORIZED => Err(Error::Unauthorized),
        StatusCode::FORBIDDEN => Err(Error::Forbidden),
        StatusCode::NOT_FOUND => Err(Error::NotFound),
        status if !status.is_success() => Err(Error::Upstream),
        _ => Ok(reply),
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
        // The identity lookup on a cache miss is part of this request, so it draws on its budget.
        let budget = Arc::new(AtomicUsize::new(MAX_REQUEST_BYTES));
        let viewer = match cached {
            Some(viewer) => viewer,
            None => {
                let viewer: User = ok(self
                    .send::<()>(Method::GET, &["user"], &[], None, authorization, &budget)
                    .await?)?
                .json()?;
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
            budget,
        })
    }

    /// The one upstream request path: URL, credentials, transport errors and the whole body, read
    /// within [`MAX_RESPONSE_BYTES`] and charged to `budget`.
    /// Callers map statuses with [`ok`], or first claim the statuses whose bodies they interpret.
    async fn send<B: serde::Serialize + ?Sized>(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&B>,
        authorization: &str,
        budget: &AtomicUsize,
    ) -> Result<Reply, Error> {
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
        let mut response = request.send().await?;
        // Kept as a flag only: following an upstream-provided URL could send the token elsewhere.
        let next = response.headers().get(reqwest::header::LINK).map(|h| {
            h.to_str()
                .is_ok_and(|h| h.split(',').any(|link| link.contains("rel=\"next\"")))
        });
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES
                || budget
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                        left.checked_sub(chunk.len())
                    })
                    .is_err()
            {
                return Err(Error::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Reply { status, next, body })
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
    ) -> Result<Reply, Error> {
        tokio::time::timeout_at(
            self.deadline,
            self.forgejo.send(
                method,
                segments,
                query,
                body,
                &self.authorization,
                &self.budget,
            ),
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

    /// One unmemoized GET.
    async fn fetch(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Page, Error> {
        let reply = ok(self.send::<()>(Method::GET, segments, query, None).await?)?;
        Ok((reply.json()?, reply.next))
    }

    /// One memoized GET, with whether Forgejo announced a next page.
    pub(crate) async fn page<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<(T, Option<bool>), Error> {
        let key = serde_json::to_string(&(segments, query)).expect("request key serializes");
        let (body, next) = self.memoize(key, || self.fetch(segments, query)).await?;
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
            // Forgejo's links are authoritative; endpoints that send none end on a short page.
            if !next.unwrap_or(count >= PAGE_SIZE) {
                return Ok(items);
            }
        }
        Err(Error::Upstream)
    }

    /// An unmemoized read of a raw body, such as a diff.
    pub async fn text(&self, segments: &[&str], query: &[(&str, String)]) -> Result<String, Error> {
        let reply = ok(self.send::<()>(Method::GET, segments, query, None).await?)?;
        Ok(String::from_utf8_lossy(&reply.body).into_owned())
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
        // Forgejo pages reviews before hiding other users' pending ones and sends no links, so a
        // short page proves nothing; without `page` it returns them all
        // (`routers/api/v1/repo/pull_review.go:84-108`, `models/issues/review_list.go:126`).
        self.get(
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
        // `/branch_protections` is admin-only (`routers/api/v1/api.go:864-872`) and unpaged. A token
        // limited to some repositories can't use admin rights even when the repository reports
        // them (`routers/api/v1/permissions/helpers.go:12-24`), so FORBIDDEN falls back too.
        let admin = self
            .repository(owner, repo)
            .await?
            .permissions
            .is_some_and(|p| p.admin);
        if admin {
            match self
                .get(&["repos", owner, repo, "branch_protections"], &[])
                .await
            {
                Ok(rules) => return Ok(Protection::for_branch(rules, branch)),
                Err(Error::Forbidden) => {}
                Err(e) => return Err(e),
            }
        }
        // Everyone else gets the branch's summary of its effective rule. The route's `*` takes the
        // branch's slashes literally.
        let mut segments = vec!["repos", owner, repo, "branches"];
        segments.extend(branch.split('/'));
        let summary: BranchSummary = match self.get(&segments, &[]).await {
            Ok(summary) => summary,
            Err(Error::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(summary.protected.then(|| Protection {
            enable_status_check: summary.enable_status_check,
            status_check_contexts: summary.status_check_contexts,
            required_approvals: summary.required_approvals,
            partial: true,
            ..Protection::default()
        }))
    }

    pub async fn behind(
        &self,
        owner: &str,
        repo: &str,
        base: &str,
        head: &str,
    ) -> Result<bool, Error> {
        // Forgejo 15's compare has no behind_by (`modules/structs/repo_compare.go`). Reverse the
        // comparison to count base commits absent from the head.
        match self
            .get::<serde_json::Value>(
                &["repos", owner, repo, "compare", &format!("{head}...{base}")],
                &[],
            )
            .await
        {
            Ok(reverse) => reverse["total_commits"]
                .as_u64()
                .map(|n| n > 0)
                .ok_or(Error::Upstream),
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
                let (mut value, _) = self.fetch(&segments, &[]).await?;
                let pr: PullRequest =
                    serde_json::from_value(value.clone()).map_err(|_| Error::Upstream)?;
                // Forgejo 15's false also means checking or error. Retry once; a persistent
                // false remains UNKNOWN, because it does not prove a conflict. Drafts always
                // report false and map to UNKNOWN anyway, so they skip the wait.
                if pr.state == "open" && !pr.merged && !pr.mergeable && !pr.draft {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    value = self.fetch(&segments, &[]).await?.0;
                }
                Ok((value, None))
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
        // Numbers follow creation order; timestamps carry the server's offset and don't sort as text.
        pulls.sort_by_key(|pr| std::cmp::Reverse(pr.number));
        Ok(pulls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// Headers arrive at once and the body only after the deadline: the read still times out.
    #[tokio::test]
    async fn the_session_deadline_covers_the_response_body() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let upstream = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _ = socket.read(&mut [0; 4096]).unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n")
                .unwrap();
            std::thread::sleep(Duration::from_millis(300));
            let _ = socket.write_all(b"{}");
        });
        let session = Session {
            forgejo: Forgejo::new(base.parse().unwrap()).unwrap(),
            authorization: "token disposable".into(),
            viewer: serde_json::from_value(serde_json::json!({"id":1,"login":"t","avatar_url":""}))
                .unwrap(),
            memo: Memo::default(),
            deadline: tokio::time::Instant::now() + Duration::from_millis(50),
            budget: Arc::new(AtomicUsize::new(MAX_REQUEST_BYTES)),
        };
        let result = session.get::<serde_json::Value>(&["slow"], &[]).await;
        assert!(matches!(result, Err(Error::Timeout)), "{result:?}");
        upstream.join().unwrap();
    }

    /// The identity lookup on a cache miss counts toward the request's budget (parallax r1#1).
    #[tokio::test]
    async fn authentication_draws_on_the_request_budget() {
        let user = r#"{"id":1,"login":"t","avatar_url":""}"#;
        let upstream = axum::Router::new().route(
            "/api/v1/user",
            axum::routing::get(move || async move { user }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let forgejo = Forgejo::new(base.parse().unwrap()).unwrap();
        let missed = forgejo.authenticate("token one").await.unwrap();
        assert_eq!(
            missed.budget.load(Ordering::Relaxed),
            MAX_REQUEST_BYTES - user.len()
        );
        let cached = forgejo.authenticate("token one").await.unwrap();
        assert_eq!(cached.budget.load(Ordering::Relaxed), MAX_REQUEST_BYTES);
        server.abort();
    }

    /// Every upstream read in one incoming request, memoized or concurrent, draws on one budget.
    #[tokio::test]
    async fn one_request_reads_at_most_its_byte_budget() {
        let upstream = axum::Router::new().route(
            "/api/v1/{name}",
            axum::routing::get(|| async { "12345678" }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let session = Session {
            forgejo: Forgejo::new(base.parse().unwrap()).unwrap(),
            authorization: "token disposable".into(),
            viewer: serde_json::from_value(serde_json::json!({"id":1,"login":"t","avatar_url":""}))
                .unwrap(),
            memo: Memo::default(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(10),
            budget: Arc::new(AtomicUsize::new(12)),
        };
        assert_eq!(session.text(&["one"], &[]).await.unwrap(), "12345678");
        let result = session.clone().text(&["two"], &[]).await;
        assert!(matches!(result, Err(Error::TooLarge)), "{result:?}");
        server.abort();
    }
}
