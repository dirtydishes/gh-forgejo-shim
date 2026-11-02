//! Review comments and the threads GitHub groups them into. Forgejo has no thread object; its UI
//! groups one review's code comments on the same path and signed line into a conversation
//! (`models/issues/comment_code.go:34-49`), which is what a thread becomes here.
use super::*;
use crate::{forgejo, forgejo::Session, ids::node_id};
use async_graphql::{Context, Enum, ID, Object};
use std::sync::Arc;

#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum DiffSide {
    Left,
    Right,
}
#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum PullRequestReviewCommentState {
    Pending,
    Submitted,
}

#[derive(Clone)]
pub struct PullRequestReviewComment {
    source: forgejo::ReviewComment,
    review: Arc<PullRequestReview>,
}
impl PullRequestReviewComment {
    /// Forgejo lets the poster or a repository writer edit, delete or resolve a code comment.
    async fn viewer_can_change(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let viewer = &ctx.data_unchecked::<Session>().viewer.login;
        Ok(self
            .source
            .user
            .as_ref()
            .is_some_and(|user| user.login.eq_ignore_ascii_case(viewer))
            || viewer_can_write(ctx, &self.review.owner, &self.review.repo).await?)
    }
}
#[Object]
impl PullRequestReviewComment {
    async fn id(&self) -> ID {
        let repo = format!("{}/{}", self.review.owner, self.review.repo);
        ID(node_id("PullRequestReviewComment", &repo, self.source.id))
    }
    async fn author(&self) -> Option<Actor> {
        self.source.user.clone().map(|user| User(user).into())
    }
    async fn body(&self) -> &str {
        &self.source.body
    }
    #[graphql(name = "bodyHTML")]
    async fn body_html(&self) -> Html {
        Html(crate::map::body_html(&self.source.body))
    }
    async fn created_at(&self) -> DateTime {
        DateTime(self.source.created_at.clone())
    }
    async fn diff_hunk(&self) -> &str {
        &self.source.diff_hunk
    }
    async fn state(&self) -> PullRequestReviewCommentState {
        if self.review.source.state == "PENDING" {
            PullRequestReviewCommentState::Pending
        } else {
            PullRequestReviewCommentState::Submitted
        }
    }
    async fn pull_request_review(&self) -> Option<PullRequestReview> {
        Some((*self.review).clone())
    }
    async fn url(&self) -> Uri {
        Uri(self.source.html_url.clone())
    }
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        self.viewer_can_change(ctx).await
    }
    async fn viewer_can_delete(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        self.viewer_can_change(ctx).await
    }
}

/// One review's code comments, in Forgejo's order.
pub(super) async fn review_comments(
    ctx: &Context<'_>,
    review: &PullRequestReview,
) -> async_graphql::Result<Vec<PullRequestReviewComment>> {
    let review = Arc::new(review.clone());
    Ok(ctx
        .data_unchecked::<Session>()
        .review_comments(&review.owner, &review.repo, review.number, review.source.id)
        .await
        .map_err(error)?
        .into_iter()
        .map(|source| PullRequestReviewComment {
            source,
            review: review.clone(),
        })
        .collect())
}

#[derive(Clone)]
pub struct PullRequestReviewThread {
    /// Never empty: a thread starts with its first comment.
    comments: Vec<PullRequestReviewComment>,
}
impl PullRequestReviewThread {
    fn first(&self) -> &PullRequestReviewComment {
        &self.comments[0]
    }
    fn resolved(&self) -> bool {
        self.comments.iter().any(|c| c.source.resolver.is_some())
    }
    /// Line numbers are unsigned in GitHub; 0 means Forgejo recorded none.
    fn line_number(&self) -> Option<i32> {
        i32::try_from(self.first().source.line().unsigned_abs())
            .ok()
            .filter(|line| *line != 0)
    }
}
#[Object]
impl PullRequestReviewThread {
    async fn id(&self) -> ID {
        let review = &self.first().review;
        let repo = format!("{}/{}", review.owner, review.repo);
        ID(node_id(
            "PullRequestReviewThread",
            &repo,
            self.first().source.id,
        ))
    }
    async fn comments(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<PullRequestReviewCommentConnection> {
        Ok(page(self.comments.clone(), first, last, after, before)?.into())
    }
    async fn path(&self) -> &str {
        &self.first().source.path
    }
    async fn diff_side(&self) -> DiffSide {
        if self.first().source.line() < 0 {
            DiffSide::Left
        } else {
            DiffSide::Right
        }
    }
    async fn line(&self) -> Option<i32> {
        self.line_number()
    }
    async fn original_line(&self) -> Option<i32> {
        self.line_number()
    }
    /// Forgejo comments cover one line, never a range.
    async fn start_line(&self) -> Option<i32> {
        None
    }
    async fn original_start_line(&self) -> Option<i32> {
        None
    }
    async fn start_diff_side(&self) -> Option<DiffSide> {
        None
    }
    async fn is_resolved(&self) -> bool {
        self.resolved()
    }
    async fn viewer_can_resolve(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        Ok(!self.resolved() && self.first().viewer_can_change(ctx).await?)
    }
    async fn viewer_can_unresolve(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        Ok(self.resolved() && self.first().viewer_can_change(ctx).await?)
    }
}

/// Every review's code comments, grouped into conversations, oldest conversation first.
pub(super) async fn review_threads(
    ctx: &Context<'_>,
    owner: &str,
    repo: &str,
    number: i32,
) -> async_graphql::Result<Vec<PullRequestReviewThread>> {
    let reviews = ctx
        .data_unchecked::<Session>()
        .reviews(owner, repo, number)
        .await
        .map_err(error)?;
    let mut threads: Vec<PullRequestReviewThread> = Vec::new();
    // Review requests appear as reviews but carry no comments.
    for source in reviews.into_iter().filter(|r| r.state != "REQUEST_REVIEW") {
        let review = PullRequestReview {
            source,
            owner: owner.into(),
            repo: repo.into(),
            number,
        };
        for comment in review_comments(ctx, &review).await? {
            let key = |c: &PullRequestReviewComment| {
                let source = &c.source;
                (
                    source.pull_request_review_id,
                    source.path.clone(),
                    source.line(),
                )
            };
            match threads.iter_mut().find(|t| key(t.first()) == key(&comment)) {
                Some(thread) => thread.comments.push(comment),
                None => threads.push(PullRequestReviewThread {
                    comments: vec![comment],
                }),
            }
        }
    }
    threads.sort_by_key(|t| t.first().source.id);
    Ok(threads)
}
