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
    /// The host `ssh -G` gives for the remote `repo` came from, when [`detect_repo_by_own_host`]
    /// read that remote under its own host instead. gh drops such a remote, so it can't find
    /// `repo` itself.
    pub ssh_hostname: Option<String>,
}

impl Detection {
    fn found(repo: RepoRef, source: &str) -> Self {
        Self {
            repo: Some(repo),
            source: source.to_string(),
            ssh_hostname: None,
        }
    }

    fn unknown() -> Self {
        Self {
            repo: None,
            source: "unknown".to_string(),
            ssh_hostname: None,
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

    /// The default host when it comes from `hosts.yml`, its only host with no `GH_HOST`, as gh
    /// takes it (`gh/internal/config/config.go:464-469` calls go-gh v2.17.0's
    /// `pkg/auth/auth.go:146-157` `defaultHost`).
    pub fn configured_default(&self) -> Option<&str> {
        (self.default_source == DefaultSource::Config).then_some(self.default.as_str())
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

    /// Every remote's host, whichever one a command would pick, and an SSH remote's own host
    /// where `ssh -G` gives another.
    pub fn remote_hosts(&self) -> impl Iterator<Item = &str> {
        self.remotes().iter().flat_map(|remote| {
            std::iter::once(remote.repo.host.as_str())
                .chain(remote.own.as_ref().map(|own| own.host.as_str()))
        })
    }

    /// The remote to fetch a repository from, as gh's `FindByRepo` picks it (`gh/context/remote.go`):
    /// the first in gh's remote order whose owner and name match without case. gh has already kept
    /// only remotes on known hosts; this matches the host too. Unlike gh, it reads only the URL
    /// git fetches from, never a push URL, so a remote that fetches from a local mirror and pushes
    /// to the forge isn't chosen. That alone doesn't keep a fetch off a mirror: an `insteadOf`
    /// rule can send any URL there, so `pr checkout` also compares what it fetched with the PR's
    /// head.
    pub fn remote_for(&self, host: &str, owner: &str, name: &str) -> Option<String> {
        let mut remotes = self.remotes().iter().collect::<Vec<_>>();
        go_sort::sort(&mut remotes, |a, b| {
            remote_rank(&a.name) > remote_rank(&b.name)
        });
        let host = gh_hostname(host);
        remotes
            .into_iter()
            .find(|remote| {
                remote.fetch_repo.as_ref().is_some_and(|repo| {
                    repo.host.eq_ignore_ascii_case(&host)
                        && repo.owner.eq_ignore_ascii_case(owner)
                        && repo.name.eq_ignore_ascii_case(name)
                })
            })
            .map(|remote| remote.name.clone())
    }
}

pub fn detect_repo(
    invocation: &Invocation,
    env: &HashMap<String, String>,
    checkout: &Checkout<'_>,
    hosts: &GhHosts,
) -> Detection {
    detect_repo_by_own_host(invocation, env, checkout, hosts, &|_| false)
}

/// As [`detect_repo`], but an SSH remote whose host `ssh -G` turns into one `hosts` doesn't know,
/// which gh drops, counts under its own host when `own` accepts that host: a forge that SSH
/// reaches through a `HostName` such as a VPN address (#334). gh can't find a repository chosen
/// so; the detection's `ssh_hostname` says when, and the caller must hand gh the repository.
pub fn detect_repo_by_own_host(
    invocation: &Invocation,
    env: &HashMap<String, String>,
    checkout: &Checkout<'_>,
    hosts: &GhHosts,
    own: &dyn Fn(&str) -> bool,
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
    let remotes = checkout
        .remotes()
        .iter()
        .map(|remote| match &remote.own {
            Some(repo) if !hosts.knows(&remote.repo.host) && own(&repo.host) => Remote {
                repo: repo.clone(),
                ssh_hostname: Some(remote.repo.host.clone()),
                ..remote.clone()
            },
            _ => remote.clone(),
        })
        .collect::<Vec<_>>();
    base_repo_from_remotes(&remotes, hosts, saved_default).map_or_else(
        Detection::unknown,
        |(repo, source, ssh_hostname)| Detection {
            ssh_hostname,
            ..Detection::found(repo, source)
        },
    )
}

/// gh's top-level commands in v2.103.3, with cobra's short names and the help topics: the names
/// an alias can't take (`gh/pkg/cmd/root/root.go:134-185`), except `help`. cobra adds its help
/// command only when it runs, after gh has registered the aliases, so an alias named `help` wins.
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
    "skill",
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
pub(crate) fn shell_split(text: &str) -> Option<Vec<String>> {
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

/// The position of the command word cobra's `Find` reaches in `words`, scanning as spf13/cobra
/// 1.10's `stripFlags` does on gh's root command. A `-x` or a `--name` without `=` takes the next
/// word as its value, except `--help` and `--version`, gh's root flags that take none. gh's root
/// has no short flags, so `-h` takes a value too. A value-taking flag with at most one word after
/// it ends the search, as does `--`.
fn command_position(words: &[String]) -> Option<usize> {
    let mut pos = 0;
    while pos < words.len() {
        let word = words[pos].as_str();
        let takes_value = word.starts_with('-')
            && !word.contains('=')
            && match word.strip_prefix("--") {
                Some(name) => !name.is_empty() && !matches!(name, "help" | "version"),
                None => word.len() == 2,
            };
        if word == "--" {
            return None;
        }
        if takes_value {
            if words.len() - pos - 1 <= 1 {
                return None;
            }
            pos += 2;
            continue;
        }
        if !word.is_empty() && !word.starts_with('-') {
            return Some(pos);
        }
        pos += 1;
    }
    None
}

/// The command word cobra's `Find` reaches in `words`, if any.
pub fn command_word(words: &[String]) -> Option<&str> {
    Some(words.get(command_position(words)?)?.as_str())
}

/// The command word cobra's `Find` reaches in `words`, when it's one of gh's own commands rather
/// than an alias or an extension. Not `help`, which an alias can replace.
pub fn gh_command_word(words: &[String]) -> Option<&str> {
    command_word(words).filter(|word| *word != "help" && GH_COMMANDS.contains(word))
}

/// `argv` as gh runs it when its command word, found past any flags before it, is a plain alias
/// in the user's gh `config.yml`, or `None`. gh registers an alias only when its name isn't a gh
/// command and its expansion's command word is one (`gh/pkg/cmd/alias/shared/validations.go`).
/// cobra hands the alias every other argument, flags before its name included
/// (`argsMinusFirstX`). gh then expands it with `expandAlias` (`gh/pkg/cmd/root/alias.go`): each
/// argument replaces `$N` while the expansion still holds a `$`, and otherwise is appended after
/// the split expansion.
pub fn expand_alias(argv: &[String], env: &HashMap<String, String>) -> Option<Vec<String>> {
    let position = command_position(argv)?;
    let name = &argv[position];
    if name != "help" && GH_COMMANDS.contains(&name.as_str()) {
        return None;
    }
    let aliases = user_gh_aliases(env);
    let expansion = aliases.get(name)?;
    let words = shell_split(expansion)?;
    if !command_position(&words).is_some_and(|i| GH_COMMANDS.contains(&words[i].as_str())) {
        return None;
    }
    let args = argv[..position].iter().chain(&argv[position + 1..]);
    let mut expanded = expansion.clone();
    let mut extra = Vec::new();
    for (i, arg) in args.enumerate() {
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
            | "skill"
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
/// wrapper takes the first candidate, which is what gh does without a terminal. Also the chosen
/// remote's `ssh_hostname`.
fn base_repo_from_remotes(
    remotes: &[Remote],
    hosts: &GhHosts,
    saved_default: bool,
) -> Option<(RepoRef, &'static str, Option<String>)> {
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
            Some("base") => {
                return Some((
                    remote.repo.clone(),
                    "repo set-default",
                    remote.ssh_hostname.clone(),
                ))
            }
            Some(value) if !value.is_empty() => {
                let saved = override_repo(value, hosts)?;
                // `ghrepo.NewWithHost` normalizes the remote's host a second time.
                return Some((
                    gh_repo(&remote.repo.host, &saved.owner, &saved.name),
                    "repo set-default",
                    remote.ssh_hostname.clone(),
                ));
            }
            _ => {}
        }
    }
    remotes
        .into_iter()
        .next()
        .map(|remote| (remote.repo, "git remote", remote.ssh_hostname))
}

#[derive(Debug, Clone)]
struct Remote {
    name: String,
    repo: RepoRef,
    /// The repository the URL git fetches from names, if any.
    fetch_repo: Option<RepoRef>,
    resolved: Option<String>,
    /// `repo` under its URL's own host, when that's an SSH URL `ssh -G` gives another host for.
    own: Option<RepoRef>,
    /// Set when `repo` is `own` ([`detect_repo_by_own_host`]): the host `ssh -G` gives.
    ssh_hostname: Option<String>,
}

/// `git remote -v` in git's order, each remote's repository taken from its fetch URL, or its
/// push URL when the fetch URL names none (`gh/context/remote.go` `TranslateRemotes`). The fetch
/// line is the first `remote.<name>.url`, after `insteadOf`, the one URL `git fetch` uses; later
/// `url` values are listed as push URLs.
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
            let fetched = fetch.and_then(|url| remote_repo(&url, &mut ssh));
            let fetch_repo = fetched.as_ref().map(|(repo, _)| repo.clone());
            let (repo, own) =
                fetched.or_else(|| push.and_then(|url| remote_repo(&url, &mut ssh)))?;
            let resolved = saved.get(name.as_str()).map(|value| (*value).to_string());
            Some(Remote {
                name,
                repo,
                fetch_repo,
                resolved,
                own,
                ssh_hostname: None,
            })
        })
        .collect()
}

/// Where git would push instead of `forge`, for a command that pushes from `cwd` (`gh pr create`
/// pushes the branch's remote, `gh repo create --push` the remote it adds): a remote fetched from
/// the forge whose push URL names another host (`remote.<name>.pushurl`, and the rewrites
/// `git remote -v` applies), or an `insteadOf` or `pushInsteadOf` rule that could rewrite a URL
/// on the forge to another host. A rule counts when its prefix and one of the forge's URL forms
/// start the same way, whatever owner or name follows (`https://forge/acme/`). A push URL or rule
/// target gh can't read counts as elsewhere. An SSH push URL that `ssh -G` can't resolve counts
/// as elsewhere when git connects through its own SSH command (`ssh_command`, such as
/// `GIT_SSH_COMMAND`), whose configuration ssh on PATH wouldn't see; without either, git can't
/// push over SSH at all. `None` when every push to the forge stays there.
pub(crate) fn push_leaves(
    forge: &str,
    cwd: Option<&Path>,
    ssh_command: Option<&str>,
) -> Option<String> {
    let hostname = gh_hostname(forge).to_ascii_lowercase();
    let mut ssh = SshHosts::default();
    let mut host_of = |raw: &str, push: bool| {
        let url = GitUrl::parse(raw)?;
        // SSH resolves the push's own `user@host`, which a `Match user` rule can send elsewhere.
        let host = if url.scheme == "ssh" {
            let target = ssh_user_host(raw).unwrap_or(url.host);
            match (ssh.try_resolve(&target), ssh_command) {
                (Some(resolved), _) => resolved,
                (None, Some(command)) if push => {
                    return Some(format!(
                        "an SSH host it can't resolve (git connects through {command}, and ssh \
                         isn't on PATH)"
                    ));
                }
                (None, _) => unresolved_host(&target),
            }
        } else {
            url.host
        };
        Some(gh_hostname(&host).to_ascii_lowercase())
    };
    let elsewhere = |host: Option<String>| match host {
        Some(host) => host,
        None => "a location gh-forgejo-shim can't read".to_string(),
    };
    let listing = git_output(&["remote", "-v"], cwd).unwrap_or_default();
    let mut fetch = HashMap::<String, String>::new();
    let mut push = Vec::<(String, String)>::new();
    for line in listing.lines() {
        let Some((rest, kind)) = line.trim_end().rsplit_once(' ') else {
            continue;
        };
        let Some((name, url)) = rest.trim_end().rsplit_once(char::is_whitespace) else {
            continue;
        };
        match kind {
            "(fetch)" => {
                fetch.insert(name.trim().to_string(), url.to_string());
            }
            "(push)" => push.push((name.trim().to_string(), url.to_string())),
            _ => {}
        }
    }
    // A remote counts as the forge's when git fetches from it there, or when its URL names the
    // forge and `ssh -G` sends it elsewhere: routing reads such a remote under its own host
    // (`detect_repo_by_own_host`), but git pushes where SSH resolves it.
    let names_forge = |raw: &str| {
        GitUrl::parse(raw).is_some_and(|url| gh_hostname(&url.host).eq_ignore_ascii_case(&hostname))
    };
    for (name, url) in push {
        let on_forge = fetch.get(&name).is_some_and(|fetch| {
            names_forge(fetch) || host_of(fetch, false).as_deref() == Some(hostname.as_str())
        });
        let host = host_of(&url, true);
        if on_forge && host.as_deref() != Some(hostname.as_str()) {
            return Some(format!("remote {name} pushes to {}", elsewhere(host)));
        }
    }
    let forms = [
        format!("https://{}/", normalize_host(forge)),
        format!("https://{hostname}/"),
        format!("http://{hostname}/"),
        format!("ssh://git@{hostname}/"),
        format!("ssh://{hostname}/"),
        format!("git@{hostname}:"),
        format!("{hostname}:"),
    ];
    let rules = git_output(
        &["config", "--get-regexp", r"^url\..*\.(push)?insteadof$"],
        cwd,
    )
    .unwrap_or_default();
    for line in rules.lines() {
        let Some((name, prefix)) = line.split_once(' ') else {
            continue;
        };
        let Some(base) = name.strip_prefix("url.").and_then(|name| {
            name.strip_suffix(".pushinsteadof")
                .or_else(|| name.strip_suffix(".insteadof"))
        }) else {
            continue;
        };
        let prefix = prefix.to_ascii_lowercase();
        let overlaps = forms
            .iter()
            .any(|form| form.starts_with(&prefix) || prefix.starts_with(form.as_str()));
        let host = host_of(base, true);
        if overlaps && host.as_deref() != Some(hostname.as_str()) {
            return Some(format!(
                "git rewrites URLs on {hostname} to {}",
                elsewhere(host)
            ));
        }
    }
    None
}

/// An SSH URL's `user@host`, for `ssh -G`: the authority of `ssh://user@host[:port]/path`, or
/// what precedes the `:` of scp-style `user@host:path`. `None` without a user.
fn ssh_user_host(raw: &str) -> Option<String> {
    let authority = match raw.split_once("://") {
        Some((_, rest)) => rest.split('/').next()?,
        None => raw.split_once(':')?.0,
    };
    let (user, host) = authority.rsplit_once('@')?;
    let host = host.rsplit_once(':').map_or(host, |(host, _)| host);
    Some(format!("{user}@{host}"))
}

/// Where SSH would take a remote gh adds on `forge` with its SSH protocol, when that isn't the
/// forge: gh's SSH remote is `git@<host>:<owner>/<repo>.git`, so SSH resolves `git@<host>`
/// (`ssh -G`, as gh translates remotes), which a `Match user git` rule can change.
/// When ssh can't resolve it and git connects through its own SSH command (`ssh_command`), that
/// command's configuration decides, so it counts as elsewhere; without either, git can't push
/// over SSH at all.
pub(crate) fn ssh_push_leaves(forge: &str, ssh_command: Option<&str>) -> Option<String> {
    let hostname = gh_hostname(forge).to_ascii_lowercase();
    match SshHosts::default().try_resolve(&format!("git@{hostname}")) {
        Some(resolved) => {
            let resolved = gh_hostname(&resolved).to_ascii_lowercase();
            (resolved != hostname).then(|| format!("ssh resolves git@{hostname} to {resolved}"))
        }
        None => ssh_command.map(|command| {
            format!(
                "git connects to git@{hostname} through {command}, which gh-forgejo-shim can't \
                 resolve without ssh on PATH"
            )
        }),
    }
}

fn remote_rank(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "upstream" => 3,
        "github" => 2,
        "origin" => 1,
        _ => 0,
    }
}

