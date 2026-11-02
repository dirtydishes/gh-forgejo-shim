//! Repository parsing and host detection for the native dispatcher.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod go_sort;

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
    /// gh compares hosts with `strings.EqualFold` (`gh/context/remote.go` `FilterByHosts`).
    fn knows(&self, host: &str) -> bool {
        self.known
            .iter()
            .any(|known| known.eq_ignore_ascii_case(host))
    }

    /// What plain gh knows: the user's own `hosts.yml` alone.
    pub fn user(env: &HashMap<String, String>) -> Self {
        Self::load(env, &[])
    }

    /// `forgejo_hosts` are the shim's allowlist entries. gh keeps only the hostname of a remote
    /// URL, never its port, so the entries count without theirs.
    pub fn load(env: &HashMap<String, String>, forgejo_hosts: &[String]) -> Self {
        let mut configured = user_gh_hosts(env);
        // A launched gh's hosts.yml keys an entry with a port twice: by its hostname, which gh's
        // API client looks up, and as `host:port`, which gh's git credential helper looks up.
        for host in forgejo_hosts {
            push_unique(&mut configured, gh_hostname(host));
            let entry = normalize_host(host);
            if entry != gh_hostname(&entry) {
                push_unique(&mut configured, entry);
            }
        }
        let (default, default_source) = match (
            env.get("GH_HOST").filter(|host| !host.is_empty()),
            configured.as_slice(),
        ) {
            (Some(host), _) => (host.clone(), DefaultSource::Env),
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
    // Routing expands a plain alias before parsing (`expand_alias`), so this is gh's command.
    let group = invocation
        .command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    // A `pr` or `issue` command takes a PR or issue URL as its selector, and gh uses that URL's
    // repository even with -R. `repo view` takes the repository itself. Any other command's URL
    // argument is data, such as a release tag, and selects nothing.
    // Only the first argument is the selector: `gh issue transfer 123 <URL>` names a destination.
    let selector = invocation
        .positionals
        .first()
        .map(String::as_str)
        .filter(|arg| {
            (matches!(group.as_str(), "pr" | "issue")
                && matches!(url_scheme(arg), Some("http" | "https")))
                || invocation.command == "repo view"
        });
    if let Some(selector) = selector {
        if let Some(repo) = parse_repo_spec(selector, Some(&hosts.default)) {
            return Detection::found(repo, "command URL");
        }
    }

    // gh's `OverrideBaseRepoFunc`: an empty -R counts as none, and GH_REPO is checked next.
    if let Some(repo_arg) = invocation
        .repo_override
        .as_deref()
        .filter(|arg| !arg.is_empty())
    {
        return override_repo(repo_arg, hosts).map_or_else(Detection::unknown, |repo| {
            Detection::found(repo, "-R/--repo")
        });
    }

    if let Some(gh_repo) = env.get("GH_REPO").filter(|value| !value.is_empty()) {
        return override_repo(gh_repo, hosts)
            .map_or_else(Detection::unknown, |repo| Detection::found(repo, "GH_REPO"));
    }

    let saved_default = honors_saved_default(&group);
    base_repo_from_remotes(checkout.remotes(), hosts, saved_default)
        .map_or_else(Detection::unknown, |(repo, source)| {
            Detection::found(repo, source)
        })
}

/// gh's top-level commands in v2.103.3, with cobra's short names and the help topics: the names
/// an alias can't take (`gh/pkg/cmd/root/root.go:134-185`).
const GH_COMMANDS: &[&str] = &[
    "version",
    "accessibility",
    "a11y",
    "actions",
    "alias",
    "auth",
    "attestation",
    "at",
    "config",
    "gist",
    "gpg-key",
    "completion",
    "extension",
    "extensions",
    "ext",
    "search",
    "secret",
    "variable",
    "ssh-key",
    "codespace",
    "cs",
    "project",
    "preview",
    "skills",
    "copilot",
    "status",
    "credits",
    "licenses",
    "send-telemetry",
    "agent-task",
    "agent-tasks",
    "agent",
    "agents",
    "browse",
    "discussion",
    "pr",
    "org",
    "issue",
    "release",
    "repo",
    "ruleset",
    "rs",
    "run",
    "workflow",
    "label",
    "cache",
    "api",
    "help",
    "mintty",
    "environment",
    "telemetry",
    "reference",
    "formatting",
    "exit-codes",
];

/// `text` split as google/shlex's `Split` splits it, or `None` where shlex fails, which makes gh
/// reject the alias. Quotes are removed. A backslash escapes the next character outside single
/// quotes. A `#` at the start of a token begins a comment that runs to the end of the line. Only
/// space, tab, CR and LF separate words.
fn shell_split(text: &str) -> Option<Vec<String>> {
    const SPACE: &[char] = &[' ', '\t', '\r', '\n'];
    let mut words = Vec::<String>::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| SPACE.contains(c)).is_some() {}
        match chars.peek() {
            None => break,
            Some('#') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            Some(_) => {}
        }
        let mut word = String::new();
        let mut quote = None;
        while let Some(c) = chars.next() {
            match (quote, c) {
                (None, c) if SPACE.contains(&c) => break,
                (None, '\\') | (Some('"'), '\\') => word.push(chars.next()?),
                (None, '\'' | '"') => quote = Some(c),
                (Some(open), c) if c == open => quote = None,
                (_, c) => word.push(c),
            }
        }
        if quote.is_some() {
            return None;
        }
        words.push(word);
    }
    Some(words)
}

