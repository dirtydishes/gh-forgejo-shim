use crate::{forgejo, ids::node_id};
use serde_json::{Value, json};
use url::Url;

pub fn user(user: &forgejo::User, public: &Url) -> Value {
    let mut value = json!({
        "login": user.login,
        "id": user.id,
        "node_id": node_id("User", "", user.id),
        "type": "User",
        "avatar_url": user.avatar_url,
        "html_url": user.html_url.clone().unwrap_or_else(|| {
            let mut url = public.clone();
            url.path_segments_mut().expect("validated HTTP URL").pop_if_empty().push(&user.login);
            url.to_string()
        }),
    });
    if let Some(name) = &user.full_name {
        value["name"] = json!(name);
    }
    value
}

pub fn repository(repo: &forgejo::Repository, owner_type: &str) -> Value {
    let mut value = json!({
        "id": repo.id,
        "node_id": node_id("Repository", &repo.full_name, repo.id),
        "name": repo.name,
        "full_name": repo.full_name,
        "owner": {"login": repo.owner.login, "id": repo.owner.id, "type": owner_type},
        "private": repo.private,
        "html_url": repo.html_url,
        "default_branch": repo.default_branch,
        "fork": repo.fork,
        "archived": repo.archived,
    });
    if let Some(permissions) = &repo.permissions {
        value["permissions"] = json!(permissions);
    }
    value
}

/// A pull request's web URL, or one under it (`/files`, `#issuecomment-…`), in GitHub's layout:
/// Forgejo's `/{o}/{r}/pulls/{n}` becomes `/{o}/{r}/pull/{n}` (MAPPING §8). ChatGPT ignores a PR whose
/// URL doesn't match `/pull/(\d+)`. Only that path segment changes: the host, the rest of the path,
/// the query and the fragment stay as Forgejo sent them, and any other URL passes through.
pub fn pull_web_url(url: &str) -> String {
    rename_pull_segment(url, "pulls", "pull")
}

/// The inverse of [`pull_web_url`]: the page Forgejo serves, for opening in a browser where no
/// redirect from `/pull/{n}` exists.
pub fn forgejo_pull_web_url(url: &str) -> String {
    rename_pull_segment(url, "pull", "pulls")
}

/// Renames `{from}/{n}` to `{to}/{n}` where it is a pull request's page: after an owner and a
/// repository, and followed by nothing or one of the PR's own tabs. That leaves an owner or
/// repository named `pulls` alone, even on a forge served under a subpath.
fn rename_pull_segment(url: &str, from: &str, to: &str) -> String {
    // The authority ends at the first `/`, `?` or `#`; only a `/` starts a path.
    let path_start = url.find("://").and_then(|scheme| {
        let authority = scheme + 3;
        url[authority..]
            .find(['/', '?', '#'])
            .map(|end| authority + end)
            .filter(|&end| url[end..].starts_with('/'))
    });
    let Some(start) = path_start else {
        return url.to_owned();
    };
    let end = url[start..]
        .find(['?', '#'])
        .map_or(url.len(), |end| start + end);
    let mut segments: Vec<&str> = url[start..end].split('/').collect();
    // Segment 0 is empty (the path's leading slash); owner and repository come before `pulls`.
    let number = |segment: &str| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit());
    let pr_tab = |segment: Option<&&str>| {
        matches!(
            segment,
            None | Some(&"" | &"files" | &"commits" | &"checks")
        )
    };
    match (3..segments.len().saturating_sub(1))
        .find(|&i| segments[i] == from && number(segments[i + 1]) && pr_tab(segments.get(i + 2)))
    {
        Some(i) => {
            segments[i] = to;
            format!("{}{}{}", &url[..start], segments.join("/"), &url[end..])
        }
        None => url.to_owned(),
    }
}

pub fn pull_request(pr: &forgejo::PullRequest, prefixes: &WipPrefixes) -> Value {
    json!({
        "number": pr.number,
        "title": title(pr, prefixes),
        "state": if pr.merged { "closed" } else { &pr.state },
        "draft": pr.draft,
        "html_url": pull_web_url(&pr.html_url),
        "head": {"ref": pr.head.name(), "sha": pr.head.sha, "label": pr.head.label()},
        "base": {"ref": pr.base.ref_name, "sha": pr.base.sha, "label": pr.base.label()},
        "user": {"login": pr.user.login},
        "merged_at": if pr.merged { pr.merged_at.as_ref() } else { None },
    })
}

/// A body as GitHub-shaped HTML without rendering its Markdown: escaped paragraphs and line
/// breaks. Forgejo's API returns no rendered HTML, rendering through it costs a request per body,
/// and clients may trust bodyHTML as sanitized, as GitHub's is.
pub fn body_html(body: &str) -> String {
    let escaped = body
        .replace('\r', "")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    escaped
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| format!("<p>{}</p>", paragraph.replace('\n', "<br>\n")))
        .collect::<Vec<_>>()
        .join("\n")
}

mod checks;
mod pulls;
pub use checks::*;
pub use pulls::*;

#[cfg(test)]
mod tests;
