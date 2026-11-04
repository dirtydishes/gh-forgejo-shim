use super::*;
use crate::{
    forgejo::Session,
    ids,
    map::{self, Check},
};
use async_graphql::{Context, ID, Object, SimpleObject, Union};
use std::sync::Arc;

pub struct StatusCheckRollup {
    pub owner: String,
    pub repo: String,
    pub sha: String,
    pub checks: Vec<Check>,
}
#[Object]
impl StatusCheckRollup {
    async fn id(&self) -> ID {
        ID(ids::node_id(
            "StatusCheckRollup",
            &format!("{}/{}", self.owner, self.repo),
            &self.sha,
        ))
    }
    async fn state(&self) -> map::StatusState {
        map::rollup(&self.checks).expect("nonempty rollup")
    }
    async fn contexts(
        &self,
        first: Option<i32>,
        last: Option<i32>,
        after: Option<String>,
        before: Option<String>,
    ) -> async_graphql::Result<StatusCheckRollupContextConnection> {
        let checks = Arc::new(self.checks.clone());
        let nodes = self
            .checks
            .iter()
            .cloned()
            .map(|check| match check {
                Check::Action {
                    source,
                    workflow,
                    job,
                    event,
                    details_url,
                    status,
                    conclusion,
                } => StatusCheckRollupContext::CheckRun(CheckRun {
                    owner: self.owner.clone(),
                    repo: self.repo.clone(),
                    id: source.id,
                    context: source.context,
                    name: job,
                    check_suite: CheckSuite {
                        workflow_run: Some(WorkflowRun {
                            event,
                            workflow: Workflow { name: workflow },
                        }),
                    },
                    status,
                    conclusion,
                    started_at: Some(DateTime(source.created_at)),
                    completed_at: (status == map::CheckStatusState::Completed)
                        .then_some(DateTime(source.updated_at)),
                    details_url: Some(Uri(details_url.to_string())),
                }),
                Check::Status {
                    source,
                    state,
                    target_url,
                } => StatusCheckRollupContext::StatusContext(StatusContext {
                    owner: self.owner.clone(),
                    repo: self.repo.clone(),
                    context: source.context,
                    state,
                    target_url: target_url.map(|u| Uri(u.to_string())),
                    description: source.description,
                    created_at: DateTime(source.created_at),
                }),
            })
            .collect();
        let p = page(nodes, first, last, after, before)?;
        Ok(StatusCheckRollupContextConnection {
            nodes: Some(p.nodes.into_iter().map(Some).collect()),
            page_info: p.info,
            total_count: p.total,
            checks,
        })
    }
}

