//! `PullRequest.timelineItems`, for the one kind of event Forgejo can answer without reading its
//! timeline: a merge queue's, which Forgejo doesn't have. T3 Code asks for a pull request's last
//! removal from a merge queue alongside its detail.
use super::*;
use async_graphql::{Enum, ID, SimpleObject, Union};

/// GitHub's timeline item kinds, all of them, as clients may name any.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Enum)]
pub enum PullRequestTimelineItemsItemType {
    AddedToMergeQueueEvent,
    AddedToProjectEvent,
    AssignedEvent,
    AutomaticBaseChangeFailedEvent,
    AutomaticBaseChangeSucceededEvent,
    AutoMergeDisabledEvent,
    AutoMergeEnabledEvent,
    AutoRebaseEnabledEvent,
    AutoSquashEnabledEvent,
    BaseRefChangedEvent,
    BaseRefDeletedEvent,
    BaseRefForcePushedEvent,
    ClosedEvent,
    CommentDeletedEvent,
    ConnectedEvent,
    ConvertedNoteToIssueEvent,
    ConvertedToDiscussionEvent,
    ConvertToDraftEvent,
    CrossReferencedEvent,
    DemilestonedEvent,
    DeployedEvent,
    DeploymentEnvironmentChangedEvent,
    DisconnectedEvent,
    HeadRefDeletedEvent,
    HeadRefForcePushedEvent,
    HeadRefRestoredEvent,
    IssueComment,
    LabeledEvent,
    LockedEvent,
    MarkedAsDuplicateEvent,
    MentionedEvent,
    MergedEvent,
    MilestonedEvent,
    MovedColumnsInProjectEvent,
    ParentIssueAddedEvent,
    ParentIssueRemovedEvent,
    PinnedEvent,
    PullRequestCommit,
    PullRequestCommitCommentThread,
    PullRequestReview,
    PullRequestReviewThread,
    PullRequestRevisionMarker,
    ReadyForReviewEvent,
    ReferencedEvent,
    RemovedFromMergeQueueEvent,
    RemovedFromProjectEvent,
    RenamedTitleEvent,
    ReopenedEvent,
    ReviewDismissedEvent,
    ReviewRequestedEvent,
    ReviewRequestRemovedEvent,
    SubscribedEvent,
    SubIssueAddedEvent,
    SubIssueRemovedEvent,
    TransferredEvent,
    UnassignedEvent,
    UnlabeledEvent,
    UnlockedEvent,
    UnmarkedAsDuplicateEvent,
    UnpinnedEvent,
    UnsubscribedEvent,
    UserBlockedEvent,
}

/// A pull request's removal from a merge queue. Forgejo has no merge queue, so none exists.
#[derive(SimpleObject)]
pub struct RemovedFromMergeQueueEvent {
    pub id: ID,
    pub created_at: DateTime,
    pub reason: Option<String>,
}
#[derive(Union)]
pub enum PullRequestTimelineItems {
    RemovedFromMergeQueueEvent(RemovedFromMergeQueueEvent),
}
#[derive(SimpleObject)]
pub struct PullRequestTimelineItemsConnection {
    pub nodes: Option<Vec<Option<PullRequestTimelineItems>>>,
    pub total_count: i32,
    pub filtered_count: i32,
    pub page_count: i32,
    pub page_info: PageInfo,
}

/// Merge queue events, the only kinds the gateway serves: always none. Any other kind, or none
/// named (no list, or an empty one), is an error rather than an empty timeline the pull request may not have.
pub(super) fn timeline_items(
    item_types: Option<Vec<PullRequestTimelineItemsItemType>>,
    first: Option<i32>,
    last: Option<i32>,
    after: Option<String>,
    before: Option<String>,
) -> async_graphql::Result<PullRequestTimelineItemsConnection> {
    use PullRequestTimelineItemsItemType as Kind;
    let queue_only = item_types.as_ref().is_some_and(|kinds| {
        !kinds.is_empty()
            && kinds.iter().all(|kind| {
                matches!(
                    kind,
                    Kind::AddedToMergeQueueEvent | Kind::RemovedFromMergeQueueEvent
                )
            })
    });
    if !queue_only {
        return Err(
            "timelineItems serves only merge queue events, which Forgejo doesn't have".into(),
        );
    }
    let p = page(
        Vec::<PullRequestTimelineItems>::new(),
        first,
        last,
        after,
        before,
    )?;
    Ok(PullRequestTimelineItemsConnection {
        nodes: Some(Vec::new()),
        total_count: p.total,
        filtered_count: p.total,
        page_count: 0,
        page_info: p.info,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use PullRequestTimelineItemsItemType as Kind;

    fn items(kinds: Option<Vec<Kind>>) -> async_graphql::Result<i32> {
        timeline_items(kinds, Some(1), None, None, None).map(|c| c.filtered_count)
    }

    #[test]
    fn only_queue_events_read_and_they_read_as_none() {
        assert_eq!(
            items(Some(vec![Kind::RemovedFromMergeQueueEvent])).unwrap(),
            0
        );
        assert_eq!(
            items(Some(vec![
                Kind::AddedToMergeQueueEvent,
                Kind::RemovedFromMergeQueueEvent
            ]))
            .unwrap(),
            0
        );
        // Forgejo has these; an empty answer would say the pull request has none.
        assert!(items(Some(vec![Kind::ReviewDismissedEvent])).is_err());
        assert!(
            items(Some(vec![
                Kind::RemovedFromMergeQueueEvent,
                Kind::IssueComment
            ]))
            .is_err()
        );
        assert!(items(None).is_err());
        // GitHub reads an empty list as every kind.
        assert!(items(Some(Vec::new())).is_err());
    }
}
