use super::*;
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
    #[error("Pull Request still has open dependencies")]
    DependenciesLeft,
    #[error("Validation failed")]
    Validation,
    #[error("The gateway can't send this branch name to Forgejo")]
    UnsendableBranch,
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
}

#[derive(Serialize)]
pub struct CreateIssue<'a> {
    pub title: &'a str,
    pub body: &'a str,
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

/// Forgejo's 400, 409, 412 and 422 on a write mean it refused the input. Its 412s are the PR
/// state refusals of `routers/api/v1/repo/pull.go:738-749`.
fn written(reply: Reply) -> Result<Reply, WriteError> {
    match reply.status {
        StatusCode::PRECONDITION_FAILED => {
            let value: serde_json::Value = reply.json().unwrap_or_default();
            Err(match value["message"].as_str() {
                Some("cannot change state of this pull request, it was already merged") => {
                    WriteError::AlreadyMerged
                }
                Some("cannot close this pull request because it still has open dependencies") => {
                    WriteError::DependenciesLeft
                }
                _ => WriteError::Validation,
            })
        }
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY | StatusCode::CONFLICT => {
            Err(WriteError::Validation)
        }
        _ => Ok(ok(reply)?),
    }
}

impl Session {
    /// Send a write and drop memoized reads, so later reads in this request see its effect.
    async fn write<B: Serialize + ?Sized>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&B>,
    ) -> Result<Reply, Error> {
        let reply = self.send(method, segments, &[], body).await?;
        self.forget_reads();
        Ok(reply)
    }

    pub async fn create_pull(
        &self,
        owner: &str,
        repo: &str,
        input: CreatePull<'_>,
    ) -> Result<PullRequest, WriteError> {
        let response = self
            .write(Method::POST, &["repos", owner, repo, "pulls"], Some(&input))
            .await?;
        Ok(written(response)?.json()?)
    }

    pub async fn create_issue(
        &self,
        owner: &str,
        repo: &str,
        input: CreateIssue<'_>,
    ) -> Result<Issue, WriteError> {
        let response = self
            .write(
                Method::POST,
                &["repos", owner, repo, "issues"],
                Some(&input),
            )
            .await?;
        Ok(written(response)?.json()?)
    }

    pub async fn edit_pull(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
        input: EditPull,
    ) -> Result<PullRequest, WriteError> {
        let segments = ["repos", owner, repo, "pulls", &number.to_string()];
        let response = self.write(Method::PATCH, &segments, Some(&input)).await?;
        Ok(written(response)?.json()?)
    }

    pub async fn add_comment(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
        body: &str,
    ) -> Result<Comment, WriteError> {
        let segments = [
            "repos",
            owner,
            repo,
            "issues",
            &number.to_string(),
            "comments",
        ];
        let body = serde_json::json!({ "body": body });
        let response = self.write(Method::POST, &segments, Some(&body)).await?;
        Ok(written(response)?.json()?)
    }

    pub async fn merge_pull(
        &self,
        owner: &str,
        repo: &str,
        number: i32,
        input: MergePull,
    ) -> Result<PullRequest, WriteError> {
        let segments = ["repos", owner, repo, "pulls", &number.to_string(), "merge"];
        // Forgejo sends "Please try again later" both while it is still checking a PR and when the
        // PR conflicts (`services/pull/check.go:97`), and a PR pushed moments ago is usually still
        // being checked, which a busy forge can take seconds over. A refused merge changes nothing,
        // so retry, waiting longer each time, before reporting a conflict. The last 2 s of the
        // request's deadline stay for the final attempt and reading the merged PR back.
        let give_up = (tokio::time::Instant::now() + self.forgejo.merge_wait)
            .min(self.deadline - Duration::from_secs(2));
        let mut pause = Duration::from_millis(250);
        let response = loop {
            let response = self.write(Method::POST, &segments, Some(&input)).await?;
            let checking = response.status == StatusCode::METHOD_NOT_ALLOWED
                && response
                    .json::<serde_json::Value>()
                    .is_ok_and(|value| value["message"] == "Please try again later");
            if !checking || tokio::time::Instant::now() + pause > give_up {
                break response;
            }
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(Duration::from_secs(2));
        };
        if matches!(
            response.status,
            StatusCode::CONFLICT | StatusCode::METHOD_NOT_ALLOWED
        ) {
            return Err(merge_error(response.status, &response.json()?));
        }
        written(response)?;
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
        let segments = ["repos", owner, repo, "pulls", &number.to_string(), "merge"];
        match written(self.write::<()>(Method::DELETE, &segments, None).await?) {
            Ok(_) | Err(WriteError::Transport(Error::NotFound)) => {}
            Err(error) => return Err(error),
        }
        Ok(self.pull_request(owner, repo, number).await?)
    }

    /// Delete a branch, for GitHub's `DELETE git/refs/heads/{branch}`. Forgejo checks the token's
    /// write access and refuses the default and protected branches (`Forbidden`). `NotFound` only
    /// for a branch known to be absent: a name with an empty part, or one Forgejo says doesn't
    /// exist; any other 404 (a hidden repository, a disabled Code unit) is `Forbidden`. A branch
    /// name with slashes goes out as several segments.
    pub async fn delete_branch(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<(), WriteError> {
        let parts: Vec<&str> = branch.split('/').collect();
        // Git allows no empty part in a branch name, so no such branch exists; sent on, Forgejo
        // would merge the slashes and delete a different branch.
        if parts.iter().any(|part| part.is_empty()) {
            return Err(Error::NotFound.into());
        }
        // `send` would refuse a part a proxy could resolve out of the path, and its "not found"
        // would read to gh as "already deleted". Say the delete didn't happen.
        if parts.iter().any(|part| climbs(part)) {
            return Err(WriteError::UnsendableBranch);
        }
        let mut segments = vec!["repos", owner, repo, "branches"];
        segments.extend(parts);
        // gh reads "not found" as "already deleted", so only Forgejo's own word that the branch
        // doesn't exist may say so. Its delete answers 500 for a missing branch, so read the branch
        // first: `GetBranch` names a missing branch in the 404's `errors`
        // (`routers/api/v1/repo/branch.go`), while a 404 from a permission check (a repository the
        // token can't see, a disabled Code unit) carries none and proves nothing.
        let read = self.send::<()>(Method::GET, &segments, &[], None).await?;
        if read.status == StatusCode::NOT_FOUND {
            let body: serde_json::Value = read.json().unwrap_or_default();
            let absent = body["errors"].as_array().is_some_and(|errors| {
                errors.iter().any(|error| {
                    error
                        .as_str()
                        .is_some_and(|error| error.starts_with("branch does not exist ["))
                })
            });
            return Err(if absent {
                Error::NotFound
            } else {
                Error::Forbidden
            }
            .into());
        }
        if ok(read)?.status != StatusCode::OK {
            return Err(Error::Upstream.into());
        }
        let reply = self.write::<()>(Method::DELETE, &segments, None).await?;
        if reply.status == StatusCode::NO_CONTENT {
            return Ok(());
        }
        // Anything but Forgejo's 204 is a delete that didn't happen. A refusal keeps its error;
        // nothing here may read as "not found".
        match written(reply) {
            Ok(_) | Err(WriteError::Transport(Error::NotFound)) => Err(Error::Upstream.into()),
            Err(error) => Err(error),
        }
    }
}
