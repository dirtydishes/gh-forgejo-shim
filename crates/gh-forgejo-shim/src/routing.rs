//! Native dispatcher and route decisions for managed `gh` invocations.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::external::{find_program, run_program_capture, run_program_inherit};
use crate::repo::{detect_repo, RepoRef};
use crate::trace;

const SUPPORTED_PR_COMMANDS: &[&str] = &[
    "checks", "checkout", "co", "comment", "create", "diff", "list", "new", "status", "view",
];
const SUPPORTED_ISSUE_COMMANDS: &[&str] = &["create", "list", "ls", "new", "view"];
const SUPPORTED_REPO_COMMANDS: &[&str] = &["view"];
const SUPPORTED_AUTH_COMMANDS: &[&str] = &["status", "token"];
const KNOWN_GITHUB_HOSTS: &[&str] = &["github.com", "www.github.com"];

const API_FLAGS_WITH_VALUES: &[&str] = &[
    "--cache",
    "-F",
    "--field",
    "-H",
    "--header",
    "--hostname",
    "--input",
    "--method",
    "-X",
    "--preview",
    "-p",
    "-f",
    "--raw-field",
];
const API_FLAGS_WITH_OPTIONAL_VALUES: &[&str] = &["--jq", "-q", "--template", "-t"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatcherConfig {
    pub hosts: Vec<String>,
    pub real_gh: Option<PathBuf>,
}

impl DispatcherConfig {
    pub fn is_forgejo_host(&self, host: Option<&str>) -> bool {
        let Some(host) = host else {
            return false;
        };
        let normalized = normalize_host(host);
        !is_known_github_host(Some(normalized.as_str()))
            && self
                .hosts
                .iter()
                .any(|candidate| normalize_host(candidate) == normalized)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteKind {
    Delegate,
    Forgejo,
}

impl RouteKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Delegate => "delegate",
            Self::Forgejo => "forgejo",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    pub kind: RouteKind,
    pub reason: String,
    pub repo: Option<RepoRef>,
    pub host: Option<String>,
}

impl RouteDecision {
    fn delegate(reason: impl Into<String>, repo: Option<RepoRef>) -> Self {
        Self {
            kind: RouteKind::Delegate,
            reason: reason.into(),
            repo,
            host: None,
        }
    }

    fn forgejo(reason: impl Into<String>, repo: Option<RepoRef>, host: Option<String>) -> Self {
        Self {
            kind: RouteKind::Forgejo,
            reason: reason.into(),
            repo,
            host,
        }
    }

    pub fn trace_host(&self) -> Option<&str> {
        self.host
            .as_deref()
            .or_else(|| self.repo.as_ref().map(|repo| repo.host.as_str()))
    }
}

enum DelegateMode<'a> {
    Inherit,
    Capture {
        stdout: &'a mut dyn Write,
        stderr: &'a mut dyn Write,
    },
}

pub fn run_gh_from_env<I>(args: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let argv = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let env = std::env::vars().collect::<HashMap<_, _>>();
    let cwd = std::env::current_dir().ok();
    run_gh(argv, env, cwd.as_deref(), DelegateMode::Inherit)
}

pub fn run_gh_captured(
    argv: Vec<String>,
    env: HashMap<String, String>,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    run_gh(argv, env, cwd, DelegateMode::Capture { stdout, stderr })
}

pub fn decide_route(
    argv: &[String],
    config: &DispatcherConfig,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> RouteDecision {
    if is_supported_auth_command(argv) {
        let host =
            hostname_arg(&argv[2..], true).or_else(|| env.get("GH_HOST").map(String::as_str));
        return decide_host_route(argv, config, env, cwd, host);
    }

    if is_supported_api_command(argv) {
        let host =
            hostname_arg(&argv[1..], false).or_else(|| env.get("GH_HOST").map(String::as_str));
        return decide_host_route(argv, config, env, cwd, host);
    }

    if argv.len() < 2 || !is_supported_command(argv) {
        return RouteDecision::delegate("unsupported command", None);
    }

    let detection = detect_repo(argv, env, cwd);
    let Some(repo) = detection.repo else {
        return RouteDecision::delegate("no repository detected", None);
    };
    if !config.is_forgejo_host(Some(repo.host.as_str())) {
        return RouteDecision::delegate(
            format!("host {} is not allowlisted", repo.host),
            Some(repo),
        );
    }

    let host = repo.host.clone();
    RouteDecision::forgejo(detection.source, Some(repo), Some(host))
}

pub fn load_dispatcher_config(env: &HashMap<String, String>) -> DispatcherConfig {
    let mut config = load_dispatcher_config_file(env).unwrap_or_else(|| DispatcherConfig {
        hosts: Vec::new(),
        real_gh: None,
    });

    if let Some(hosts) = env.get("FJ_SHIM_HOSTS") {
        config.hosts = split_hosts(hosts)
            .into_iter()
            .filter(|host| !is_known_github_host(Some(host.as_str())))
            .collect();
    }

    if let Some(real_gh) = optional_env(env, "FJ_SHIM_REAL_GH") {
        config.real_gh = Some(PathBuf::from(real_gh));
    }

    config.hosts = dedupe_hosts(config.hosts);
    config
}

pub fn normalize_host(host: &str) -> String {
    let without_scheme = host
        .trim()
        .split_once("://")
        .map_or(host.trim(), |(_, rest)| rest);
    without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .to_ascii_lowercase()
}

pub fn is_known_github_host(host: Option<&str>) -> bool {
    host.map(|host| {
        let normalized = normalize_host(host);
        KNOWN_GITHUB_HOSTS.contains(&normalized.as_str())
    })
    .unwrap_or(false)
}

fn run_gh(
    argv: Vec<String>,
    env: HashMap<String, String>,
    cwd: Option<&Path>,
    mut mode: DelegateMode<'_>,
) -> i32 {
    let started = Instant::now();
    let config = load_dispatcher_config(&env);
    let decision = decide_route(&argv, &config, &env, cwd);
    let trace_enabled = trace::tracing_enabled(&env);

    let exit_code = match decision.kind {
        RouteKind::Delegate => run_delegate(&argv, &env, cwd, &config, &mut mode),
        RouteKind::Forgejo => run_forgejo_placeholder(&decision, &mut mode),
    };

    if trace_enabled {
        trace::append_gh_record(
            &env,
            &argv,
            cwd,
            Some(&decision),
            started.elapsed().as_secs_f64() * 1000.0,
            exit_code,
        );
    }

    exit_code
}

fn run_delegate(
    argv: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    config: &DispatcherConfig,
    mode: &mut DelegateMode<'_>,
) -> i32 {
    let Some(real_gh) = find_program("gh", config.real_gh.as_deref(), env) else {
        write_error(
            mode,
            "gh-forgejo-shim: could not find the real gh executable; set FJ_SHIM_REAL_GH",
        );
        return 127;
    };

    match mode {
        DelegateMode::Inherit => match run_program_inherit(&real_gh, argv, env, cwd) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{error}");
                127
            }
        },
        DelegateMode::Capture { stdout, stderr } => {
            match run_program_capture(&real_gh, argv, env, cwd) {
                Ok(output) => {
                    let _ = stdout.write_all(&output.stdout);
                    let _ = stderr.write_all(&output.stderr);
                    output.code
                }
                Err(error) => {
                    let _ = writeln!(stderr, "{error}");
                    127
                }
            }
        }
    }
}