/// The command word cobra's `Find` reaches in `words` (`stripFlags` in spf13/cobra 1.10). Every
/// `-x` or `--name` flag without `=` is taken to swallow the next word, except gh's root flags
/// that take no value (`--help`, `-h`, `--version`).
fn command_word(words: &[String]) -> Option<&str> {
    let mut words = words.iter().map(String::as_str);
    while let Some(word) = words.next() {
        match word {
            "--" => return None,
            "--help" | "-h" | "--version" | "" => {}
            flag if flag.starts_with('-')
                && !flag.contains('=')
                && (flag.starts_with("--") || flag.len() == 2) =>
            {
                words.next()?;
            }
            flag if flag.starts_with('-') => {}
            word => return Some(word),
        }
    }
    None
}

/// `argv` as gh runs it when its first word is a plain alias in the user's gh `config.yml`, or
/// `None`. gh registers an alias only when its name isn't a gh command and its expansion's
/// command word is one (`gh/pkg/cmd/alias/shared/validations.go`). It then expands it with
/// `expandAlias` (`gh/pkg/cmd/root/alias.go`): each argument replaces `$N` while the expansion
/// still holds a `$`, and otherwise is appended after the split expansion.
pub fn expand_alias(argv: &[String], env: &HashMap<String, String>) -> Option<Vec<String>> {
    let name = argv.first()?;
    if GH_COMMANDS.contains(&name.as_str()) {
        return None;
    }
    let aliases = user_gh_aliases(env);
    let expansion = aliases.get(name)?;
    if !command_word(&shell_split(expansion)?).is_some_and(|word| GH_COMMANDS.contains(&word)) {
        return None;
    }
    let mut expanded = expansion.clone();
    let mut extra = Vec::new();
    for (i, arg) in argv[1..].iter().enumerate() {
        if expanded.contains('$') {
            expanded = expanded.replace(&format!("${}", i + 1), arg);
        } else {
            extra.push(arg.clone());
        }
    }
    let lingering = expanded
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0] == b'$' && pair[1].is_ascii_digit());
    if lingering {
        return None;
    }
    let mut words = shell_split(&expanded)?;
    words.extend(extra);
    Some(words)
}

