//! A pull request's review comments, reviews, files and commits, and an issue's or pull request's
//! conversation, in GitHub's REST shapes and paging (MAPPING §11).
use crate::{
    Host,
    rest::{Paging, invalid_query, paged},
    rest_error,
};
use axum::{
    Extension,
    extract::{Path, Query},
    response::Response,
};
use fga_core::{
    forgejo::{Error, Session},
    map,
};
use std::sync::Arc;

type Paged = Result<Query<Paging>, axum::extract::rejection::QueryRejection>;

/// Runs `items` for a paged list under `/repos/{owner}/{repo}/{tail…}`.
async fn list<F>(
    host: &Host,
    owner: &str,
    repo: &str,
    tail: &[&str],
    paging: Paged,
    items: F,
) -> Response
where
    F: Future<Output = Result<Vec<serde_json::Value>, Error>>,
{
    let Some((per_page, page)) = paging.ok().and_then(|Query(p)| p.resolve()) else {
        return invalid_query();
    };
    match items.await {
        Ok(items) => {
            let mut segments = vec!["repos", owner, repo];
            segments.extend(tail);
            paged(host, &segments, &[], &[], per_page, page, items)
        }
        Err(e) => rest_error(e),
    }
}

/// `GET /repos/{o}/{r}/pulls/{n}/comments`: every review's code comments, oldest first.
pub async fn review_comments(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, number)): Path<(String, String, i32)>,
    paging: Paged,
) -> Response {
    let n = number.to_string();
    list(
        &host,
        &owner,
        &repo,
        &["pulls", &n, "comments"],
        paging,
        async {
            let pr = session.pull_request(&owner, &repo, number).await?;
            let mut reviews = Vec::new();
            for review in session.reviews(&owner, &repo, number).await? {
                if !map::listed(&review, &session.viewer.login) {
                    continue;
                }
                // A review without code comments needs no read.
                let comments = if review.comments_count == Some(0) {
                    Vec::new()
                } else {
                    session
                        .review_comments(&owner, &repo, number, review.id)
                        .await?
                };
                reviews.push((review, comments));
            }
            Ok(map::review_comments(
                &reviews,
                &owner,
                &repo,
                number,
                &pr.head.sha,
                &host.public_url,
            ))
        },
    )
    .await
}

/// `GET /repos/{o}/{r}/pulls/{n}/reviews`, oldest first; review requests aren't reviews.
pub async fn reviews(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, number)): Path<(String, String, i32)>,
    paging: Paged,
) -> Response {
    let n = number.to_string();
    list(
        &host,
        &owner,
        &repo,
        &["pulls", &n, "reviews"],
        paging,
        async {
            session.pull_request(&owner, &repo, number).await?;
            let mut reviews = session.reviews(&owner, &repo, number).await?;
            reviews.retain(|review| map::listed(review, &session.viewer.login));
            Ok(map::reviews(
                &reviews,
                &owner,
                &repo,
                number,
                &host.public_url,
            ))
        },
    )
    .await
}

/// `GET /repos/{o}/{r}/pulls/{n}/files`, each with its patch from the pull request's diff.
pub async fn files(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, number)): Path<(String, String, i32)>,
    paging: Paged,
) -> Response {
    let n = number.to_string();
    list(
        &host,
        &owner,
        &repo,
        &["pulls", &n, "files"],
        paging,
        async {
            let pr = session.pull_request(&owner, &repo, number).await?;
            let files = session.pull_files(&owner, &repo, number).await?;
            let patches = map::diff_patches(&session.pull_diff(&owner, &repo, number).await?);
            Ok(map::changed_files(
                &files,
                &patches,
                &owner,
                &repo,
                &pr.head.sha,
                &host.public_url,
            ))
        },
    )
    .await
}

/// `GET /repos/{o}/{r}/pulls/{n}/commits`, oldest first as on GitHub; Forgejo lists newest first.
pub async fn commits(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, number)): Path<(String, String, i32)>,
    paging: Paged,
) -> Response {
    let n = number.to_string();
    list(
        &host,
        &owner,
        &repo,
        &["pulls", &n, "commits"],
        paging,
        async {
            let commits = session.commits(&owner, &repo, number).await?;
            Ok(commits
                .iter()
                .rev()
                .map(|c| map::commit(c.into(), &owner, &repo, &host.public_url))
                .collect())
        },
    )
    .await
}

/// `GET /repos/{o}/{r}/issues/{n}/comments`: the conversation of an issue or a pull request.
pub async fn issue_comments(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, number)): Path<(String, String, i32)>,
    paging: Paged,
) -> Response {
    let n = number.to_string();
    list(
        &host,
        &owner,
        &repo,
        &["issues", &n, "comments"],
        paging,
        async {
            let comments = session.comments(&owner, &repo, number).await?;
            Ok(map::issue_comments(
                &comments,
                &owner,
                &repo,
                number,
                &host.public_url,
            ))
        },
    )
    .await
}