fn run_forgejo_placeholder(decision: &RouteDecision, mode: &mut DelegateMode<'_>) -> i32 {
    let host = decision.trace_host().unwrap_or("unknown host");
    write_error(
        mode,
        &format!(
            "gh-forgejo-shim: Forgejo route selected for {host} ({}); Rust command handling is not implemented in this phase",
            decision.reason
        ),
    );
    2
}

fn write_error(mode: &mut DelegateMode<'_>, message: &str) {
    match mode {
        DelegateMode::Inherit => eprintln!("{message}"),
        DelegateMode::Capture { stderr, .. } => {
            let _ = writeln!(stderr, "{message}");
        }
    }
}

fn decide_host_route(
    argv: &[String],
    config: &DispatcherConfig,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    host: Option<&str>,
) -> RouteDecision {
    if let Some(host) = host.filter(|host| !host.trim().is_empty()) {
        let normalized = normalize_host(host);
        if config.is_forgejo_host(Some(normalized.as_str())) {
            return RouteDecision::forgejo("host", None, Some(normalized));
        }
        return RouteDecision::delegate(format!("host {normalized} is not allowlisted"), None);
    }

    let detection = detect_repo(argv, env, cwd);
    let Some(repo) = detection.repo else {
        return RouteDecision::delegate("no repository detected", None);
    };
    if !config.is_forgejo_host(Some(repo.host.as_str())) {
        return RouteDecision::delegate(
            format!("host {} is not allowlisted", repo.host),
            Some(repo),
        );
    }

    let host = repo.host.clone();
    RouteDecision::forgejo(detection.source, Some(repo), Some(host))
}