#[derive(Union)]
pub enum StatusCheckRollupContext {
    CheckRun(CheckRun),
    StatusContext(StatusContext),
}
#[derive(SimpleObject)]
#[graphql(complex)]
pub struct StatusCheckRollupContextConnection {
    pub nodes: Option<Vec<Option<StatusCheckRollupContext>>>,
    pub page_info: PageInfo,
    pub total_count: i32,
    #[graphql(skip)]
    pub checks: Arc<Vec<Check>>,
}
#[async_graphql::ComplexObject]
impl StatusCheckRollupContextConnection {
    async fn check_run_count(&self) -> i32 {
        self.checks
            .iter()
            .filter(|c| c.run_state().is_some())
            .count() as i32
    }
    async fn status_context_count(&self) -> i32 {
        self.checks
            .iter()
            .filter(|c| c.run_state().is_none())
            .count() as i32
    }
    async fn check_run_counts_by_state(&self) -> Option<Vec<CheckRunStateCount>> {
        use map::CheckRunState::*;
        Some(
            [
                ActionRequired,
                Cancelled,
                Completed,
                Failure,
                InProgress,
                Neutral,
                Pending,
                Queued,
                Skipped,
                Stale,
                StartupFailure,
                Success,
                TimedOut,
                Waiting,
            ]
            .into_iter()
            .map(|state| CheckRunStateCount {
                state,
                count: self
                    .checks
                    .iter()
                    .filter(|c| c.run_state() == Some(state))
                    .count() as i32,
            })
            .collect(),
        )
    }
    async fn status_context_counts_by_state(&self) -> Option<Vec<StatusContextStateCount>> {
        use map::StatusState::*;
        Some(
            [Expected, Error, Failure, Pending, Success]
                .into_iter()
                .map(|state| StatusContextStateCount {
                    state,
                    count: self
                        .checks
                        .iter()
                        .filter(|c| matches!(c, Check::Status {state:s, ..} if *s == state))
                        .count() as i32,
                })
                .collect(),
        )
    }
}
#[derive(SimpleObject)]
pub struct CheckRunStateCount {
    pub state: map::CheckRunState,
    pub count: i32,
}
#[derive(SimpleObject)]
pub struct StatusContextStateCount {
    pub state: map::StatusState,
    pub count: i32,
}
#[derive(SimpleObject)]
#[graphql(complex)]
pub struct CheckRun {
    #[graphql(skip)]
    pub owner: String,
    #[graphql(skip)]
    pub repo: String,
    #[graphql(skip)]
    pub context: String,
    #[graphql(skip)]
    pub id: i64,
    pub name: String,
    pub status: map::CheckStatusState,
    pub conclusion: Option<map::CheckConclusionState>,
    pub started_at: Option<DateTime>,
    pub completed_at: Option<DateTime>,
    pub details_url: Option<Uri>,
    pub check_suite: CheckSuite,
}
#[async_graphql::ComplexObject]
impl CheckRun {
    async fn id(&self) -> ID {
        ID(ids::node_id(
            "CheckRun",
            &format!("{}/{}", self.owner, self.repo),
            self.id,
        ))
    }
    async fn is_required(
        &self,
        ctx: &Context<'_>,
        pull_request_id: Option<ID>,
        pull_request_number: Option<i32>,
    ) -> async_graphql::Result<bool> {
        is_required(
            ctx,
            &self.owner,
            &self.repo,
            &self.context,
            pull_request_id,
            pull_request_number,
        )
        .await
    }
}
#[derive(SimpleObject)]
#[graphql(complex)]
pub struct StatusContext {
    #[graphql(skip)]
    pub owner: String,
    #[graphql(skip)]
    pub repo: String,
    pub context: String,
    pub state: map::StatusState,
    pub target_url: Option<Uri>,
    pub description: Option<String>,
    pub created_at: DateTime,
}
#[async_graphql::ComplexObject]
impl StatusContext {
    async fn is_required(
        &self,
        ctx: &Context<'_>,
        pull_request_id: Option<ID>,
        pull_request_number: Option<i32>,
    ) -> async_graphql::Result<bool> {
        is_required(
            ctx,
            &self.owner,
            &self.repo,
            &self.context,
            pull_request_id,
            pull_request_number,
        )
        .await
    }
}
async fn is_required(
    ctx: &Context<'_>,
    owner: &str,
    repo: &str,
    context: &str,
    id: Option<ID>,
    number: Option<i32>,
) -> async_graphql::Result<bool> {
    let number = match id {
        Some(id) => {
            let node = ids::decode(&id)
                .filter(|n| n.kind == "PullRequest" && n.owner == owner && n.repository == repo)
                .ok_or_else(|| error(crate::forgejo::Error::NotFound))?;
            node.number
        }
        None => match number {
            Some(n) => n,
            None => return Ok(false),
        },
    };
    let session = ctx.data_unchecked::<Session>();
    let pr = session
        .pull_request(owner, repo, number)
        .await
        .map_err(error)?;
    let protection = session
        .protection(owner, repo, &pr.base.ref_name)
        .await
        .map_err(error)?;
    Ok(map::required(context, protection.as_ref()))
}
#[derive(SimpleObject)]
pub struct CheckSuite {
    pub workflow_run: Option<WorkflowRun>,
}
#[derive(SimpleObject)]
pub struct WorkflowRun {
    pub event: String,
    pub workflow: Workflow,
}
#[derive(SimpleObject)]
pub struct Workflow {
    pub name: String,
}
