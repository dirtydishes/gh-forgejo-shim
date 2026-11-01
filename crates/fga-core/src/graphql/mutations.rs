use super::*;
use crate::{
    forgejo::{self, Session},
    ids, map,
};
use async_graphql::{Context, ErrorExtensions, ID, Object, SimpleObject};

pub struct Mutation;

fn write_error(error: forgejo::WriteError) -> async_graphql::Error {
    match error {
        forgejo::WriteError::Transport(e) => super::error(e),
        other => async_graphql::Error::new(other.to_string())
            .extend_with(|_, extensions| extensions.set("type", "UNPROCESSABLE")),
    }
}
fn id(value: &ID, kind: &str) -> async_graphql::Result<ids::NodeId> {
    ids::decode(value)
        .filter(|id| id.kind == kind)
        .ok_or_else(|| super::error(forgejo::Error::NotFound))
}
fn draft_title(ctx: &Context<'_>, title: &str) -> async_graphql::Result<String> {
    ctx.data_unchecked::<map::WipPrefixes>()
        .draft_title(title)
        .ok_or_else(|| {
            async_graphql::Error::new("Drafts require a configured WIP prefix")
                .extend_with(|_, e| e.set("type", "UNPROCESSABLE"))
        })
}
async fn edit(
    ctx: &Context<'_>,
    value: &ID,
    input: forgejo::EditPull,
) -> async_graphql::Result<PullRequest> {
    let id = id(value, "PullRequest")?;
    let source = ctx
        .data_unchecked::<Session>()
        .edit_pull(&id.owner, &id.repository, id.number, input)
        .await
        .map_err(write_error)?;
    Ok(PullRequest::new(source, &id.owner, &id.repository))
}