fn is_supported_command(argv: &[String]) -> bool {
    if argv.len() < 2 {
        return false;
    }
    match argv[0].as_str() {
        "pr" => SUPPORTED_PR_COMMANDS.contains(&argv[1].as_str()),
        "issue" => SUPPORTED_ISSUE_COMMANDS.contains(&argv[1].as_str()),
        "repo" => SUPPORTED_REPO_COMMANDS.contains(&argv[1].as_str()),
        _ => false,
    }
}

fn is_supported_auth_command(argv: &[String]) -> bool {
    argv.len() >= 2 && argv[0] == "auth" && SUPPORTED_AUTH_COMMANDS.contains(&argv[1].as_str())
}

fn is_supported_api_command(argv: &[String]) -> bool {
    argv.len() >= 2
        && argv[0] == "api"
        && matches!(api_endpoint(&argv[1..]).as_deref(), Some("user" | "/user"))
}

fn hostname_arg(argv: &[String], allow_short: bool) -> Option<&str> {
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if (arg == "--hostname" || (allow_short && arg == "-h")) && index + 1 < argv.len() {
            return Some(argv[index + 1].as_str());
        }
        if let Some(value) = arg.strip_prefix("--hostname=") {
            return Some(value);
        }
        if allow_short {
            if let Some(value) = arg.strip_prefix("-h=") {
                return Some(value);
            }
        }
        index += 1;
    }
    None
}

fn api_endpoint(argv: &[String]) -> Option<String> {
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--" {
            return argv.get(index + 1).cloned();
        }
        if API_FLAGS_WITH_VALUES.contains(&arg) || API_FLAGS_WITH_OPTIONAL_VALUES.contains(&arg) {
            index += 2;
        } else if has_flag_value_prefix(arg, API_FLAGS_WITH_VALUES)
            || has_flag_value_prefix(arg, API_FLAGS_WITH_OPTIONAL_VALUES)
            || arg.starts_with('-')
        {
            index += 1;
        } else {
            return Some(arg.to_string());
        }
    }
    None
}

fn has_flag_value_prefix(arg: &str, flags: &[&str]) -> bool {
    flags.iter().any(|flag| {
        arg.strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
            .is_some()
    })
}

fn load_dispatcher_config_file(env: &HashMap<String, String>) -> Option<DispatcherConfig> {
    let path = config_path(env)?;
    let text = fs::read_to_string(path).ok()?;
    Some(parse_config_text(&text))
}

fn config_path(env: &HashMap<String, String>) -> Option<PathBuf> {
    env.get("HOME")
        .filter(|home| !home.trim().is_empty())
        .map(|home| {
            Path::new(home)
                .join(".config")
                .join("gh-forgejo-shim")
                .join("config.toml")
        })
}

fn parse_config_text(text: &str) -> DispatcherConfig {
    let mut table = "";
    let mut hosts = Vec::new();
    let mut real_gh = None;

    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            table = line.trim_matches(&['[', ']'][..]).trim();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if table.is_empty() && key == "hosts" {
            hosts = parse_host_list(value);
        } else if table == "paths" && key == "gh" {
            real_gh = parse_toml_string(value).map(PathBuf::from);
        }
    }

    DispatcherConfig {
        hosts: hosts
            .into_iter()
            .filter(|host| !is_known_github_host(Some(host.as_str())))
            .collect(),
        real_gh,
    }
}

fn parse_host_list(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    else {
        return Vec::new();
    };
    inner
        .split(',')
        .filter_map(parse_toml_string)
        .map(|host| normalize_host(&host))
        .filter(|host| !host.is_empty())
        .collect()
}

