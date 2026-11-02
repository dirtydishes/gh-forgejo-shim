//! Repository parsing and host detection for the native dispatcher.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::normalize_host;
use crate::external::git_output;
use crate::invocation::Invocation;

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

const GITHUB: &str = "github.com";

/// The hosts gh knows and its default host, as the wrapper models them: the user's gh
/// `hosts.yml` plus every Forgejo host the shim serves, which gh can reach through the shim.
/// gh v2.103.3 rules: `go-gh/pkg/auth/auth.go` `defaultHost`, and
/// `gh/pkg/cmd/factory/remote_resolver.go` `Resolver`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhHosts {
    known: Vec<String>,
    default: String,
    default_source: DefaultSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefaultSource {
    /// `GH_HOST`: gh keeps only remotes on that host, even when none match.
    Env,
    /// The only configured host: gh prefers remotes on it and falls back to all of them.
    Config,
    /// No single configured host, so github.com, and remotes aren't filtered by it.
    Fallback,
}

impl GhHosts {
    /// What plain gh knows: the user's own `hosts.yml` alone.
    pub fn user(env: &HashMap<String, String>) -> Self {
        Self::load(env, &[])
    }

    /// `forgejo_hosts` are the shim's allowlist entries. gh keeps only the hostname of a remote
    /// URL, never its port, so the entries count without theirs.
    pub fn load(env: &HashMap<String, String>, forgejo_hosts: &[String]) -> Self {
        let mut configured = user_gh_hosts(env);
        for host in forgejo_hosts {
            push_unique(&mut configured, gh_hostname(host));
        }
        let (default, default_source) = match (
            env.get("GH_HOST").filter(|host| !host.is_empty()),
            configured.as_slice(),
        ) {
            (Some(host), _) => (host.to_ascii_lowercase(), DefaultSource::Env),
            (None, [only]) => (only.clone(), DefaultSource::Config),
            (None, _) => (GITHUB.to_string(), DefaultSource::Fallback),
        };
        let mut known = configured;
        push_unique(&mut known, default.clone());
        push_unique(&mut known, GITHUB.to_string());
        Self {
            known,
            default,
            default_source,
        }
    }
}

/// The host part of a configured host, as gh compares hosts: no port, lowercase.
pub fn gh_hostname(host: &str) -> String {
    let host = normalize_host(host);
    url_hostname(&host).to_string()
}

/// A checkout's remotes, read from git at most once, and only if a resolution needs them.
pub struct Checkout<'a> {
    cwd: Option<&'a Path>,
    remotes: OnceCell<Vec<Remote>>,
}

impl<'a> Checkout<'a> {
    pub fn new(cwd: Option<&'a Path>) -> Self {
        Self {
            cwd,
            remotes: OnceCell::new(),
        }
    }

    fn remotes(&self) -> &[Remote] {
        self.remotes.get_or_init(|| git_remotes(self.cwd))
    }
}

pub fn detect_repo(
    invocation: &Invocation,
    env: &HashMap<String, String>,
    checkout: &Checkout<'_>,
    hosts: &GhHosts,
) -> Detection {
    if let Some(repo_arg) = invocation.repo_override.as_deref() {
        return override_repo(repo_arg, hosts).map_or_else(Detection::unknown, |repo| {
            Detection::found(repo, "-R/--repo")
        });
    }

    if let Some(url_arg) = invocation
        .positionals
        .iter()
        .map(String::as_str)
        .find(|arg| {
            matches!(url_scheme(arg), Some("http" | "https" | "ssh" | "git"))
                || invocation.command == "repo view"
        })
    {
        if let Some(repo) = parse_repo_spec(url_arg, Some(&hosts.default)) {
            return Detection::found(repo, "command URL");
        }
    }

    if let Some(gh_repo) = env.get("GH_REPO").filter(|value| !value.trim().is_empty()) {
        return override_repo(gh_repo, hosts)
            .map_or_else(Detection::unknown, |repo| Detection::found(repo, "GH_REPO"));
    }

    let saved_default = honors_saved_default(&invocation.command);
    base_repo_from_remotes(checkout.remotes(), hosts, saved_default)
        .map_or_else(Detection::unknown, |(repo, source)| {
            Detection::found(repo, source)
        })
}

