use super::*;
use crate::search::{Filters, Kind, State};
use base64::{Engine, engine::general_purpose::STANDARD};

impl Session {
    pub async fn auto_merge_event(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<Option<TimelineEvent>, Error> {
        let mut latest: Option<TimelineEvent> = None;
        for page in 1.. {
            let (events, next): (Vec<TimelineEvent>, _) = self
                .get(
                    &[
                        "repos",
                        owner,
                        repo,
                        "issues",
                        &number.to_string(),
                        "timeline",
                    ],
                    &[("page", page.to_string()), ("limit", "50".into())],
                )
                .await?;
            let count = events.len();
            for event in events {
                if matches!(
                    event.kind.as_str(),
                    "pull_scheduled_merge" | "pull_cancel_scheduled_merge"
                ) && latest.as_ref().is_none_or(|old| {
                    (event.created_at.as_str(), event.id) > (old.created_at.as_str(), old.id)
                }) {
                    latest = Some(event);
                }
            }
            if !next && count < 50 {
                break;
            }
        }
        Ok(latest.filter(|event| event.kind == "pull_scheduled_merge"))
    }
    /// Page using only local page numbers; never follow an upstream URL with credentials.
    pub async fn issues(
        &self,
        owner: &str,
        repo: &str,
        state: State,
        kind: Kind,
        filters: &Filters,
    ) -> Result<Vec<Issue>, Error> {
        let mut result = Vec::new();
        for page in 1.. {
            let mut query = filters.upstream(state, kind);
            query.extend([("page", page.to_string()), ("limit", "50".into())]);
            let (batch, next): (Vec<Issue>, _) =
                self.get(&["repos", owner, repo, "issues"], &query).await?;
            let count = batch.len();
            result.extend(batch);
            if !next && count < 50 {
                break;
            }
        }
        Ok(result)
    }
    pub async fn issue(&self, owner: &str, repo: &str, number: i32) -> Result<Issue, Error> {
        let issue: Issue = self
            .get(&["repos", owner, repo, "issues", &number.to_string()], &[])
            .await?
            .0;
        if issue.pull_request.is_some() {
            return Err(Error::NotFound);
        }
        Ok(issue)
    }
    pub async fn file_text(&self, owner: &str, repo: &str, path: &str) -> Result<String, Error> {
        let value = self
            .read_json(&["repos", owner, repo, "contents", path], &[])
            .await?;
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
    pub async fn read_json(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<serde_json::Value, Error> {
        Ok(self.get(segments, query).await?.0)
    }
    pub async fn read_text(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<String, Error> {
        tokio::time::timeout_at(self.deadline, async {
            self.forgejo
                .request(segments, query, &self.authorization)
                .await?
                .text()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        Error::Timeout
                    } else {
                        Error::Upstream
                    }
                })
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
}
