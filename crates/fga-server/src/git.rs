//! Comparisons and file contents in GitHub's REST shapes (MAPPING §11).
use crate::{Host, rest_error};
use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode, header::ACCEPT},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use fga_core::{
    forgejo::{Comparison, Error, GitCommit, PullRequest, Session, is_ref_name},
    map,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// GitHub serves a file's content inline up to 1 MiB; a larger file comes with an empty
/// `content` and `encoding: none`, to be read raw.
const MAX_INLINE_CONTENT: u64 = 1024 * 1024;

/// A comparison whose commits leave more candidate merge bases than this gets no merge base.
const MAX_MERGE_BASE_CANDIDATES: usize = 20;

fn accepts(headers: &HeaderMap, media: &[&str]) -> bool {
    headers
        .get_all(ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|v| media.iter().any(|m| v.trim().starts_with(m)))
}

/// The commit the comparison starts from: the newest common ancestor of `base` and `head`.
/// Forgejo's comparison names none, so it is the parent, outside the compared commits, that
/// every other such parent is an ancestor of. `None` when the two share no history, or when
/// Forgejo cut the commit list short.
async fn merge_base(
    session: &Session,
    owner: &str,
    repo: &str,
    comparison: &Comparison,
    head: &str,
) -> Result<Option<String>, Error> {
    if comparison.total_commits == 0 {
        return Ok(Some(head.to_owned()));
    }
    if (comparison.commits.len() as i64) < comparison.total_commits {
        return Ok(None);
    }
    let compared: HashSet<&str> = comparison.commits.iter().map(|c| c.sha.as_str()).collect();
    let mut candidates: Vec<&str> = Vec::new();
    for parent in comparison.commits.iter().flat_map(|c| &c.parents) {
        if !compared.contains(parent.sha.as_str()) && !candidates.contains(&parent.sha.as_str()) {
            candidates.push(&parent.sha);
        }
    }
    if candidates.len() > MAX_MERGE_BASE_CANDIDATES {
        return Ok(None);
    }
    let Some((&first, rest)) = candidates.split_first() else {
        return Ok(None);
    };
    let mut best = first;
    for &candidate in rest {
        // Nothing in `best` that `candidate` lacks: `best` is its ancestor, so it is newer.
        if session
            .compare(owner, repo, candidate, best)
            .await?
            .total_commits
            == 0
        {
            best = candidate;
        }
    }
    // Prove it: every other candidate must be an ancestor of `best`. Two that aren't ordered
    // (a criss-cross merge) leave no single merge base.
    for &candidate in &candidates {
        if candidate != best
            && session
                .compare(owner, repo, best, candidate)
                .await?
                .total_commits
                != 0
        {
            return Ok(None);
        }
    }
    Ok(Some(best.to_owned()))
}

/// The pull request whose diff is this comparison's: the same head, branched from the same merge
/// base. Open pull requests are searched first.
async fn matching_pull(
    session: &Session,
    owner: &str,
    repo: &str,
    head: &str,
    merge_base: &str,
) -> Result<Option<PullRequest>, Error> {
    for state in ["open", "closed"] {
        let pulls = session
            .pull_requests(owner, repo, state, None, None)
            .await?;
        if let Some(pr) = pulls
            .into_iter()
            .find(|pr| pr.head.sha == head && pr.merge_base.as_deref() == Some(merge_base))
        {
            return Ok(Some(pr));
        }
    }
    Ok(None)
}

fn no_diff() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(
            json!({"message": "Forgejo gives a comparison's diff only when it is a pull request's: \
            compare a pull request's base and head"}),
        ),
    )
        .into_response()
}

