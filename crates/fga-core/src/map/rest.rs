//! GitHub's REST shapes for a pull request's review comments, reviews, conversation, files and
//! commits, from Forgejo's (MAPPING §11).
use super::{PullRequestReviewState, commented_line, pull_web_url, review_state, user};
use crate::{forgejo, ids::node_id};
use serde_json::{Value, json};
use std::collections::HashMap;
use url::Url;

/// `{public}/api/v3/repos/{owner}/{repo}/{rest…}`: where the gateway serves a resource.
pub fn api_url(public: &Url, owner: &str, repo: &str, rest: &[&str]) -> Url {
    let mut url = public.clone();
    url.path_segments_mut()
        .expect("validated base URL")
        .pop_if_empty()
        .extend(["api", "v3", "repos", owner, repo])
        .extend(rest);
    url
}

fn association(login: Option<&str>, owner: &str) -> &'static str {
    if login.is_some_and(|login| login.eq_ignore_ascii_case(owner)) {
        "OWNER"
    } else {
        "NONE"
    }
}

/// A pull request's review comments, oldest first. Forgejo groups one review's comments on the
/// same path and line into a conversation, so each later one replies to the first.
pub fn review_comments(
    reviews: &[(forgejo::Review, Vec<forgejo::ReviewComment>)],
    owner: &str,
    repo: &str,
    number: i32,
    head_sha: &str,
    public: &Url,
) -> Vec<Value> {
    let mut comments: Vec<(&forgejo::Review, &forgejo::ReviewComment)> = reviews
        .iter()
        .flat_map(|(review, comments)| comments.iter().map(move |c| (review, c)))
        .collect();
    comments.sort_by_key(|(_, comment)| comment.id);
    let mut roots: HashMap<(i64, &str, i64), i64> = HashMap::new();
    let full_name = format!("{owner}/{repo}");
    let pull_url = api_url(public, owner, repo, &["pulls", &number.to_string()]).to_string();
    comments
        .into_iter()
        .map(|(review, comment)| {
            let key = (comment.pull_request_review_id, comment.path.as_str(), comment.line());
            let root = *roots.entry(key).or_insert(comment.id);
            let current = review.commit_id == head_sha;
            let line = commented_line(comment);
            let url = api_url(public, owner, repo, &["pulls", "comments", &comment.id.to_string()]);
            let html_url = pull_web_url(&comment.html_url);
            let mut value = json!({
                "url": url.as_str(),
                "pull_request_review_id": comment.pull_request_review_id,
                "id": comment.id,
                "node_id": node_id("PullRequestReviewComment", &full_name, comment.id),
                "diff_hunk": comment.diff_hunk,
                "path": comment.path,
                "position": null,
                "original_position": null,
                "commit_id": review.commit_id,
                "original_commit_id": review.commit_id,
                "user": comment.user.as_ref().map(|u| user(u, public)),
                "body": comment.body,
                "created_at": comment.created_at,
                "updated_at": comment.updated_at.as_deref().unwrap_or(&comment.created_at),
                "html_url": html_url,
                "pull_request_url": pull_url,
                "author_association": association(comment.user.as_ref().map(|u| u.login.as_str()), owner),
                "_links": {
                    "self": {"href": url.as_str()},
                    "html": {"href": html_url},
                    "pull_request": {"href": pull_url},
                },
                "start_line": null,
                "original_start_line": null,
                "start_side": null,
                "line": line.filter(|_| current),
                "original_line": line,
                "side": if comment.line() < 0 { "LEFT" } else { "RIGHT" },
                "subject_type": "line",
            });
            if root != comment.id {
                value["in_reply_to_id"] = json!(root);
            }
            value
        })
        .collect()
}

