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
    /// Forgejo's 403 for a token that lacks the route's scope (`read:issue` for `/issues/…`,
    /// `read:organization` for `/orgs/…`), as opposed to one the user may not see at all, with the
    /// scopes Forgejo named ([`crate::scopes::named_in`]). Clients get the same message as
    /// [`Error::Forbidden`].
    #[error("Resource not accessible by token")]
    MissingScope(String),
    #[error("Forgejo request timed out")]
    Timeout,
    #[error("Forgejo returned an invalid response")]
    Upstream,
    #[error(
        "Forgejo's reply is too large: the gateway reads at most {} MiB per response and {} MiB per request",
        MAX_RESPONSE_BYTES >> 20,
        MAX_REQUEST_BYTES >> 20
    )]
    TooLarge,
}

#[derive(Clone)]
pub struct Forgejo {
    client: Client,
    base: Url,
    sessions: Arc<Mutex<SessionCache>>,
    /// [`Session::page_size`] and when it was read; Forgejo changes it only on restart.
    page_size: Arc<Mutex<Option<(usize, Instant)>>>,
    /// How long a merge keeps retrying while Forgejo is still checking the PR.
    pub(crate) merge_wait: Duration,
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
    /// Markdown work this request may still do (see [`Session::body_html`]).
    render_budget: Arc<AtomicUsize>,
}

/// Paged reads ask Forgejo for this many items per page.
pub(crate) const PAGE_SIZE: usize = 50;
/// A list longer than this many pages is treated as an upstream fault, not read forever.
pub(crate) const MAX_PAGES: usize = 200;
/// Diffs, comments and timelines are read unpaged, so every body is capped, and so is the total one
/// incoming request reads; the messages in [`Error::TooLarge`] name both.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;
/// Markdown work one request does, shared by every `bodyHTML` it serves: each body's length and
/// parsed nodes, and the HTML rendered from it (see [`crate::map::body_html_within`]). A document
/// with more, such as a long timeline, shows the rest as escaped paragraphs, so its cost stays
/// bounded however many bodies it holds.
const MAX_REQUEST_RENDERED_BYTES: usize = 1024 * 1024;

/// Map Forgejo's error statuses onto the gateway's errors; success passes through.
fn ok(reply: Reply) -> Result<Reply, Error> {
    match reply.status {
        StatusCode::UNAUTHORIZED => Err(Error::Unauthorized),
        StatusCode::FORBIDDEN => Err(
            // `routers/api/v1/api.go` `tokenRequiresScopes`, Forgejo 15.
            match reply.json::<serde_json::Value>().ok().and_then(|body| {
                body["message"]
                    .as_str()
                    .filter(|message| {
                        message.starts_with("token does not have at least one of required scope(s)")
                    })
                    .map(crate::scopes::named_in)
            }) {
                Some(scopes) => Error::MissingScope(scopes),
                None => Error::Forbidden,
            },
        ),
        StatusCode::NOT_FOUND => Err(Error::NotFound),
        status if !status.is_success() => Err(Error::Upstream),
        _ => Ok(reply),
    }
}

/// Whether a path segment could leave the base path once a proxy in front of Forgejo decodes and
/// resolves it before choosing where to send the request, as nginx does: it is `.` or `..`, or
/// holds `/` or `\`, as written or after up to three rounds of percent-decoding (each proxy in a
/// chain may decode once). Such a segment would take the token to another service behind the same
/// proxy, so it is never sent. A tab, CR or LF counts too: the URL parser drops them, so `.\t.`
/// would go out as `..`. Names never need one; multi-part paths are split into segments. The
/// shim's built-in client refuses the same segments.
pub fn climbs(segment: &str) -> bool {
    let mut text = segment.to_owned();
    for _ in 0..=3 {
        if matches!(text.as_str(), "." | "..") || text.contains(['/', '\\', '\t', '\r', '\n']) {
            return true;
        }
        let decoded = percent_decode(&text);
        if decoded == text {
            return false;
        }
        text = decoded;
    }
    // Still decoding after three rounds: refuse rather than guess what a proxy will make of it.
    true
}

/// Whether `name` could name a commit: a SHA, or a branch or tag name as `git check-ref-format`
/// allows one, the rule Forgejo applies to `git/commits/{sha}` (`modules/git/ref.go`
/// `IsValidRefPattern`). It holds no `..`, no part starting with `.`, no `\`, no control character
/// or any other character git refuses, and so no part [`climbs`]. As there, each of up to three
/// rounds of percent-decoding must pass too, so `%2e%2e` and its double-encoded forms are refused
/// like `..`. Forgejo resolves a name sent in a query without checking it, so the gateway checks.
pub fn is_ref_name(name: &str) -> bool {
    let mut text = name.to_owned();
    for _ in 0..=3 {
        if !ref_format(&text) {
            return false;
        }
        let decoded = percent_decode(&text);
        if decoded == text {
            return true;
        }
        text = decoded;
    }
    false
}

