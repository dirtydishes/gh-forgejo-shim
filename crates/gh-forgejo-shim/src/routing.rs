//! Native dispatcher and route decisions for managed `gh` invocations.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::{self, EnvMap};
use crate::external::{find_program, run_program_capture, run_program_inherit};
use crate::provider::{HostProfile, HostRegistry, ProviderResolution};
use crate::read_only;
use crate::repo::{detect_repo, RepoRef};
use crate::trace;
use crate::Result;

const SUPPORTED_PR_COMMANDS: &[&str] = &[
    "checks", "checkout", "co", "comment", "create", "diff", "list", "new", "status", "view",
];
const SUPPORTED_ISSUE_COMMANDS: &[&str] = &["create", "list", "ls", "new", "view"];
const SUPPORTED_REPO_COMMANDS: &[&str] = &["view"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatcherConfig {
    pub registry: HostRegistry,
    pub real_gh: Option<PathBuf>,
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
    pub profile: Option<HostProfile>,
}

impl RouteDecision {
    fn delegate(reason: impl Into<String>, repo: Option<RepoRef>) -> Self {
        Self {
            kind: RouteKind::Delegate,
            reason: reason.into(),
            repo,
            host: None,
            profile: None,
        }
    }

    fn forgejo(reason: impl Into<String>, repo: Option<RepoRef>, profile: HostProfile) -> Self {
        Self {
            kind: RouteKind::Forgejo,
            reason: reason.into(),
            repo,
            host: Some(profile.canonical_host.clone()),
            profile: Some(profile),
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
    if argv.first().is_some_and(|command| command == "auth") {
        let host =
            hostname_arg(&argv[2..], true).or_else(|| env.get("GH_HOST").map(String::as_str));
        if let Some(decision) = decide_explicit_host_route(config, host) {
            return decision;
        }
    }

    if argv.first().is_some_and(|command| command == "api") {
        let host =
            hostname_arg(&argv[1..], false).or_else(|| env.get("GH_HOST").map(String::as_str));
        if let Some(decision) = decide_explicit_host_route(config, host) {
            return decision;
        }
    }

    let detection = detect_repo(argv, env, cwd);
    if let Some(repo) = detection.repo {
        return route_resolution(config.registry.resolve(&repo), detection.source);
    }

    if argv.len() < 2 || !is_supported_command(argv) {
        RouteDecision::delegate("unsupported command", None)
    } else {
        RouteDecision::delegate("no repository detected", None)
    }
}

pub fn load_dispatcher_config(env: &HashMap<String, String>) -> Result<DispatcherConfig> {
    let env_map = env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<EnvMap>();
    let path = config_path(env).unwrap_or_default();
    let loaded = config::load_config_with_env(Some(&path), &env_map)?;
    let registry = config::load_host_registry_with_env(Some(&path), &env_map)?;
    Ok(DispatcherConfig {
        registry,
        real_gh: loaded.paths.gh.map(PathBuf::from),
    })
}

fn run_gh(
    argv: Vec<String>,
    env: HashMap<String, String>,
    cwd: Option<&Path>,
    mut mode: DelegateMode<'_>,
) -> i32 {
    let started = Instant::now();
    let config = match load_dispatcher_config(&env) {
        Ok(config) => config,
        Err(error) => {
            write_error(&mut mode, &format!("gh-forgejo-shim: {error}"));
            return 1;
        }
    };
    let decision = decide_route(&argv, &config, &env, cwd);
    let trace_enabled = trace::tracing_enabled(&env);

    let exit_code = match decision.kind {
        RouteKind::Delegate => run_delegate(&argv, &env, cwd, &config, &mut mode),
        RouteKind::Forgejo => run_forgejo(&argv, &env, cwd, &decision, &mut mode),
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

fn run_forgejo(
    argv: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    decision: &RouteDecision,
    mode: &mut DelegateMode<'_>,
) -> i32 {
    match mode {
        DelegateMode::Inherit => {
            let stdout = io::stdout();
            let stderr = io::stderr();
            let stdin = io::stdin();
            read_only::run(
                argv,
                decision,
                env,
                cwd,
                &mut stdout.lock(),
                &mut stderr.lock(),
                &mut stdin.lock(),
            )
        }
        DelegateMode::Capture { stdout, stderr } => {
            let stdin = io::stdin();
            read_only::run(
                argv,
                decision,
                env,
                cwd,
                *stdout,
                *stderr,
                &mut stdin.lock(),
            )
        }
    }
}

fn write_error(mode: &mut DelegateMode<'_>, message: &str) {
    match mode {
        DelegateMode::Inherit => eprintln!("{message}"),
        DelegateMode::Capture { stderr, .. } => {
            let _ = writeln!(stderr, "{message}");
        }
    }
}

fn decide_explicit_host_route(
    config: &DispatcherConfig,
    host: Option<&str>,
) -> Option<RouteDecision> {
    let host = host.filter(|host| !host.trim().is_empty())?;
    let probe = RepoRef::new(host, "_", "_");
    Some(match config.registry.resolve(&probe) {
        ProviderResolution::Forgejo { profile, .. } => {
            RouteDecision::forgejo("host", None, profile)
        }
        ProviderResolution::GitHub { repo } | ProviderResolution::Unconfigured { repo } => {
            RouteDecision::delegate(format!("host {} is not allowlisted", repo.host), None)
        }
    })
}

fn route_resolution(resolution: ProviderResolution, source: String) -> RouteDecision {
    match resolution {
        ProviderResolution::Forgejo { repo, profile } => {
            RouteDecision::forgejo(source, Some(repo), profile)
        }
        ProviderResolution::GitHub { repo } | ProviderResolution::Unconfigured { repo } => {
            RouteDecision::delegate(format!("host {} is not allowlisted", repo.host), Some(repo))
        }
    }
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
            registry: HostRegistry::new(
                hosts
                    .iter()
                    .map(|host| HostProfile {
                        canonical_host: (*host).to_string(),
                        aliases: Vec::new(),
                        api_root: format!("https://{host}/api/v1"),
                        credential_host: (*host).to_string(),
                    })
                    .collect(),
            )
            .expect("test host registry should be valid"),
            real_gh: None,
        }
    }

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn delegates_unsupported_command() {
        let cwd = std::env::temp_dir();
        let decision = decide_route(
            &argv(&["api", "repos/owner/repo"]),
            &config(&["git.example.com"]),
            &env(&[]),
            Some(&cwd),
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
    fn delegates_github_when_forgejo_profiles_exist() {
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
        let parsed = load_dispatcher_config(&env(&[
            ("FJ_SHIM_HOSTS", "git.example.com,github.com"),
            ("FJ_SHIM_REAL_GH", "/opt/homebrew/bin/gh"),
        ]))
        .expect("environment dispatcher config should load");

        assert_eq!(
            parsed
                .registry
                .profiles()
                .iter()
                .map(|profile| profile.canonical_host.as_str())
                .collect::<Vec<_>>(),
            ["git.example.com"]
        );
        assert_eq!(parsed.real_gh, Some(PathBuf::from("/opt/homebrew/bin/gh")));
    }
}
