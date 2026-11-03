use super::*;
use crate::{forgejo, forgejo::Session, ids::node_id, map};
use async_graphql::{
    Context, Enum, ID, Interface, Object, Scalar, ScalarType, SimpleObject, Union, Value,
};

macro_rules! string_scalar {
    ($rust:ident, $name:literal) => {
        #[derive(Clone)]
        pub struct $rust(pub String);
        #[Scalar(name = $name)]
        impl ScalarType for $rust {
            fn parse(value: Value) -> async_graphql::InputValueResult<Self> {
                match value {
                    Value::String(s) => Ok(Self(s)),
                    other => Err(async_graphql::InputValueError::expected_type(other)),
                }
            }
            fn to_value(&self) -> Value {
                Value::String(self.0.clone())
            }
        }
    };
}
string_scalar!(Uri, "URI");
string_scalar!(GitSshRemote, "GitSSHRemote");
string_scalar!(DateTime, "DateTime");
string_scalar!(GitObjectId, "GitObjectID");
string_scalar!(BigInt, "BigInt");
string_scalar!(Html, "HTML");

#[derive(Clone, Interface)]
#[graphql(
    field(name = "login", ty = "String"),
    field(
        name = "avatar_url",
        ty = "Uri",
        arg(name = "size", ty = "Option<i32>")
    )
)]
pub enum Actor {
    User(User),
    Bot(Bot),
}
#[derive(Clone, Interface)]
#[graphql(field(name = "id", ty = "ID"), field(name = "login", ty = "String"))]
pub enum RepositoryOwner {
    User(User),
    Organization(Organization),
}
#[derive(Clone)]
pub struct User(pub forgejo::User);
#[Object]
impl User {
    async fn login(&self) -> String {
        self.0.login.clone()
    }
    async fn id(&self) -> ID {
        ID(node_id("User", "", self.0.id))
    }
    async fn name(&self) -> Option<String> {
        self.0.full_name.clone().filter(|s| !s.is_empty())
    }
    async fn database_id(&self) -> Option<i32> {
        i32::try_from(self.0.id).ok()
    }
    /// Forgejo's avatar at its own size; clients render whatever URL they get.
    async fn avatar_url(&self, _size: Option<i32>) -> Uri {
        Uri(self.0.avatar_url.clone())
    }
}
#[derive(Clone)]
pub struct Bot(pub forgejo::User);
#[Object]
impl Bot {
    async fn login(&self) -> String {
        self.0.login.clone()
    }
    async fn avatar_url(&self, _size: Option<i32>) -> Uri {
        Uri(self.0.avatar_url.clone())
    }
}

/// Whether a review belongs in GitHub's lists: review requests are reviews in Forgejo, and Forgejo
/// shows site admins other users' pending reviews, which GitHub shows no one
/// (`services/convert/pull_review.go:74-77`).
pub(super) fn listed(review: &forgejo::Review, viewer: &str) -> bool {
    review.state != "REQUEST_REVIEW"
        && (review.state != "PENDING"
            || review
                .user
                .as_ref()
                .is_some_and(|user| user.login.eq_ignore_ascii_case(viewer)))
}

/// Whether the viewer may write to the repository, which is what lets them edit or close others'
/// pull requests and comments. Forgejo checks write access to its pull request or issue unit, but
/// its API reports only code access, which stands in for both (MAPPING §7).
pub(super) async fn viewer_can_write(
    ctx: &Context<'_>,
    owner: &str,
    repo: &str,
) -> async_graphql::Result<bool> {
    let repository = ctx
        .data_unchecked::<Session>()
        .repository(owner, repo)
        .await
        .map_err(error)?;
    Ok(repository.permissions.is_some_and(|p| p.admin || p.push))
}
/// Whether the viewer may change something in the repository: its author, or a writer, and no one
/// in an archived repository, where Forgejo's write routes answer 423 (`mustNotBeArchived`) and
/// GitHub reports every write flag false.
pub(super) async fn viewer_may_change(
    ctx: &Context<'_>,
    owner: &str,
    repo: &str,
    author: bool,
) -> async_graphql::Result<bool> {
    let repository = ctx
        .data_unchecked::<Session>()
        .repository(owner, repo)
        .await
        .map_err(error)?;
    Ok(!repository.archived
        && (author || repository.permissions.is_some_and(|p| p.admin || p.push)))
}
#[derive(Clone)]
pub struct Organization {
    pub id: i64,
    pub login: String,
}
impl From<forgejo::User> for Organization {
    fn from(user: forgejo::User) -> Self {
        Self {
            id: user.id,
            login: user.login,
        }
    }
}
#[Object]
impl Organization {
    async fn id(&self) -> ID {
        ID(node_id("Organization", "", self.id))
    }
    async fn login(&self) -> String {
        self.login.clone()
    }
}
#[derive(Clone, SimpleObject)]
pub struct Team {
    pub name: String,
    pub slug: String,
    pub organization: Organization,
}
#[derive(Clone, Union)]
pub enum RequestedReviewer {
    User(User),
    Bot(Bot),
    Team(Team),
}
#[derive(Clone, SimpleObject)]
pub struct ReviewRequest {
    pub requested_reviewer: Option<RequestedReviewer>,
}

