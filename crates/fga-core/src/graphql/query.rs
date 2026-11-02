use super::*;
use crate::{
    forgejo::{self, Session},
    ids,
};
use async_graphql::{Context, Enum, ID, InputObject, Interface, Object, Union};

pub struct Query;
#[Object]
impl Query {
    async fn viewer(&self, ctx: &Context<'_>) -> User {
        User(ctx.data_unchecked::<Session>().viewer.clone())
    }
    async fn repository(
        &self,
        ctx: &Context<'_>,
        owner: String,
        name: String,
    ) -> async_graphql::Result<Option<Repository>> {
        let source = ctx
            .data_unchecked::<Session>()
            .repository(&owner, &name)
            .await
            .map_err(error)?;
        Ok(Some(Repository { source }))
    }
    async fn node(&self, ctx: &Context<'_>, id: ID) -> async_graphql::Result<Option<Node>> {
        let Some(id) = ids::decode(&id) else {
            return Ok(None);
        };
        let session = ctx.data_unchecked::<Session>();
        let result = match id.kind.as_str() {
            "PullRequest" => session
                .pull_request(&id.owner, &id.repository, id.number)
                .await
                .map(|source| PullRequest::new(source, &id.owner, &id.repository).into()),
            "Issue" => session
                .issue(&id.owner, &id.repository, id.number)
                .await
                .map(|source| Issue::new(source, &id.owner, &id.repository).into()),
            _ => return Ok(None),
        };
        match result {
            Ok(node) => Ok(Some(node)),
            Err(forgejo::Error::NotFound) => Ok(None),
            Err(e) => Err(error(e)),
        }
    }
    /// Repo-scoped PR and issue search, including gh status and list filters.
    async fn search(
        &self,
        ctx: &Context<'_>,
        query: String,
        r#type: SearchType,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
    ) -> async_graphql::Result<SearchResultItemConnection> {
        if !matches!(r#type, SearchType::Issue | SearchType::IssueAdvanced) {
            return Err("only ISSUE searches are supported".into());
        }
        super::search::search(ctx, &query, first, last, after).await
    }
}
#[derive(Interface)]
#[graphql(field(name = "id", ty = "ID"))]
pub enum Node {
    PullRequest(PullRequest),
    Issue(Issue),
}
#[derive(Clone, Union)]
pub enum SearchResultItem {
    PullRequest(PullRequest),
    Issue(Issue),
}
#[derive(Clone, Copy, Eq, PartialEq, Enum)]
pub enum SearchType {
    Discussion,
    Issue,
    IssueAdvanced,
    IssueSemantic,
    IssueHybrid,
    Repository,
    User,
}
#[derive(Clone, Copy, Eq, PartialEq, Enum)]
pub enum OrderDirection {
    Asc,
    Desc,
}
#[derive(Clone, Copy, Eq, PartialEq, Enum)]
pub enum IssueOrderField {
    Comments,
    CreatedAt,
    UpdatedAt,
}
#[derive(InputObject)]
pub struct IssueOrder {
    pub field: IssueOrderField,
    pub direction: OrderDirection,
}

