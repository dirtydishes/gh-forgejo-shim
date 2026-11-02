use super::query::{IssueOrder, IssueOrderField, OrderDirection};
use super::*;
use crate::{
    forgejo,
    ids::node_id,
    search::{Filters, Kind, State},
};
use async_graphql::{Context, Enum, ID, InputObject, Object, SimpleObject, Union};

#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum IssueState {
    Closed,
    Open,
}
#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum IssueStateReason {
    Completed,
    Duplicate,
    NotPlanned,
    Reopened,
}
#[derive(InputObject)]
pub struct IssueFilters {
    pub assignee: Option<String>,
    pub created_by: Option<String>,
    pub mentioned: Option<String>,
    pub labels: Option<Vec<String>>,
}
#[derive(Union)]
pub enum IssueOrPullRequest {
    Issue(Issue),
    PullRequest(PullRequest),
}

#[derive(Clone)]
pub struct Issue {
    pub source: std::sync::Arc<forgejo::Issue>,
    pub owner: String,
    pub repo: String,
}
impl Issue {
    pub fn new(source: forgejo::Issue, owner: &str, repo: &str) -> Self {
        Self {
            source: std::sync::Arc::new(source),
            owner: owner.into(),
            repo: repo.into(),
        }
    }
}
#[Object]
impl Issue {
    pub(super) async fn id(&self) -> ID {
        ID(node_id(
            "Issue",
            &format!("{}/{}", self.owner, self.repo),
            self.source.number,
        ))
    }
    async fn number(&self) -> i32 {
        self.source.number
    }
    async fn title(&self) -> &str {
        &self.source.title
    }
    async fn body(&self) -> &str {
        self.source.body.as_deref().unwrap_or("")
    }
    async fn url(&self) -> Uri {
        Uri(self.source.html_url.clone())
    }
    async fn state(&self) -> IssueState {
        if self.source.state == "closed" {
            IssueState::Closed
        } else {
            IssueState::Open
        }
    }
    async fn state_reason(
        &self,
        #[graphql(default_with = "Some(false)")] enable_duplicate: Option<bool>,
    ) -> Option<IssueStateReason> {
        let _ = enable_duplicate;
        None
    }
    async fn closed(&self) -> bool {
        self.source.state == "closed"
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
    async fn locked(&self) -> bool {
        self.source.is_locked
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
    async fn reaction_groups(&self) -> Option<Vec<ReactionGroup>> {
        no_reactions()
    }
    async fn comments(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<IssueCommentConnection> {
        let session = ctx.data_unchecked::<forgejo::Session>();
        let nodes = session
            .comments(&self.owner, &self.repo, self.source.number)
            .await
            .map_err(error)?
            .into_iter()
            .map(|source| IssueComment {
                source,
                owner: self.owner.clone(),
                viewer: session.viewer.login.clone(),
            })
            .collect();
        Ok(page(nodes, first, last, after, before)?.into())
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
    async fn repository(&self, ctx: &Context<'_>) -> async_graphql::Result<Repository> {
        Ok(Repository {
            source: ctx
                .data_unchecked::<forgejo::Session>()
                .repository(&self.owner, &self.repo)
                .await
                .map_err(error)?,
        })
    }
    async fn issue_type(&self) -> Option<IssueType> {
        None
    }
    async fn blocked_by(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<IssueConnection> {
        Ok(page(Vec::new(), first, last, after, before)?.into())
    }
    async fn blocking(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<IssueConnection> {
        Ok(page(Vec::new(), first, last, after, before)?.into())
    }
    async fn parent(&self) -> Option<Issue> {
        None
    }
    async fn sub_issues(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<IssueConnection> {
        Ok(page(Vec::new(), first, last, after, before)?.into())
    }
    async fn sub_issues_summary(&self) -> SubIssuesSummary {
        SubIssuesSummary {
            total: 0,
            completed: 0,
            percent_completed: 0,
        }
    }
}
#[derive(SimpleObject)]
pub struct SubIssuesSummary {
    pub total: i32,
    pub completed: i32,
    pub percent_completed: i32,
}
#[derive(SimpleObject)]
pub struct IssueConnection {
    pub nodes: Option<Vec<Option<Issue>>>,
    pub total_count: i32,
    pub page_info: PageInfo,
}
impl From<Page<Issue>> for IssueConnection {
    fn from(p: Page<Issue>) -> Self {
        Self {
            nodes: Some(p.nodes.into_iter().map(Some).collect()),
            total_count: p.total,
            page_info: p.info,
        }
    }
}

pub async fn list(
    ctx: &Context<'_>,
    repo: &forgejo::Repository,
    states: Option<Vec<IssueState>>,
    filters: Option<IssueFilters>,
    first: Option<i32>,
    after: Option<String>,
    order: Option<IssueOrder>,
) -> async_graphql::Result<IssueConnection> {
    if order.as_ref().is_some_and(|o| {
        o.field != IssueOrderField::CreatedAt || o.direction != OrderDirection::Desc
    }) {
        return Err("only CREATED_AT DESC ordering is supported".into());
    }
    let session = ctx.data_unchecked::<forgejo::Session>();
    let login = |value: String| {
        if value == "@me" {
            session.viewer.login.clone()
        } else {
            value
        }
    };
    let filters = filters
        .map(|f| Filters {
            author: f.created_by.map(login),
            assignee: f.assignee.map(login),
            mention: f.mentioned.map(login),
            labels: f.labels.unwrap_or_default(),
            ..Filters::default()
        })
        .unwrap_or_default();
    let state = match states.as_deref() {
        Some([IssueState::Open]) => State::Open,
        Some([IssueState::Closed]) => State::Closed,
        _ => State::All,
    };
    let issues = session
        .issues(&repo.owner.login, &repo.name, state, Kind::Issues, &filters)
        .await
        .map_err(error)?;
    let nodes = issues
        .into_iter()
        .filter(|issue| {
            states.as_ref().is_none_or(|states| {
                states.contains(&if issue.state == "closed" {
                    IssueState::Closed
                } else {
                    IssueState::Open
                })
            }) && filters.labels.iter().all(|name| {
                issue
                    .labels
                    .iter()
                    .any(|l| l.name.eq_ignore_ascii_case(name))
            })
        })
        .map(|source| Issue::new(source, &repo.owner.login, &repo.name))
        .collect();
    Ok(page(nodes, first, None, after, None)?.into())
}

#[derive(SimpleObject)]
pub struct IssueType {
    pub id: ID,
    pub name: String,
    pub description: Option<String>,
    pub color: IssueTypeColor,
}
#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum IssueTypeColor {
    Gray,
    Blue,
    Green,
    Yellow,
    Orange,
    Red,
    Pink,
    Purple,
}