/// gh reads `gh repo set-default` only in the commands it builds with `SmartBaseRepoFunc`
/// (`gh/pkg/cmd/root/root.go:161-178`). The groups added before that line take the first ranked
/// remote (`gh/pkg/cmd/factory/default.go:83-90`). A command the wrapper doesn't know, such as an
/// alias, is taken to be a smart one: most aliases expand to `pr` or `issue`.
fn honors_saved_default(command: &str) -> bool {
    !matches!(
        command.split_whitespace().next().unwrap_or_default(),
        "version"
            | "accessibility"
            | "actions"
            | "alias"
            | "auth"
            | "attestation"
            | "config"
            | "gist"
            | "gpg-key"
            | "completion"
            | "extension"
            | "search"
            | "secret"
            | "variable"
            | "ssh-key"
            | "codespace"
            | "project"
            | "preview"
            | "skills"
            | "copilot"
            | "status"
            | "credits"
            | "licenses"
            | "send-telemetry"
    )
}

/// The first remote that parses, `origin` first. `doctor` and `bootstrap` report it; routing uses
/// [`detect_repo`], which follows gh's own choice.
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

/// `-R`, `GH_REPO` and saved defaults: gh's `[HOST/]OWNER/REPO` or a URL
/// (`go-gh/pkg/repository/repository.go` `ParseWithHost`).
fn override_repo(spec: &str, hosts: &GhHosts) -> Option<RepoRef> {
    if spec.starts_with("git@") || is_supported_protocol(spec) {
        // go-gh's `RepoInfoFromURL` drops `www.` after lowercasing (`go-gh/internal/git/url.go:78`),
        // then `ghrepo.NewWithHost` drops it again (`gh_repo`), so `WWW.` goes too.
        let url = GitUrl::parse(spec)?;
        let host = url.host.to_ascii_lowercase();
        return url.repo(host.strip_prefix("www.").unwrap_or(&host));
    }
    let parts = spec.splitn(4, '/').collect::<Vec<_>>();
    if parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    match parts.as_slice() {
        [host, owner, name] => Some(gh_repo(host, owner, name)),
        [owner, name] => Some(gh_repo(&hosts.default, owner, name)),
        _ => None,
    }
}

/// gh's choice among the checkout's remotes when no override is given
/// (`gh/pkg/cmd/factory/remote_resolver.go`, `gh/context/context.go` `BaseRepo`). With several
/// candidates and a terminal, gh would also ask the API about the repository network; the
/// wrapper takes the first candidate, which is what gh does without a terminal.
fn base_repo_from_remotes(
    remotes: &[Remote],
    hosts: &GhHosts,
    saved_default: bool,
) -> Option<(RepoRef, &'static str)> {
    let mut remotes = remotes
        .iter()
        .filter(|remote| hosts.known.contains(&remote.repo.host))
        .cloned()
        .collect::<Vec<_>>();
    remotes.sort_by_key(|remote| std::cmp::Reverse(remote_rank(&remote.name)));
    if hosts.default_source != DefaultSource::Fallback {
        let on_default = remotes
            .iter()
            .filter(|remote| remote.repo.host == hosts.default)
            .cloned()
            .collect::<Vec<_>>();
        if hosts.default_source == DefaultSource::Env || !on_default.is_empty() {
            remotes = on_default;
        }
    }

    for remote in remotes.iter().filter(|_| saved_default) {
        match remote.resolved.as_deref() {
            Some("base") => return Some((remote.repo.clone(), "repo set-default")),
            Some(value) if !value.is_empty() => {
                let saved = override_repo(value, hosts)?;
                return Some((
                    RepoRef::new(remote.repo.host.as_str(), saved.owner, saved.name),
                    "repo set-default",
                ));
            }
            _ => {}
        }
    }
    remotes
        .into_iter()
        .next()
        .map(|remote| (remote.repo, "git remote"))
}

#[derive(Debug, Clone)]
struct Remote {
    name: String,
    repo: RepoRef,
    resolved: Option<String>,
}

