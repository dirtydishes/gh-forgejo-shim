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
        forgejo::WriteError::MergeDenied => async_graphql::Error::new(error.to_string())
            .extend_with(|_, extensions| extensions.set("type", "FORBIDDEN")),
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
        let mut source = session
            .create_pull(
                &id.owner,
                &id.repository,
                forgejo::CreatePull {
                    base: &input.base_ref_name,
                    head: &input.head_ref_name,
                    title: &title,
                    body: input.body.as_deref().unwrap_or(""),
                },
            )
            .await
            .map_err(write_error)?;
        // Forgejo's create ignores maintainer edits (`modules/structs/pull.go:80-91`), which only
        // matter for a fork. The PR exists now, so a refused edit returns it alongside an error
        // that names it, rather than failing in a way that invites creating it again.
        let fork = source.head.repo.as_ref().is_some_and(|head| {
            !head.owner.login.eq_ignore_ascii_case(&id.owner)
                || head
                    .name
                    .as_ref()
                    .is_some_and(|name| name != &id.repository)
        });
        if fork && input.maintainer_can_modify.unwrap_or(true) {
            let edit = forgejo::EditPull {
                allow_maintainer_edit: Some(true),
                ..Default::default()
            };
            match session
                .edit_pull(&id.owner, &id.repository, source.number, edit)
                .await
            {
                Ok(edited) => source = edited,
                Err(_) => ctx.add_error(
                    async_graphql::Error::new(format!(
                        "Created {}, but Forgejo refused to let maintainers edit its branch",
                        source.html_url
                    ))
                    .extend_with(|_, e| e.set("type", "UNPROCESSABLE"))
                    .into_server_error(ctx.item.pos),
                ),
            }
        }
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
        let session = ctx.data_unchecked::<Session>();
        let source = session
            .pull_request(&id.owner, &id.repository, id.number)
            .await
            .map_err(error)?;
        // Forgejo applies title, body and state before it checks that a new base exists, and skips
        // the check for merged PRs (`routers/api/v1/repo/pull.go:649-759`), so check first. The
        // route's `*` takes the branch's slashes literally.
        if let Some(base) = input
            .base_ref_name
            .as_deref()
            .filter(|base| !source.merged && *base != source.base.ref_name)
        {
            let mut segments = vec!["repos", &id.owner, &id.repository, "branches"];
            segments.extend(base.split('/'));
            // URL building drops `.` and `..` segments, so only a branch that names itself as
            // `base` proves it exists.
            let branch = session
                .get::<serde_json::Value>(&segments, &[])
                .await
                .map_err(error)?;
            if branch["name"] != base {
                return Err(error(forgejo::Error::NotFound));
            }
        }
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
        // Remove only the draft marker. A title that still starts with a WIP prefix would stay a
        // draft in Forgejo (`models/issues/pull.go:799`), and the rest is the user's text.
        let prefixes = ctx.data_unchecked::<map::WipPrefixes>();
        let title = prefixes.strip(&source.title);
        if prefixes.strip(title) != title {
            return Err(async_graphql::Error::new(
                "The title still starts with a WIP prefix, so Forgejo would keep this a draft",
            )
            .extend_with(|_, e| e.set("type", "UNPROCESSABLE")));
        }
        title.to_owned()
    };
    // Forgejo decides draft by its own configured prefixes, which no API exposes
    // (`models/issues/pull.go:798`). A title the gateway's prefixes leave alone can't change it,
    // and a changed draft state is checked.
    let unchanged = |outcome: &str| {
        async_graphql::Error::new(format!(
            "Forgejo didn't change the draft state: the forge's WIP title prefixes differ from the gateway's{outcome}"
        ))
        .extend_with(|_, e| e.set("type", "UNPROCESSABLE"))
    };
    if title == source.title {
        return Err(unchanged(""));
    }
    let edited = ctx
        .data_unchecked::<Session>()
        .edit_pull(
            &id.owner,
            &id.repository,
            id.number,
            forgejo::EditPull {
                title: Some(title),
                ..Default::default()
            },
        )
        .await
        .map_err(write_error)?;
    if edited.draft != draft {
        // Forgejo's edit has no conditional write, so putting the title back could overwrite
        // someone else's edit made meanwhile. Say what the title is now instead.
        return Err(unchanged(&format!("; the title is now {:?}", edited.title)));
    }
    Ok(PullRequest::new(edited, &id.owner, &id.repository))
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
    let method = method.unwrap_or(PullRequestMergeMethod::Merge);
    let (method, style) = match method {
        PullRequestMergeMethod::Merge => ("merge", "Merge commits"),
        PullRequestMergeMethod::Squash => ("squash", "Squash merges"),
        PullRequestMergeMethod::Rebase => ("rebase", "Rebase merges"),
    };
    if auto {
        // A schedule stores no expected head and later merges whatever head the PR has
        // (`routers/api/v1/repo/pull.go:993-1009`), so it can't honor one.
        if expected.is_some() {
            return Err(async_graphql::Error::new(
                "Forgejo can't tie an auto-merge to a head commit; merge without auto-merge to check the head",
            )
            .extend_with(|_, e| e.set("type", "UNPROCESSABLE")));
        }
        // Nor does it check the style; its worker drops a disallowed one silently
        // (`services/automerge/automerge.go:227`). A direct merge checks it itself.
        let repository = session
            .repository(&id.owner, &id.repository)
            .await
            .map_err(error)?;
        let allowed = match method {
            "merge" => repository.allow_merge_commits,
            "squash" => repository.allow_squash_merge,
            _ => repository.allow_rebase,
        };
        if !allowed {
            return Err(write_error(forgejo::WriteError::DisallowedMergeStyle(
                style,
            )));
        }
    }
    let input = |merge_when_checks_succeed| forgejo::MergePull {
        method,
        headline: headline.clone(),
        body: body.clone(),
        head_commit_id: expected.as_ref().map(|e| e.0.clone()),
        merge_when_checks_succeed,
        delete_branch_after_merge: false,
    };
    if auto {
        // Forgejo always schedules, and only a later status, review or push wakes its worker
        // (`services/automerge/automerge.go:56-67`, `services/shared/automerge/automerge.go:26`),
        // so a PR that is ready now would wait. Merge it now if Forgejo allows; schedule only
        // after a refusal that such an event can lift. A push that lost a race to the base changed
        // nothing (`services/pull/merge.go:393-404`), so it gets one more try.
        let mut retries = 1;
        loop {
            match session
                .merge_pull(&id.owner, &id.repository, id.number, input(false))
                .await
            {
                Ok(source) => return Ok(PullRequest::new(source, &id.owner, &id.repository)),
                Err(forgejo::WriteError::BaseMismatch) if retries > 0 => retries -= 1,
                Err(forgejo::WriteError::ProtectedBranch | forgejo::WriteError::MergeConflicts) => {
                    break;
                }
                Err(e) => return Err(write_error(e)),
            }
        }
    }
    let source = session
        .merge_pull(&id.owner, &id.repository, id.number, input(auto))
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
