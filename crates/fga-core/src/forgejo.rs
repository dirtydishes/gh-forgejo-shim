use reqwest::{Client, StatusCode, header::AUTHORIZATION};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
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

type PullMemo =
    Arc<Mutex<HashMap<(String, String, i32), Arc<OnceCell<Result<PullRequest, Error>>>>>>;

type RequestMemo = Arc<Mutex<HashMap<String, Arc<OnceCell<Result<serde_json::Value, Error>>>>>>;

/// Credentials and memoized upstream data live only for the incoming request.
#[derive(Clone)]
pub struct Session {
    pub forgejo: Forgejo,
    authorization: String,
    pub viewer: User,
    memo: RequestMemo,
    pulls: PullMemo,
    deadline: tokio::time::Instant,
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
                let viewer: User = self.get(&["user"], &[], authorization).await?.0;
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
            pulls: Arc::default(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(10),
        })
    }

    async fn request(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
        authorization: &str,
    ) -> Result<reqwest::Response, Error> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| Error::Upstream)?
            .pop_if_empty()
            .extend(["api", "v1"])
            .extend(segments);
        let response = self
            .client
            .get(url)
            .query(query)
            .header(AUTHORIZATION, authorization)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    Error::Timeout
                } else {
                    Error::Upstream
                }
            })?;
        match response.status() {
            StatusCode::UNAUTHORIZED => return Err(Error::Unauthorized),
            StatusCode::FORBIDDEN => return Err(Error::Forbidden),
            StatusCode::NOT_FOUND => return Err(Error::NotFound),
            status if !status.is_success() => return Err(Error::Upstream),
            _ => {}
        }
        Ok(response)
    }

    async fn get<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
        authorization: &str,
    ) -> Result<(T, bool), Error> {
        let response = self.request(segments, query, authorization).await?;
        // Follow only the page number, never an upstream-provided URL with our token.
        let has_next = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| h.split(',').any(|link| link.contains("rel=\"next\"")));
        let value = response.json().await.map_err(|_| Error::Upstream)?;
        Ok((value, has_next))
    }
}

impl Session {
    async fn fetch<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<(T, bool), Error> {
        tokio::time::timeout_at(
            self.deadline,
            self.forgejo.get(segments, query, &self.authorization),
        )
        .await
        .map_err(|_| Error::Timeout)?
    }

    async fn get<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<(T, bool), Error> {
        // Store the response and pagination marker together. OnceCell coalesces concurrent
        // GraphQL aliases as well as sequential reads. Never hold the map lock across await.
        let key = serde_json::to_string(&(segments, query)).expect("request key serializes");
        let cell = self
            .memo
            .lock()
            .expect("request memo lock")
            .entry(key)
            .or_default()
            .clone();
        let value = cell
            .get_or_init(|| async {
                let (body, next): (serde_json::Value, _) = self.fetch(segments, query).await?;
                Ok(serde_json::json!([body, next]))
            })
            .await
            .clone()?;
        let body = serde_json::from_value(value[0].clone()).map_err(|_| Error::Upstream)?;
        Ok((body, value[1].as_bool().expect("cached pagination marker")))
    }

    async fn all<T: DeserializeOwned>(&self, segments: &[&str]) -> Result<Vec<T>, Error> {
        let mut items = Vec::new();
        for page in 1.. {
            let (batch, next): (Vec<T>, _) = self
                .get(
                    segments,
                    &[("limit", "50".into()), ("page", page.to_string())],
                )
                .await?;
            let count = batch.len();
            items.extend(batch);
            if !next && count < 50 {
                break;
            }
        }
        Ok(items)
    }

    pub async fn statuses(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<CommitStatus>, Error> {
        self.all(&["repos", owner, repo, "commits", sha, "statuses"])
            .await
    }

    pub async fn reviews(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Vec<Review>, Error> {
        self.all(&[
            "repos",
            owner,
            repo,
            "pulls",
            &number.to_string(),
            "reviews",
        ])
        .await
    }

    pub async fn commits(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Vec<PullCommit>, Error> {
        self.all(&[
            "repos",
            owner,
            repo,
            "pulls",
            &number.to_string(),
            "commits",
        ])
        .await
    }

    pub async fn comments(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Vec<Comment>, Error> {
        self.all(&[
            "repos",
            owner,
            repo,
            "issues",
            &number.to_string(),
            "comments",
        ])
        .await
    }

    pub async fn protection(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Option<Protection>, Error> {
        let rules: Vec<Protection> = self
            .all(&["repos", owner, repo, "branch_protections"])
            .await?;
        Ok(rules
            .into_iter()
            .find(|r| glob::Pattern::new(&r.rule_name).is_ok_and(|p| p.matches(branch))))
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
            Ok((value, _)) => {
                if let Some(behind) = value["behind_by"].as_u64() {
                    return Ok(behind > 0);
                }
                // Forgejo 15 omits behind_by. Reverse the comparison to count base
                // commits absent from the head, using the same merge-base semantics.
                let (reverse, _): (serde_json::Value, _) = self
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
        Ok(self.get(&["repos", owner, name], &[]).await?.0)
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
        let key = (owner.to_owned(), name.to_owned(), number);
        let cell = self
            .pulls
            .lock()
            .expect("PR memo lock")
            .entry(key)
            .or_default()
            .clone();
        cell.get_or_init(|| async {
            let segments = ["repos", owner, name, "pulls", &number.to_string()];
            let (mut pr, _): (PullRequest, _) = self.fetch(&segments, &[]).await?;
            // Forgejo 15's false also means draft, checking, or error. Retry once;
            // a persistent false remains UNKNOWN, because it does not prove conflict.
            if pr.state == "open" && !pr.merged && !pr.mergeable {
                tokio::time::sleep(Duration::from_millis(300)).await;
                pr = self.fetch(&segments, &[]).await?.0;
            }
            Ok(pr)
        })
        .await
        .clone()
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
        let mut pulls = Vec::new();
        let mut page = 1_u64;
        loop {
            let mut query = vec![
                ("state", state.to_owned()),
                ("limit", "50".into()),
                ("page", page.to_string()),
            ];
            if let Some(poster) = poster {
                query.push(("poster", poster.into()));
            }
            let (batch, has_next): (Vec<Option<PullRequest>>, _) =
                self.get(&["repos", owner, name, "pulls"], &query).await?;
            let count = batch.len();
            pulls.extend(batch.into_iter().flatten().filter(|pr| {
                let state_matches =
                    state == "all" || (if pr.merged { "closed" } else { &pr.state }) == state;
                state_matches && head.is_none_or(|filter| pr.head.matches(filter))
            }));
            if !has_next && count < 50 {
                break;
            }
            page += 1;
        }
        pulls.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.number.cmp(&a.number))
        });
        Ok(pulls)
    }
}