/// `git remote -v` in git's order, each remote's repository taken from its fetch URL, or its
/// push URL when the fetch URL names none (`gh/context/remote.go` `TranslateRemotes`).
fn git_remotes(cwd: Option<&Path>) -> Vec<Remote> {
    let Some(listing) = git_output(&["remote", "-v"], cwd) else {
        return Vec::new();
    };
    let mut urls = Vec::<(String, Option<String>, Option<String>)>::new();
    for line in listing.lines() {
        let Some((rest, kind)) = line.trim_end().rsplit_once(' ') else {
            continue;
        };
        let Some((name, url)) = rest.trim_end().rsplit_once(char::is_whitespace) else {
            continue;
        };
        let name = name.trim();
        if urls.last().is_none_or(|(last, _, _)| last != name) {
            urls.push((name.to_string(), None, None));
        }
        let Some(entry) = urls.last_mut() else {
            continue;
        };
        match kind {
            "(fetch)" => entry.1 = Some(url.to_string()),
            "(push)" => entry.2 = Some(url.to_string()),
            _ => {}
        }
    }

    let resolved = git_output(
        &["config", "--get-regexp", r"^remote\..*\.gh-resolved$"],
        cwd,
    )
    .unwrap_or_default();
    // gh's `populateResolvedRemotes` (`gh/git/client.go:1050-1068`): the name is the text between
    // the first two dots, so a remote with a dot in its name never matches, and a later line for
    // the same remote replaces an earlier one.
    let mut saved = HashMap::<&str, &str>::new();
    for line in resolved.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        if let Some(name) = key.split('.').nth(1) {
            saved.insert(name, value);
        }
    }

    let mut ssh = SshHosts::default();
    urls.into_iter()
        .filter_map(|(name, fetch, push)| {
            let repo = [fetch, push]
                .into_iter()
                .flatten()
                .find_map(|url| remote_repo(&url, &mut ssh))?;
            let resolved = saved.get(name.as_str()).map(|value| (*value).to_string());
            Some(Remote {
                name,
                repo,
                resolved,
            })
        })
        .collect()
}

fn remote_rank(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "upstream" => 3,
        "github" => 2,
        "origin" => 1,
        _ => 0,
    }
}

fn remote_repo(raw: &str, ssh: &mut SshHosts) -> Option<RepoRef> {
    let url = GitUrl::parse(raw)?;
    let host = if url.scheme == "ssh" {
        ssh.resolve(&url.host)
    } else {
        url.host.clone()
    };
    url.repo(&host)
}

/// A remote URL as gh's `git.ParseURL` reads it: scp-style `host:path` becomes `ssh://host/path`.
struct GitUrl {
    scheme: String,
    host: String,
    path: String,
}

impl GitUrl {
    fn parse(raw: &str) -> Option<Self> {
        let raw = if !is_possible_protocol(raw) && raw.contains(':') && !raw.contains('\\') {
            format!("ssh://{}", raw.replacen(':', "/", 1))
        } else {
            raw.to_string()
        };
        let (scheme, rest) = raw.split_once("://")?;
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "git+https" => "https".to_string(),
            "git+ssh" => "ssh".to_string(),
            other => other.to_string(),
        };
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = url_hostname(
            authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host),
        );
        if host.is_empty() {
            return None;
        }
        Some(Self {
            scheme,
            host: host.to_string(),
            path: path.to_string(),
        })
    }

    /// gh's `ghrepo.FromURL`: exactly `OWNER/REPO` after the host.
    fn repo(&self, host: &str) -> Option<RepoRef> {
        match self
            .path
            .trim_matches('/')
            .splitn(3, '/')
            .collect::<Vec<_>>()
            .as_slice()
        {
            [owner, name] => Some(gh_repo(host, owner, name)),
            _ => None,
        }
    }
}

/// gh's `ghrepo.NewWithHost`: a literal leading `www.` is dropped, then the host is lowercased.
fn gh_repo(host: &str, owner: &str, name: &str) -> RepoRef {
    let host = host
        .strip_prefix("www.")
        .unwrap_or(host)
        .to_ascii_lowercase();
    RepoRef {
        host,
        owner: owner.to_string(),
        name: strip_git_suffix(name),
    }
}

/// `Hostname()` of a URL authority: the port, and the brackets around an IPv6 address, removed.
fn url_hostname(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    // More than one colon and no brackets: a bare IPv6 address, which carries no port.
    if authority.matches(':').count() > 1 {
        return authority;
    }
    authority.split(':').next().unwrap_or(authority)
}

