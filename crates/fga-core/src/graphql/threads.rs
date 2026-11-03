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
        let author = self
            .source
            .user
            .as_ref()
            .is_some_and(|user| user.login.eq_ignore_ascii_case(viewer));
        viewer_may_change(ctx, &self.review.owner, &self.review.repo, author).await
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
        Uri(crate::map::pull_web_url(&self.source.html_url))
    }
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        self.viewer_can_change(ctx).await
    }
    async fn viewer_can_delete(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        self.viewer_can_change(ctx).await
    }
}

/// One review's code comments, oldest first. Forgejo builds the list by walking a map, so its own
/// order changes between requests (`services/convert/pull_review.go:117-128`).
pub(super) async fn review_comments(
    ctx: &Context<'_>,
    review: &PullRequestReview,
) -> async_graphql::Result<Vec<PullRequestReviewComment>> {
    // Reviews without code comments, such as plain approvals, need no read. Forgejo also reports 0
    // when its count query fails, which would hide that review's comments; an upstream fault.
    if review.source.comments_count == Some(0) {
        return Ok(Vec::new());
    }
    let review = Arc::new(review.clone());
    let mut comments = ctx
        .data_unchecked::<Session>()
        .review_comments(&review.owner, &review.repo, review.number, review.source.id)
        .await
        .map_err(error)?;
    comments.sort_by_key(|comment| comment.id);
    Ok(comments
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
    /// The pull request's author, who may resolve any of its conversations.
    pr_author: Arc<str>,
    /// The pull request's head commit, which the thread's review must have been made on for its
    /// line to be current.
    head_sha: Arc<str>,
}
impl PullRequestReviewThread {
    /// Forgejo lets the PR's author or a repository writer resolve a conversation
    /// (`CanMarkConversation`, Forgejo v15.0.0 `models/issues/review.go:900-928`), as GitHub does.
    /// Forgejo also lets an official reviewer without write access; the gateway can't tell.
    async fn viewer_can_mark(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let review = &self.first().review;
        let viewer = &ctx.data_unchecked::<Session>().viewer.login;
        let author = self.pr_author.eq_ignore_ascii_case(viewer);
        viewer_may_change(ctx, &review.owner, &review.repo, author).await
    }
    fn first(&self) -> &PullRequestReviewComment {
        &self.comments[0]
    }
    fn resolved(&self) -> bool {
        self.comments.iter().any(|c| c.source.resolver.is_some())
    }
    fn line_number(&self) -> Option<i32> {
        commented_line(&self.first().source)
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
    /// GitHub's current line, null once the thread is outdated. Forgejo's API doesn't say whether
    /// a later push moved or deleted the line, so the line is current only while the PR's head is
    /// still the commit its review was made on.
    async fn line(&self) -> Option<i32> {
        self.line_number()
            .filter(|_| self.first().review.source.commit_id == *self.head_sha)
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
        Ok(!self.resolved() && self.viewer_can_mark(ctx).await?)
    }
    async fn viewer_can_unresolve(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        Ok(self.resolved() && self.viewer_can_mark(ctx).await?)
    }
}

/// The commented line as the reviewer saw it: the last line of the comment's diff hunk, which
/// Forgejo cuts to end there (`CutDiffAroundLine`, Forgejo v15.0.0 `modules/git/diff.go`). Without
/// a hunk (Forgejo's `CODE_COMMENT_LINES = 0`, or one that can't be read) a new-side line is
/// unknown: its `position` is the line in the commit that last changed it (`LineBlame`), which can
/// be another line entirely. An old-side position is never blamed, so it stands.
fn commented_line(source: &forgejo::ReviewComment) -> Option<i32> {
    // Forgejo blames only new-side lines, so an old-side position is the line the reviewer chose.
    let old = source.line() < 0;
    let line = hunk_end(&source.diff_hunk, old).or(old.then_some(source.original_position))?;
    i32::try_from(line).ok().filter(|line| *line != 0)
}

/// The number, on the old or new side, of a diff hunk's last line.
fn hunk_end(hunk: &str, old: bool) -> Option<u64> {
    let mut lines = hunk.lines();
    let header = lines.next()?.strip_prefix("@@ -")?;
    let (old_start, rest) = header.split_once(" +")?;
    let new_start = rest.split_once(' ')?.0;
    let start = |range: &str| range.split(',').next()?.parse::<u64>().ok();
    let (mut old_line, mut new_line) = (start(old_start)?, start(new_start)?);
    let mut last = None;
    for line in lines {
        match line.as_bytes().first() {
            Some(b'+') => {
                last = (!old).then_some(new_line);
                new_line += 1;
            }
            Some(b'-') => {
                last = old.then_some(old_line);
                old_line += 1;
            }
            Some(b'\\') => {}
            _ => {
                last = Some(if old { old_line } else { new_line });
                old_line += 1;
                new_line += 1;
            }
        }
    }
    last
}

/// Every review's code comments, grouped into conversations, oldest conversation first.
pub(super) async fn review_threads(
    ctx: &Context<'_>,
    owner: &str,
    repo: &str,
    number: i32,
    pr_author: &str,
    head_sha: &str,
) -> async_graphql::Result<Vec<PullRequestReviewThread>> {
    let session = ctx.data_unchecked::<Session>();
    let reviews = session.reviews(owner, repo, number).await.map_err(error)?;
    let pr_author: Arc<str> = pr_author.into();
    let head_sha: Arc<str> = head_sha.into();
    let mut threads: Vec<PullRequestReviewThread> = Vec::new();
    for source in reviews
        .into_iter()
        .filter(|r| listed(r, &session.viewer.login))
    {
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
                    pr_author: pr_author.clone(),
                    head_sha: head_sha.clone(),
                }),
            }
        }
    }
    threads.sort_by_key(|t| t.first().source.id);
    Ok(threads)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(diff_hunk: &str, position: u64, original_position: u64) -> forgejo::ReviewComment {
        serde_json::from_value(
            serde_json::json!({"id":1,"body":"","user":null,"resolver":null,
            "pull_request_review_id":1,"created_at":"","path":"a.rs","diff_hunk":diff_hunk,
            "position":position,"original_position":original_position,"html_url":""}),
        )
        .unwrap()
    }

    /// Forgejo blames a comment's line to the commit that last changed it, so its `position` is
    /// no stand-in for the line the reviewer saw: without a hunk, the line is unknown.
    #[test]
    fn a_comment_without_a_hunk_has_no_line() {
        assert_eq!(
            commented_line(&comment("@@ -5,2 +5,3 @@\n a\n+b\n c", 3, 0)),
            Some(7)
        );
        assert_eq!(
            commented_line(&comment("@@ -5,1 +5,0 @@\n-x", 0, 5)),
            Some(5)
        );
        assert_eq!(commented_line(&comment("", 3, 0)), None);
        assert_eq!(commented_line(&comment("not a hunk", 3, 0)), None);
        // Forgejo blames only new-side lines: an old-side position is the reviewer's own line.
        assert_eq!(commented_line(&comment("", 0, 5)), Some(5));
    }
}
