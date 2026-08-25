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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
pub struct ForgejoTarget {
    profile: HostProfile,
    repo: Option<RepoRef>,
}

impl ForgejoTarget {
    pub fn profile(&self) -> &HostProfile {
        &self.profile
    }

    pub fn repo(&self) -> Option<&RepoRef> {
        self.repo.as_ref()
    }

    pub fn canonical_host(&self) -> &str {
        &self.profile.canonical_host
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    Delegate {
        reason: String,
        repo: Option<RepoRef>,
    },
    Forgejo {
        reason: String,
        target: ForgejoTarget,
    },
}

impl RouteDecision {
    fn delegate(reason: impl Into<String>, repo: Option<RepoRef>) -> Self {
        Self::Delegate {
            reason: reason.into(),
            repo,
        }
    }

    fn forgejo(reason: impl Into<String>, repo: Option<RepoRef>, profile: HostProfile) -> Self {
        Self::Forgejo {
            reason: reason.into(),
            target: ForgejoTarget { profile, repo },
        }
    }

    pub const fn kind(&self) -> RouteKind {
        match self {
            Self::Delegate { .. } => RouteKind::Delegate,
            Self::Forgejo { .. } => RouteKind::Forgejo,
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::Delegate { reason, .. } | Self::Forgejo { reason, .. } => reason,
        }
    }

    pub fn repo(&self) -> Option<&RepoRef> {
        match self {
            Self::Delegate { repo, .. } => repo.as_ref(),
            Self::Forgejo { target, .. } => target.repo(),
        }
    }

    pub fn forgejo_target(&self) -> Option<&ForgejoTarget> {
        match self {
            Self::Delegate { .. } => None,
            Self::Forgejo { target, .. } => Some(target),
        }
    }

