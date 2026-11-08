use super::*;
use crate::{
    forgejo::{self, Session},
    ids::node_id,
    map,
};
use async_graphql::{Context, ID, Object};
use std::sync::Arc;
use url::Url;

#[derive(Clone)]
pub struct PullRequest {
    pub source: Arc<forgejo::PullRequest>,
    pub owner: String,
    pub repo: String,
}
impl PullRequest {
    pub fn new(source: forgejo::PullRequest, owner: &str, repo: &str) -> Self {
        Self {
            source: Arc::new(source),
            owner: owner.into(),
            repo: repo.into(),
        }
    }

    /// Whether the head branch is the PR's head: an open, unmerged PR whose branch exists and isn't
    /// named like a ref ([`forgejo::Branch::exists`]), and whose tip is the commit of the PR's own ref
    /// `refs/pull/N/head`. gh decides on this both to fetch the branch (`headRepository`) and, for a
    /// same-repository PR, to delete it (`isCrossRepository`). A closed or merged PR's branch, a
    /// recreated or moved one, or an AGit PR's ref is not the PR's head; a failed read counts as not.
    async fn head_is_pr_head(&self, session: &Session) -> bool {
        self.source.head.exists()
            && self.source.state == "open"
            && !self.source.merged
            && session
                .pull_head(&self.owner, &self.repo, self.source.number)
                .await
                .is_ok_and(|sha| sha == self.source.head.sha)
    }
    /// Forgejo lets the poster or a repository writer edit, close and reopen a PR.
    async fn viewer_can_change(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let viewer = &ctx.data_unchecked::<Session>().viewer.login;
        let author = self.source.user.login.eq_ignore_ascii_case(viewer);
        viewer_may_change(ctx, &self.owner, &self.repo, author).await
    }
    async fn archived(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let session = ctx.data_unchecked::<Session>();
        let repository = session
            .repository(&self.owner, &self.repo)
            .await
            .map_err(error)?;
        Ok(repository.archived)
    }
    /// Whether the viewer may schedule or cancel a merge (`IsUserAllowedToMerge`). Forgejo refuses
    /// both in an archived repository. It checks code write access as the request's token allows
    /// it, which the repository's permissions report, and a merge allowlist, which only the base
    /// branch's summary reflects; only code writers can join an allowlist. A deleted base branch
    /// has no summary, and then write access decides.
    async fn viewer_can_merge(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        if self.archived(ctx).await? || !viewer_can_write(ctx, &self.owner, &self.repo).await? {
            return Ok(false);
        }
        Ok(ctx
            .data_unchecked::<Session>()
            .branch_summary(&self.owner, &self.repo, &self.source.base.ref_name)
            .await
            .map_err(error)?
            .is_none_or(|summary| summary.user_can_merge))
    }
    async fn checks(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<map::Check>> {
        let statuses = ctx
            .data_unchecked::<Session>()
            .statuses(&self.owner, &self.repo, &self.source.head.sha)
            .await
            .map_err(error)?;
        Ok(map::checks(
            &statuses,
            &self.owner,
            &self.repo,
            ctx.data_unchecked::<Url>(),
        ))
    }
}
#[Object]
impl PullRequest {
    pub(super) async fn id(&self) -> ID {
        ID(node_id(
            "PullRequest",
            &format!("{}/{}", self.owner, self.repo),
            self.source.number,
        ))
    }
    async fn database_id(&self) -> Option<i32> {
        i32::try_from(self.source.id).ok()
    }
    async fn full_database_id(&self) -> Option<BigInt> {
        Some(BigInt(self.source.id.to_string()))
    }
    async fn number(&self) -> i32 {
        self.source.number
    }
    async fn title(&self, ctx: &Context<'_>) -> &str {
        map::title(&self.source, ctx.data_unchecked::<map::WipPrefixes>())
    }
    async fn body(&self) -> &str {
        &self.source.body
    }
    #[graphql(name = "bodyHTML")]
    async fn body_html(&self, ctx: &Context<'_>) -> Html {
        Html(
            ctx.data_unchecked::<Session>()
                .body_html(&self.source.body, ctx.data_opt::<url::Url>()),
        )
    }
    async fn url(&self) -> Uri {
        Uri(map::pull_web_url(&self.source.html_url))
    }
    async fn state(&self) -> map::PullRequestState {
        map::state(&self.source)
    }
    async fn is_draft(&self) -> bool {
        self.source.draft
    }
    async fn closed(&self) -> bool {
        map::state(&self.source) != map::PullRequestState::Open
    }
    async fn merged(&self) -> bool {
        self.source.merged
    }
    async fn merged_at(&self) -> Option<DateTime> {
        self.source.merged_at.clone().map(DateTime)
    }
    async fn closed_at(&self) -> Option<DateTime> {
        self.source.closed_at.clone().map(DateTime)
    }
    async fn created_at(&self) -> DateTime {
        DateTime(self.source.created_at.clone())
    }
    async fn updated_at(&self) -> DateTime {
        DateTime(self.source.updated_at.clone())
    }
    async fn author(&self) -> Option<Actor> {
        Some(User(self.source.user.clone()).into())
    }
    async fn merged_by(&self) -> Option<Actor> {
        self.source.merged_by.clone().map(|u| User(u).into())
    }
    async fn additions(&self) -> i32 {
        self.source.additions
    }
    async fn deletions(&self) -> i32 {
        self.source.deletions
    }
    async fn changed_files(&self) -> i32 {
        self.source.changed_files
    }
    async fn maintainer_can_modify(&self) -> bool {
        self.source.allow_maintainer_edit
    }
    /// Forgejo's own name, never one made up: a made-up name could equal a real branch's, and gh's
    /// branch finder would then take one PR for the other. An AGit PR's is `refs/pull/N/head`.
    async fn head_ref_name(&self) -> &str {
        self.source.head.name()
    }
    async fn base_ref_name(&self) -> &str {
        &self.source.base.ref_name
    }
    async fn head_ref_oid(&self) -> GitObjectId {
        GitObjectId(self.source.head.sha.clone())
    }
    async fn base_ref_oid(&self) -> GitObjectId {
        GitObjectId(self.source.base.sha.clone())
    }
    /// A same-repository PR always has its head repository; Forgejo sends none only once a PR's
    /// fork is deleted. GitHub still calls that PR cross-repository, and gh relies on it:
    /// `pr close --delete-branch` deletes the head branch in the base repository unless this is
    /// true, which would delete the base's branch of the same name.
    /// GitHub's meaning, with one safety turn. `gh pr close -d` and `gh pr merge -d` delete the head
    /// branch in the base repository unless this is true (`close.go:150-157`, `merge.go:519,592`),
    /// so an open same-repository PR counts as cross-repository unless its branch is the PR's head:
    /// otherwise gh would delete whatever branch now carries the name (an AGit PR's
    /// `refs/pull/N/head`, or a branch recreated for other work). gh deletes no remote branch for a
    /// closed or merged PR (`pr close` stops at one, `close.go:77-83`; `pr merge` skips it for one
    /// already merged, `merge.go:523`), and checks one out from its own ref, since it has no head
    /// repository; so such a PR is same-repository, as on GitHub, and `gh pr list` labels it by
    /// branch alone. A deleted fork (`head.repo` null) is cross-repository, as GitHub reports it.
    async fn is_cross_repository(&self, ctx: &Context<'_>) -> bool {
        let Some(head) = self.source.head.repo.as_ref() else {
            return true;
        };
        let same_repository = head.owner.login.eq_ignore_ascii_case(&self.owner)
            && head.name.as_ref().is_none_or(|name| name == &self.repo);
        let open = self.source.state == "open" && !self.source.merged;
        !same_repository || (open && !self.head_is_pr_head(ctx.data_unchecked::<Session>()).await)
    }
    async fn head_repository_owner(&self) -> Option<RepositoryOwner> {
        self.source
            .head
            .repo
            .as_ref()
            .map(|r| User(r.owner.clone()).into())
    }
    /// The base repository, which the PR belongs to.
    async fn repository(&self, ctx: &Context<'_>) -> async_graphql::Result<Repository> {
        let source = ctx
            .data_unchecked::<Session>()
            .repository(&self.owner, &self.repo)
            .await
            .map_err(error)?;
        Ok(Repository { source })
    }
    async fn head_repository(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Option<Repository>> {
        // Deleted head repositories are represented by null upstream. GitHub keeps one for a
        // deleted branch, but gh pr checkout fetches `refs/heads/{headRefName}` from it and falls
        // back to the PR's own ref `refs/pull/N/head` only without one. So the gateway reports a
        // head repository only for an open PR whose head branch exists and whose tip is the PR's
        // head; a branch that's gone, recreated or moved (a closed PR, or one reopened through the
        // API, which leaves the PR's ref where it was) would otherwise be checked out in its place.
        // The owner stays, as on GitHub; gh's checkout reads only `headRepository` to choose.
        // A closed or merged PR's branch can move at any time after the check, and gh would fetch
        // it; the PR's own ref can't. An open PR moves with its branch. Unknown counts as moved.
        let Some(head) = self.source.head.repo.as_ref() else {
            return Ok(None);
        };
        if !self.head_is_pr_head(ctx.data_unchecked::<Session>()).await {
            return Ok(None);
        }
        let name = head.name.as_deref().unwrap_or(&self.repo);
        let source = ctx
            .data_unchecked::<Session>()
            .repository(&head.owner.login, name)
            .await
            .map_err(error)?;
        Ok(Some(Repository { source }))
    }
    async fn base_ref(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Ref>> {
        let p = ctx
            .data_unchecked::<Session>()
            .protection(&self.owner, &self.repo, &self.source.base.ref_name)
            .await
            .map_err(error)?;
        Ok(Some(Ref {
            name: self.source.base.ref_name.clone(),
            branch_protection_rule: p.map(|p| BranchProtectionRule {
                requires_strict_status_checks: p.block_on_outdated_branch,
                required_approving_review_count: Some(p.required_approvals),
            }),
        }))
    }
    async fn mergeable(&self) -> map::MergeableState {
        map::mergeable(&self.source)
    }
    async fn review_decision(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Option<map::PullRequestReviewDecision>> {
        let session = ctx.data_unchecked::<Session>();
        let p = session
            .protection(&self.owner, &self.repo, &self.source.base.ref_name)
            .await
            .map_err(error)?;
        let Some(p) = p.filter(|p| p.gates_reviews()) else {
            return Ok(None);
        };
        let reviews = session
            .reviews(&self.owner, &self.repo, self.source.number)
            .await
            .map_err(error)?;
        Ok(map::review_decision(&reviews, Some(&p)))
    }
    async fn merge_state_status(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<map::MergeStateStatus> {
        let session = ctx.data_unchecked::<Session>();
        let checks = self.checks(ctx).await?;
        let protection = session
            .protection(&self.owner, &self.repo, &self.source.base.ref_name)
            .await
            .map_err(error)?;
        let behind = if protection
            .as_ref()
            .is_some_and(|p| p.block_on_outdated_branch)
        {
            session
                .behind(
                    &self.owner,
                    &self.repo,
                    &self.source.base.sha,
                    &self.source.head.sha,
                )
                .await
                .map_err(error)?
        } else {
            false
        };
        let decision = self.review_decision(ctx).await?;
        Ok(map::merge_state(
            &self.source,
            map::mergeable(&self.source),
            &checks,
            protection.as_ref(),
            decision,
            behind,
        ))
    }
    async fn auto_merge_request(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Option<AutoMergeRequest>> {
        if self.source.state != "open" || self.source.merged {
            return Ok(None);
        }
        let session = ctx.data_unchecked::<Session>();
        // The schedule shows only on the PR's timeline, which Forgejo serves under `read:issue`.
        // Without that scope the token can't see one, and GitHub answers a reader null rather
        // than an error; `viewerCan*AutoMerge` then read false.
        let event = match session
            .auto_merge_event(&self.owner, &self.repo, self.source.number)
            .await
        {
            Ok(event) => event,
            Err(forgejo::Error::MissingScope(scope)) => {
                session.hidden(&scope);
                None
            }
            Err(other) => return Err(error(other)),
        };
        let Some(event) = event else {
            return Ok(None);
        };
        let repository = session
            .repository(&self.owner, &self.repo)
            .await
            .map_err(error)?;
        let method = match repository.default_merge_style.as_str() {
            "squash" => PullRequestMergeMethod::Squash,
            "rebase" | "rebase-merge" => PullRequestMergeMethod::Rebase,
            _ => PullRequestMergeMethod::Merge,
        };
        Ok(Some(AutoMergeRequest {
            author_email: None,
            commit_body: None,
            commit_headline: None,
            merge_method: method,
            enabled_at: Some(DateTime(event.created_at)),
            enabled_by: Some(Actor::User(User(event.user))),
        }))
    }
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        self.viewer_can_change(ctx).await
    }
    /// GitHub answers by permission alone, even for a closed or merged PR.
    async fn viewer_can_close(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        self.viewer_can_change(ctx).await
    }
    async fn viewer_can_reopen(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        Ok(self.source.state == "closed"
            && !self.source.merged
            && self.viewer_can_change(ctx).await?)
    }
    /// GitHub offers auto-merge only while the PR can't merge yet.
    async fn viewer_can_enable_auto_merge(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        if self.source.state != "open" || self.source.draft || !self.viewer_can_merge(ctx).await? {
            return Ok(false);
        }
        // A token that can't read the timeline can't see an existing schedule either, so it isn't
        // offered one it might already have.
        let session = ctx.data_unchecked::<Session>();
        match session
            .auto_merge_event(&self.owner, &self.repo, self.source.number)
            .await
        {
            Ok(None) => {}
            Ok(Some(_)) => return Ok(false),
            Err(forgejo::Error::MissingScope(scope)) => {
                session.hidden(&scope);
                return Ok(false);
            }
            Err(other) => return Err(error(other)),
        }
        Ok(matches!(
            self.merge_state_status(ctx).await?,
            map::MergeStateStatus::Blocked | map::MergeStateStatus::Behind
        ))
    }
    /// Forgejo lets whoever scheduled the merge, or anyone who may merge, cancel it, unless the
    /// repository is archived (`RemoveScheduledAutoMerge`, Forgejo v15.0.0
    /// `services/automerge/automerge.go`).
    async fn viewer_can_disable_auto_merge(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<bool> {
        let Some(request) = self.auto_merge_request(ctx).await? else {
            return Ok(false);
        };
        let viewer = &ctx.data_unchecked::<Session>().viewer.login;
        let scheduler = matches!(&request.enabled_by, Some(Actor::User(user)) if user.0.login.eq_ignore_ascii_case(viewer));
        if scheduler {
            return Ok(!self.archived(ctx).await?);
        }
        self.viewer_can_merge(ctx).await
    }
    async fn is_merge_queue_enabled(&self) -> bool {
        false
    }
    async fn is_in_merge_queue(&self) -> bool {
        false
    }
    async fn reaction_groups(&self) -> Option<Vec<ReactionGroup>> {
        no_reactions()
    }
    async fn milestone(&self) -> Option<Milestone> {
        self.source.milestone.clone().map(Into::into)
    }
    async fn labels(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<Option<LabelConnection>> {
        Ok(Some(
            page(
                self.source
                    .labels
                    .clone()
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                first,
                last,
                after,
                before,
            )?
            .into(),
        ))
    }
    async fn assignees(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<UserConnection> {
        Ok(page(
            self.source
                .assignees
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(User)
                .collect(),
            first,
            last,
            after,
            before,
        )?
        .into())
    }
    async fn review_requests(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<Option<ReviewRequestConnection>> {
        let reviewers = map::review_requests(&self.source)
            .into_iter()
            .map(|reviewer| ReviewRequest {
                requested_reviewer: Some(match reviewer {
                    map::Reviewer::User(user) => User(user.clone()).into(),
                    map::Reviewer::Team(team) => Team {
                        name: team.name.clone(),
                        slug: team.name.clone(),
                        organization: Organization {
                            id: team.organization.id,
                            login: team.organization.name.clone(),
                        },
                    }
                    .into(),
                }),
            })
            .collect();
        Ok(Some(page(reviewers, first, last, after, before)?.into()))
    }
    #[allow(clippy::too_many_arguments)]
    async fn reviews(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
        author: Option<String>,
        states: Option<Vec<map::PullRequestReviewState>>,
    ) -> async_graphql::Result<Option<PullRequestReviewConnection>> {
        let reviews = ctx
            .data_unchecked::<Session>()
            .reviews(&self.owner, &self.repo, self.source.number)
            .await
            .map_err(error)?;
        // Forgejo lists review requests as reviews; GitHub keeps them in reviewRequests.
        let nodes = reviews
            .into_iter()
            .filter(|review| listed(review, &ctx.data_unchecked::<Session>().viewer.login))
            .filter(|review| {
                author.as_ref().is_none_or(|author| {
                    review
                        .user
                        .as_ref()
                        .is_some_and(|user| user.login.eq_ignore_ascii_case(author))
                })
            })
            .filter(|review| {
                states
                    .as_ref()
                    .is_none_or(|states| states.contains(&map::review_state(review)))
            })
            .map(|source| PullRequestReview {
                source,
                owner: self.owner.clone(),
                repo: self.repo.clone(),
                number: self.source.number,
                head_sha: self.source.head.sha.clone(),
            })
            .collect();
        Ok(Some(page(nodes, first, last, after, before)?.into()))
    }
    async fn latest_reviews(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<Option<PullRequestReviewConnection>> {
        let reviews = ctx
            .data_unchecked::<Session>()
            .reviews(&self.owner, &self.repo, self.source.number)
            .await
            .map_err(error)?;
        let nodes = map::latest_reviews(&reviews)
            .into_iter()
            .map(|source| PullRequestReview {
                source: source.clone(),
                owner: self.owner.clone(),
                repo: self.repo.clone(),
                number: self.source.number,
                head_sha: self.source.head.sha.clone(),
            })
            .collect();
        Ok(Some(page(nodes, first, last, after, before)?.into()))
    }
    async fn review_threads(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<PullRequestReviewThreadConnection> {
        let nodes = review_threads(
            ctx,
            &self.owner,
            &self.repo,
            self.source.number,
            &self.source.user.login,
            &self.source.head.sha,
        )
        .await?;
        Ok(page(nodes, first, last, after, before)?.into())
    }
    async fn comments(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<IssueCommentConnection> {
        let session = ctx.data_unchecked::<Session>();
        // Forgejo serves a PR's conversation under `read:issue`. Without it the PR still reads, and
        // its comments read as none, rather than failing the whole document (README: tokens).
        let comments = match session
            .comments(&self.owner, &self.repo, self.source.number)
            .await
        {
            Ok(comments) => comments,
            Err(forgejo::Error::MissingScope(scope)) => {
                session.hidden(&scope);
                Vec::new()
            }
            Err(other) => return Err(error(other)),
        };
        let nodes = comments
            .into_iter()
            .map(|source| IssueComment {
                source,
                owner: self.owner.clone(),
                repo: self.repo.clone(),
                viewer: session.viewer.login.clone(),
            })
            .collect();
        Ok(page(nodes, first, last, after, before)?.into())
    }
    async fn commits(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<PullRequestCommitConnection> {
        let commits = ctx
            .data_unchecked::<Session>()
            .commits(&self.owner, &self.repo, self.source.number)
            .await
            .map_err(error)?;
        // Forgejo lists `git log base..head`, newest first; GitHub lists oldest first, so
        // gh's `commits(last: 1)` reads the head.
        let nodes = commits
            .into_iter()
            .rev()
            .map(|source| PullRequestCommit {
                commit: Commit {
                    owner: self.owner.clone(),
                    repo: self.repo.clone(),
                    sha: source.sha.clone(),
                    source: Some(source),
                },
            })
            .collect();
        Ok(page(nodes, first, last, after, before)?.into())
    }
    /// The files the pull request changes, with line counts, in Forgejo's order.
    async fn files(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<Option<PullRequestChangedFileConnection>> {
        let files = ctx
            .data_unchecked::<Session>()
            .pull_files(&self.owner, &self.repo, self.source.number)
            .await
            .map_err(error)?;
        let nodes = files
            .into_iter()
            .map(PullRequestChangedFile::from)
            .collect();
        Ok(Some(page(nodes, first, last, after, before)?.into()))
    }
    async fn merge_commit(&self) -> Option<Commit> {
        self.source
            .merge_commit_sha
            .as_deref()
            .filter(|sha| self.source.merged && !sha.is_empty())
            .map(|sha| Commit::head(&self.owner, &self.repo, sha))
    }
    /// The issues the body's closing keywords name (`Fixes #12`, `closes owner/repo#3`), which
    /// Forgejo closes when the PR merges. Forgejo has no manually linked issues, so
    /// `userLinkedOnly` lists none; issues the viewer can't read are left out.
    #[allow(clippy::too_many_arguments)]
    async fn closing_issues_references(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
        order_by: Option<super::query::IssueOrder>,
        #[graphql(default_with = "Some(false)")] user_linked_only: Option<bool>,
    ) -> async_graphql::Result<Option<IssueConnection>> {
        let _ = order_by;
        let session = ctx.data_unchecked::<Session>();
        let mut nodes = Vec::new();
        let references = if user_linked_only.unwrap_or(false) {
            Vec::new()
        } else {
            map::closing_references(&self.source.body)
        };
        for (repository, number) in references {
            let (owner, repo) =
                repository.unwrap_or_else(|| (self.owner.clone(), self.repo.clone()));
            match session.issue(&owner, &repo, number).await {
                Ok(issue) => nodes.push(Issue::new(issue, &owner, &repo)),
                Err(forgejo::Error::NotFound | forgejo::Error::Forbidden) => {}
                Err(forgejo::Error::MissingScope(scope)) => session.hidden(&scope),
                Err(e) => return Err(error(e)),
            }
        }
        Ok(Some(page(nodes, first, last, after, before)?.into()))
    }
    /// GitHub's test merge of an open pull request. Forgejo makes one but doesn't expose it.
    async fn potential_merge_commit(&self) -> Option<Commit> {
        None
    }
    async fn project_items(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<ProjectV2ItemConnection> {
        Ok(page(Vec::new(), first, last, after, before)?.into())
    }
    async fn project_cards(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<ProjectCardConnection> {
        Ok(page(Vec::new(), first, last, after, before)?.into())
    }
}

#[derive(async_graphql::SimpleObject)]
pub struct PullRequestCommit {
    pub commit: Commit,
}
#[derive(Clone)]
pub struct Commit {
    pub owner: String,
    pub repo: String,
    pub sha: String,
    pub source: Option<forgejo::PullCommit>,
}
impl Commit {
    pub fn head(owner: &str, repo: &str, sha: &str) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            sha: sha.into(),
            source: None,
        }
    }
}
#[Object]
impl Commit {
    pub(super) async fn oid(&self) -> GitObjectId {
        GitObjectId(self.sha.clone())
    }
    pub(super) async fn abbreviated_oid(&self) -> String {
        self.sha.chars().take(7).collect()
    }
    /// Forgejo's commit page has GitHub's layout, under the forge's public base.
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        let mut url = ctx.data_unchecked::<Url>().clone();
        url.path_segments_mut()
            .expect("validated HTTP URL")
            .pop_if_empty()
            .extend([&self.owner, &self.repo, "commit", &self.sha]);
        Uri(url.to_string())
    }
    async fn status_check_rollup(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Option<StatusCheckRollup>> {
        let statuses = ctx
            .data_unchecked::<Session>()
            .statuses(&self.owner, &self.repo, &self.sha)
            .await
            .map_err(error)?;
        let checks = map::checks(
            &statuses,
            &self.owner,
            &self.repo,
            ctx.data_unchecked::<Url>(),
        );
        Ok((!checks.is_empty()).then(|| StatusCheckRollup {
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            sha: self.sha.clone(),
            checks,
        }))
    }
    async fn message_headline(&self) -> String {
        self.source
            .as_ref()
            .map(|s| s.commit.message.lines().next().unwrap_or("").into())
            .unwrap_or_default()
    }
    async fn message_body(&self) -> String {
        self.source
            .as_ref()
            .and_then(|s| s.commit.message.split_once('\n'))
            .map(|(_, b)| b.trim_start_matches('\n').trim_end_matches('\n').into())
            .unwrap_or_default()
    }
    async fn authored_date(&self) -> DateTime {
        DateTime(
            self.source
                .as_ref()
                .map(|s| s.commit.author.date.clone())
                .unwrap_or_default(),
        )
    }
    async fn committed_date(&self) -> DateTime {
        DateTime(
            self.source
                .as_ref()
                .map(|s| s.commit.committer.date.clone())
                .unwrap_or_default(),
        )
    }
    async fn authors(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<GitActorConnection> {
        let nodes = self
            .source
            .as_ref()
            .map(|s| GitActor {
                name: Some(s.commit.author.name.clone()),
                email: Some(s.commit.author.email.clone()),
                user: s.author.clone().map(User),
            })
            .into_iter()
            .collect();
        Ok(page(nodes, first, last, after, before)?.into())
    }
}
#[derive(async_graphql::SimpleObject)]
pub struct GitActor {
    pub name: Option<String>,
    pub email: Option<String>,
    pub user: Option<User>,
}