/// `GET /repos/{o}/{r}/compare/{base}...{head}`: GitHub's comparison, or with
/// `Accept: application/vnd.github.diff` (or `.patch`) the raw diff. Each end is a SHA, branch or
/// tag, and a branch may hold slashes (`main...feature/topic`); both are resolved to commits
/// first. GitHub's API refuses two dots, and so does the gateway.
pub async fn compare(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, basehead)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    // Both ends are checked before either is looked up.
    let Some((base, head)) = basehead
        .split_once("...")
        .filter(|(base, head)| is_ref_name(base) && is_ref_name(head))
    else {
        return rest_error(Error::NotFound);
    };
    let raw = if accepts(
        &headers,
        &[
            "application/vnd.github.v3.diff",
            "application/vnd.github.diff",
        ],
    ) {
        Some("diff")
    } else if accepts(
        &headers,
        &[
            "application/vnd.github.v3.patch",
            "application/vnd.github.patch",
        ],
    ) {
        Some("patch")
    } else {
        None
    };
    let result = async {
        let base_commit = session.resolve_commit(&owner, &repo, base).await?;
        let head_commit = session.resolve_commit(&owner, &repo, head).await?;
        let comparison = session
            .compare(&owner, &repo, &base_commit.sha, &head_commit.sha)
            .await?;
        let Some(merge_base) =
            merge_base(&session, &owner, &repo, &comparison, &head_commit.sha).await?
        else {
            return Err(Error::NotFound);
        };
        let pr = matching_pull(&session, &owner, &repo, &head_commit.sha, &merge_base).await?;
        if let Some(kind) = raw {
            let Some(pr) = pr else {
                return Ok(no_diff());
            };
            let text = session
                .text(
                    &[
                        "repos",
                        &owner,
                        &repo,
                        "pulls",
                        &format!("{}.{kind}", pr.number),
                    ],
                    &[],
                )
                .await?;
            return Ok(([("content-type", "text/plain; charset=utf-8")], text).into_response());
        }
        let behind_by = session
            .compare(&owner, &repo, &head_commit.sha, &base_commit.sha)
            .await?
            .total_commits;
        let ahead_by = comparison.total_commits;
        let files = match &pr {
            Some(pr) => {
                let files = session.pull_files(&owner, &repo, pr.number).await?;
                let patches =
                    map::diff_patches(&session.pull_diff(&owner, &repo, pr.number).await?);
                map::changed_files(
                    &files,
                    &patches,
                    &owner,
                    &repo,
                    &head_commit.sha,
                    &host.public_url,
                )
            }
            // Forgejo's comparison counts no lines; without a pull request there is no diff.
            None => map::changed_files(
                &comparison.files,
                &HashMap::new(),
                &owner,
                &repo,
                &head_commit.sha,
                &host.public_url,
            ),
        };
        let merge_base_commit: GitCommit = if merge_base == base_commit.sha {
            base_commit.clone()
        } else {
            session.git_commit(&owner, &repo, &merge_base).await?
        };
        let commit = |c: &GitCommit| map::commit(c.into(), &owner, &repo, &host.public_url);
        let web = format!(
            "{}/{owner}/{repo}/compare",
            host.public_url.as_str().trim_end_matches('/')
        );
        let html_url = format!("{web}/{basehead}");
        // A branch's slashes stay slashes, as in GitHub's own `url`.
        let mut url = vec!["compare"];
        url.extend(basehead.split('/'));
        Ok(Json(json!({
            "url": map::api_url(&host.public_url, &owner, &repo, &url).as_str(),
            "html_url": html_url,
            "permalink_url": format!("{web}/{}...{}", base_commit.sha, head_commit.sha),
            "diff_url": format!("{html_url}.diff"),
            "patch_url": format!("{html_url}.patch"),
            "base_commit": commit(&base_commit),
            "merge_base_commit": commit(&merge_base_commit),
            "status": match (ahead_by, behind_by) {
                (0, 0) => "identical",
                (_, 0) => "ahead",
                (0, _) => "behind",
                _ => "diverged",
            },
            "ahead_by": ahead_by,
            "behind_by": behind_by,
            "total_commits": ahead_by,
            // Forgejo lists newest first; GitHub, oldest first.
            "commits": comparison.commits.iter().rev().map(commit).collect::<Vec<_>>(),
            "files": files,
        }))
        .into_response())
    }
    .await;
    result.unwrap_or_else(rest_error)
}

#[derive(Default, Deserialize)]
pub struct ContentsQuery {
    #[serde(rename = "ref")]
    rev: Option<String>,
}

/// `GET /repos/{o}/{r}/contents`: the root directory.
pub async fn contents_root(
    host: Extension<Arc<Host>>,
    session: Extension<Session>,
    Path((owner, repo)): Path<(String, String)>,
    query: Query<ContentsQuery>,
    headers: HeaderMap,
) -> Response {
    contents(
        host,
        session,
        Path((owner, repo, String::new())),
        query,
        headers,
    )
    .await
}

