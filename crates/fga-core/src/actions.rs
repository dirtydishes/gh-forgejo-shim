//! Forgejo Actions uses workflow filenames and native run states, not GitHub IDs.
use crate::forgejo::{Error, MAX_PAGES, Session};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

mod workflow_name;

#[derive(Deserialize)]
pub struct RunList {
    pub total_count: u64,
    pub workflow_runs: Vec<Run>,
}
#[derive(Deserialize)]
pub struct Run {
    pub id: i64,
    pub workflow_id: String,
    pub index_in_repo: i64,
    pub title: String,
    pub prettyref: String,
    pub commit_sha: String,
    pub status: String,
    pub event: String,
    pub html_url: String,
    pub created: String,
    pub updated: String,
    pub started: String,
}
#[derive(Serialize)]
pub struct Workflow {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub state: &'static str,
}
pub fn workflow_id(filename: &str) -> i64 {
    let digest = Sha256::digest(filename.as_bytes());
    i64::from_be_bytes(digest[..8].try_into().expect("eight hash bytes")) & i64::MAX
}

pub fn status(native: &str) -> Result<(&'static str, Option<&'static str>), Error> {
    Ok(match native {
        "waiting" | "blocked" => ("queued", None),
        "running" => ("in_progress", None),
        "success" => ("completed", Some("success")),
        "failure" => ("completed", Some("failure")),
        "cancelled" => ("completed", Some("cancelled")),
        "skipped" => ("completed", Some("skipped")),
        _ => return Err(Error::Upstream),
    })
}

pub fn run(run: &Run, workflow: &Workflow) -> Result<Value, Error> {
    let (status, conclusion) = status(&run.status)?;
    Ok(
        json!({"id":run.id,"workflow_id":workflow.id,"name":workflow.name,
        "display_title":run.title,"head_branch":run.prettyref.strip_prefix("refs/heads/").unwrap_or(&run.prettyref),
        "head_sha":run.commit_sha,"run_number":run.index_in_repo,"event":run.event,
        "status":status,"conclusion":conclusion,"html_url":run.html_url,"path":workflow.path,
        "created_at":run.created,"updated_at":run.updated,"run_started_at":run.started}),
    )
}

impl Session {
    pub async fn action_runs(
        &self,
        owner: &str,
        repo: &str,
        filters: &[(&str, String)],
        start: usize,
        limit: usize,
    ) -> Result<RunList, Error> {
        // Read only the upstream pages that overlap the requested window.
        let size = self.page_size().await?;
        let skip = start % size;
        let first = (start / size).saturating_add(1);
        let mut collected = Vec::new();
        for page in first..first.saturating_add(MAX_PAGES) {
            let mut query = filters.to_vec();
            query.extend([("page", page.to_string()), ("limit", size.to_string())]);
            let (batch, _): (RunList, _) = self
                .page(&["repos", owner, repo, "actions", "runs"], &query)
                .await?;
            let count = batch.workflow_runs.len();
            collected.extend(batch.workflow_runs);
            if count == 0
                || collected.len() >= skip.saturating_add(limit)
                || page.saturating_mul(size) as u64 >= batch.total_count
            {
                return Ok(RunList {
                    total_count: batch.total_count,
                    workflow_runs: collected.into_iter().skip(skip).take(limit).collect(),
                });
            }
        }
        Err(Error::Upstream)
    }

    pub async fn workflows(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<BTreeMap<String, Workflow>, Error> {
        // Forgejo reads the first of these directories that exists, even if it holds no workflows
        // (`modules/actions/workflows.go:62-102`).
        let mut workflows = BTreeMap::new();
        for directory in [
            ".forgejo/workflows",
            ".gitea/workflows",
            ".github/workflows",
        ] {
            let entries: serde_json::Value = match self
                .get(&["repos", owner, repo, "contents", directory], &[])
                .await
            {
                Ok(entries) => entries,
                Err(Error::NotFound) => continue,
                Err(e) => return Err(e),
            };
            let entries = entries.as_array().ok_or(Error::Upstream)?;
            for entry in entries {
                let filename = entry["name"].as_str().ok_or(Error::Upstream)?;
                if entry["type"] != "file"
                    || !(filename.ends_with(".yml") || filename.ends_with(".yaml"))
                {
                    continue;
                }
                let path = format!("{directory}/{filename}");
                let content = self.file_text(owner, repo, &path).await?;
                let name = workflow_name::from_content(&content, &path);
                workflows.insert(
                    filename.into(),
                    Workflow {
                        id: workflow_id(filename),
                        name,
                        path,
                        state: "active",
                    },
                );
            }
            break;
        }
        Ok(workflows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_native_run_states_without_inventing_success() {
        for (native, expected) in [
            ("waiting", ("queued", None)),
            ("blocked", ("queued", None)),
            ("running", ("in_progress", None)),
            ("success", ("completed", Some("success"))),
            ("failure", ("completed", Some("failure"))),
            ("cancelled", ("completed", Some("cancelled"))),
            ("skipped", ("completed", Some("skipped"))),
        ] {
            assert_eq!(status(native).unwrap(), expected);
        }
        assert!(status("unknown").is_err());
        assert!(status("future-state").is_err());
        assert_eq!(workflow_id("ci.yml"), workflow_id("ci.yml"));
        assert_ne!(workflow_id("ci.yml"), workflow_id("other.yml"));
    }
}