pub struct Repository {
    pub source: forgejo::Repository,
}
#[Object]
impl Repository {
    async fn id(&self) -> ID {
        ID(ids::node_id(
            "Repository",
            &self.source.full_name,
            self.source.id,
        ))
    }
    async fn name(&self) -> &str {
        &self.source.name
    }
    async fn name_with_owner(&self) -> &str {
        &self.source.full_name
    }
    async fn url(&self) -> Uri {
        Uri(self.source.html_url.clone())
    }
    async fn owner(&self, ctx: &Context<'_>) -> async_graphql::Result<RepositoryOwner> {
        let kind = ctx
            .data_unchecked::<Session>()
            .owner_type(&self.source.owner.login)
            .await
            .map_err(error)?;
        Ok(if kind == "Organization" {
            Organization::from(self.source.owner.clone()).into()
        } else {
            User(self.source.owner.clone()).into()
        })
    }
    async fn description(&self) -> Option<String> {
        Some(self.source.description.clone())
    }
    async fn is_private(&self) -> bool {
        self.source.private
    }
    async fn is_fork(&self) -> bool {
        self.source.fork
    }
    async fn is_archived(&self) -> bool {
        self.source.archived
    }
    async fn merge_commit_allowed(&self) -> bool {
        self.source.allow_merge_commits
    }
    async fn rebase_merge_allowed(&self) -> bool {
        self.source.allow_rebase
    }
    async fn squash_merge_allowed(&self) -> bool {
        self.source.allow_squash_merge
    }
    async fn has_issues_enabled(&self) -> bool {
        self.source.has_issues
    }
    async fn has_wiki_enabled(&self) -> bool {
        self.source.has_wiki
    }
    async fn database_id(&self) -> Option<i32> {
        i32::try_from(self.source.id).ok()
    }
    async fn ssh_url(&self) -> GitSshRemote {
        GitSshRemote(self.source.ssh_url.clone())
    }
    async fn viewer_permission(&self) -> Option<RepositoryPermission> {
        self.source.permissions.as_ref().map(|p| {
            if p.admin {
                RepositoryPermission::Admin
            } else if p.push {
                RepositoryPermission::Write
            } else {
                RepositoryPermission::Read
            }
        })
    }
    async fn parent(&self) -> Option<Repository> {
        self.source
            .parent
            .as_ref()
            .map(|r| Repository { source: *r.clone() })
    }
    async fn issue(&self, ctx: &Context<'_>, number: i32) -> async_graphql::Result<Option<Issue>> {
        let source = ctx
            .data_unchecked::<Session>()
            .issue(&self.source.owner.login, &self.source.name, number)
            .await
            .map_err(error)?;
        Ok(Some(Issue::new(
            source,
            &self.source.owner.login,
            &self.source.name,
        )))
    }
    async fn issue_or_pull_request(
        &self,
        ctx: &Context<'_>,
        number: i32,
    ) -> async_graphql::Result<Option<IssueOrPullRequest>> {
        match ctx
            .data_unchecked::<Session>()
            .issue(&self.source.owner.login, &self.source.name, number)
            .await
        {
            Ok(source) => Ok(Some(
                Issue::new(source, &self.source.owner.login, &self.source.name).into(),
            )),
            Err(forgejo::Error::NotFound) => {
                Ok(self.pull_request(ctx, number).await?.map(Into::into))
            }
            Err(e) => Err(error(e)),
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn issues(
        &self,
        ctx: &Context<'_>,
        states: Option<Vec<IssueState>>,
        filter_by: Option<IssueFilters>,
        first: Option<i32>,
        after: Option<String>,
        order_by: Option<IssueOrder>,
    ) -> async_graphql::Result<IssueConnection> {
        super::issues::list(ctx, &self.source, states, filter_by, first, after, order_by).await
    }
    async fn default_branch_ref(&self) -> Option<Ref> {
        Some(Ref {
            name: self.source.default_branch.clone(),
            branch_protection_rule: None,
        })
    }
    async fn pull_request(
        &self,
        ctx: &Context<'_>,
        number: i32,
    ) -> async_graphql::Result<Option<PullRequest>> {
        let source = ctx
            .data_unchecked::<Session>()
            .pull_request(&self.source.owner.login, &self.source.name, number)
            .await
            .map_err(error)?;
        Ok(Some(PullRequest::new(
            source,
            &self.source.owner.login,
            &self.source.name,
        )))
    }
    #[allow(clippy::too_many_arguments)]
    async fn pull_requests(
        &self,
        ctx: &Context<'_>,
        head_ref_name: Option<String>,
        base_ref_name: Option<String>,
        states: Option<Vec<crate::map::PullRequestState>>,
        first: Option<i32>,
        after: Option<String>,
        order_by: Option<IssueOrder>,
    ) -> async_graphql::Result<PullRequestConnection> {
        if order_by.as_ref().is_some_and(|o| {
            o.field != IssueOrderField::CreatedAt || o.direction != OrderDirection::Desc
        }) {
            return Err("only CREATED_AT DESC ordering is supported".into());
        }
        let owner = &self.source.owner.login;
        let pulls = ctx
            .data_unchecked::<Session>()
            .pull_requests(
                owner,
                &self.source.name,
                "all",
                head_ref_name.as_deref(),
                None,
            )
            .await
            .map_err(error)?;
        Ok(page(
            pulls
                .into_iter()
                .filter(|pr| {
                    base_ref_name
                        .as_ref()
                        .is_none_or(|base| pr.base.ref_name == *base)
                        && states
                            .as_ref()
                            .is_none_or(|states| states.contains(&crate::map::state(pr)))
                })
                .map(|source| PullRequest::new(source, owner, &self.source.name))
                .collect(),
            first,
            None,
            after,
            None,
        )?
        .into())
    }
}

#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum RepositoryPermission {
    Admin,
    Maintain,
    Read,
    Triage,
    Write,
}