#[derive(SimpleObject)]
pub struct Label {
    pub id: ID,
    pub name: String,
    pub description: Option<String>,
    pub color: String,
}
impl From<forgejo::Label> for Label {
    fn from(l: forgejo::Label) -> Self {
        Self {
            id: ID(node_id("Label", "", l.id)),
            name: l.name,
            description: l.description,
            color: l.color,
        }
    }
}
#[derive(SimpleObject)]
pub struct Milestone {
    pub number: i32,
    pub title: String,
    pub description: Option<String>,
    pub due_on: Option<DateTime>,
}
impl From<forgejo::Milestone> for Milestone {
    fn from(m: forgejo::Milestone) -> Self {
        Self {
            number: m.id as i32,
            title: m.title,
            description: m.description,
            due_on: m.due_on.map(DateTime),
        }
    }
}
#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum CommentAuthorAssociation {
    Collaborator,
    Contributor,
    FirstTimer,
    FirstTimeContributor,
    Mannequin,
    Member,
    None,
    Owner,
}
#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum ReactionContent {
    ThumbsUp,
    ThumbsDown,
    Laugh,
    Hooray,
    Confused,
    Heart,
    Rocket,
    Eyes,
}
#[derive(SimpleObject)]
pub struct ReactionGroup {
    pub content: ReactionContent,
    pub users: ReactingUserConnection,
}
#[derive(SimpleObject)]
pub struct ReactingUserConnection {
    pub total_count: i32,
}

pub fn no_reactions() -> Option<Vec<ReactionGroup>> {
    Some(
        [
            ReactionContent::ThumbsUp,
            ReactionContent::ThumbsDown,
            ReactionContent::Laugh,
            ReactionContent::Hooray,
            ReactionContent::Confused,
            ReactionContent::Heart,
            ReactionContent::Rocket,
            ReactionContent::Eyes,
        ]
        .into_iter()
        .map(|content| ReactionGroup {
            content,
            users: ReactingUserConnection { total_count: 0 },
        })
        .collect(),
    )
}

#[derive(Clone)]
pub struct PullRequestReview {
    pub source: forgejo::Review,
    pub owner: String,
    pub repo: String,
    pub number: i32,
}
#[Object]
impl PullRequestReview {
    async fn id(&self) -> ID {
        ID(node_id(
            "PullRequestReview",
            &format!("{}/{}", self.owner, self.repo),
            self.source.id,
        ))
    }
    async fn author(&self) -> Option<Actor> {
        self.source.user.clone().map(|user| User(user).into())
    }
    async fn author_association(&self) -> CommentAuthorAssociation {
        if self
            .source
            .user
            .as_ref()
            .is_some_and(|user| user.login.eq_ignore_ascii_case(&self.owner))
        {
            CommentAuthorAssociation::Owner
        } else {
            CommentAuthorAssociation::None
        }
    }
    /// Forgejo sends every review's creation time as `submitted_at`, pending ones included; GitHub
    /// gives a pending review none.
    async fn submitted_at(&self) -> Option<DateTime> {
        (self.source.state != "PENDING")
            .then(|| self.source.submitted_at.clone().map(DateTime))
            .flatten()
    }
    /// Forgejo's `submitted_at` is the review's creation time (`services/convert/pull_review.go:35`).
    async fn created_at(&self) -> DateTime {
        DateTime(
            self.source
                .submitted_at
                .clone()
                .or_else(|| self.source.updated_at.clone())
                .unwrap_or_default(),
        )
    }
    /// A pending review has no comment yet, so Forgejo sends an empty URL (Forgejo v15.0.0
    /// `models/issues/review.go:1009-1021`); it falls back to the PR's page as Forgejo gives it, so
    /// every review URL shares Forgejo's root.
    async fn url(&self) -> Uri {
        let url = if self.source.html_url.is_empty() {
            &self.source.pull_request_url
        } else {
            &self.source.html_url
        };
        Uri(crate::map::pull_web_url(url))
    }
    async fn is_minimized(&self) -> bool {
        false
    }
    /// The body is a comment the poster or a repository writer may edit (`issue_comment.go:611`).
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let viewer = &ctx.data_unchecked::<Session>().viewer.login;
        let author = self
            .source
            .user
            .as_ref()
            .is_some_and(|user| user.login.eq_ignore_ascii_case(viewer));
        viewer_may_change(ctx, &self.owner, &self.repo, author).await
    }
    async fn body(&self) -> &str {
        &self.source.body
    }
    async fn state(&self) -> map::PullRequestReviewState {
        map::review_state(&self.source)
    }
    async fn commit(&self) -> Option<Commit> {
        Some(Commit::head(
            &self.owner,
            &self.repo,
            &self.source.commit_id,
        ))
    }
    async fn comments(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<PullRequestReviewCommentConnection> {
        let nodes = review_comments(ctx, self).await?;
        Ok(page(nodes, first, last, after, before)?.into())
    }
    async fn reaction_groups(&self) -> Option<Vec<ReactionGroup>> {
        no_reactions()
    }
}

