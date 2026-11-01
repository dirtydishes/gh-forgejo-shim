use super::*;
use reqwest::Method;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error(transparent)]
    Transport(#[from] Error),
    #[error("Head branch was modified. Review and try the merge again.")]
    HeadMismatch,
    #[error("Pull Request is not mergeable")]
    NotMergeable,
    #[error("Pull Request has merge conflicts")]
    MergeConflicts,
    #[error("User does not have permission to merge this pull request")]
    MergeDenied,
    #[error("Base branch was modified. Review and try the merge again.")]
    BaseMismatch,
    #[error("Pull Request is already merged")]
    AlreadyMerged,
    #[error("Pull Request is still a draft")]
    Draft,
    #[error("Protected branch hook declined")]
    ProtectedBranch,
    #[error("Commits must have verified signatures.")]
    Unsigned,
    #[error("Pull Request merge failed")]
    PushRejected,
    #[error("Auto merge is already enabled")]
    AlreadyScheduled,
    #[error("{0} are not allowed on this repository.")]
    DisallowedMergeStyle(&'static str),
    #[error("Validation failed")]
    Validation,
}

/// Forgejo v15 pull.go:940-1033 uses both ctx.Error messages and raw Go structs.
/// Never expose upstream git output or branch-protection details to the client.
fn merge_error(status: StatusCode, value: &serde_json::Value) -> WriteError {
    match (status, value["message"].as_str()) {
        (StatusCode::METHOD_NOT_ALLOWED, Some("User not allowed to merge PR")) => {
            WriteError::MergeDenied
        }
        // ctx.Error serializes its third argument, not its title. "PR already
        // merged" has an explicitly empty message; missing messages fall back.
        (StatusCode::METHOD_NOT_ALLOWED, Some("")) => WriteError::AlreadyMerged,
        (StatusCode::METHOD_NOT_ALLOWED, Some("Work in progress PRs cannot be merged")) => {
            WriteError::Draft
        }
        (StatusCode::METHOD_NOT_ALLOWED, Some("Please try again later")) => {
            WriteError::MergeConflicts
        }
        (StatusCode::METHOD_NOT_ALLOWED, Some(message)) => {
            if message.starts_with("not allowed to merge [reason: ") && message.ends_with(']') {
                WriteError::ProtectedBranch
            } else if message.starts_with("won't sign: ") {
                WriteError::Unsigned
            } else {
                match message.strip_suffix(" is not an allowed merge style for this repository") {
                    Some("merge") => WriteError::DisallowedMergeStyle("Merge commits"),
                    Some("squash") => WriteError::DisallowedMergeStyle("Squash merges"),
                    Some("rebase") => WriteError::DisallowedMergeStyle("Rebase merges"),
                    _ => WriteError::MergeConflicts,
                }
            }
        }
        (StatusCode::CONFLICT, Some(message)) => {
            if message.contains("head out of date") {
                WriteError::HeadMismatch
            } else if message == "merge push out of date" {
                WriteError::BaseMismatch
            } else if message == "PushRejected without remote error message"
                || message.starts_with("PushRejected with remote message: ")
            {
                WriteError::PushRejected
            } else if message.starts_with(
                "pull request is already scheduled to auto merge when checks succeed [pull_id: ",
            ) && message.ends_with(']')
            {
                WriteError::AlreadyScheduled
            } else {
                WriteError::NotMergeable
            }
        }
        (StatusCode::CONFLICT, None)
            if ["ID", "IssueID", "HeadRepoID", "BaseRepoID"]
                .iter()
                .all(|key| value[key].is_i64())
                && ["HeadBranch", "BaseBranch"]
                    .iter()
                    .all(|key| value[key].is_string()) =>
        {
            WriteError::AlreadyMerged
        }
        (StatusCode::CONFLICT, None)
            if ["Style", "StdOut", "StdErr"]
                .iter()
                .all(|key| value[key].is_string())
                && value.get("Err").is_some() =>
        {
            // ErrMergeConflicts, ErrRebaseConflicts, ErrMergeUnrelatedHistories.
            WriteError::MergeConflicts
        }
        (StatusCode::METHOD_NOT_ALLOWED, _) => WriteError::MergeConflicts,
        _ => WriteError::NotMergeable,
    }
}

#[derive(Serialize)]
pub struct CreatePull<'a> {
    pub base: &'a str,
    pub head: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub allow_maintainer_edit: bool,
}

