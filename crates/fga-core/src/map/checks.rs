use crate::forgejo::{CommitStatus, CommitStatusState, Protection};
use async_graphql::Enum;
use std::collections::BTreeMap;
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Enum)]
pub enum StatusState {
    Expected,
    Error,
    Failure,
    Pending,
    Success,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Enum)]
pub enum CheckStatusState {
    Completed,
    InProgress,
    Pending,
    Queued,
    Requested,
    Waiting,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Enum)]
pub enum CheckConclusionState {
    ActionRequired,
    Cancelled,
    Failure,
    Neutral,
    Skipped,
    Stale,
    StartupFailure,
    Success,
    TimedOut,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Enum)]
pub enum CheckRunState {
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
}

#[derive(Clone, Debug)]
pub enum Check {
    Action {
        source: CommitStatus,
        workflow: String,
        job: String,
        event: String,
        details_url: Url,
        status: CheckStatusState,
        conclusion: Option<CheckConclusionState>,
    },
    Status {
        source: CommitStatus,
        state: StatusState,
        target_url: Option<Url>,
    },
}

impl Check {
    pub fn source(&self) -> &CommitStatus {
        match self {
            Self::Action { source, .. } | Self::Status { source, .. } => source,
        }
    }
    pub fn state(&self) -> StatusState {
        match self {
            Self::Status { state, .. } => *state,
            Self::Action {
                status, conclusion, ..
            } => match conclusion {
                Some(
                    CheckConclusionState::Failure
                    | CheckConclusionState::TimedOut
                    | CheckConclusionState::Cancelled
                    | CheckConclusionState::ActionRequired
                    | CheckConclusionState::StartupFailure,
                ) => StatusState::Failure,
                _ if *status != CheckStatusState::Completed => StatusState::Pending,
                _ => StatusState::Success,
            },
        }
    }
    pub fn run_state(&self) -> Option<CheckRunState> {
        if let Self::Action {
            status, conclusion, ..
        } = self
        {
            Some(match conclusion {
                Some(CheckConclusionState::Success) => CheckRunState::Success,
                Some(CheckConclusionState::Failure) => CheckRunState::Failure,
                Some(CheckConclusionState::Neutral) => CheckRunState::Neutral,
                Some(CheckConclusionState::Skipped) => CheckRunState::Skipped,
                Some(CheckConclusionState::Cancelled) => CheckRunState::Cancelled,
                Some(CheckConclusionState::TimedOut) => CheckRunState::TimedOut,
                Some(CheckConclusionState::ActionRequired) => CheckRunState::ActionRequired,
                Some(CheckConclusionState::Stale) => CheckRunState::Stale,
                Some(CheckConclusionState::StartupFailure) => CheckRunState::StartupFailure,
                None => match status {
                    CheckStatusState::Completed => CheckRunState::Completed,
                    CheckStatusState::InProgress => CheckRunState::InProgress,
                    CheckStatusState::Pending => CheckRunState::Pending,
                    CheckStatusState::Queued | CheckStatusState::Requested => CheckRunState::Queued,
                    CheckStatusState::Waiting => CheckRunState::Waiting,
                },
            })
        } else {
            None
        }
    }
}

pub fn required(context: &str, protection: Option<&Protection>) -> bool {
    protection.is_some_and(|p| {
        p.required_contexts()
            .iter()
            .any(|glob| glob.is_match(context))
    })
}

pub fn checks(statuses: &[CommitStatus], owner: &str, repo: &str, public: &Url) -> Vec<Check> {
    let mut latest = BTreeMap::<&str, &CommitStatus>::new();
    for status in statuses {
        let entry = latest.entry(&status.context).or_insert(status);
        if (&status.updated_at, status.id) > (&entry.updated_at, entry.id) {
            *entry = status;
        }
    }
    latest
        .into_values()
        .map(|source| {
            let target = if source.target_url.is_empty() {
                None
            } else {
                public.join(&source.target_url).ok()
            };
            let action = source.context.rsplit_once(" (").and_then(|(name, event)| {
                let event = event.strip_suffix(')')?;
                let (workflow, job) = name.split_once(" / ")?;
                if workflow.is_empty() || job.is_empty() || event.is_empty() {
                    return None;
                }
                let url = target.as_ref()?;
                if !url
                    .path()
                    .starts_with(&format!("/{owner}/{repo}/actions/runs/"))
                    || url.origin() != public.origin()
                {
                    return None;
                }
                Some((workflow, job, event, url.clone()))
            });
            if let Some((workflow, job, event, details_url)) = action {
                let (status, conclusion) = match source.state {
                    CommitStatusState::Pending => (
                        if source.description.as_deref() == Some("Waiting to run") {
                            CheckStatusState::Queued
                        } else {
                            CheckStatusState::InProgress
                        },
                        None,
                    ),
                    CommitStatusState::Success => (
                        CheckStatusState::Completed,
                        Some(
                            if source
                                .description
                                .as_deref()
                                .is_some_and(|d| d.to_lowercase().contains("skipped"))
                            {
                                CheckConclusionState::Skipped
                            } else {
                                CheckConclusionState::Success
                            },
                        ),
                    ),
                    CommitStatusState::Failure | CommitStatusState::Error => (
                        CheckStatusState::Completed,
                        Some(CheckConclusionState::Failure),
                    ),
                    CommitStatusState::Warning => (
                        CheckStatusState::Completed,
                        Some(CheckConclusionState::Neutral),
                    ),
                };
                Check::Action {
                    source: source.clone(),
                    workflow: workflow.into(),
                    job: job.into(),
                    event: event.into(),
                    details_url,
                    status,
                    conclusion,
                }
            } else {
                let state = match source.state {
                    CommitStatusState::Pending => StatusState::Pending,
                    CommitStatusState::Success | CommitStatusState::Warning => StatusState::Success,
                    CommitStatusState::Error => StatusState::Error,
                    CommitStatusState::Failure => StatusState::Failure,
                };
                Check::Status {
                    source: source.clone(),
                    state,
                    target_url: target,
                }
            }
        })
        .collect()
}

pub fn rollup(checks: &[Check]) -> Option<StatusState> {
    if checks.is_empty() {
        None
    } else if checks
        .iter()
        .any(|c| matches!(c.state(), StatusState::Failure | StatusState::Error))
    {
        Some(StatusState::Failure)
    } else if checks
        .iter()
        .any(|c| matches!(c.state(), StatusState::Pending | StatusState::Expected))
    {
        Some(StatusState::Pending)
    } else {
        Some(StatusState::Success)
    }
}