pub struct IssueComment {
    pub source: forgejo::Comment,
    pub owner: String,
    pub repo: String,
    pub viewer: String,
}
impl IssueComment {
    /// Forgejo lets the poster or a repository writer edit and delete a comment.
    async fn viewer_can_change(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let author = self.source.user.login.eq_ignore_ascii_case(&self.viewer);
        viewer_may_change(ctx, &self.owner, &self.repo, author).await
    }
}
#[Object]
impl IssueComment {
    async fn id(&self) -> ID {
        ID(node_id("IssueComment", "", self.source.id))
    }
    async fn author(&self) -> Option<Actor> {
        Some(User(self.source.user.clone()).into())
    }
    async fn author_association(&self) -> CommentAuthorAssociation {
        if self.source.user.login.eq_ignore_ascii_case(&self.owner) {
            CommentAuthorAssociation::Owner
        } else {
            CommentAuthorAssociation::None
        }
    }
    async fn body(&self) -> &str {
        &self.source.body
    }
    #[graphql(name = "bodyHTML")]
    async fn body_html(&self) -> Html {
        Html(map::body_html(&self.source.body))
    }
    async fn created_at(&self) -> DateTime {
        DateTime(self.source.created_at.clone())
    }
    async fn includes_created_edit(&self) -> bool {
        false
    }
    async fn is_minimized(&self) -> bool {
        false
    }
    async fn minimized_reason(&self) -> Option<String> {
        None
    }
    async fn reaction_groups(&self) -> Option<Vec<ReactionGroup>> {
        no_reactions()
    }
    /// A comment on a pull request links under its GitHub-style URL.
    async fn url(&self) -> Uri {
        Uri(crate::map::pull_web_url(&self.source.html_url))
    }
    async fn viewer_did_author(&self) -> bool {
        self.source.user.login.eq_ignore_ascii_case(&self.viewer)
    }
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        self.viewer_can_change(ctx).await
    }
    async fn viewer_can_delete(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        self.viewer_can_change(ctx).await
    }
}
#[derive(SimpleObject)]
pub struct AutoMergeRequest {
    pub author_email: Option<String>,
    pub commit_body: Option<String>,
    pub commit_headline: Option<String>,
    pub merge_method: PullRequestMergeMethod,
    pub enabled_at: Option<DateTime>,
    pub enabled_by: Option<Actor>,
}
#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum PullRequestMergeMethod {
    Merge,
    Rebase,
    Squash,
}
#[derive(Clone, SimpleObject)]
pub struct Ref {
    pub name: String,
    pub branch_protection_rule: Option<BranchProtectionRule>,
}
#[derive(Clone, SimpleObject)]
pub struct BranchProtectionRule {
    pub requires_strict_status_checks: bool,
    pub required_approving_review_count: Option<i32>,
}
#[derive(SimpleObject)]
#[graphql(complex)]
pub struct ProjectV2Item {
    pub id: ID,
    pub project: ProjectV2,
}
#[async_graphql::ComplexObject]
impl ProjectV2Item {
    async fn field_value_by_name(&self, name: String) -> Option<ProjectV2ItemFieldValue> {
        let _ = name;
        None
    }
}
#[derive(SimpleObject)]
pub struct ProjectV2 {
    pub id: ID,
    pub title: String,
}
#[derive(Union)]
pub enum ProjectV2ItemFieldValue {
    ProjectV2ItemFieldSingleSelectValue(ProjectV2ItemFieldSingleSelectValue),
}
#[derive(SimpleObject)]
pub struct ProjectV2ItemFieldSingleSelectValue {
    pub option_id: Option<String>,
    pub name: Option<String>,
}