#[Object]
impl Mutation {
    async fn create_pull_request(
        &self,
        ctx: &Context<'_>,
        input: CreatePullRequestInput,
    ) -> async_graphql::Result<Option<CreatePullRequestPayload>> {
        let id = id(&input.repository_id, "Repository")?;
        let session = ctx.data_unchecked::<Session>();
        let repository = session
            .repository(&id.owner, &id.repository)
            .await
            .map_err(error)?;
        if repository.id != i64::from(id.number) {
            return Err(error(forgejo::Error::NotFound));
        }
        let title = if input.draft.unwrap_or(false) {
            draft_title(ctx, &input.title)?
        } else {
            input.title
        };
        let source = session
            .create_pull(
                &id.owner,
                &id.repository,
                forgejo::CreatePull {
                    base: &input.base_ref_name,
                    head: &input.head_ref_name,
                    title: &title,
                    body: input.body.as_deref().unwrap_or(""),
                    allow_maintainer_edit: input.maintainer_can_modify.unwrap_or(true),
                },
            )
            .await
            .map_err(write_error)?;
        Ok(Some(CreatePullRequestPayload {
            client_mutation_id: input.client_mutation_id,
            pull_request: Some(PullRequest::new(source, &id.owner, &id.repository)),
        }))
    }
    async fn add_comment(
        &self,
        ctx: &Context<'_>,
        input: AddCommentInput,
    ) -> async_graphql::Result<Option<AddCommentPayload>> {
        let id = id(&input.subject_id, "PullRequest")?;
        let session = ctx.data_unchecked::<Session>();
        // The issues endpoint also accepts ordinary issues; confirm the typed ID targets a PR.
        session
            .pull_request(&id.owner, &id.repository, id.number)
            .await
            .map_err(error)?;
        let source = session
            .add_pull_comment(&id.owner, &id.repository, id.number, &input.body)
            .await
            .map_err(write_error)?;
        Ok(Some(AddCommentPayload {
            client_mutation_id: input.client_mutation_id,
            comment_edge: Some(IssueCommentEdge {
                node: Some(IssueComment {
                    source,
                    owner: id.owner,
                    viewer: session.viewer.login.clone(),
                }),
            }),
        }))
    }
    async fn update_pull_request(
        &self,
        ctx: &Context<'_>,
        input: UpdatePullRequestInput,
    ) -> async_graphql::Result<Option<UpdatePullRequestPayload>> {
        let id = id(&input.pull_request_id, "PullRequest")?;
        let source = ctx
            .data_unchecked::<Session>()
            .pull_request(&id.owner, &id.repository, id.number)
            .await
            .map_err(error)?;
        let title = input
            .title
            .map(|title| {
                if source.draft {
                    draft_title(ctx, &title)
                } else {
                    Ok(title)
                }
            })
            .transpose()?;
        let pull = edit(
            ctx,
            &input.pull_request_id,
            forgejo::EditPull {
                title,
                body: input.body,
                base: input.base_ref_name,
                state: input.state.map(|s| match s {
                    PullRequestUpdateState::Open => "open",
                    PullRequestUpdateState::Closed => "closed",
                }),
                allow_maintainer_edit: input.maintainer_can_modify,
            },
        )
        .await?;
        Ok(Some(UpdatePullRequestPayload {
            client_mutation_id: input.client_mutation_id,
            pull_request: Some(pull),
        }))
    }
    async fn close_pull_request(
        &self,
        ctx: &Context<'_>,
        input: ClosePullRequestInput,
    ) -> async_graphql::Result<Option<ClosePullRequestPayload>> {
        let pull = edit(
            ctx,
            &input.pull_request_id,
            forgejo::EditPull {
                state: Some("closed"),
                ..Default::default()
            },
        )
        .await?;
        Ok(Some(ClosePullRequestPayload {
            client_mutation_id: input.client_mutation_id,
            pull_request: Some(pull),
        }))
    }
    async fn reopen_pull_request(
        &self,
        ctx: &Context<'_>,
        input: ReopenPullRequestInput,
    ) -> async_graphql::Result<Option<ReopenPullRequestPayload>> {
        let pull = edit(
            ctx,
            &input.pull_request_id,
            forgejo::EditPull {
                state: Some("open"),
                ..Default::default()
            },
        )
        .await?;
        Ok(Some(ReopenPullRequestPayload {
            client_mutation_id: input.client_mutation_id,
            pull_request: Some(pull),
        }))
    }
    async fn convert_pull_request_to_draft(
        &self,
        ctx: &Context<'_>,
        input: ConvertPullRequestToDraftInput,
    ) -> async_graphql::Result<Option<ConvertPullRequestToDraftPayload>> {
        let pull = set_draft(ctx, &input.pull_request_id, true).await?;
        Ok(Some(ConvertPullRequestToDraftPayload {
            client_mutation_id: input.client_mutation_id,
            pull_request: Some(pull),
        }))
    }
    async fn mark_pull_request_ready_for_review(
        &self,
        ctx: &Context<'_>,
        input: MarkPullRequestReadyForReviewInput,
    ) -> async_graphql::Result<Option<MarkPullRequestReadyForReviewPayload>> {
        let pull = set_draft(ctx, &input.pull_request_id, false).await?;
        Ok(Some(MarkPullRequestReadyForReviewPayload {
            client_mutation_id: input.client_mutation_id,
            pull_request: Some(pull),
        }))
    }
    async fn merge_pull_request(
        &self,
        ctx: &Context<'_>,
        input: MergePullRequestInput,
    ) -> async_graphql::Result<Option<MergePullRequestPayload>> {
        let pull = merge(
            ctx,
            &input.pull_request_id,
            input.merge_method,
            input.expected_head_oid,
            input.commit_headline,
            input.commit_body,
            false,
        )
        .await?;
        Ok(Some(MergePullRequestPayload {
            client_mutation_id: input.client_mutation_id,
            pull_request: Some(pull),
        }))
    }
    async fn enable_pull_request_auto_merge(
        &self,
        ctx: &Context<'_>,
        input: EnablePullRequestAutoMergeInput,
    ) -> async_graphql::Result<Option<EnablePullRequestAutoMergePayload>> {
        let pull = merge(
            ctx,
            &input.pull_request_id,
            input.merge_method,
            input.expected_head_oid,
            input.commit_headline,
            input.commit_body,
            true,
        )
        .await?;
        Ok(Some(EnablePullRequestAutoMergePayload {
            client_mutation_id: input.client_mutation_id,
            pull_request: Some(pull),
        }))
    }
    async fn disable_pull_request_auto_merge(
        &self,
        ctx: &Context<'_>,
        input: DisablePullRequestAutoMergeInput,
    ) -> async_graphql::Result<Option<DisablePullRequestAutoMergePayload>> {
        let id = id(&input.pull_request_id, "PullRequest")?;
        let source = ctx
            .data_unchecked::<Session>()
            .cancel_auto_merge(&id.owner, &id.repository, id.number)
            .await
            .map_err(write_error)?;
        Ok(Some(DisablePullRequestAutoMergePayload {
            client_mutation_id: input.client_mutation_id,
            pull_request: Some(PullRequest::new(source, &id.owner, &id.repository)),
        }))
    }
}