fn parse_toml_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let quote = trimmed.chars().next()?;
    if !matches!(quote, '"' | '\'') || !trimmed.ends_with(quote) || trimmed.len() < 2 {
        return None;
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    if quote == '\'' {
        return Some(inner.to_string());
    }
    Some(unescape_basic_toml_string(inner))
}

fn unescape_basic_toml_string(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match chars.next() {
            Some('"') => result.push('"'),
            Some('\\') => result.push('\\'),
            Some('n') => result.push('\n'),
            Some('t') => result.push('\t'),
            Some(other) => {
                result.push('\\');
                result.push(other);
            }
            None => result.push('\\'),
        }
    }
    result
}

fn split_hosts(value: &str) -> Vec<String> {
    value
        .replace([';', ' '], ",")
        .split(',')
        .map(normalize_host)
        .filter(|host| !host.is_empty())
        .collect()
}

fn optional_env<'a>(env: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    env.get(name)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn dedupe_hosts(hosts: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for host in hosts {
        if !deduped.contains(&host) {
            deduped.push(host);
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(values: &[(&str, &str)]) -> HashMap<String, String> {
        values
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    fn config(hosts: &[&str]) -> DispatcherConfig {
        DispatcherConfig {
            hosts: hosts.iter().map(|host| (*host).to_string()).collect(),
            real_gh: None,
        }
    }

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn delegates_unsupported_command() {
        let decision = decide_route(
            &argv(&["api", "repos/owner/repo"]),
            &config(&["git.example.com"]),
            &env(&[]),
            None,
        );

        assert_eq!(decision.kind, RouteKind::Delegate);
        assert_eq!(decision.reason, "unsupported command");
    }

    #[test]
    fn routes_auth_status_for_allowlisted_gh_host() {
        let decision = decide_route(
            &argv(&["auth", "status"]),
            &config(&["git.example.com"]),
            &env(&[("GH_HOST", "git.example.com")]),
            None,
        );

        assert_eq!(decision.kind, RouteKind::Forgejo);
        assert_eq!(decision.host.as_deref(), Some("git.example.com"));
    }

    #[test]
    fn routes_issue_url_for_allowlisted_host() {
        let decision = decide_route(
            &argv(&[
                "issue",
                "view",
                "https://git.example.com/owner/repo/issues/13",
            ]),
            &config(&["git.example.com"]),
            &env(&[]),
            None,
        );

        assert_eq!(decision.kind, RouteKind::Forgejo);
        assert_eq!(
            decision.repo.map(|repo| (repo.owner, repo.name)),
            Some(("owner".to_string(), "repo".to_string()))
        );
    }

    #[test]
    fn delegates_non_allowlisted_host() {
        let decision = decide_route(
            &argv(&["pr", "view", "-R", "github.com/owner/repo"]),
            &config(&["git.example.com"]),
            &env(&[]),
            None,
        );

        assert_eq!(decision.kind, RouteKind::Delegate);
        assert_eq!(decision.reason, "host github.com is not allowlisted");
    }

    #[test]
    fn delegates_github_even_when_accidentally_allowlisted() {
        let decision = decide_route(
            &argv(&["pr", "view", "-R", "github.com/owner/repo"]),
            &config(&["github.com", "git.example.com"]),
            &env(&[]),
            None,
        );

        assert_eq!(decision.kind, RouteKind::Delegate);
        assert_eq!(decision.reason, "host github.com is not allowlisted");
    }

    #[test]
    fn gh_repo_is_checked_before_git_remote() {
        let decision = decide_route(
            &argv(&["pr", "list"]),
            &config(&["git.example.com"]),
            &env(&[
                ("GH_REPO", "git.example.com/owner/repo"),
                ("GH_HOST", "github.com"),
            ]),
            None,
        );

        assert_eq!(decision.kind, RouteKind::Forgejo);
        assert_eq!(decision.reason, "GH_REPO");
    }

    #[test]
    fn parses_dispatcher_config_hosts_and_paths() {
        let parsed = parse_config_text(
            r#"
            hosts = ["git.example.com", "github.com"]

            [paths]
            gh = "/opt/homebrew/bin/gh"
            "#,
        );

        assert_eq!(parsed.hosts, vec!["git.example.com"]);
        assert_eq!(parsed.real_gh, Some(PathBuf::from("/opt/homebrew/bin/gh")));
    }
}
