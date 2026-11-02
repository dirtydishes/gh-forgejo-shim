//! Native dispatcher and route decisions for managed `gh` invocations.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::{self, normalize_host};
use crate::external::{find_program, run_program_capture, run_program_inherit};
use crate::invocation::Invocation;
use crate::read_only;
use crate::repo::{detect_repo, expand_alias, gh_hostname, Checkout, GhHosts, RepoRef};
use crate::trace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatcherConfig {
    pub hosts: Vec<String>,
    pub real_gh: Option<PathBuf>,
}

impl DispatcherConfig {
    pub fn is_forgejo_host(&self, host: Option<&str>) -> bool {
        config::is_forgejo_host(&self.hosts, host)
    }

    /// The allowlist entry that serves `host`. gh drops the port from a remote URL, so a host
    /// without a port is served by the one entry with that hostname, port or not. A host with a
    /// port needs an entry with that exact port. A hostname that two entries share serves
    /// neither, under any spelling: gh's API client looks up tokens by hostname alone
    /// (`gh/api/http_client.go:284-290`), so a launched gh could present neither entry's token.
    pub fn forgejo_host(&self, host: &str) -> Option<String> {
        let host = normalize_host(host);
        let hostname = gh_hostname(&host);
        let mut entries = self
            .hosts
            .iter()
            .map(|entry| normalize_host(entry))
            .filter(|entry| gh_hostname(entry) == hostname && self.is_forgejo_host(Some(entry)));
        let entry = entries.next()?;
        if entries.next().is_some() {
            return None;
        }
        (host == hostname || entry == host).then_some(entry)
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
    let argv = args.into_iter().collect::<Vec<_>>();
    // `std::env::vars` panics on a value that isn't UTF-8 (audit R7). Routing reads only named
    // variables, so one it can't read is left out of the map.
    let env = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect::<HashMap<_, _>>();
    // Guards 2 and 3 (launcher spec §8) come before routing, so a loop stops at once.
    if let Some(code) = crate::launcher::recursion_guard(&env) {
        return code;
    }
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
    run_gh(
        argv.into_iter().map(OsString::from).collect(),
        env,
        cwd,
        DelegateMode::Capture { stdout, stderr },
    )
}

pub fn decide_route(
    argv: &[String],
    config: &DispatcherConfig,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> RouteDecision {
    decide_invocation_route(&routing_invocation(argv, env), config, env, cwd)
}

/// The invocation routing decides on: `argv` with a plain gh alias expanded as gh expands it, so
/// an alias's own flags and arguments count. gh itself still gets the original `argv`.
fn routing_invocation(argv: &[String], env: &HashMap<String, String>) -> Invocation {
    match expand_alias(argv, env) {
        Some(expanded) => Invocation::parse(&expanded),
        None => Invocation::parse(argv),
    }
}

fn decide_invocation_route(
    invocation: &Invocation,
    config: &DispatcherConfig,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> RouteDecision {
    // gh sends `gh api https://host/...` to that URL as it is, whatever --hostname says
    // (`gh/pkg/cmd/api/http.go:19-21`).
    if invocation.command == "api" {
        if let Some((_, rest)) = invocation
            .positionals
            .first()
            .and_then(|path| path.split_once("://"))
        {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            return host_route(host, config);
        }
    }
    if invocation.command.starts_with("auth ") || invocation.command == "api" {
        if let Some(host) = invocation
            .flag("hostname")
            .or_else(|| env.get("GH_HOST").map(String::as_str))
            .filter(|host| !host.trim().is_empty())
        {
            return host_route(host, config);
        }
    }
    let checkout = Checkout::new(cwd);
    let detection = detect_repo(
        invocation,
        env,
        &checkout,
        &GhHosts::load(env, &config.hosts),
    );
    // A launched gh knows the user's hosts plus the allowlist hosts, so routing models that.
    // Plain gh knows only the user's hosts. Before delegating, check that plain gh wouldn't pick a
    // Forgejo host itself: it does when the user's own hosts.yml lists one. Such a call launches
    // with the user's key set instead, which the launched gh then resolves exactly as plain gh.
    if detection
        .repo
        .as_ref()
        .is_none_or(|repo| config.forgejo_host(&repo.host).is_none())
    {
        let plain = detect_repo(invocation, env, &checkout, &GhHosts::user(env));
        if let Some(repo) = plain.repo {
            if let Some(host) = config.forgejo_host(&repo.host) {
                return RouteDecision::forgejo(
                    format!("{}, user's gh hosts", plain.source),
                    Some(repo),
                    Some(host),
                );
            }
        }
    }
    let Some(repo) = detection.repo else {
        if let Some(host) = env.get("GH_HOST").filter(|host| !host.trim().is_empty()) {
            return host_route(host, config);
        }
        return RouteDecision::delegate(
            if !invocation.has_grammar() {
                "unsupported command"
            } else {
                "no repository detected"
            },
            None,
        );
    };
    let Some(host) = config.forgejo_host(&repo.host) else {
        return RouteDecision::delegate(
            format!("host {} is not allowlisted", repo.host),
            Some(repo),
        );
    };
    RouteDecision::forgejo(detection.source, Some(repo), Some(host))
}

fn host_route(host: &str, config: &DispatcherConfig) -> RouteDecision {
    match config.forgejo_host(host) {
        Some(host) => RouteDecision::forgejo("host", None, Some(host)),
        None => RouteDecision::delegate(
            format!("host {} is not allowlisted", normalize_host(host)),
            None,
        ),
    }
}

pub fn load_dispatcher_config(env: &HashMap<String, String>) -> crate::Result<DispatcherConfig> {
    let env = env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let config = config::load_config_with_env(None, &env)?;
    Ok(DispatcherConfig {
        hosts: config.hosts,
        real_gh: config.paths.gh.map(PathBuf::from),
    })
}

fn run_gh(
    original_argv: Vec<OsString>,
    env: HashMap<String, String>,
    cwd: Option<&Path>,
    mut mode: DelegateMode<'_>,
) -> i32 {
    let argv = original_argv
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let invocation = Invocation::parse(&argv);
    let started = Instant::now();
    let config = match load_dispatcher_config(&env) {
        Ok(config) => config,
        Err(error) => {
            write_error(&mut mode, &format!("gh-forgejo-shim: {error}"));
            return 1;
        }
    };
    let routed = routing_invocation(&argv, &env);
    let decision = decide_invocation_route(&routed, &config, &env, cwd);
    let trace_enabled = trace::tracing_enabled(&env);

    let exit_code = match decision.kind {
        RouteKind::Delegate => run_delegate(&original_argv, &env, cwd, &config, &mut mode),
        RouteKind::Forgejo => {
            launch_listed(&routed, &decision, &original_argv, &env, &config, &mode)
                .unwrap_or_else(|| run_forgejo(&invocation, &env, cwd, &decision, &mut mode))
        }
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
    argv: &[OsString],
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
        // The routing map leaves out values that aren't UTF-8; real gh gets the environment whole.
        DelegateMode::Inherit => {
            match run_program_inherit(&real_gh, argv, std::env::vars_os(), cwd) {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("{error}");
                    127
                }
            }
        }
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

/// Runs a launch-list command through the launcher (spec §2), or returns `None` for the built-in
/// path. Only on the terminal path: a captured run has no gh to hand the terminal to.
fn launch_listed(
    routed: &Invocation,
    decision: &RouteDecision,
    argv: &[OsString],
    env: &HashMap<String, String>,
    config: &DispatcherConfig,
    mode: &DelegateMode<'_>,
) -> Option<i32> {
    #[cfg(unix)]
    if matches!(mode, DelegateMode::Inherit) && crate::launcher::launches(routed) {
        let host = decision.trace_host()?;
        return crate::launcher::start::launch(host, argv, env, config.real_gh.as_deref());
    }
    let _ = (routed, decision, argv, env, config, mode);
    None
}

fn run_forgejo(
    invocation: &Invocation,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    decision: &RouteDecision,
    mode: &mut DelegateMode<'_>,
) -> i32 {
    if let Err(error) = invocation.validate() {
        write_stderr(mode, &error.render());
        return error.exit_code();
    }
    if let Some(help) = invocation.help() {
        match mode {
            DelegateMode::Inherit => {
                let _ = io::stdout().write_all(help.as_bytes());
            }
            DelegateMode::Capture { stdout, .. } => {
                let _ = stdout.write_all(help.as_bytes());
            }
        }
        return 0;
    }
    if !invocation.is_native() {
        write_error(
            mode,
            &format!(
                "gh-forgejo-shim: \"gh {}\" is not supported for Forgejo host {} yet",
                invocation.command,
                decision.trace_host().unwrap_or_default(),
            ),
        );
        return 1;
    }
    let argv = invocation.handler_argv();
    match mode {
        DelegateMode::Inherit => {
            let stdout = io::stdout();
            let stderr = io::stderr();
            let stdin = io::stdin();
            read_only::run(
                &argv,
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
                &argv,
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

fn write_stderr(mode: &mut DelegateMode<'_>, message: &str) {
    match mode {
        DelegateMode::Inherit => {
            let _ = io::stderr().write_all(message.as_bytes());
        }
        DelegateMode::Capture { stderr, .. } => {
            let _ = stderr.write_all(message.as_bytes());
        }
    }
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
    fn delegates_non_forgejo_api_command() {
        let decision = decide_route(
            &argv(&["api", "repos/owner/repo"]),
            &config(&["git.example.com"]),
            &env(&[]),
            Some(Path::new("/nonexistent-gfjs-repo")),
        );

        assert_eq!(decision.kind, RouteKind::Delegate);
        assert_eq!(decision.reason, "no repository detected");
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
}
