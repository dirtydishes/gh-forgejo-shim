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

pub fn pull_request(pr: &forgejo::PullRequest, prefixes: &WipPrefixes) -> Value {
    json!({
        "number": pr.number,
        "title": title(pr, prefixes),
        "state": if pr.merged { "closed" } else { &pr.state },
        "draft": pr.draft,
        "html_url": pr.html_url,
        "head": {"ref": pr.head.name(), "sha": pr.head.sha, "label": pr.head.label()},
        "base": {"ref": pr.base.ref_name, "sha": pr.base.sha, "label": pr.base.label()},
        "user": {"login": pr.user.login},
        "merged_at": if pr.merged { pr.merged_at.as_ref() } else { None },
    })
}

mod checks;
mod pulls;
pub use checks::*;
pub use pulls::*;

#[cfg(test)]
mod tests;