/// A pull request's reviews, oldest first.
pub fn reviews(
    reviews: &[forgejo::Review],
    owner: &str,
    repo: &str,
    number: i32,
    public: &Url,
) -> Vec<Value> {
    let full_name = format!("{owner}/{repo}");
    let pull_url = api_url(public, owner, repo, &["pulls", &number.to_string()]).to_string();
    let mut reviews: Vec<&forgejo::Review> = reviews.iter().collect();
    reviews.sort_by_key(|review| review.id);
    reviews
        .into_iter()
        .map(|review| {
            let state = review_state(review);
            let html_url = pull_web_url(if review.html_url.is_empty() {
                &review.pull_request_url
            } else {
                &review.html_url
            });
            json!({
                "id": review.id,
                "node_id": node_id("PullRequestReview", &full_name, review.id),
                "user": review.user.as_ref().map(|u| user(u, public)),
                "body": review.body,
                "state": match state {
                    PullRequestReviewState::Approved => "APPROVED",
                    PullRequestReviewState::ChangesRequested => "CHANGES_REQUESTED",
                    PullRequestReviewState::Commented => "COMMENTED",
                    PullRequestReviewState::Dismissed => "DISMISSED",
                    PullRequestReviewState::Pending => "PENDING",
                },
                "html_url": html_url,
                "pull_request_url": pull_url,
                "author_association": association(review.user.as_ref().map(|u| u.login.as_str()), owner),
                "_links": {"html": {"href": html_url}, "pull_request": {"href": pull_url}},
                "submitted_at": (state != PullRequestReviewState::Pending)
                    .then_some(review.submitted_at.as_deref())
                    .flatten(),
                "commit_id": review.commit_id,
            })
        })
        .collect()
}

/// An issue's or a pull request's conversation comments, oldest first.
pub fn issue_comments(
    comments: &[forgejo::Comment],
    owner: &str,
    repo: &str,
    number: i32,
    public: &Url,
) -> Vec<Value> {
    let full_name = format!("{owner}/{repo}");
    let issue_url = api_url(public, owner, repo, &["issues", &number.to_string()]).to_string();
    comments
        .iter()
        .map(|comment| {
            json!({
                "url": api_url(public, owner, repo, &["issues", "comments", &comment.id.to_string()]).as_str(),
                "html_url": pull_web_url(&comment.html_url),
                "issue_url": issue_url,
                "id": comment.id,
                "node_id": node_id("IssueComment", &full_name, comment.id),
                "user": user(&comment.user, public),
                "created_at": comment.created_at,
                "updated_at": comment.updated_at.as_deref().unwrap_or(&comment.created_at),
                "author_association": association(Some(&comment.user.login), owner),
                "body": comment.body,
                "performed_via_github_app": null,
            })
        })
        .collect()
}

/// GitHub's file status for Forgejo's: a modification is `changed` in Forgejo's pull request
/// files and `modified` in its comparisons, and a deletion `deleted` or `removed`.
pub fn file_status(status: &str) -> &str {
    match status {
        "deleted" | "removed" => "removed",
        "changed" | "modified" => "modified",
        other => other,
    }
}

/// Changed files in GitHub's shape, each with its patch from `patches` when it has one. Forgejo
/// gives no blob SHA per file, so `sha` is null.
pub fn changed_files(
    files: &[forgejo::ChangedFile],
    patches: &HashMap<String, String>,
    owner: &str,
    repo: &str,
    head_sha: &str,
    public: &Url,
) -> Vec<Value> {
    files
        .iter()
        .map(|file| {
            let additions = file.additions.unwrap_or(0);
            let deletions = file.deletions.unwrap_or(0);
            let mut contents = api_url(public, owner, repo, &["contents"]);
            contents
                .path_segments_mut()
                .expect("validated base URL")
                .extend(file.filename.split('/'));
            contents.query_pairs_mut().append_pair("ref", head_sha);
            let mut value = json!({
                "sha": null,
                "filename": file.filename,
                "status": file_status(&file.status),
                "additions": additions,
                "deletions": deletions,
                "changes": additions + deletions,
                "blob_url": file.html_url,
                "raw_url": file.raw_url,
                "contents_url": contents.as_str(),
            });
            if let Some(patch) = patches.get(&file.filename) {
                value["patch"] = json!(patch);
            }
            if let Some(previous) = file.previous_filename.as_deref().filter(|p| !p.is_empty()) {
                value["previous_filename"] = json!(previous);
            }
            value
        })
        .collect()
}

