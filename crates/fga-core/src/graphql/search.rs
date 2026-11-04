use super::*;
use crate::search::{Kind, Search, Sort};
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
    let pulls = if query.kind == Kind::Pulls {
        Some(
            session
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
                        // Forgejo's `poster` filter does this too; the check keeps a PR-only search
                        // as strict as one the issue list also filtered by author.
                        && query
                            .filters
                            .author
                            .as_ref()
                            .is_none_or(|author| pr.user.login.eq_ignore_ascii_case(author))
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
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };
    // Forgejo's pull list filters by state and author, and lists newest first, which is search's
    // default order. The issue list adds labels, keywords, assignee, mention and other orders,
    // and needs a token with issue scope, which a PR-only query such as ChatGPT's
    // `head:… author:@me` must not require.
    let needs_issues = query.kind == Kind::Issues
        || !query.filters.labels.is_empty()
        || !query.filters.words.is_empty()
        || query.title_only
        || query.filters.assignee.is_some()
        || query.filters.mention.is_some()
        || !matches!(query.filters.sort, Sort::CreatedDesc);
    let nodes = match pulls {
        Some(pulls) if !needs_issues => pulls
            .into_iter()
            .map(|pr| PullRequest::new(pr, &query.owner, &query.repo).into())
            .collect(),
        pulls => {
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
            // Forgejo drops unknown label names (`routers/api/v1/repo/issue.go:447-462`), so a
            // query naming only unknown labels would match everything. Check returned names too.
            let issues = issues.into_iter().filter(|issue| {
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
            });
            match pulls {
                Some(pulls) => {
                    let mut pulls: HashMap<_, _> =
                        pulls.into_iter().map(|pr| (pr.number, pr)).collect();
                    issues
                        .filter_map(|issue| pulls.remove(&issue.number))
                        .map(|pr| PullRequest::new(pr, &query.owner, &query.repo).into())
                        .collect()
                }
                None => issues
                    .map(|source| Issue::new(source, &query.owner, &query.repo).into())
                    .collect(),
            }
        }
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
