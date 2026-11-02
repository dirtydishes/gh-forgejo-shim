use super::*;
use crate::search::{Kind, Search};
use async_graphql::{Context, ErrorExtensions};
use std::collections::HashMap;

pub async fn search(
    ctx: &Context<'_>,
    query: &str,
    first: Option<i32>,
    last: Option<i32>,
    after: Option<String>,
) -> async_graphql::Result<SearchResultItemConnection> {
    let session = ctx.data_unchecked::<forgejo::Session>();
    let query = Search::parse(query, &session.viewer.login).map_err(|message| {
        async_graphql::Error::new(message).extend_with(|_, e| e.set("type", "UNPROCESSABLE"))
    })?;
    let issues = session
        .issues(
            &query.owner,
            &query.repo,
            query.state,
            query.kind,
            &query.filters,
        )
        .await
        .map_err(error)?;
    // Forgejo drops unknown label names (`routers/api/v1/repo/issue.go:447-462`), so a query
    // naming only unknown labels would match everything. Check returned names too.
    let issues: Vec<_> = issues
        .into_iter()
        .filter(|issue| {
            query.filters.labels.iter().all(|name| {
                issue
                    .labels
                    .iter()
                    .any(|label| label.name.eq_ignore_ascii_case(name))
            }) && (!query.title_only
                || query
                    .filters
                    .words
                    .iter()
                    .all(|word| issue.title.to_lowercase().contains(&word.to_lowercase())))
        })
        .collect();
    let nodes = if query.kind == Kind::Pulls {
        let mut pulls: HashMap<_, _> = session
            .pull_requests(
                &query.owner,
                &query.repo,
                query.state.upstream(),
                query.head.as_deref(),
                query.filters.author.as_deref(),
            )
            .await
            .map_err(error)?
            .into_iter()
            .filter(|pr| {
                query.state.matches_pull(pr)
                    && query
                        .base
                        .as_ref()
                        .is_none_or(|base| pr.base.ref_name == *base)
                    && query.reviewer.as_ref().is_none_or(|reviewer| {
                        pr.requested_reviewers.as_ref().is_some_and(|users| {
                            users.iter().any(|u| u.login.eq_ignore_ascii_case(reviewer))
                        })
                    })
            })
            .map(|pr| (pr.number, pr))
            .collect();
        issues
            .into_iter()
            .filter_map(|issue| pulls.remove(&issue.number))
            .map(|pr| PullRequest::new(pr, &query.owner, &query.repo).into())
            .collect()
    } else {
        issues
            .into_iter()
            .map(|source| Issue::new(source, &query.owner, &query.repo).into())
            .collect()
    };
    let p = page(nodes, first, last, after, None)?;
    Ok(SearchResultItemConnection {
        issue_count: p.total,
        page_info: p.info,
        nodes: Some(p.nodes.iter().cloned().map(Some).collect()),
        edges: Some(
            p.nodes
                .into_iter()
                .map(|node| Some(SearchResultItemEdge { node: Some(node) }))
                .collect(),
        ),
    })
}
