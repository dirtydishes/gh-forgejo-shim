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

/// Whether `context` counts toward Forgejo's status gate. With checks enabled but no valid
/// pattern, every status does (`services/pull/commit_status.go:23-71`).
pub fn required(context: &str, protection: Option<&Protection>) -> bool {
    protection.is_some_and(|p| {
        let globs = p.required_contexts();
        p.enable_status_check
            && (globs.is_empty() || globs.iter().any(|glob| glob.is_match(context)))
    })
}

/// Forgejo's status gate (`services/pull/commit_status.go:23-91`), on raw states: only `success`
/// passes, so a required `warning` blocks. Each pattern needs a match. When no pattern matched,
/// Forgejo combines every status instead, and no statuses at all fail.
pub fn required_checks_pass(checks: &[Check], protection: Option<&Protection>) -> bool {
    let Some(p) = protection.filter(|p| p.enable_status_check) else {
        return true;
    };
    let success = |c: &&Check| matches!(c.source().state, CommitStatusState::Success);
    let mut matched = false;
    for glob in p.required_contexts() {
        let mut matching = checks
            .iter()
            .filter(|c| glob.is_match(&c.source().context))
            .peekable();
        if matching.peek().is_none() {
            return false;
        }
        matched = true;
        if !matching.all(|c| success(&c)) {
            return false;
        }
    }
    matched || (!checks.is_empty() && checks.iter().all(|c| success(&c)))
}

pub fn checks(statuses: &[CommitStatus], owner: &str, repo: &str, public: &Url) -> Vec<Check> {
    let mut latest = BTreeMap::<&str, &CommitStatus>::new();
    for status in statuses {
        let entry = latest.entry(&status.context).or_insert(status);
        if (&status.updated_at, status.id) > (&entry.updated_at, entry.id) {
            *entry = status;
        }
    }
    // Actions link jobs under the repository's path, below any subpath Forgejo is served from
    // (`services/actions/commit_status.go:141`, `models/repo/repo.go:598`).
    let mut runs = public.clone();
    runs.path_segments_mut()
        .expect("validated HTTP URL")
        .pop_if_empty()
        .extend([owner, repo, "actions", "runs", ""]);
    let runs = runs.path().to_lowercase();
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
                if !url.path().to_lowercase().starts_with(&runs) || url.origin() != public.origin()
                {
                    return None;
                }
                Some((workflow, job, event, url.clone()))
            });
            if let Some((workflow, job, event, details_url)) = action {
                // The descriptions carry what the commit state drops
                // (`services/actions/commit_status.go:88-110,153-163`).
                let description = source.description.as_deref().unwrap_or("");
                let (status, conclusion) = match source.state {
                    CommitStatusState::Pending => (
                        match description {
                            "Waiting to run" => CheckStatusState::Queued,
                            "Blocked by required conditions" => CheckStatusState::Waiting,
                            _ => CheckStatusState::InProgress,
                        },
                        None,
                    ),
                    CommitStatusState::Success => (
                        CheckStatusState::Completed,
                        Some(if description == "Has been skipped" {
                            CheckConclusionState::Skipped
                        } else {
                            CheckConclusionState::Success
                        }),
                    ),
                    CommitStatusState::Failure
                    | CommitStatusState::Error
                    | CommitStatusState::Warning => (
                        CheckStatusState::Completed,
                        Some(if description == "Has been cancelled" {
                            CheckConclusionState::Cancelled
                        } else {
                            CheckConclusionState::Failure
                        }),
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
                    CommitStatusState::Success => StatusState::Success,
                    CommitStatusState::Error => StatusState::Error,
                    // Forgejo ranks a warning below pending and never lets it pass its gate
                    // (`modules/structs/commit_status.go:23-29,55`).
                    CommitStatusState::Failure | CommitStatusState::Warning => StatusState::Failure,
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