    pub fn trace_host(&self) -> Option<&str> {
        self.forgejo_target()
            .map(ForgejoTarget::canonical_host)
            .or_else(|| self.repo().map(|repo| repo.host.as_str()))
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
    if is_global_delegate_command(argv) {
        return RouteDecision::delegate("unsupported command", None);
    }

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

    if let Some(decision) =
        decide_explicit_host_route(config, env.get("GH_HOST").map(String::as_str))
    {
        return decision;
    }

    if !is_supported_command(argv) {
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
    let loaded = config::load_runtime_config_with_env(Some(&path), &env_map)?;
    Ok(DispatcherConfig {
        registry: loaded.registry,
        real_gh: loaded.config.paths.gh.map(PathBuf::from),
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

    let exit_code = match &decision {
        RouteDecision::Delegate { .. } => run_delegate(&argv, &env, cwd, &config, &mut mode),
        RouteDecision::Forgejo { target, .. } => run_forgejo(&argv, &env, cwd, target, &mut mode),
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
    target: &ForgejoTarget,
    mode: &mut DelegateMode<'_>,
) -> i32 {
    match mode {
        DelegateMode::Inherit => {
            let stdout = io::stdout();
            let stderr = io::stderr();
            let stdin = io::stdin();
            read_only::run(
                argv,
                target,
                env,
                cwd,
                &mut stdout.lock(),
                &mut stderr.lock(),
                &mut stdin.lock(),
            )
        }
        DelegateMode::Capture { stdout, stderr } => {
            let stdin = io::stdin();
            read_only::run(argv, target, env, cwd, *stdout, *stderr, &mut stdin.lock())
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

fn is_global_delegate_command(argv: &[String]) -> bool {
    match argv {
        [] => true,
        [command] if matches!(command.as_str(), "--version" | "version") => true,
        [command, ..] => matches!(command.as_str(), "--help" | "-h" | "help"),
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
    use std::fs;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_ROUTE_REPO_ID: AtomicU64 = AtomicU64::new(1);

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

    fn profile_config() -> DispatcherConfig {
        DispatcherConfig {
            registry: HostRegistry::new(vec![HostProfile {
                canonical_host: "git.dirtydishes.dev".to_string(),
                aliases: vec!["127.0.0.1".to_string(), "127.0.0.2:2222".to_string()],
                api_root: "https://git.dirtydishes.dev/api/v1".to_string(),
                credential_host: "git.dirtydishes.dev".to_string(),
            }])
            .expect("route matrix profile should be valid"),
            real_gh: None,
        }
    }

    struct TempGitRepo(PathBuf);

    impl TempGitRepo {
        fn with_remote(url: &str) -> Self {
            let id = NEXT_ROUTE_REPO_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("gh-forgejo-shim-route-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("route fixture directory should be created");
            let init = Command::new("git")
                .args(["init", "-q"])
                .current_dir(&path)
                .status()
                .expect("git init should run");
            assert!(init.success());
            let remote = Command::new("git")
                .args(["remote", "add", "origin", url])
                .current_dir(&path)
                .status()
                .expect("git remote add should run");
            assert!(remote.success());
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempGitRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
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

        assert_eq!(decision.kind(), RouteKind::Delegate);
        assert_eq!(decision.reason(), "unsupported command");
    }

    #[test]
    fn routes_auth_status_for_allowlisted_gh_host() {
        let decision = decide_route(
            &argv(&["auth", "status"]),
            &config(&["git.example.com"]),
            &env(&[("GH_HOST", "git.example.com")]),
            None,
        );

        assert_eq!(decision.kind(), RouteKind::Forgejo);
        assert_eq!(decision.trace_host(), Some("git.example.com"));
    }

    #[test]
    fn routes_issue_url_for_allowlisted_host() {
        let decision = decide_route(
            &argv(&[
                "issue",
                "view",
                "--comments",
                "https://git.example.com/owner/repo/issues/13",
            ]),
            &config(&["git.example.com"]),
            &env(&[]),
            None,
        );

        assert_eq!(decision.kind(), RouteKind::Forgejo);
        assert_eq!(
            decision
                .repo()
                .map(|repo| (repo.owner.as_str(), repo.name.as_str())),
            Some(("owner", "repo"))
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

        assert_eq!(decision.kind(), RouteKind::Delegate);
        assert_eq!(decision.reason(), "host github.com is not allowlisted");
    }

    #[test]
    fn delegates_github_when_forgejo_profiles_exist() {
        let decision = decide_route(
            &argv(&["pr", "view", "-R", "github.com/owner/repo"]),
            &config(&["git.example.com"]),
            &env(&[]),
            None,
        );

        assert_eq!(decision.kind(), RouteKind::Delegate);
        assert_eq!(decision.reason(), "host github.com is not allowlisted");
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

        assert_eq!(decision.kind(), RouteKind::Forgejo);
        assert_eq!(decision.reason(), "GH_REPO");
    }

    #[test]
    fn option_payload_urls_do_not_override_repository_identity() {
        let forgejo = decide_route(
            &argv(&[
                "pr",
                "create",
                "--title",
                "test",
                "--body",
                "https://github.com/other/project",
            ]),
            &config(&["git.example.com"]),
            &env(&[("GH_REPO", "git.example.com/owner/repo")]),
            None,
        );
        assert_eq!(forgejo.kind(), RouteKind::Forgejo);
        assert_eq!(forgejo.reason(), "GH_REPO");

        let forgejo_view = decide_route(
            &argv(&[
                "pr",
                "view",
                "--template",
                "https://github.com/other/project",
            ]),
            &config(&["git.example.com"]),
            &env(&[("GH_REPO", "git.example.com/owner/repo")]),
            None,
        );
        assert_eq!(forgejo_view.kind(), RouteKind::Forgejo);
        assert_eq!(forgejo_view.reason(), "GH_REPO");

        let github = decide_route(
            &argv(&[
                "pr",
                "create",
                "--title",
                "test",
                "--body",
                "https://git.example.com/other/project",
            ]),
            &config(&["git.example.com"]),
            &env(&[("GH_REPO", "github.com/owner/repo")]),
            None,
        );
        assert_eq!(github.kind(), RouteKind::Delegate);
        assert_eq!(github.reason(), "host github.com is not allowlisted");
    }

    #[test]
    fn consumed_route_flag_values_do_not_override_provider_identity() {
        let forgejo_repo = env(&[("GH_REPO", "git.example.com/owner/repo")]);
        let github_repo = env(&[("GH_REPO", "github.com/owner/repo")]);
        let forgejo_host = env(&[("GH_HOST", "git.example.com")]);
        let github_host = env(&[("GH_HOST", "github.com")]);

        for (name, argv, env, expected) in [
            (
                "repo-shaped body value stays Forgejo",
                argv(&[
                    "pr",
                    "create",
                    "--title",
                    "test",
                    "--body",
                    "--repo=https://github.com/other/project",
                ]),
                &forgejo_repo,
                RouteKind::Forgejo,
            ),
            (
                "repo-shaped body value stays GitHub",
                argv(&[
                    "pr",
                    "create",
                    "--title",
                    "test",
                    "--body",
                    "--repo=https://git.example.com/other/project",
                ]),
                &github_repo,
                RouteKind::Delegate,
            ),
            (
                "hostname-shaped API field stays Forgejo",
                argv(&["api", "user", "-f", "--hostname=github.com"]),
                &forgejo_host,
                RouteKind::Forgejo,
            ),
            (
                "hostname-shaped API field stays GitHub",
                argv(&["api", "user", "-f", "--hostname=git.example.com"]),
                &github_host,
                RouteKind::Delegate,
            ),
        ] {
            let decision = decide_route(&argv, &config(&["git.example.com"]), env, None);
            assert_eq!(decision.kind(), expected, "{name}");
        }
    }

    #[test]
    fn command_grammar_preserves_explicit_provider_targets() {
        let cases = [
            argv(&[
                "issue",
                "view",
                "-c",
                "https://git.example.com/owner/repo/issues/13",
            ]),
            argv(&[
                "pr",
                "checks",
                "-i",
                "10",
                "https://git.example.com/owner/repo/pulls/7",
            ]),
            argv(&[
                "pr",
                "checks",
                "--interval=10",
                "https://git.example.com/owner/repo/pulls/7",
            ]),
            argv(&[
                "pr",
                "diff",
                "--exclude",
                "generated",
                "https://git.example.com/owner/repo/pulls/7",
            ]),
            argv(&[
                "pr",
                "diff",
                "-e=generated",
                "https://git.example.com/owner/repo/pulls/7",
            ]),
            argv(&["repo", "view", "git.example.com/owner/repo"]),
        ];

        for argv in cases {
            let decision = decide_route(
                &argv,
                &config(&["git.example.com"]),
                &env(&[("GH_REPO", "github.com/base/repo")]),
                None,
            );
            assert_eq!(decision.kind(), RouteKind::Forgejo, "argv: {argv:?}");
            assert_eq!(decision.trace_host(), Some("git.example.com"));
        }

        let decision = decide_route(
            &argv(&["repo", "view", "github.com/owner/repo"]),
            &config(&["git.example.com"]),
            &env(&[("GH_REPO", "git.example.com/base/repo")]),
            None,
        );
        assert_eq!(decision.kind(), RouteKind::Delegate);
        assert_eq!(decision.reason(), "host github.com is not allowlisted");
    }

    #[test]
    fn configured_gh_host_stays_local_without_repository_context() {
        let decision = decide_route(
            &argv(&["workflow", "run"]),
            &config(&["git.example.com"]),
            &env(&[("GH_HOST", "git.example.com")]),
            Some(std::env::temp_dir().as_path()),
        );

        assert_eq!(decision.kind(), RouteKind::Forgejo);
        assert_eq!(decision.reason(), "host");
        assert_eq!(decision.trace_host(), Some("git.example.com"));
        assert!(decision.repo().is_none());
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

    #[test]
    fn provider_first_route_matrix_covers_every_repository_source() {
        let config = profile_config();
        let remote = TempGitRepo::with_remote("https://127.0.0.2:2222/dirtydishes/dirtypages.git");
        let cases = [
            (
                "canonical Forgejo",
                argv(&[
                    "pr",
                    "list",
                    "--repo",
                    "git.dirtydishes.dev/dirtydishes/dirtypages",
                ]),
                env(&[]),
                None,
                RouteKind::Forgejo,
            ),
            (
                "alias Forgejo",
                argv(&["pr", "list", "--repo", "127.0.0.1/dirtydishes/dirtypages"]),
                env(&[]),
                None,
                RouteKind::Forgejo,
            ),
            (
                "unknown Forgejo operation",
                argv(&[
                    "workflow",
                    "run",
                    "--repo",
                    "127.0.0.1/dirtydishes/dirtypages",
                ]),
                env(&[]),
                None,
                RouteKind::Forgejo,
            ),
            (
                "unconfigured host",
                argv(&[
                    "pr",
                    "list",
                    "--repo",
                    "git.other.test/dirtydishes/dirtypages",
                ]),
                env(&[]),
                None,
                RouteKind::Delegate,
            ),
            (
                "GitHub",
                argv(&["pr", "list", "--repo", "github.com/dirtydishes/dirtypages"]),
                env(&[]),
                None,
                RouteKind::Delegate,
            ),
            (
                "GH_REPO",
                argv(&["pr", "list"]),
                env(&[("GH_REPO", "127.0.0.1/dirtydishes/dirtypages")]),
                None,
                RouteKind::Forgejo,
            ),
            (
                "GH_HOST",
                argv(&["pr", "list", "--repo", "dirtydishes/dirtypages"]),
                env(&[("GH_HOST", "127.0.0.1")]),
                None,
                RouteKind::Forgejo,
            ),
            (
                "git remote with port alias",
                argv(&["pr", "list"]),
                env(&[]),
                Some(remote.path()),
                RouteKind::Forgejo,
            ),
        ];

        for (name, argv, env, cwd, expected) in cases {
            let decision = decide_route(&argv, &config, &env, cwd);
            assert_eq!(decision.kind(), expected, "route matrix case: {name}");
            if decision.kind() == RouteKind::Forgejo {
                assert_eq!(
                    decision.trace_host(),
                    Some("git.dirtydishes.dev"),
                    "canonical identity case: {name}"
                );
            }
        }
    }
}