async fn set_draft(
    ctx: &Context<'_>,
    value: &ID,
    draft: bool,
) -> async_graphql::Result<PullRequest> {
    let id = id(value, "PullRequest")?;
    let source = ctx
        .data_unchecked::<Session>()
        .pull_request(&id.owner, &id.repository, id.number)
        .await
        .map_err(error)?;
    if source.draft == draft {
        return Ok(PullRequest::new(source, &id.owner, &id.repository));
    }
    let title = if draft {
        draft_title(ctx, &source.title)?
    } else {
        let prefixes = ctx.data_unchecked::<map::WipPrefixes>();
        let mut title = source.title.as_str();
        loop {
            let stripped = prefixes.strip(title);
            if stripped == title {
                break;
            }
            title = stripped;
        }
        title.to_owned()
    };
    edit(
        ctx,
        value,
        forgejo::EditPull {
            title: Some(title),
            ..Default::default()
        },
    )
    .await
}

async fn merge(
    ctx: &Context<'_>,
    value: &ID,
    method: Option<PullRequestMergeMethod>,
    expected: Option<GitObjectId>,
    headline: Option<String>,
    body: Option<String>,
    auto: bool,
) -> async_graphql::Result<PullRequest> {
    let id = id(value, "PullRequest")?;
    let session = ctx.data_unchecked::<Session>();
    // Forgejo's scheduler does not pass head_commit_id to its later merge worker.
    if auto && let Some(expected) = &expected {
        let source = session
            .pull_request(&id.owner, &id.repository, id.number)
            .await
            .map_err(error)?;
        if source.head.sha != expected.0 {
            return Err(write_error(forgejo::WriteError::HeadMismatch));
        }
    }
    let method = match method.unwrap_or(PullRequestMergeMethod::Merge) {
        PullRequestMergeMethod::Merge => "merge",
        PullRequestMergeMethod::Squash => "squash",
        PullRequestMergeMethod::Rebase => "rebase",
    };
    let source = session
        .merge_pull(
            &id.owner,
            &id.repository,
            id.number,
            forgejo::MergePull {
                method,
                headline,
                body,
                head_commit_id: expected.map(|s| s.0),
                merge_when_checks_succeed: auto,
                delete_branch_after_merge: false,
            },
        )
        .await
        .map_err(write_error)?;
    Ok(PullRequest::new(source, &id.owner, &id.repository))
}

macro_rules! pull_payload {
    ($name:ident) => {
        #[derive(SimpleObject)]
        pub struct $name {
            pub client_mutation_id: Option<String>,
            pub pull_request: Option<PullRequest>,
        }
    };
}
pull_payload!(CreatePullRequestPayload);
pull_payload!(UpdatePullRequestPayload);
pull_payload!(ClosePullRequestPayload);
pull_payload!(ReopenPullRequestPayload);
pull_payload!(ConvertPullRequestToDraftPayload);
pull_payload!(MarkPullRequestReadyForReviewPayload);
pull_payload!(MergePullRequestPayload);
pull_payload!(EnablePullRequestAutoMergePayload);
pull_payload!(DisablePullRequestAutoMergePayload);
#[derive(SimpleObject)]
pub struct AddCommentPayload {
    pub client_mutation_id: Option<String>,
    pub comment_edge: Option<IssueCommentEdge>,
}
#[derive(SimpleObject)]
pub struct IssueCommentEdge {
    pub node: Option<IssueComment>,
}