/// A remote URL's repository as gh reads it, an SSH URL's host resolved by `ssh -G`; and, when
/// `ssh -G` gives another host, the repository under the URL's own host.
fn remote_repo(raw: &str, ssh: &mut SshHosts) -> Option<(RepoRef, Option<RepoRef>)> {
    let url = GitUrl::parse(raw)?;
    let own = url.repo(&url.host)?;
    if url.scheme != "ssh" {
        return Some((own, None));
    }
    let repo = url.repo(&ssh.resolve(&url.host))?;
    let own = (own.host != repo.host).then_some(own);
    Some((repo, own))
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

/// Unit tests resolve hosts without any ssh config, so the developer's own `~/.ssh/config` can't
/// change what they see. `-F` skips the system-wide file too.
#[cfg(test)]
const SSH_CONFIG: &[&str] = &["-F", "/dev/null"];
#[cfg(not(test))]
const SSH_CONFIG: &[&str] = &[];

/// SSH host aliases resolved with `ssh -G`, as go-gh's `ssh.Translator` does. A failure leaves
/// the host as it was.
#[derive(Default)]
struct SshHosts {
    resolved: HashMap<String, Option<String>>,
}

impl SshHosts {
    /// The hostname `ssh -G` resolves `host` (or `user@host`) to, or `None` when ssh can't run
    /// or answers nothing.
    fn try_resolve(&mut self, host: &str) -> Option<String> {
        if let Some(resolved) = self.resolved.get(&host.to_ascii_lowercase()) {
            return resolved.clone();
        }
        let resolved = Command::new("ssh")
            .args(SSH_CONFIG)
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
            .map(|resolved| github_ssh_alias(&resolved));
        self.resolved
            .insert(host.to_ascii_lowercase(), resolved.clone());
        resolved
    }

    /// As [`SshHosts::try_resolve`], or without ssh the hostname as given ([`unresolved_host`]).
    fn resolve(&mut self, host: &str) -> String {
        self.try_resolve(host)
            .unwrap_or_else(|| unresolved_host(host))
    }
}

/// GitHub's SSH-over-443 host, `ssh.github.com`, is github.com to gh.
fn github_ssh_alias(host: &str) -> String {
    if host.eq_ignore_ascii_case("ssh.github.com") {
        GITHUB.to_string()
    } else {
        host.to_string()
    }
}

/// A `host` or `user@host` ssh couldn't resolve, taken as given: without the user, as `ssh -G`
/// prints none, and with `ssh.github.com` read as github.com, as for a resolved host.
fn unresolved_host(host: &str) -> String {
    github_ssh_alias(host.rsplit_once('@').map_or(host, |(_, host)| host))
}

/// The user's gh config directory, found the way go-gh's `ConfigDir` does.
pub(crate) fn user_gh_config_dir(env: &HashMap<String, String>) -> Option<PathBuf> {
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
pub(crate) fn user_gh_hosts(env: &HashMap<String, String>) -> Vec<String> {
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

/// Whether the user's gh `hosts.yml` exists but can't be read as a map of hosts.
pub(crate) fn user_gh_hosts_unreadable(env: &HashMap<String, String>) -> bool {
    let Some(path) = user_gh_config_dir(env).map(|dir| dir.join("hosts.yml")) else {
        return false;
    };
    path.exists()
        && !fs::read_to_string(&path).is_ok_and(|text| {
            matches!(
                serde_yaml::from_str(&text),
                Ok(serde_yaml::Value::Mapping(_) | serde_yaml::Value::Null)
            )
        })
}

/// Whether the user's gh `config.yml` exists but its aliases can't be read: YAML the reader
/// rejects (a repeated key among them), or `aliases` that aren't a map. gh may still run them.
pub(crate) fn user_gh_aliases_unreadable(env: &HashMap<String, String>) -> bool {
    let Some(path) = user_gh_config_dir(env).map(|dir| dir.join("config.yml")) else {
        return false;
    };
    path.exists()
        && !fs::read_to_string(&path).is_ok_and(|text| {
            serde_yaml::from_str::<serde_yaml::Value>(&text).is_ok_and(|config| {
                config
                    .get("aliases")
                    .is_none_or(|aliases| aliases.is_mapping() || aliases.is_null())
            })
        })
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
    #[test]
    fn an_unresolved_ssh_host_drops_its_user_and_reads_ssh_github_com_as_github() {
        assert_eq!(unresolved_host("git@forge.test"), "forge.test");
        assert_eq!(unresolved_host("forge.test"), "forge.test");
        assert_eq!(unresolved_host("git@ssh.github.com"), GITHUB);
        assert_eq!(unresolved_host("SSH.GitHub.com"), GITHUB);
    }

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

    /// A throwaway repository with `remotes`, as `(name, URL)`, which [`Checkout`] reads.
    struct RemotesFixture(PathBuf);

    impl RemotesFixture {
        fn new(remotes: &[(&str, &str)]) -> Self {
            Self::with_config(remotes, &[])
        }

        /// As [`RemotesFixture::new`], then `git config --add` each `(key, value)` in `config`.
        fn with_config(remotes: &[(&str, &str)], config: &[(&str, &str)]) -> Self {
            static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gh-forgejo-shim-remotes-{}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create repo directory");
            let git = |args: &[&str]| {
                let status = Command::new("git")
                    .args(args)
                    .current_dir(&root)
                    .stdout(Stdio::null())
                    .status()
                    .expect("run git");
                assert!(status.success(), "git {args:?}");
            };
            git(&["init", "-q"]);
            for (name, url) in remotes {
                git(&["remote", "add", name, url]);
            }
            for (key, value) in config {
                git(&["config", "--add", key, value]);
            }
            Self(root)
        }

        fn remote_for(&self, host: &str, owner: &str, name: &str) -> Option<String> {
            Checkout::new(Some(&self.0)).remote_for(host, owner, name)
        }
    }

    impl Drop for RemotesFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn remote_for_takes_gh_s_remote_order() {
        const URL: &str = "ssh://git@git.example.com/owner/repo.git";
        let names = ["mine", "origin", "github", "upstream"];
        for (i, expected) in names.iter().enumerate().rev() {
            let remotes = names[..=i]
                .iter()
                .map(|name| (*name, URL))
                .collect::<Vec<_>>();
            assert_eq!(
                RemotesFixture::new(&remotes)
                    .remote_for("git.example.com", "owner", "repo")
                    .as_deref(),
                Some(*expected),
                "{remotes:?}"
            );
        }
        // The rank ignores case, and a remote that names another repository doesn't count.
        let fixture = RemotesFixture::new(&[
            ("Upstream", "ssh://git@git.example.com/other/repo.git"),
            ("Origin", URL),
            ("mine", URL),
        ]);
        assert_eq!(
            fixture
                .remote_for("git.example.com", "owner", "repo")
                .as_deref(),
            Some("Origin")
        );
    }

    #[test]
    fn remote_for_reads_every_url_form() {
        for url in [
            "ssh://git@git.example.com/owner/repo.git",
            "ssh://git@git.example.com/owner/repo",
            "ssh://git@git.example.com:2222/owner/repo.git",
            "git@git.example.com:owner/repo.git",
            "git.example.com:owner/repo",
            "https://git.example.com/owner/repo.git",
            "https://git.example.com/owner/repo",
        ] {
            assert_eq!(
                RemotesFixture::new(&[("origin", url)])
                    .remote_for("git.example.com", "owner", "repo")
                    .as_deref(),
                Some("origin"),
                "{url}"
            );
        }
    }

    #[test]
    fn remote_for_matches_without_case_but_only_on_the_host() {
        let fixture = RemotesFixture::new(&[
            ("elsewhere", "https://other.example.com/owner/repo.git"),
            ("origin", "git@GIT.example.com:Owner/Repo.git"),
        ]);
        for (host, owner, name) in [
            ("git.example.com", "owner", "repo"),
            ("Git.Example.com", "OWNER", "REPO"),
            ("git.example.com:3000", "owner", "repo"),
        ] {
            assert_eq!(
                fixture.remote_for(host, owner, name).as_deref(),
                Some("origin"),
                "{host} {owner}/{name}"
            );
        }
        assert_eq!(
            fixture
                .remote_for("other.example.com", "owner", "repo")
                .as_deref(),
            Some("elsewhere")
        );
        for (host, owner, name) in [
            ("third.example.com", "owner", "repo"),
            ("git.example.com", "owner", "other"),
            ("git.example.com", "someone", "repo"),
        ] {
            assert_eq!(
                fixture.remote_for(host, owner, name),
                None,
                "{host} {owner}/{name}"
            );
        }
    }

    #[test]
    fn remote_for_takes_only_the_url_git_fetches_from() {
        const FORGE: &str = "ssh://git@git.example.com/owner/repo.git";
        // A local mirror that pushes to the forge, by push URL or a second `url`: git fetches
        // from the first `url` alone, which names no repository on the forge.
        for key in ["remote.origin.pushurl", "remote.origin.url"] {
            let fixture = RemotesFixture::with_config(
                &[("origin", "/srv/mirrors/owner/repo.git")],
                &[(key, FORGE)],
            );
            assert_eq!(
                fixture.remote_for("git.example.com", "owner", "repo"),
                None,
                "{key}"
            );
        }
        // A remote fetched from the forge counts, wherever it pushes, and so does one whose URL
        // an `insteadOf` rule rewrites to the forge.
        let fixture = RemotesFixture::with_config(
            &[("origin", FORGE), ("upstream", "forge:owner/repo.git")],
            &[
                ("remote.origin.pushurl", "/srv/mirrors/owner/repo.git"),
                ("url.ssh://git@git.example.com/.insteadOf", "forge:"),
            ],
        );
        assert_eq!(
            fixture
                .remote_for("git.example.com", "owner", "repo")
                .as_deref(),
            Some("upstream")
        );
    }

    #[test]
    fn ipv6_hostnames_lose_brackets_and_port() {
        assert_eq!(url_hostname("[::1]:3000"), "::1");
        assert_eq!(url_hostname("forge.test:3443"), "forge.test");
        assert_eq!(url_hostname("::1"), "::1");
        assert_eq!(gh_hostname("[::1]:3000"), "::1");
    }
}
