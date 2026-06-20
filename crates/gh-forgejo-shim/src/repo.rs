//! Repository parsing and host detection for the native dispatcher.

use std::collections::HashMap;
use std::path::Path;

use crate::external::git_output;
use crate::routing::normalize_host;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl RepoRef {
    pub fn new(host: impl Into<String>, owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            host: normalize_host(&host.into()),
            owner: owner.into(),
            name: strip_git_suffix(&name.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub repo: Option<RepoRef>,
    pub source: String,
}

impl Detection {
    fn found(repo: RepoRef, source: &str) -> Self {
        Self {
            repo: Some(repo),
            source: source.to_string(),
        }
    }

    fn unknown() -> Self {
        Self {
            repo: None,
            source: "unknown".to_string(),
        }
    }
}

pub fn parse_repo_spec(spec: &str, default_host: Option<&str>) -> Option<RepoRef> {
    let value = spec.trim();
    if value.is_empty() {
        return None;
    }

    if let Some(repo) = parse_url_repo(value) {
        return Some(repo);
    }
    if let Some(repo) = parse_scp_repo(value) {
        return Some(repo);
    }

    let parts = path_parts(value);
    if parts.len() >= 3 && looks_like_host(parts[0]) {
        return Some(RepoRef::new(
            parts[0],
            parts[parts.len() - 2],
            parts[parts.len() - 1],
        ));
    }
    if parts.len() == 2 {
        return default_host.map(|host| RepoRef::new(host, parts[0], parts[1]));
    }
    None
}

pub fn detect_repo(
    argv: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> Detection {
    if let Some(repo_arg) = extract_repo_arg(argv) {
        if let Some(repo) = parse_repo_spec(repo_arg, env.get("GH_HOST").map(String::as_str)) {
            return Detection::found(repo, "-R/--repo");
        }
    }

    if let Some(url_arg) = extract_repo_url_arg(argv) {
        if let Some(repo) = parse_repo_spec(url_arg, env.get("GH_HOST").map(String::as_str)) {
            return Detection::found(repo, "command URL");
        }
    }

    if let Some(gh_repo) = env.get("GH_REPO").filter(|value| !value.trim().is_empty()) {
        if let Some(repo) = parse_repo_spec(gh_repo, env.get("GH_HOST").map(String::as_str)) {
            return Detection::found(repo, "GH_REPO");
        }
    }

    let remote_repo = detect_from_git(cwd);
    if let (Some(host), Some(remote)) = (env.get("GH_HOST"), remote_repo.as_ref()) {
        if !host.trim().is_empty() {
            return Detection::found(
                RepoRef::new(host.as_str(), remote.owner.as_str(), remote.name.as_str()),
                "GH_HOST",
            );
        }
    }

    if let Some(repo) = remote_repo {
        return Detection::found(repo, "git remote");
    }

    Detection::unknown()
}

pub fn detect_from_git(cwd: Option<&Path>) -> Option<RepoRef> {
    if let Some(remote) = git_output(&["remote", "get-url", "origin"], cwd) {
        if let Some(repo) = parse_repo_spec(&remote, None) {
            return Some(repo);
        }
    }

    let remotes = git_output(&["remote"], cwd)?;
    for name in remotes.lines().filter(|name| !name.trim().is_empty()) {
        if let Some(remote) = git_output(&["remote", "get-url", name], cwd) {
            if let Some(repo) = parse_repo_spec(&remote, None) {
                return Some(repo);
            }
        }
    }
    None
}

pub fn extract_repo_arg(argv: &[String]) -> Option<&str> {
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if matches!(arg, "-R" | "--repo") && index + 1 < argv.len() {
            return Some(argv[index + 1].as_str());
        }
        if let Some(value) = arg.strip_prefix("--repo=") {
            return Some(value);
        }
        index += 1;
    }
    None
}

pub fn extract_repo_url_arg(argv: &[String]) -> Option<&str> {
    argv.iter()
        .map(String::as_str)
        .find(|arg| matches!(url_scheme(arg), Some("http" | "https" | "ssh" | "git")))
}

fn parse_url_repo(value: &str) -> Option<RepoRef> {
    let (scheme, rest) = value.split_once("://")?;
    if !matches!(scheme, "http" | "https" | "ssh" | "git") {
        return None;
    }

    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || path.is_empty() {
        return None;
    }
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .split(':')
        .next()
        .unwrap_or(authority);
    if host.is_empty() {
        return None;
    }

    let parts = path_parts(path);
    if parts.len() >= 4 && matches!(parts[2], "issue" | "issues" | "pull" | "pulls") {
        return Some(RepoRef::new(host, parts[0], parts[1]));
    }
    if parts.len() >= 2 {
        return Some(RepoRef::new(
            host,
            parts[parts.len() - 2],
            parts[parts.len() - 1],
        ));
    }
    None
}

fn parse_scp_repo(value: &str) -> Option<RepoRef> {
    let (prefix, path) = value.split_once(':')?;
    if prefix.contains('/') || prefix.chars().any(char::is_whitespace) || !path.contains('/') {
        return None;
    }
    let host = prefix.rsplit_once('@').map_or(prefix, |(_, host)| host);
    if host.is_empty() || host.contains('/') || host.chars().any(char::is_whitespace) {
        return None;
    }
    let parts = path_parts(path);
    if parts.len() >= 2 {
        return Some(RepoRef::new(
            host,
            parts[parts.len() - 2],
            parts[parts.len() - 1],
        ));
    }
    None
}

fn url_scheme(value: &str) -> Option<&str> {
    let (scheme, _) = value.split_once("://")?;
    Some(scheme)
}

fn path_parts(path: &str) -> Vec<&str> {
    path.trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect()
}

fn strip_git_suffix(value: &str) -> String {
    value.strip_suffix(".git").unwrap_or(value).to_string()
}

fn looks_like_host(value: &str) -> bool {
    value.contains('.') || value.contains(':') || value == "localhost"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_tuple(repo: Option<RepoRef>) -> Option<(String, String, String)> {
        repo.map(|repo| (repo.host, repo.owner, repo.name))
    }

    #[test]
    fn parses_https_url() {
        assert_eq!(
            repo_tuple(parse_repo_spec(
                "https://git.example.com/owner/repo.git",
                None
            )),
            Some(("git.example.com".into(), "owner".into(), "repo".into()))
        );
    }

    #[test]
    fn parses_ssh_url() {
        assert_eq!(
            repo_tuple(parse_repo_spec(
                "ssh://git@git.example.com/owner/repo.git",
                None
            )),
            Some(("git.example.com".into(), "owner".into(), "repo".into()))
        );
    }

    #[test]
    fn parses_scp_style_ssh_url() {
        assert_eq!(
            repo_tuple(parse_repo_spec("git@git.example.com:owner/repo.git", None)),
            Some(("git.example.com".into(), "owner".into(), "repo".into()))
        );
    }

    #[test]
    fn parses_host_prefixed_owner_repo() {
        assert_eq!(
            repo_tuple(parse_repo_spec("git.example.com/owner/repo", None)),
            Some(("git.example.com".into(), "owner".into(), "repo".into()))
        );
    }

    #[test]
    fn parses_plain_owner_repo_with_default_host() {
        assert_eq!(
            repo_tuple(parse_repo_spec("owner/repo", Some("git.example.com"))),
            Some(("git.example.com".into(), "owner".into(), "repo".into()))
        );
    }

    #[test]
    fn leaves_plain_owner_repo_unknown_without_default_host() {
        assert_eq!(parse_repo_spec("owner/repo", None), None);
    }

    #[test]
    fn parses_pull_url_as_repository_path() {
        assert_eq!(
            repo_tuple(parse_repo_spec(
                "https://git.example.com/owner/repo/pulls/7",
                None
            )),
            Some(("git.example.com".into(), "owner".into(), "repo".into()))
        );
    }

    #[test]
    fn parses_issue_url_as_repository_path() {
        assert_eq!(
            repo_tuple(parse_repo_spec(
                "https://git.example.com/owner/repo/issues/13",
                None
            )),
            Some(("git.example.com".into(), "owner".into(), "repo".into()))
        );
    }
}
