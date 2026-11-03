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

    /// The open PRs that would close an issue when merged: GitHub's `closedByPullRequestsReferences`
    /// with its default `includeClosedPrs: false`. Forgejo records a PR whose description or commits
    /// say `closes #N` as a `pull_ref` event on issue N whose action is `closes`, and turns it to
    /// `neutered` if the keyword is edited away (`models/issues/issue_xref.go`). Each PR once, in
    /// timeline order, as `(owner, repository, number)`.
    pub async fn closing_pulls(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Vec<(String, String, i32)>, Error> {
        let mut pulls: Vec<(String, String, i32)> = Vec::new();
        for event in self.timeline(owner, repo, number).await? {
            let Some(source) = event.ref_issue else {
                continue;
            };
            if event.kind != "pull_ref"
                || event.ref_action != "closes"
                || source.pull_request.is_none()
                || source.state != "open"
            {
                continue;
            }
            let (pull_owner, pull_repo) = match source.repository {
                Some(repository) => (repository.owner, repository.name),
                None => (owner.to_owned(), repo.to_owned()),
            };
            let pull = (pull_owner, pull_repo, source.number);
            if !pulls.contains(&pull) {
                pulls.push(pull);
            }
        }
        Ok(pulls)
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
    pub async fn issue(&self, owner: &str, repo: &str, number: i32) -> Result<Issue, Error> {
        let issue: Issue = self
            .get(&["repos", owner, repo, "issues", &number.to_string()], &[])
            .await?;
        if issue.pull_request.is_some() {
            return Err(Error::NotFound);
        }
        Ok(issue)
    }
    pub async fn file_text(&self, owner: &str, repo: &str, path: &str) -> Result<String, Error> {
        // The route's `*` takes the path's slashes as segments; one segment may not hold a slash.
        let mut segments = vec!["repos", owner, repo, "contents"];
        segments.extend(path.split('/'));
        let value: serde_json::Value = self.get(&segments, &[]).await?;
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
