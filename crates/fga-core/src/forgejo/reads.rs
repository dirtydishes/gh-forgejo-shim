use super::*;
use crate::search::{Filters, Kind, State};
use base64::{Engine, engine::general_purpose::STANDARD};

impl Session {
    /// Every event on an issue's timeline. Forgejo pages the timeline before dropping code comments
    /// and hidden references and sends no links, so a short page proves nothing; without `page` it
    /// returns every event, and `null` for none, and a `null` entry for an event it failed to
    /// convert (`routers/api/v1/repo/issue_comment.go:199-233`).
    async fn timeline(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Vec<TimelineEvent>, Error> {
        let events: Option<Vec<Option<TimelineEvent>>> = self
            .get(
                &[
                    "repos",
                    owner,
                    repo,
                    "issues",
                    &number.to_string(),
                    "timeline",
                ],
                &[],
            )
            .await?;
        Ok(events.into_iter().flatten().flatten().collect())
    }

    /// The PRs that close, or would close, an issue when merged: GitHub's
    /// `closedByPullRequestsReferences` with its default `includeClosedPrs: false`, which lists open
    /// and merged PRs but not closed unmerged ones. A closing keyword in a PR's title or description
    /// (`pull_ref`) or in a comment on it (`comment_ref`) is a cross-reference from that PR whose
    /// action is `closes` or `reopens`, `neutered` once the keyword is edited away. On merge Forgejo
    /// applies each PR's newest `closes` or `reopens` reference to the issue
    /// (`PullRequest::ResolveCrossReferences`, `models/issues/issue_xref.go:337`), so that one decides.
    /// Each PR once, in the order it first refers to the issue, as `(owner, repository, number)`.
    pub async fn closing_pulls(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Vec<(String, String, i32)>, Error> {
        // Per source PR, its newest deciding reference.
        struct Decision {
            id: i64,
            closes: bool,
            open_or_merged: bool,
        }
        let mut pulls: Vec<((String, String, i32), Decision)> = Vec::new();
        for event in self.timeline(owner, repo, number).await? {
            let Some(source) = event.ref_issue else {
                continue;
            };
            let Some(pull) = source.pull_request.as_ref() else {
                continue;
            };
            if !matches!(event.ref_action.as_str(), "closes" | "reopens") {
                continue;
            }
            let merged = pull["merged"].as_bool().unwrap_or(false);
            let decision = Decision {
                id: event.id,
                closes: event.ref_action == "closes",
                open_or_merged: source.state == "open" || merged,
            };
            let (pull_owner, pull_repo) = match source.repository {
                Some(repository) => (repository.owner, repository.name),
                None => (owner.to_owned(), repo.to_owned()),
            };
            let key = (pull_owner, pull_repo, source.number);
            match pulls.iter_mut().find(|(seen, _)| *seen == key) {
                Some((_, newest)) if decision.id > newest.id => *newest = decision,
                Some(_) => {}
                None => pulls.push((key, decision)),
            }
        }
        Ok(pulls
            .into_iter()
            .filter(|(_, decision)| decision.closes && decision.open_or_merged)
            .map(|(key, _)| key)
            .collect())
    }

    pub async fn auto_merge_event(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Option<TimelineEvent>, Error> {
        let events = self.timeline(owner, repo, number).await?;
        let latest = events
            .into_iter()
            .filter(|event| {
                matches!(
                    event.kind.as_str(),
                    "pull_scheduled_merge" | "pull_cancel_scheduled_merge"
                )
            })
            .max_by(|a, b| (a.created_at.as_str(), a.id).cmp(&(b.created_at.as_str(), b.id)));
        Ok(latest.filter(|event| event.kind == "pull_scheduled_merge"))
    }
    pub async fn issues(
        &self,
        owner: &str,
        repo: &str,
        state: State,
        kind: Kind,
        filters: &Filters,
    ) -> Result<Vec<Issue>, Error> {
        self.pages(
            &["repos", owner, repo, "issues"],
            &filters.upstream(state, kind),
        )
        .await
    }
    /// The files a pull request changes, with line counts.
    pub async fn pull_files(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Vec<ChangedFile>, Error> {
        self.pages(
            &["repos", owner, repo, "pulls", &number.to_string(), "files"],
            &[],
        )
        .await
    }

    /// A pull request's diff from its merge base, as `git diff` writes it.
    pub async fn pull_diff(&self, owner: &str, repo: &str, number: i32) -> Result<String, Error> {
        self.text(
            &["repos", owner, repo, "pulls", &format!("{number}.diff")],
            &[],
        )
        .await
    }

    /// The commits in `head` and not in `base`, newest first. Forgejo answers 404 when either
    /// isn't a commit of the repository.
    pub async fn compare(
        &self,
        owner: &str,
        repo: &str,
        base: &str,
        head: &str,
    ) -> Result<Comparison, Error> {
        self.get(
            &["repos", owner, repo, "compare", &format!("{base}...{head}")],
            &[],
        )
        .await
    }

    /// One commit by SHA or ref; 404 when there is none.
    pub async fn git_commit(&self, owner: &str, repo: &str, rev: &str) -> Result<GitCommit, Error> {
        self.get(
            &["repos", owner, repo, "git", "commits", rev],
            &[("stat", "false".into()), ("files", "false".into())],
        )
        .await
    }

    /// The commit a SHA, branch or tag names; 404 when there is none, and for a name git wouldn't
    /// allow ([`is_ref_name`]), which is never sent. A name with a slash, such as
    /// `feature/topic`, can't be one path segment, so it goes to the commit list as its `sha`
    /// parameter: Forgejo resolves both the same way, and a query can't move the request's path.
    pub async fn resolve_commit(
        &self,
        owner: &str,
        repo: &str,
        rev: &str,
    ) -> Result<GitCommit, Error> {
        if !is_ref_name(rev) {
            return Err(Error::NotFound);
        }
        if !climbs(rev) {
            return self.git_commit(owner, repo, rev).await;
        }
        let commits: Vec<GitCommit> = self
            .get(
                &["repos", owner, repo, "commits"],
                &[
                    ("sha", rev.to_owned()),
                    ("limit", "1".into()),
                    ("stat", "false".into()),
                    ("verification", "false".into()),
                    ("files", "false".into()),
                ],
            )
            .await?;
        commits.into_iter().next().ok_or(Error::NotFound)
    }

    /// The entry at `path` in `rev`, or the default branch: one object for a file, an array for a
    /// directory, 404 for no such path or revision. An empty `path` is the root directory.
    pub async fn contents(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        rev: Option<&str>,
    ) -> Result<serde_json::Value, Error> {
        let mut segments = vec!["repos", owner, repo, "contents"];
        segments.extend(path.split('/').filter(|segment| !segment.is_empty()));
        let query: Vec<_> = rev.map(|rev| ("ref", rev.to_owned())).into_iter().collect();
        self.get(&segments, &query).await
    }

    /// A file's bytes at `rev`, or the default branch, however large.
    pub async fn raw_file(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        rev: Option<&str>,
    ) -> Result<Vec<u8>, Error> {
        let mut segments = vec!["repos", owner, repo, "raw"];
        segments.extend(path.split('/').filter(|segment| !segment.is_empty()));
        let query: Vec<_> = rev.map(|rev| ("ref", rev.to_owned())).into_iter().collect();
        self.bytes(&segments, &query).await
    }

    pub async fn issue(&self, owner: &str, repo: &str, number: i32) -> Result<Issue, Error> {
        let issue: Issue = self
            .get(&["repos", owner, repo, "issues", &number.to_string()], &[])
            .await?;
        if issue.pull_request.is_some() {
            return Err(Error::NotFound);
        }
        Ok(issue)
    }
    /// The commit a PR's own ref `refs/pull/N/head` points to, in the base repository. Forgejo's
    /// refs API answers an exact match as one object (`routers/api/v1/repo/git_ref.go:101-104`).
    pub async fn pull_head(&self, owner: &str, repo: &str, number: i32) -> Result<String, Error> {
        let value: serde_json::Value = self
            .get(
                &[
                    "repos",
                    owner,
                    repo,
                    "git",
                    "refs",
                    "pull",
                    &number.to_string(),
                    "head",
                ],
                &[],
            )
            .await?;
        let wanted = format!("refs/pull/{number}/head");
        let reference = match &value {
            serde_json::Value::Array(refs) => refs.iter().find(|r| r["ref"] == wanted.as_str()),
            single if single["ref"] == wanted.as_str() => Some(single),
            _ => None,
        };
        reference
            .and_then(|r| r["object"]["sha"].as_str())
            .map(str::to_owned)
            .ok_or(Error::NotFound)
    }
    pub async fn file_text(&self, owner: &str, repo: &str, path: &str) -> Result<String, Error> {
        self.file_text_at(owner, repo, path, None).await
    }

    /// [`Session::file_text`] at a commit, branch or tag; `None` reads the default branch.
    pub async fn file_text_at(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        reference: Option<&str>,
    ) -> Result<String, Error> {
        // The route's `*` takes the path's slashes as segments; one segment may not hold a slash.
        let mut segments = vec!["repos", owner, repo, "contents"];
        segments.extend(path.split('/'));
        let query: Vec<(&str, String)> = reference
            .map(|r| ("ref", r.to_owned()))
            .into_iter()
            .collect();
        let value: serde_json::Value = self.get(&segments, &query).await?;
        let content = value["content"].as_str().ok_or(Error::Upstream)?;
        let bytes = STANDARD
            .decode(
                content
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>(),
            )
            .map_err(|_| Error::Upstream)?;
        String::from_utf8(bytes).map_err(|_| Error::Upstream)
    }
}
