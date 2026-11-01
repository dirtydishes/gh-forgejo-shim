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
        let events: Vec<TimelineEvent> = self
            .pages(
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
        let value: serde_json::Value = self
            .get(&["repos", owner, repo, "contents", path], &[])
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
}