fn is_supported_protocol(value: &str) -> bool {
    ["ssh:", "git+ssh:", "git:", "http:", "git+https:", "https:"]
        .iter()
        .any(|prefix| value.starts_with(prefix))
}

fn is_possible_protocol(value: &str) -> bool {
    is_supported_protocol(value)
        || ["ftp:", "ftps:", "file:"]
            .iter()
            .any(|prefix| value.starts_with(prefix))
}

/// SSH host aliases resolved with `ssh -G`, as go-gh's `ssh.Translator` does. A failure leaves
/// the host as it was.
#[derive(Default)]
struct SshHosts {
    resolved: HashMap<String, String>,
}

impl SshHosts {
    fn resolve(&mut self, host: &str) -> String {
        if let Some(resolved) = self.resolved.get(&host.to_ascii_lowercase()) {
            return resolved.clone();
        }
        let resolved = Command::new("ssh")
            .args(["-G", "--", host])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter_map(|line| line.strip_prefix("hostname "))
                    .next_back()
                    .map(str::to_string)
            })
            .filter(|resolved| !resolved.is_empty())
            .unwrap_or_else(|| host.to_string());
        let resolved = if resolved.eq_ignore_ascii_case("ssh.github.com") {
            GITHUB.to_string()
        } else {
            resolved
        };
        self.resolved
            .insert(host.to_ascii_lowercase(), resolved.clone());
        resolved
    }
}

/// Top-level hosts in the user's gh `hosts.yml`, found the way go-gh's `ConfigDir` does. The file
/// is only read; a missing or unreadable one lists no hosts.
fn user_gh_hosts(env: &HashMap<String, String>) -> Vec<String> {
    let non_empty = |name: &str| env.get(name).filter(|value| !value.is_empty());
    let dir = if let Some(dir) = non_empty("GH_CONFIG_DIR") {
        PathBuf::from(dir)
    } else if let Some(dir) = non_empty("XDG_CONFIG_HOME") {
        Path::new(dir).join("gh")
    } else if let Some(home) = non_empty("HOME") {
        Path::new(home).join(".config").join("gh")
    } else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(dir.join("hosts.yml")) else {
        return Vec::new();
    };
    let Ok(serde_yaml::Value::Mapping(hosts)) = serde_yaml::from_str(&text) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for host in hosts.keys().filter_map(serde_yaml::Value::as_str) {
        push_unique(&mut result, host.to_ascii_lowercase());
    }
    result
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
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

    fn hosts(forgejo: &[&str]) -> GhHosts {
        GhHosts::load(
            &HashMap::new(),
            &forgejo
                .iter()
                .map(|host| (*host).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn override_takes_gh_repo_forms() {
        let hosts = hosts(&["git.example.com:3443"]);
        assert_eq!(
            repo_tuple(override_repo("owner/repo", &hosts)),
            Some(("git.example.com".into(), "owner".into(), "repo".into()))
        );
        assert_eq!(
            repo_tuple(override_repo(
                "git+ssh://git@WWW.example.com:22/o/r.git",
                &hosts
            )),
            Some(("example.com".into(), "o".into(), "r".into()))
        );
        assert_eq!(
            repo_tuple(override_repo("git@www.example.com:o/r.git", &hosts)),
            Some(("example.com".into(), "o".into(), "r".into()))
        );
    }

    #[test]
    fn override_rejects_what_gh_rejects() {
        let hosts = hosts(&["git.example.com"]);
        for spec in [
            "a//b",
            "a/b/c/d",
            "repo",
            "https://git.example.com/a/b/pulls/7",
        ] {
            assert_eq!(override_repo(spec, &hosts), None, "{spec}");
        }
    }

    #[test]
    fn two_known_hosts_default_to_github() {
        assert_eq!(hosts(&["a.example", "b.example"]).default, GITHUB);
        assert_eq!(hosts(&["a.example"]).default, "a.example");
    }

    #[test]
    fn ipv6_hostnames_lose_brackets_and_port() {
        assert_eq!(url_hostname("[::1]:3000"), "::1");
        assert_eq!(url_hostname("forge.test:3443"), "forge.test");
        assert_eq!(url_hostname("::1"), "::1");
        assert_eq!(gh_hostname("[::1]:3000"), "::1");
    }
}
