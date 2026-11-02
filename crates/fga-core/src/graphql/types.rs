use super::*;
use crate::{forgejo, ids::node_id, map};
use async_graphql::{Enum, ID, Interface, Object, Scalar, ScalarType, SimpleObject, Union, Value};

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

#[derive(Clone, Interface)]
#[graphql(field(name = "login", ty = "String"))]
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
}
#[derive(Clone)]
pub struct Bot(pub forgejo::User);
#[Object]
impl Bot {
    async fn login(&self) -> String {
        self.0.login.clone()
    }
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

pub struct PullRequestReview {
    pub source: forgejo::Review,
    pub owner: String,
    pub repo: String,
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
    async fn submitted_at(&self) -> Option<DateTime> {
        self.source.submitted_at.clone().map(DateTime)
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
    async fn reaction_groups(&self) -> Option<Vec<ReactionGroup>> {
        no_reactions()
    }
}

pub struct IssueComment {
    pub source: forgejo::Comment,
    pub owner: String,
    pub viewer: String,
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
