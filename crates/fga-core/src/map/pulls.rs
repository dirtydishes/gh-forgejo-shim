use super::{Check, StatusState, rollup};
use crate::forgejo::{Protection, PullRequest, RequestedTeam, Review, User};
use async_graphql::Enum;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Enum)]
pub enum PullRequestState {
    Closed,
    Merged,
    Open,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Enum)]
pub enum MergeableState {
    Conflicting,
    Mergeable,
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Enum)]
pub enum MergeStateStatus {
    Behind,
    Blocked,
    Clean,
    Dirty,
    Draft,
    HasHooks,
    Unknown,
    Unstable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Enum)]
pub enum PullRequestReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Enum)]
pub enum PullRequestReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    Pending,
}

#[derive(Clone, Debug)]
pub struct WipPrefixes(Vec<String>);

impl WipPrefixes {
    pub fn strip<'a>(&self, title: &'a str) -> &'a str {
        for prefix in &self.0 {
            if title
                .get(..prefix.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
            {
                return title[prefix.len()..].trim_start();
            }
        }
        title
    }

    pub fn draft_title(&self, title: &str) -> Option<String> {
        self.0.first().map(|prefix| format!("{prefix} {title}"))
    }

    pub fn from_csv(value: &str) -> Self {
        Self(
            value
                .split(',')
                .map(str::trim)
                .filter(|prefix| !prefix.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    }
}

impl Default for WipPrefixes {
    fn default() -> Self {
        Self::from_csv("WIP:,[WIP]")
    }
}

pub fn title<'a>(pr: &'a PullRequest, prefixes: &WipPrefixes) -> &'a str {
    if pr.draft {
        prefixes.strip(&pr.title)
    } else {
        &pr.title
    }
}

pub fn state(pr: &PullRequest) -> PullRequestState {
    if pr.merged {
        PullRequestState::Merged
    } else if pr.state == "closed" {
        PullRequestState::Closed
    } else {
        PullRequestState::Open
    }
}

pub fn mergeable(pr: &PullRequest) -> MergeableState {
    if pr.draft || state(pr) != PullRequestState::Open {
        MergeableState::Unknown
    } else if pr.mergeable {
        MergeableState::Mergeable
    } else {
        MergeableState::Unknown
    }
}

pub fn review_state(review: &Review) -> PullRequestReviewState {
    if review.dismissed {
        return PullRequestReviewState::Dismissed;
    }
    match review.state.as_str() {
        "APPROVED" => PullRequestReviewState::Approved,
        "REQUEST_CHANGES" => PullRequestReviewState::ChangesRequested,
        "COMMENT" => PullRequestReviewState::Commented,
        _ => PullRequestReviewState::Pending,
    }
}

fn latest<'a>(reviews: impl Iterator<Item = &'a Review>) -> Vec<&'a Review> {
    let mut users = BTreeMap::<i64, &Review>::new();
    for review in reviews {
        let entry = users.entry(review.user.id).or_insert(review);
        if (&review.submitted_at, review.id) > (&entry.submitted_at, entry.id) {
            *entry = review;
        }
    }
    let mut result: Vec<_> = users.into_values().collect();
    result.sort_by_key(|r| (&r.submitted_at, r.id));
    result
}

pub fn latest_reviews(reviews: &[Review]) -> Vec<&Review> {
    latest(
        reviews
            .iter()
            .filter(|r| review_state(r) != PullRequestReviewState::Pending),
    )
}

pub fn review_decision(
    reviews: &[Review],
    protection: Option<&Protection>,
) -> Option<PullRequestReviewDecision> {
    let p = protection.filter(|p| p.required_approvals > 0 || p.block_on_rejected_reviews)?;
    let reviews = latest(reviews.iter().filter(|r| {
        !r.dismissed
            && !(r.stale && p.dismiss_stale_approvals)
            && matches!(
                review_state(r),
                PullRequestReviewState::Approved | PullRequestReviewState::ChangesRequested
            )
    }));
    if reviews.iter().any(|r| {
        review_state(r) == PullRequestReviewState::ChangesRequested
            && (p.block_on_rejected_reviews || r.official)
    }) {
        return Some(PullRequestReviewDecision::ChangesRequested);
    }
    let count = reviews
        .iter()
        .filter(|r| {
            review_state(r) == PullRequestReviewState::Approved
                && (!p.enable_approvals_whitelist || r.official)
        })
        .count();
    Some(if count >= p.required_approvals.max(0) as usize {
        PullRequestReviewDecision::Approved
    } else {
        PullRequestReviewDecision::ReviewRequired
    })
}

pub fn merge_state(
    pr: &PullRequest,
    mergeability: MergeableState,
    checks: &[Check],
    protection: Option<&Protection>,
    decision: Option<PullRequestReviewDecision>,
    behind: bool,
) -> MergeStateStatus {
    if state(pr) != PullRequestState::Open {
        return MergeStateStatus::Unknown;
    }
    match mergeability {
        MergeableState::Unknown => return MergeStateStatus::Unknown,
        MergeableState::Conflicting => return MergeStateStatus::Dirty,
        MergeableState::Mergeable => {}
    }
    if protection.is_some_and(|p| p.block_on_outdated_branch) && behind {
        return MergeStateStatus::Behind;
    }
    let missing_checks = protection.is_some_and(|p| {
        p.enable_status_check
            && p.status_check_contexts.iter().any(|pattern| {
                let matching: Vec<_> = checks
                    .iter()
                    .filter(|c| {
                        glob::Pattern::new(pattern).is_ok_and(|p| p.matches(&c.source().context))
                    })
                    .collect();
                matching.is_empty() || matching.iter().any(|c| c.state() != StatusState::Success)
            })
    });
    if matches!(
        decision,
        Some(
            PullRequestReviewDecision::ReviewRequired | PullRequestReviewDecision::ChangesRequested
        )
    ) || missing_checks
    {
        return MergeStateStatus::Blocked;
    }
    if rollup(checks).is_some_and(|state| state != StatusState::Success) {
        MergeStateStatus::Unstable
    } else {
        MergeStateStatus::Clean
    }
}

pub enum Reviewer<'a> {
    User(&'a User),
    Team(&'a RequestedTeam),
}

pub fn review_requests(pr: &PullRequest) -> Vec<Reviewer<'_>> {
    pr.requested_reviewers
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(Reviewer::User)
        .chain(
            pr.requested_reviewers_teams
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(Reviewer::Team),
        )
        .collect()
}