/// The parts of a commit GitHub's REST commit object needs; Forgejo's pull request commits and
/// its git commits both carry them.
pub struct CommitParts<'a> {
    pub sha: &'a str,
    pub commit: &'a forgejo::CommitMetadata,
    pub author: Option<&'a forgejo::User>,
    pub committer: Option<&'a forgejo::User>,
    pub parents: &'a [forgejo::CommitParent],
    pub html_url: &'a str,
}
impl<'a> From<&'a forgejo::PullCommit> for CommitParts<'a> {
    fn from(c: &'a forgejo::PullCommit) -> Self {
        Self {
            sha: &c.sha,
            commit: &c.commit,
            author: c.author.as_ref(),
            committer: c.committer.as_ref(),
            parents: &c.parents,
            html_url: &c.html_url,
        }
    }
}
impl<'a> From<&'a forgejo::GitCommit> for CommitParts<'a> {
    fn from(c: &'a forgejo::GitCommit) -> Self {
        Self {
            sha: &c.sha,
            commit: &c.commit,
            author: c.author.as_ref(),
            committer: c.committer.as_ref(),
            parents: &c.parents,
            html_url: &c.html_url,
        }
    }
}

/// A commit in GitHub's REST shape.
pub fn commit(c: CommitParts<'_>, owner: &str, repo: &str, public: &Url) -> Value {
    let actor =
        |a: &forgejo::CommitAuthor| json!({"name": a.name, "email": a.email, "date": a.date});
    let web = |sha: &str| {
        let mut url = public.clone();
        url.path_segments_mut()
            .expect("validated base URL")
            .pop_if_empty()
            .extend([owner, repo, "commit", sha]);
        url.to_string()
    };
    json!({
        "sha": c.sha,
        "url": api_url(public, owner, repo, &["commits", c.sha]).as_str(),
        "html_url": if c.html_url.is_empty() { web(c.sha) } else { c.html_url.to_owned() },
        "commit": {
            "author": actor(&c.commit.author),
            "committer": actor(&c.commit.committer),
            "message": c.commit.message.trim_end_matches('\n'),
            "url": api_url(public, owner, repo, &["git", "commits", c.sha]).as_str(),
            "comment_count": 0,
        },
        "author": c.author.map(|u| user(u, public)),
        "committer": c.committer.map(|u| user(u, public)),
        "parents": c.parents.iter().map(|p| json!({
            "sha": p.sha,
            "url": api_url(public, owner, repo, &["commits", &p.sha]).as_str(),
            "html_url": web(&p.sha),
        })).collect::<Vec<_>>(),
    })
}

/// Each file's patch in a `git diff`, keyed by its path on the new side (the old one for a
/// deleted file): its hunks, from the first `@@`, without a trailing newline. A file with no
/// hunks, such as a binary file or a pure rename, has no patch.
pub fn diff_patches(diff: &str) -> HashMap<String, String> {
    let mut patches = HashMap::new();
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in diff.lines() {
        if line.starts_with("diff --git ") || blocks.is_empty() {
            blocks.push(Vec::new());
        }
        blocks.last_mut().expect("a block was pushed").push(line);
    }
    let path = |line: &str, prefix: &str| -> Option<String> {
        let rest = line.strip_prefix(prefix)?.trim_end();
        let rest = rest
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or(rest);
        Some(rest.split_once('/')?.1.to_owned())
    };
    for block in blocks {
        let Some(start) = block.iter().position(|line| line.starts_with("@@")) else {
            continue;
        };
        let header = &block[..start];
        let name = header
            .iter()
            .find_map(|l| path(l, "+++ ").filter(|_| !l.ends_with("/dev/null")))
            .or_else(|| header.iter().find_map(|l| path(l, "--- ")));
        if let Some(name) = name {
            patches.insert(name, block[start..].join("\n"));
        }
    }
    patches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_file_gets_its_own_hunks() {
        let diff = "diff --git a/README.md b/README.md\nindex 1..2 100644\n--- a/README.md\n+++ b/README.md\n@@ -1,1 +1,1 @@\n-a\n+b\ndiff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\ndiff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n";
        let patches = diff_patches(diff);
        assert_eq!(patches["README.md"], "@@ -1,1 +1,1 @@\n-a\n+b");
        assert_eq!(patches["gone.txt"], "@@ -1 +0,0 @@\n-x");
        assert!(!patches.contains_key("img.png"));
        assert_eq!(patches.len(), 2);
    }
}