/// `GET /repos/{o}/{r}/contents/{path}?ref=`: a file with its base64 content up to 1 MiB, or a
/// directory's entries; with `Accept: application/vnd.github.object+json` a directory is one
/// object holding `entries`, and with `application/vnd.github.raw` a file is its bytes.
pub async fn contents(
    Extension(host): Extension<Arc<Host>>,
    Extension(session): Extension<Session>,
    Path((owner, repo, path)): Path<(String, String, String)>,
    Query(query): Query<ContentsQuery>,
    headers: HeaderMap,
) -> Response {
    let rev = query.rev.as_deref().filter(|rev| !rev.is_empty());
    let path = path.trim_matches('/').to_owned();
    let result = async {
        let value = session.contents(&owner, &repo, &path, rev).await?;
        if accepts(
            &headers,
            &[
                "application/vnd.github.raw",
                "application/vnd.github.v3.raw",
            ],
        ) {
            if value.is_array() || value["type"] != "file" {
                return Err(Error::NotFound);
            }
            let bytes = session.raw_file(&owner, &repo, &path, rev).await?;
            return Ok(([("content-type", "application/octet-stream")], bytes).into_response());
        }
        let entry = |v: &Value, inline: bool| {
            content_entry(v, inline, &owner, &repo, rev, &host.public_url)
        };
        Ok(match &value {
            Value::Array(entries) => {
                let entries: Vec<Value> = entries.iter().map(|e| entry(e, false)).collect();
                if accepts(&headers, &["application/vnd.github.object"]) {
                    let name = path.rsplit('/').next().unwrap_or_default();
                    let mut dir = entry(&json!({"type": "dir", "name": name, "path": path}), false);
                    dir["entries"] = json!(entries);
                    Json(dir).into_response()
                } else {
                    Json(entries).into_response()
                }
            }
            file => Json(entry(file, true)).into_response(),
        })
    }
    .await;
    result.unwrap_or_else(rest_error)
}

/// One entry of a contents reply in GitHub's shape. Links point at the gateway where it serves
/// the resource and at Forgejo's pages otherwise; GitHub's `git_url` (a blob under `/git/blobs`)
/// isn't served, so it is null.
fn content_entry(
    v: &Value,
    inline: bool,
    owner: &str,
    repo: &str,
    rev: Option<&str>,
    public: &url::Url,
) -> Value {
    let path = v["path"].as_str().unwrap_or_default();
    let mut url = map::api_url(public, owner, repo, &["contents"]);
    url.path_segments_mut()
        .expect("validated base URL")
        .extend(path.split('/').filter(|s| !s.is_empty()));
    if let Some(rev) = rev {
        url.query_pairs_mut().append_pair("ref", rev);
    }
    let size = v["size"].as_u64().unwrap_or(0);
    let mut entry = json!({
        "type": v["type"],
        "name": v["name"],
        "path": path,
        "sha": v.get("sha").cloned().unwrap_or(Value::Null),
        "size": size,
        "url": url.as_str(),
        "html_url": v.get("html_url").cloned().unwrap_or(Value::Null),
        "git_url": null,
        "download_url": v.get("download_url").cloned().unwrap_or(Value::Null),
        "_links": {"self": url.as_str(), "git": null, "html": v.get("html_url").cloned().unwrap_or(Value::Null)},
    });
    match v["type"].as_str() {
        Some("file") if inline => {
            let content = v["content"].as_str().filter(|_| size <= MAX_INLINE_CONTENT);
            entry["encoding"] = json!(if content.is_some() { "base64" } else { "none" });
            // GitHub wraps base64 at 60 columns; decoders accept either.
            entry["content"] = json!(content.map(wrap).unwrap_or_default());
        }
        Some("symlink") => entry["target"] = v["target"].clone(),
        Some("submodule") => entry["submodule_git_url"] = v["submodule_git_url"].clone(),
        _ => {}
    }
    entry
}

fn wrap(content: &str) -> String {
    let compact: String = content
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    // Re-encode only what decodes, so a bad reply never passes through as content.
    match STANDARD.decode(&compact) {
        Ok(_) => {
            compact
                .as_bytes()
                .chunks(60)
                .map(|line| std::str::from_utf8(line).expect("base64 is ASCII"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        }
        Err(_) => String::new(),
    }
}