fn ref_format(name: &str) -> bool {
    !name.is_empty()
        && name != "@"
        && !name.starts_with(['/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
        && !name.contains([' ', '~', '^', ':', '?', '*', '[', '\\'])
        && !name.chars().any(|c| c.is_ascii_control())
        && !["/.", "..", "//", "@{", ".lock/"]
            .iter()
            .any(|part| name.contains(part))
}

/// One round of percent-decoding; an escape that isn't two hex digits stays as written.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (
            bytes[i],
            bytes.get(i + 1).and_then(|&b| hex(b)),
            bytes.get(i + 2).and_then(|&b| hex(b)),
        ) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Forgejo {
    /// A client for the Forgejo at `base`, the forge's own address. A base with a path, such as a
    /// forge served under `/forgejo/`, keeps it: every API call goes below it, to `{base}/api/v1/...`.
    pub fn new(base: Url) -> Result<Self, reqwest::Error> {
        Self::new_with_roots(base, Vec::new())
    }

    /// [`Forgejo::new`], also trusting `roots`, for a forge behind a private CA. The roots join the
    /// built-in trust store; certificate and hostname checks stay on. The caller checks `base`: an
    /// HTTP or HTTPS URL without credentials, query or fragment, as fga-server's config does.
    pub fn new_with_roots(
        base: Url,
        roots: Vec<reqwest::Certificate>,
    ) -> Result<Self, reqwest::Error> {
        let mut client = Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none());
        for root in roots {
            client = client.add_root_certificate(root);
        }
        Ok(Self {
            client: client.build()?,
            base,
            sessions: Arc::default(),
            page_size: Arc::default(),
            merge_wait: Duration::from_secs(5),
        })
    }

    /// The same client, retrying a merge for `wait` while Forgejo is still checking the PR, and never
    /// into the last 2 s of the request's 10 s deadline. The default, 5 s, leaves a merge that
    /// succeeds on its last attempt at least 5 s to do its git work and read the PR back.
    pub fn with_merge_wait(mut self, wait: Duration) -> Self {
        self.merge_wait = wait;
        self
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
            render_budget: Arc::new(AtomicUsize::new(MAX_REQUEST_RENDERED_BYTES)),
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
        if segments.iter().any(|segment| climbs(segment)) {
            return Err(Error::NotFound);
        }
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
        // A declared length past the cap fails before any of the body is read.
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(Error::TooLarge);
        }
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
    /// A body as `bodyHTML`, rendered while this request's render budget lasts and escaped after,
    /// keeping images only from `forge`, the forge's public web URL.
    pub fn body_html(&self, body: &str, forge: Option<&Url>) -> String {
        crate::map::body_html_within(body, forge, &self.render_budget)
    }

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

    /// Logs, once per process, that Forgejo refused this token for a missing `scope` and the
    /// answer goes without that data ([`crate::scopes`]).
    pub(crate) fn hidden(&self, scope: &str) {
        crate::scopes::warn_hidden(self.forgejo.base.as_str(), scope);
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

    /// How many items Forgejo returns per page when asked for [`PAGE_SIZE`]: it clamps every page to
    /// its `MAX_RESPONSE_ITEMS` (`services/convert/utils.go:15-20`), which may be lower.
    /// An upstream that hides the settings endpoint keeps Forgejo's default of 50.
    pub(crate) async fn page_size(&self) -> Result<usize, Error> {
        #[derive(serde::Deserialize)]
        struct ApiSettings {
            max_response_items: usize,
        }
        let cached = *self.forgejo.page_size.lock().expect("page size lock");
        if let Some((size, read)) = cached
            && read.elapsed() < Duration::from_secs(600)
        {
            return Ok(size);
        }
        let size = match self.get::<ApiSettings>(&["settings", "api"], &[]).await {
            Ok(settings) => settings.max_response_items.clamp(1, PAGE_SIZE),
            // A forge or proxy that hides the settings keeps the default; a failure that may pass
            // (a timeout, a 5xx) still fails the request and isn't cached.
            Err(Error::NotFound | Error::Forbidden) => PAGE_SIZE,
            Err(Error::MissingScope(scope)) => {
                self.hidden(&scope);
                PAGE_SIZE
            }
            Err(e) => return Err(e),
        };
        *self.forgejo.page_size.lock().expect("page size lock") = Some((size, Instant::now()));
        Ok(size)
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
        Ok(String::from_utf8_lossy(&self.bytes(segments, query).await?).into_owned())
    }

    /// An unmemoized read of a raw body as bytes, such as a file.
    pub async fn bytes(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<Vec<u8>, Error> {
        let reply = ok(self.send::<()>(Method::GET, segments, query, None).await?)?;
        Ok(reply.body)
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

    pub async fn review_comments(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
        review: i64,
    ) -> Result<Vec<ReviewComment>, Error> {
        // No `page` parameter in swagger: one request returns every comment.
        self.get(
            &[
                "repos",
                owner,
                repo,
                "pulls",
                &number.to_string(),
                "reviews",
                &review.to_string(),
                "comments",
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
                Err(Error::MissingScope(scope)) => self.hidden(&scope),
                Err(e) => return Err(e),
            }
        }
        // Everyone else gets the branch's summary of its effective rule.
        let Some(summary) = self.branch_summary(owner, repo, branch).await? else {
            return Ok(None);
        };
        Ok(summary.protected.then(|| Protection {
            enable_status_check: summary.enable_status_check,
            status_check_contexts: summary.status_check_contexts,
            required_approvals: summary.required_approvals,
            partial: true,
            ..Protection::default()
        }))
    }

    /// The branch's summary for the caller, or `None` if it doesn't exist. The route's `*` takes the
    /// branch's slashes literally.
    pub async fn branch_summary(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Option<BranchSummary>, Error> {
        let mut segments = vec!["repos", owner, repo, "branches"];
        segments.extend(branch.split('/'));
        match self.get(&segments, &[]).await {
            Ok(summary) => Ok(Some(summary)),
            Err(Error::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
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

    /// Forgejo's repository owner is a User-shaped object even for organizations. A token without
    /// `read:organization` gets 403 where an organization lookup would say 404, so the owner reads
    /// as a user then: its login and repositories stay right, and only its type can be wrong.
    pub async fn owner_type(&self, owner: &str) -> Result<&'static str, Error> {
        match self.get::<serde_json::Value>(&["orgs", owner], &[]).await {
            Ok(_) => Ok("Organization"),
            Err(Error::NotFound) => Ok("User"),
            Err(Error::MissingScope(scope)) => {
                self.hidden(&scope);
                Ok("User")
            }
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
            render_budget: Arc::new(AtomicUsize::new(MAX_REQUEST_RENDERED_BYTES)),
        };
        let result = session.get::<serde_json::Value>(&["slow"], &[]).await;
        assert!(matches!(result, Err(Error::Timeout)), "{result:?}");
        upstream.join().unwrap();
    }

    /// A reply that declares more than the cap fails on its headers, without waiting for a body.
    #[tokio::test]
    async fn a_declared_oversize_reply_fails_before_its_body() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let upstream = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _ = socket.read(&mut [0; 4096]).unwrap();
            let declared = MAX_RESPONSE_BYTES + 1;
            let _ = socket.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\n\r\n").as_bytes(),
            );
            // Hold the body back; the client should give up long before this.
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let _ = socket.read(&mut [0; 1]);
        });
        let session = Session {
            forgejo: Forgejo::new(base.parse().unwrap()).unwrap(),
            authorization: "token disposable".into(),
            viewer: serde_json::from_value(serde_json::json!({"id":1,"login":"t","avatar_url":""}))
                .unwrap(),
            memo: Memo::default(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(2),
            budget: Arc::new(AtomicUsize::new(MAX_REQUEST_BYTES)),
            render_budget: Arc::new(AtomicUsize::new(MAX_REQUEST_RENDERED_BYTES)),
        };
        let result = session.text(&["big.diff"], &[]).await;
        assert!(matches!(result, Err(Error::TooLarge)), "{result:?}");
        drop(session);
        // Join off the runtime, so the client's connection task can close the socket.
        tokio::task::spawn_blocking(move || upstream.join().unwrap())
            .await
            .unwrap();
    }

    /// A chunked reply declares no length, so the read itself stops at the per-reply cap.
    #[tokio::test]
    async fn a_chunked_reply_stops_at_the_response_cap() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let upstream = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _ = socket.read(&mut [0; 4096]).unwrap();
            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
            let chunk = vec![b'd'; 1024 * 1024];
            for _ in 0..17 {
                let sent = socket
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .and_then(|()| socket.write_all(&chunk))
                    .and_then(|()| socket.write_all(b"\r\n"));
                if sent.is_err() {
                    return;
                }
            }
            let _ = socket.write_all(b"0\r\n\r\n");
        });
        let session = Session {
            forgejo: Forgejo::new(base.parse().unwrap()).unwrap(),
            authorization: "token disposable".into(),
            viewer: serde_json::from_value(serde_json::json!({"id":1,"login":"t","avatar_url":""}))
                .unwrap(),
            memo: Memo::default(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(10),
            budget: Arc::new(AtomicUsize::new(MAX_REQUEST_BYTES)),
            render_budget: Arc::new(AtomicUsize::new(MAX_REQUEST_RENDERED_BYTES)),
        };
        let result = session.text(&["big.diff"], &[]).await;
        assert!(matches!(result, Err(Error::TooLarge)), "{result:?}");
        drop(session);
        upstream.join().unwrap();
    }

    /// A forge served under a path keeps it: the identity read goes to `/forgejo/api/v1/user`,
    /// with or without a trailing slash on the base.
    #[tokio::test]
    async fn a_base_with_a_path_keeps_it() {
        let user = r#"{"id":1,"login":"t","avatar_url":""}"#;
        let upstream = axum::Router::new().route(
            "/forgejo/api/v1/user",
            axum::routing::get(move || async move { user }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        for base in [
            format!("http://{addr}/forgejo"),
            format!("http://{addr}/forgejo/"),
        ] {
            let forgejo = Forgejo::new(base.parse().unwrap()).unwrap();
            let session = forgejo.authenticate("token t").await.unwrap();
            assert_eq!(session.viewer.login, "t", "{base}");
        }
        server.abort();
    }

    /// A name that a proxy would resolve out of the base path never leaves the gateway: behind
    /// nginx, `..%2F..%2Foutside` under `/forgejo/` routes to `/outside`. A path with slashes
    /// still reaches Forgejo as segments.
    #[tokio::test]
    async fn names_that_climb_out_of_the_base_path_never_leave() {
        use std::sync::atomic::AtomicUsize;
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = hits.clone();
        let upstream = axum::Router::new()
            .route(
                "/forgejo/api/v1/user",
                axum::routing::get(|| async { r#"{"id":1,"login":"t","avatar_url":""}"# }),
            )
            .route(
                "/forgejo/api/v1/repos/o/r/contents/.forgejo/workflows",
                axum::routing::get(|| async { "[]" }),
            )
            .fallback(move || {
                seen.fetch_add(1, Ordering::SeqCst);
                async { axum::http::StatusCode::NOT_FOUND }
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let base = format!("http://{addr}/forgejo/").parse().unwrap();
        let session = Forgejo::new(base)
            .unwrap()
            .authenticate("token t")
            .await
            .unwrap();
        // Literal, once-encoded (a chain of two decoding proxies) and twice-encoded forms.
        for owner in [
            "../../outside",
            "..",
            ".",
            "a/b",
            "a\\b",
            "..%2F..%2Foutside",
            "%2e%2e",
            ".%2E",
            "a%5Cb",
            "%252e%252e%252foutside",
            "%25252e%25252e",
            // The URL parser drops tabs and newlines, so these would go out as `..`.
            ".\t.",
            "\n..",
            "..\r",
            ".%09.",
        ] {
            let result = session
                .get::<serde_json::Value>(&["repos", owner, "r"], &[])
                .await;
            assert!(
                matches!(result, Err(Error::NotFound)),
                "{owner}: {result:?}"
            );
        }
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "a refused name reached the upstream"
        );
        let entries = session
            .get::<serde_json::Value>(
                &["repos", "o", "r", "contents", ".forgejo", "workflows"],
                &[],
            )
            .await
            .unwrap();
        assert_eq!(entries, serde_json::json!([]));
        server.abort();
    }

    /// Names with a `%` that never decodes to a separator or a dot segment still go out: a branch
    /// or file may be called `100%`, `a%zz` or `%2e.txt`.
    #[test]
    fn only_climbing_segments_are_refused() {
        for name in [
            "100%",
            "a%zz",
            "%2e.txt",
            "50%25off",
            "v1.2",
            "..x",
            "...",
            "%E2%9C%93",
        ] {
            assert!(!climbs(name), "{name}");
        }
        for name in [
            "..",
            "%2E%2E",
            "a%2Fb",
            "%5C",
            "%252F",
            "%25252F",
            "%2525252F",
            ".\t.",
            "a\nb",
            "%0D",
        ] {
            assert!(climbs(name), "{name}");
        }
    }

    /// Branch and tag names with slashes are names; anything `git check-ref-format` refuses, or
    /// that decodes to one, is not.
    #[test]
    fn only_names_git_allows_are_revisions() {
        for name in [
            "main",
            "feature/topic",
            "refs/heads/feature/topic",
            "release/v1.2",
            "HEAD",
            "d1af82ca1f38838d9316683eab5811e751b3474a",
            "100%",
            "50%25off",
            "a%2Fb",
            "fix-#12",
        ] {
            assert!(is_ref_name(name), "{name}");
        }
        for name in [
            "",
            "@",
            "..",
            "../x",
            "a/../b",
            "a/..",
            "a..b",
            ".hidden",
            "a/.b",
            "a/",
            "/a",
            "a//b",
            "a.",
            "a.lock",
            "a.lock/b",
            "a\\b",
            "a b",
            "a~1",
            "a^",
            "a:b",
            "a?",
            "a*",
            "a[",
            "a@{1}",
            "a\nb",
            "a\tb",
            "a\x7fb",
            "%2e%2e",
            "a/%2E%2E/b",
            "%252e%252e",
            "%25252e%25252e",
            "%2525252e",
            "a%5Cb",
            "a%255Cb",
            "a%0Ab",
            "a%2F%2Fb",
        ] {
            assert!(!is_ref_name(name), "{name:?}");
        }
    }

    /// A private CA's root is trusted only where passed, and hostname checks stay on: `new` refuses
    /// the forge's certificate, `new_with_roots` with its CA reads the user, and the same client
    /// refuses the certificate at a host it doesn't name. The fixtures are the shim's: a CA and a
    /// leaf for `localhost`.
    #[tokio::test]
    async fn extra_roots_are_trusted_with_hostname_checks_on() {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        fn serve() -> (u16, std::thread::JoinHandle<()>) {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let cert =
                CertificateDer::from(include_bytes!("../tests/fixtures/test-server.der").to_vec());
            let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                include_bytes!("../tests/fixtures/test-server-key.der").to_vec(),
            ));
            let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
            let handle = std::thread::spawn(move || {
                let (socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
                let mut stream = rustls::StreamOwned::new(connection, socket);
                let mut request = Vec::new();
                let mut byte = [0; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        return; // the client refused the handshake
                    }
                    request.push(byte[0]);
                }
                let body = r#"{"id":1,"login":"t","avatar_url":""}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
            (port, handle)
        }
        let join = |handle: std::thread::JoinHandle<()>| {
            tokio::task::spawn_blocking(move || handle.join().unwrap())
        };
        let ca = reqwest::Certificate::from_pem(include_bytes!("../tests/fixtures/test-ca.pem"))
            .unwrap();
        let (port, handle) = serve();
        let base = |host: &str| format!("https://{host}:{port}").parse().unwrap();
        assert!(
            Forgejo::new(base("localhost"))
                .unwrap()
                .authenticate("token t")
                .await
                .is_err()
        );
        join(handle).await.unwrap();
        let (port, handle) = serve();
        let base = |host: &str| format!("https://{host}:{port}").parse().unwrap();
        let session = Forgejo::new_with_roots(base("localhost"), vec![ca.clone()])
            .unwrap()
            .authenticate("token t")
            .await
            .unwrap();
        assert_eq!(session.viewer.login, "t");
        join(handle).await.unwrap();
        let (port, handle) = serve();
        let wrong_host = format!("https://127.0.0.1:{port}").parse().unwrap();
        let result = Forgejo::new_with_roots(wrong_host, vec![ca])
            .unwrap()
            .authenticate("token t")
            .await;
        assert!(result.is_err());
        join(handle).await.unwrap();
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

    /// Every upstream read in one incoming request, memoized or concurrent, draws on one budget, and
    /// a read that concurrent callers share is charged once.
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
            budget: Arc::new(AtomicUsize::new(20)),
            render_budget: Arc::new(AtomicUsize::new(MAX_REQUEST_RENDERED_BYTES)),
        };
        let other = session.clone();
        let (a, b) = tokio::join!(
            session.get::<serde_json::Value>(&["same"], &[]),
            other.get::<serde_json::Value>(&["same"], &[])
        );
        assert_eq!((a.unwrap(), b.unwrap()), (12345678.into(), 12345678.into()));
        assert_eq!(session.budget.load(Ordering::Relaxed), 12);
        assert_eq!(session.text(&["one"], &[]).await.unwrap(), "12345678");
        let result = session.clone().text(&["two"], &[]).await;
        assert!(matches!(result, Err(Error::TooLarge)), "{result:?}");
        server.abort();
    }
}
