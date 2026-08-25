//! Repository parsing and host detection for the native dispatcher.

use std::collections::HashMap;
use std::path::Path;

use crate::external::git_output;
use crate::invocation::{ParsedInvocation, ProviderTarget};
use crate::provider::normalize_host;

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
    let invocation = ParsedInvocation::parse(argv);
    detect_repo_for_target(invocation.provider_target(), env, cwd)
}

pub fn detect_repo_for_target(
    command_target: &ProviderTarget,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> Detection {
    if let Some(repo_arg) = command_target.repo_spec() {
        if let Some(repo) = parse_repo_spec(repo_arg, env.get("GH_HOST").map(String::as_str)) {
            let source = if command_target.repo_is_flag() {
                "-R/--repo"
            } else {
                "command target"
            };
            return Detection::found(repo, source);
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

pub fn command_provider_target(argv: &[String]) -> ProviderTarget {
    ParsedInvocation::parse(argv).provider_target().clone()
}

fn parse_url_repo(value: &str) -> Option<RepoRef> {
    let (scheme, rest) = value.split_once("://")?;
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ssh" | "git"
    ) {
        return None;
    }

    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || path.is_empty() {
        return None;
    }
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
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

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
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
    fn parses_url_schemes_case_insensitively() {
        for spec in [
            "HTTP://git.example.com/owner/repo.git",
            "HTTPS://git.example.com/owner/repo.git",
            "SSH://git@git.example.com/owner/repo.git",
            "GIT://git.example.com/owner/repo.git",
        ] {
            assert_eq!(
                repo_tuple(parse_repo_spec(spec, None)),
                Some(("git.example.com".into(), "owner".into(), "repo".into())),
                "URL scheme case for {spec}"
            );
        }
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

    #[test]
    fn command_target_grammar_skips_every_accepted_url_option_shape() {
        let issue_url = "https://git.example.com/owner/repo/issues/13";
        let pull_url = "https://git.example.com/owner/repo/pulls/7";
        let cases = vec![
            argv(&["issue", "view", "--comments", issue_url]),
            argv(&["issue", "view", "-c", issue_url]),
            argv(&["issue", "view", "--web", issue_url]),
            argv(&["issue", "view", "-w", issue_url]),
            argv(&["issue", "view", "--json", "number", issue_url]),
            argv(&["issue", "view", "--json=number", issue_url]),
            argv(&["issue", "view", "--jq", ".number", issue_url]),
            argv(&["issue", "view", "-q=.number", issue_url]),
            argv(&["issue", "view", "--template", "{{.number}}", issue_url]),
            argv(&["issue", "view", "-t={{.number}}", issue_url]),
            argv(&["pr", "checks", "--json", "name", pull_url]),
            argv(&["pr", "checks", "--json=name", pull_url]),
            argv(&["pr", "checks", "--jq", ".name", pull_url]),
            argv(&["pr", "checks", "-q=.name", pull_url]),
            argv(&["pr", "checks", "--template", "{{.name}}", pull_url]),
            argv(&["pr", "checks", "-t={{.name}}", pull_url]),
            argv(&["pr", "checks", "--interval", "10", pull_url]),
            argv(&["pr", "checks", "-i=10", pull_url]),
            argv(&["pr", "checks", "--web", pull_url]),
            argv(&["pr", "checks", "-w", pull_url]),
            argv(&["pr", "checks", "--watch", pull_url]),
            argv(&["pr", "checks", "--fail-fast", pull_url]),
            argv(&["pr", "checks", "--required", pull_url]),
            argv(&["pr", "diff", "--web", pull_url]),
            argv(&["pr", "diff", "-w", pull_url]),
            argv(&["pr", "diff", "--name-only", pull_url]),
            argv(&["pr", "diff", "--patch", pull_url]),
            argv(&["pr", "diff", "--color", "always", pull_url]),
            argv(&["pr", "diff", "--color=always", pull_url]),
            argv(&["pr", "diff", "--exclude", "generated", pull_url]),
            argv(&["pr", "diff", "-e=generated", pull_url]),
            argv(&["pr", "checkout", "--branch", "local", pull_url]),
            argv(&["pr", "checkout", "-b=local", pull_url]),
            argv(&["pr", "checkout", "--detach", pull_url]),
            argv(&["pr", "checkout", "--force", pull_url]),
            argv(&["pr", "checkout", "-f", pull_url]),
            argv(&["pr", "checkout", "--recurse-submodules", pull_url]),
            argv(&["pr", "comment", "--body", "text", pull_url]),
            argv(&["pr", "comment", "-b=text", pull_url]),
            argv(&["pr", "comment", "--body-file", "body.md", pull_url]),
            argv(&["pr", "comment", "-F=body.md", pull_url]),
            argv(&["pr", "comment", "--web", pull_url]),
            argv(&["pr", "comment", "-w", pull_url]),
            argv(&["pr", "view", "--json", "number", pull_url]),
            argv(&["pr", "view", "--json=number", pull_url]),
            argv(&["pr", "view", "--jq", ".number", pull_url]),
            argv(&["pr", "view", "-q=.number", pull_url]),
            argv(&["pr", "view", "--template", "{{.number}}", pull_url]),
            argv(&["pr", "view", "-t={{.number}}", pull_url]),
            argv(&["pr", "view", "--web", pull_url]),
            argv(&["pr", "view", "-w", pull_url]),
        ];

        for argv in cases {
            assert_eq!(
                command_provider_target(&argv).repo_spec(),
                Some(if argv[0] == "issue" {
                    issue_url
                } else {
                    pull_url
                }),
                "argv: {argv:?}"
            );
        }
    }

    #[test]
    fn command_target_grammar_covers_real_gh_flags_and_attached_short_values() {
        let issue_url = "https://git.example.com/owner/repo/issues/13";
        let pull_url = "https://git.example.com/owner/repo/pulls/7";
        let repo = "git.example.com/owner/repo";
        let repo_cases = vec![
            (argv(&["issue", "view", "-q.number", issue_url]), issue_url),
            (
                argv(&["issue", "view", "-t{{.number}}", issue_url]),
                issue_url,
            ),
            (argv(&["pr", "view", "--comments", pull_url]), pull_url),
            (argv(&["pr", "view", "-c", pull_url]), pull_url),
            (argv(&["pr", "checks", "-i10", pull_url]), pull_url),
            (argv(&["pr", "checkout", "-blocal", pull_url]), pull_url),
            (argv(&["pr", "comment", "--editor", pull_url]), pull_url),
            (argv(&["pr", "comment", "-e", pull_url]), pull_url),
            (argv(&["pr", "comment", "--edit-last", pull_url]), pull_url),
            (argv(&["pr", "list", "--draft", "--repo", repo]), repo),
            (argv(&["pr", "list", "-d", "--repo", repo]), repo),
            (argv(&["pr", "list", "--web", "--repo", repo]), repo),
            (argv(&["pr", "list", "-w", "--repo", repo]), repo),
            (argv(&["pr", "list", "-Ame", "--repo", repo]), repo),
            (argv(&["pr", "list", "-aapp", "--repo", repo]), repo),
            (argv(&["pr", "list", "-lbug", "--repo", repo]), repo),
            (argv(&["pr", "list", "-Sdraft", "--repo", repo]), repo),
            (argv(&["pr", "create", "--dry-run", "--repo", repo]), repo),
            (argv(&["pr", "create", "-f", "--repo", repo]), repo),
            (
                argv(&["pr", "status", "--conflict-status", "--repo", repo]),
                repo,
            ),
            (argv(&["pr", "status", "-c", "--repo", repo]), repo),
            (
                argv(&["issue", "view", "-Rgit.example.com/owner/repo", "13"]),
                repo,
            ),
        ];

        for (argv, expected) in repo_cases {
            assert_eq!(
                command_provider_target(&argv).repo_spec(),
                Some(expected),
                "argv: {argv:?}"
            );
        }

        let host_cases = vec![
            argv(&["auth", "status", "-hgit.example.com"]),
            argv(&["api", "-i", "--hostname", "git.example.com", "user"]),
            argv(&[
                "api",
                "--paginate",
                "--hostname",
                "git.example.com",
                "repos/owner/repo/issues",
            ]),
            argv(&["api", "--verbose", "--hostname", "git.example.com", "user"]),
        ];

        for argv in host_cases {
            assert_eq!(
                command_provider_target(&argv).host(),
                Some("git.example.com"),
                "argv: {argv:?}"
            );
        }
    }
}