/// gh reads `gh repo set-default` only in the commands it builds with `SmartBaseRepoFunc`
/// (`gh/pkg/cmd/root/root.go:161-178`). The groups added before that line take the first ranked
/// remote (`gh/pkg/cmd/factory/default.go:83-90`). A command the wrapper still doesn't know, such
/// as a shell alias or an extension, is taken to be a smart one.
fn honors_saved_default(group: &str) -> bool {
    !matches!(
        group,
        "version"
            | "accessibility"
            | "a11y"
            | "actions"
            | "alias"
            | "auth"
            | "attestation"
            | "at"
            | "config"
            | "gist"
            | "gpg-key"
            | "completion"
            | "extension"
            | "extensions"
            | "ext"
            | "search"
            | "secret"
            | "variable"
            | "ssh-key"
            | "codespace"
            | "cs"
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
    // gh sorts every remote with Go's unstable `sort.Sort`, then filters by host
    // (`remote_resolver.go`). Equally ranked remotes beyond 12 come out in that sort's own order.
    let mut remotes = remotes.to_vec();
    go_sort::sort(&mut remotes, |a, b| {
        remote_rank(&a.name) > remote_rank(&b.name)
    });
    let mut remotes = remotes
        .into_iter()
        .filter(|remote| hosts.knows(&remote.repo.host))
        .collect::<Vec<_>>();
    if hosts.default_source != DefaultSource::Fallback {
        let on_default = remotes
            .iter()
            .filter(|remote| remote.repo.host.eq_ignore_ascii_case(&hosts.default))
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
                // `ghrepo.NewWithHost` normalizes the remote's host a second time.
                return Some((
                    gh_repo(&remote.repo.host, &saved.owner, &saved.name),
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
        // Go's url.Parse refuses control characters, a `%` not followed by two hex digits, and
        // userinfo outside its allowed characters (`net/url` `validUserinfo`); gh then drops the
        // remote.
        // The query isn't checked; the path, userinfo, host and fragment are.
        let (before_fragment, fragment) = raw.split_once('#').unwrap_or((&raw, ""));
        let before_query = before_fragment.split('?').next().unwrap_or(before_fragment);
        if before_fragment.bytes().any(|b| b < 0x20 || b == 0x7f)
            || !valid_escapes(before_query)
            || !valid_escapes(fragment)
        {
            return None;
        }
        let (scheme, rest) = raw.split_once("://")?;
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "git+https" => "https".to_string(),
            "git+ssh" => "ssh".to_string(),
            other => other.to_string(),
        };
        // Go's url.Parse keeps `?query` and `#fragment` out of the path.
        let rest = rest.split('#').next().unwrap_or(rest);
        let rest = rest.split('?').next().unwrap_or(rest);
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        if let Some((userinfo, _)) = authority.rsplit_once('@') {
            if !userinfo.chars().all(valid_userinfo_char) {
                return None;
            }
        }
        if !valid_optional_port(
            authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host),
        ) {
            return None;
        }
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
            // Only a URL's name loses `.git`; `-R o/r.git` keeps it (`ghrepo.FromURL`).
            [owner, name] => Some(gh_repo(host, owner, &strip_git_suffix(name))),
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
        name: name.to_string(),
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

fn valid_escapes(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().all(|(i, byte)| {
        *byte != b'%'
            || (bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
    })
}

/// Go's `validOptionalPort`, applied as `parseHost` does: after `]` for a bracketed host, else
/// after the last `:`, either nothing or `:` and digits.
fn valid_optional_port(host: &str) -> bool {
    let port = match host.strip_prefix('[') {
        // Go takes the last `]`, and its parser accepts only an IPv6 literal, zone allowed,
        // inside the brackets.
        Some(rest) => match rest.rsplit_once(']') {
            Some((literal, after))
                if literal
                    .split('%')
                    .next()
                    .is_some_and(|ip| ip.parse::<std::net::Ipv6Addr>().is_ok()) =>
            {
                after
            }
            _ => return false,
        },
        None => host.rfind(':').map_or("", |i| &host[i..]),
    };
    port.strip_prefix(':').map_or(port.is_empty(), |digits| {
        digits.bytes().all(|b| b.is_ascii_digit())
    })
}

fn valid_userinfo_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '-' | '.'
                | '_'
                | ':'
                | '~'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%'
                | '@'
        )
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

/// The user's gh config directory, found the way go-gh's `ConfigDir` does.
fn user_gh_config_dir(env: &HashMap<String, String>) -> Option<PathBuf> {
    let non_empty = |name: &str| env.get(name).filter(|value| !value.is_empty());
    if let Some(dir) = non_empty("GH_CONFIG_DIR") {
        Some(PathBuf::from(dir))
    } else if let Some(dir) = non_empty("XDG_CONFIG_HOME") {
        Some(Path::new(dir).join("gh"))
    } else {
        non_empty("HOME").map(|home| Path::new(home).join(".config").join("gh"))
    }
}

/// Top-level hosts in the user's gh `hosts.yml`. The file is only read; a missing or unreadable
/// one lists no hosts.
fn user_gh_hosts(env: &HashMap<String, String>) -> Vec<String> {
    let Some(dir) = user_gh_config_dir(env) else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(dir.join("hosts.yml")) else {
        return Vec::new();
    };
    let Ok(serde_yaml::Value::Mapping(hosts)) = serde_yaml::from_str(&text) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    // Kept as written: gh counts the keys as they are, and `Forge.Test` and `forge.test` are two.
    for host in hosts.keys().filter_map(serde_yaml::Value::as_str) {
        push_unique(&mut result, host.to_string());
    }
    result
}

/// Plain (not `!` shell) aliases in the user's gh `config.yml`: name to expansion.
fn user_gh_aliases(env: &HashMap<String, String>) -> HashMap<String, String> {
    let Some(dir) = user_gh_config_dir(env) else {
        return HashMap::new();
    };
    let Ok(text) = fs::read_to_string(dir.join("config.yml")) else {
        return HashMap::new();
    };
    let Ok(config) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return HashMap::new();
    };
    let Some(aliases) = config
        .get("aliases")
        .and_then(serde_yaml::Value::as_mapping)
    else {
        return HashMap::new();
    };
    aliases
        .iter()
        .filter_map(|(name, expansion)| Some((name.as_str()?, expansion.as_str()?)))
        .filter(|(_, expansion)| !expansion.starts_with('!'))
        .map(|(name, expansion)| (name.to_string(), expansion.to_string()))
        .collect()
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
        let hosts = hosts(&["git.example.com"]);
        assert_eq!(
            repo_tuple(override_repo("owner/repo", &hosts)),
            Some(("git.example.com".into(), "owner".into(), "repo".into()))
        );
        // An entry with a port is two keys to gh, so the default host is github.com.
        assert_eq!(
            repo_tuple(override_repo(
                "owner/repo",
                &super::GhHosts::load(&HashMap::new(), &["git.example.com:3443".to_string()])
            )),
            Some((GITHUB.into(), "owner".into(), "repo".into()))
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
