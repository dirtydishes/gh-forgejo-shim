//! Files and git objects: a pull request's changed files, and `Repository.object`'s blobs and
//! commits, read from Forgejo's changed-files, contents and commits APIs.
use super::*;
use crate::{forgejo, forgejo::Session};
use async_graphql::{Context, Enum, Interface, Object, SimpleObject};
use base64::{Engine, engine::general_purpose::STANDARD};

/// A blob's text past this many bytes is cut, and `isTruncated` says so.
const MAX_BLOB_TEXT: usize = 1024 * 1024;

#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum PatchStatus {
    Added,
    Changed,
    Copied,
    Deleted,
    Modified,
    Renamed,
}
impl PatchStatus {
    /// Forgejo's changed-file status; a modification is `changed` in a pull request's files and
    /// `modified` in a comparison's.
    pub fn from_forgejo(status: &str) -> Self {
        match status {
            "added" => Self::Added,
            "deleted" | "removed" => Self::Deleted,
            "renamed" => Self::Renamed,
            "copied" => Self::Copied,
            _ => Self::Modified,
        }
    }
}
#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum FileViewedState {
    Dismissed,
    Unviewed,
    Viewed,
}

#[derive(Clone, SimpleObject)]
pub struct PullRequestChangedFile {
    pub path: String,
    pub additions: i32,
    pub deletions: i32,
    pub change_type: PatchStatus,
    /// Forgejo keeps which files a reviewer marked viewed, but its API doesn't serve it.
    pub viewer_viewed_state: FileViewedState,
}
impl From<forgejo::ChangedFile> for PullRequestChangedFile {
    fn from(file: forgejo::ChangedFile) -> Self {
        let count = |n: Option<i64>| n.and_then(|n| i32::try_from(n).ok()).unwrap_or(0);
        Self {
            change_type: PatchStatus::from_forgejo(&file.status),
            additions: count(file.additions),
            deletions: count(file.deletions),
            path: file.filename,
            viewer_viewed_state: FileViewedState::Unviewed,
        }
    }
}

#[derive(Interface)]
#[graphql(
    field(name = "oid", ty = "GitObjectId"),
    field(name = "abbreviated_oid", ty = "String")
)]
pub enum GitObject {
    Blob(Blob),
    Commit(Box<Commit>),
}

pub struct Blob {
    sha: String,
    size: u64,
    /// The file's bytes, when Forgejo sent them.
    content: Option<Vec<u8>>,
}
#[Object]
impl Blob {
    async fn oid(&self) -> GitObjectId {
        GitObjectId(self.sha.clone())
    }
    async fn abbreviated_oid(&self) -> String {
        self.sha.chars().take(7).collect()
    }
    async fn byte_size(&self) -> i32 {
        i32::try_from(self.size).unwrap_or(i32::MAX)
    }
    /// Text with a NUL byte, or that isn't UTF-8, is binary, as git decides.
    async fn is_binary(&self) -> Option<bool> {
        self.content.as_deref().map(binary)
    }
    async fn is_truncated(&self) -> bool {
        self.content
            .as_deref()
            .is_some_and(|bytes| !binary(bytes) && bytes.len() > MAX_BLOB_TEXT)
    }
    /// Null for a binary blob, or one Forgejo sent no content for (over its blob size limit).
    async fn text(&self) -> Option<String> {
        let bytes = self.content.as_deref().filter(|bytes| !binary(bytes))?;
        let mut end = bytes.len().min(MAX_BLOB_TEXT);
        // Cut on a character boundary.
        while std::str::from_utf8(&bytes[..end]).is_err() {
            end -= 1;
        }
        Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

fn binary(bytes: &[u8]) -> bool {
    bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
}

/// `Repository.object(expression:)`: `REV:PATH` names a blob, and a bare `REV` a commit. A path
/// or revision that doesn't exist is null, as on GitHub, and so is a directory or a submodule:
/// the gateway serves no trees.
pub(super) async fn object(
    ctx: &Context<'_>,
    owner: &str,
    repo: &str,
    default_branch: &str,
    expression: &str,
) -> async_graphql::Result<Option<GitObject>> {
    let session = ctx.data_unchecked::<Session>();
    let (rev, path) = match expression.split_once(':') {
        Some((rev, path)) => (rev, Some(path)),
        None => (expression, None),
    };
    let rev = match rev {
        "" | "HEAD" => default_branch,
        rev => rev,
    };
    let Some(path) = path else {
        return match session.resolve_commit(owner, repo, rev).await {
            Ok(commit) => Ok(Some(GitObject::Commit(Box::new(Commit {
                owner: owner.into(),
                repo: repo.into(),
                sha: commit.sha.clone(),
                source: Some(commit.into()),
            })))),
            Err(forgejo::Error::NotFound) => Ok(None),
            Err(e) => Err(error(e)),
        };
    };
    let path = path.trim_matches('/');
    if path.is_empty() {
        return Ok(None);
    }
    let value = match session.contents(owner, repo, path, Some(rev)).await {
        Ok(value) => value,
        Err(forgejo::Error::NotFound) => return Ok(None),
        Err(e) => return Err(error(e)),
    };
    // A directory is an array; a file, a symlink or a submodule is one entry.
    let Ok(entry) = serde_json::from_value::<forgejo::Contents>(value) else {
        return Ok(None);
    };
    if !matches!(entry.kind.as_str(), "file" | "symlink") {
        return Ok(None);
    }
    let content = match (entry.content, entry.target) {
        (Some(content), _) => Some(
            STANDARD
                .decode(content.replace(['\n', '\r'], ""))
                .map_err(|_| error(forgejo::Error::Upstream))?,
        ),
        // A symlink's blob is its target.
        (None, Some(target)) if entry.kind == "symlink" => Some(target.into_bytes()),
        (None, _) => None,
    };
    Ok(Some(GitObject::Blob(Blob {
        sha: entry.sha,
        size: entry.size,
        content,
    })))
}
