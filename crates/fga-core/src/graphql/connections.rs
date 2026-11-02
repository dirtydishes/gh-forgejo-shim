use super::*;
use async_graphql::SimpleObject;
use base64::{Engine, engine::general_purpose::STANDARD};

#[derive(Clone, SimpleObject)]
pub struct PageInfo {
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub start_cursor: Option<String>,
    pub end_cursor: Option<String>,
}

pub struct Page<T> {
    pub nodes: Vec<T>,
    pub total: i32,
    pub info: PageInfo,
}

pub fn page<T>(
    items: Vec<T>,
    first: Option<i32>,
    last: Option<i32>,
    after: Option<String>,
    before: Option<String>,
) -> async_graphql::Result<Page<T>> {
    let total = i32::try_from(items.len()).map_err(|_| "connection is too large")?;
    let decode = |value: String| -> async_graphql::Result<usize> {
        String::from_utf8(STANDARD.decode(value).map_err(|_| "invalid cursor")?)
            .map_err(|_| "invalid cursor")?
            .strip_prefix("fga:cursor:")
            .ok_or_else(|| async_graphql::Error::new("invalid cursor"))?
            .parse()
            .map_err(|_| "invalid cursor".into())
    };
    if first.is_some_and(|n| n < 0) || last.is_some_and(|n| n < 0) {
        return Err("negative page size".into());
    }
    let mut start = after
        .map(decode)
        .transpose()?
        .map_or(0, |n| n.saturating_add(1))
        .min(items.len());
    let mut end = before
        .map(decode)
        .transpose()?
        .unwrap_or(items.len())
        .min(items.len())
        .max(start);
    if let Some(first) = first {
        end = end.min(start.saturating_add(first as usize));
    }
    if let Some(last) = last {
        start = start.max(end.saturating_sub(last as usize));
    }
    let cursor = |n| STANDARD.encode(format!("fga:cursor:{n}"));
    Ok(Page {
        nodes: items.into_iter().skip(start).take(end - start).collect(),
        total,
        info: PageInfo {
            has_next_page: end < total as usize,
            has_previous_page: start > 0,
            start_cursor: (start < end).then(|| cursor(start)),
            end_cursor: (start < end).then(|| cursor(end - 1)),
        },
    })
}

macro_rules! connection {
    ($name:ident, $node:ty) => {
        #[derive(SimpleObject)]
        pub struct $name {
            pub nodes: Option<Vec<Option<$node>>>,
            pub total_count: i32,
            pub page_info: PageInfo,
        }
        impl From<Page<$node>> for $name {
            fn from(p: Page<$node>) -> Self {
                Self {
                    nodes: Some(p.nodes.into_iter().map(Some).collect()),
                    total_count: p.total,
                    page_info: p.info,
                }
            }
        }
    };
}
connection!(UserConnection, User);
connection!(LabelConnection, Label);
connection!(PullRequestReviewConnection, PullRequestReview);
connection!(ReviewRequestConnection, ReviewRequest);
connection!(IssueCommentConnection, IssueComment);
connection!(PullRequestCommitConnection, PullRequestCommit);
connection!(GitActorConnection, GitActor);
connection!(ProjectV2ItemConnection, ProjectV2Item);

#[derive(SimpleObject)]
pub struct PullRequestEdge {
    pub node: Option<PullRequest>,
}
#[derive(SimpleObject)]
pub struct PullRequestConnection {
    pub nodes: Option<Vec<Option<PullRequest>>>,
    pub edges: Option<Vec<Option<PullRequestEdge>>>,
    pub total_count: i32,
    pub page_info: PageInfo,
}
impl From<Page<PullRequest>> for PullRequestConnection {
    fn from(p: Page<PullRequest>) -> Self {
        Self {
            nodes: Some(p.nodes.iter().cloned().map(Some).collect()),
            edges: Some(
                p.nodes
                    .into_iter()
                    .map(|node| Some(PullRequestEdge { node: Some(node) }))
                    .collect(),
            ),
            total_count: p.total,
            page_info: p.info,
        }
    }
}
#[derive(SimpleObject)]
pub struct SearchResultItemEdge {
    pub node: Option<SearchResultItem>,
}
#[derive(SimpleObject)]
pub struct SearchResultItemConnection {
    pub nodes: Option<Vec<Option<SearchResultItem>>>,
    pub edges: Option<Vec<Option<SearchResultItemEdge>>>,
    pub issue_count: i32,
    pub page_info: PageInfo,
}