#[derive(Default, Serialize)]
pub struct EditPull {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_maintainer_edit: Option<bool>,
}

#[derive(Serialize)]
pub struct MergePull {
    #[serde(rename = "Do")]
    pub method: &'static str,
    #[serde(rename = "MergeTitleField", skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    #[serde(rename = "MergeMessageField", skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_commit_id: Option<String>,
    pub merge_when_checks_succeed: bool,
    pub delete_branch_after_merge: bool,
}

impl Session {
    /// Writes share the incoming request's deadline and credentials, but never its GET memo.
    async fn write(
        &self,
        method: Method,
        segments: &[&str],
        body: &impl Serialize,
        merge: bool,
    ) -> Result<reqwest::Response, WriteError> {
        tokio::time::timeout_at(self.deadline, async {
            let mut url = self.forgejo.base.clone();
            url.path_segments_mut()
                .map_err(|_| Error::Upstream)?
                .pop_if_empty()
                .extend(["api", "v1"])
                .extend(segments);
            let response = self
                .forgejo
                .client
                .request(method, url)
                .header(AUTHORIZATION, &self.authorization)
                .json(body)
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
                StatusCode::UNAUTHORIZED => Err(Error::Unauthorized.into()),
                StatusCode::FORBIDDEN => Err(Error::Forbidden.into()),
                StatusCode::NOT_FOUND => Err(Error::NotFound.into()),
                status @ (StatusCode::CONFLICT | StatusCode::METHOD_NOT_ALLOWED) if merge => {
                    let value: serde_json::Value =
                        response.json().await.map_err(|_| Error::Upstream)?;
                    Err(merge_error(status, &value))
                }
                StatusCode::BAD_REQUEST
                | StatusCode::UNPROCESSABLE_ENTITY
                | StatusCode::CONFLICT => Err(WriteError::Validation),
                status if !status.is_success() => Err(Error::Upstream.into()),
                _ => Ok(response),
            }
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    fn invalidate_reads(&self) {
        self.memo.lock().expect("request memo lock").clear();
        self.pulls.lock().expect("PR memo lock").clear();
    }

    pub async fn create_pull(
        &self,
        owner: &str,
        repo: &str,
        input: CreatePull<'_>,
    ) -> Result<PullRequest, WriteError> {
        let response = self
            .write(
                Method::POST,
                &["repos", owner, repo, "pulls"],
                &input,
                false,
            )
            .await?;
        let pull = response.json().await.map_err(|_| Error::Upstream)?;
        self.invalidate_reads();
        Ok(pull)
    }

    pub async fn edit_pull(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
        input: EditPull,
    ) -> Result<PullRequest, WriteError> {
        let response = self
            .write(
                Method::PATCH,
                &["repos", owner, repo, "pulls", &number.to_string()],
                &input,
                false,
            )
            .await?;
        let pull = response.json().await.map_err(|_| Error::Upstream)?;
        self.invalidate_reads();
        Ok(pull)
    }

    pub async fn add_pull_comment(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
        body: &str,
    ) -> Result<Comment, WriteError> {
        let response = self
            .write(
                Method::POST,
                &[
                    "repos",
                    owner,
                    repo,
                    "issues",
                    &number.to_string(),
                    "comments",
                ],
                &serde_json::json!({"body":body}),
                false,
            )
            .await?;
        let comment = response.json().await.map_err(|_| Error::Upstream)?;
        self.invalidate_reads();
        Ok(comment)
    }

    pub async fn merge_pull(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
        input: MergePull,
    ) -> Result<PullRequest, WriteError> {
        self.write(
            Method::POST,
            &["repos", owner, repo, "pulls", &number.to_string(), "merge"],
            &input,
            true,
        )
        .await?;
        self.invalidate_reads();
        Ok(self.pull_request(owner, repo, number).await?)
    }

    pub async fn cancel_auto_merge(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
    ) -> Result<PullRequest, WriteError> {
        // A missing PR must still fail. Forgejo returns the same 404 for "not scheduled".
        self.pull_request(owner, repo, number).await?;
        match self
            .write(
                Method::DELETE,
                &["repos", owner, repo, "pulls", &number.to_string(), "merge"],
                &(),
                false,
            )
            .await
        {
            Ok(_) | Err(WriteError::Transport(Error::NotFound)) => {}
            Err(error) => return Err(error),
        }
        self.invalidate_reads();
        Ok(self.pull_request(owner, repo, number).await?)
    }
}
