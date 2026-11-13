//! Native dispatcher and route decisions for managed `gh` invocations.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::{self, normalize_host};
use crate::external::{find_program, run_program_capture};
use crate::invocation::Invocation;
use crate::read_only;
use crate::repo::{
    command_word, detect_repo, detect_repo_by_own_host, expand_alias, gh_command_word, gh_hostname,
    user_gh_aliases_unreadable, user_gh_config_dir, user_gh_hosts_unreadable, Checkout, GhHosts,
    RepoRef,
};
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
    Refuse,
}

impl RouteKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Delegate => "delegate",
            Self::Forgejo => "forgejo",
            Self::Refuse => "refuse",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    pub kind: RouteKind,
    pub reason: String,
    pub repo: Option<RepoRef>,
    pub host: Option<String>,
    /// `HOST/OWNER/REPO` for gh's `GH_REPO`, when routing chose a remote gh drops itself (#334):
    /// the launched or stepped-aside gh would otherwise find no repository.
    pub gh_repo: Option<String>,
}

impl RouteDecision {
    fn delegate(reason: impl Into<String>, repo: Option<RepoRef>) -> Self {
        Self {
            kind: RouteKind::Delegate,
            reason: reason.into(),
            repo,
            host: None,
            gh_repo: None,
        }
    }

    /// A refusal keeps the repository routing detected, so the trace still says what it found.
    fn refuse(reason: impl Into<String>, repo: Option<RepoRef>) -> Self {
        Self {
            kind: RouteKind::Refuse,
            reason: reason.into(),
            repo,
            host: None,
            gh_repo: None,
        }
    }

    fn forgejo(reason: impl Into<String>, repo: Option<RepoRef>, host: Option<String>) -> Self {
        Self {
            kind: RouteKind::Forgejo,
            reason: reason.into(),
            repo,
            host,
            gh_repo: None,
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
    // The launcher's git credential helper is answered before anything else, ahead of the guards
    // and routing, and never launches (launcher spec §3).
    #[cfg(unix)]
    if let Some(code) = crate::launcher::credentials::answer(
        std::env::var_os(crate::launcher::credentials::CREDENTIALS_VAR).as_deref(),
        &argv,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    ) {
        return code;
    }
    // Not a credential request the helper answers: the variable the run directory's script set
    // must not reach anything this call starts, or a later `auth git-credential` would be answered
    // from a launcher's file instead of delegating (§3). Single-threaded here. That it was set at
    // all, even empty, makes this a launched call's helper context (`run_gh`).
    let helper_call = std::env::var_os(crate::launcher::credentials::CREDENTIALS_VAR).is_some();
    #[cfg(unix)]
    std::env::remove_var(crate::launcher::credentials::CREDENTIALS_VAR);
    // Guards 2 and 3 (launcher spec §8) come before routing, so a loop stops at once.
    let entry_env = routing_env();
    if let Some(code) = crate::launcher::recursion_guard(&entry_env) {
        return code;
    }
    // A nested call gets the user's environment back (§7), and everything this invocation
    // starts sees one more level of depth (§8). Both edit this process's own environment while
    // it is single-threaded, so real gh inherits it byte for byte.
    #[cfg(unix)]
    {
        crate::launcher::env::revert();
        crate::launcher::env::increment_depth(crate::launcher::depth(&entry_env));
    }
    let env = routing_env();
    // The day's usage report, if it's on and not yet sent: started in the background, never
    // waited for, and silent (docs/telemetry.md).
    crate::telemetry::daily(&|name| env.get(name).cloned());
    let cwd = std::env::current_dir().ok();
    run_gh(
        argv,
        env,
        cwd.as_deref(),
        DelegateMode::Inherit,
        helper_call,
    )
}

/// The environment routing reads. `std::env::vars` panics on a value that isn't UTF-8 (audit
/// R7); routing reads only named variables, so one it can't read is left out of the map. Real gh
/// never gets this map: it inherits the process environment.
fn routing_env() -> HashMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

pub fn run_gh_captured(
    argv: Vec<String>,
    mut env: HashMap<String, String>,
    cwd: Option<&Path>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    // As `run_gh_from_env`, from this call's own environment: the marker makes it a launched
    // call's helper context, and goes no further.
    let helper_call = env
        .remove(crate::launcher::credentials::CREDENTIALS_VAR)
        .is_some();
    run_gh(
        argv.into_iter().map(OsString::from).collect(),
        env,
        cwd,
        DelegateMode::Capture { stdout, stderr },
        helper_call,
    )
}

pub fn decide_route(
    argv: &[String],
    config: &DispatcherConfig,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> RouteDecision {
    let (words, invocation, leads) = routing_invocation(argv, env);
    decide_invocation_route(&words, &invocation, leads, config, env, cwd)
}

/// The invocation routing decides on: `argv` with a plain gh alias expanded as gh expands it, so
/// an alias's own flags and arguments count. gh itself still gets the original `argv`. Also
/// whether that argv starts with the command's first word: a flag before it may take a value the
/// parser doesn't know, so `gh --notes config release create v1` is `release create` to gh, not
/// `config` (cobra reads `config` as `--notes`'s value).
fn routing_invocation(
    argv: &[String],
    env: &HashMap<String, String>,
) -> (Vec<String>, Invocation, bool) {
    let words = expand_alias(argv, env).unwrap_or_else(|| argv.to_vec());
    let invocation = Invocation::parse(&words);
    let leads = invocation
        .command
        .split_whitespace()
        .next()
        .is_some_and(|first| words.first().map(String::as_str) == Some(first));
    (words, invocation, leads)
}

fn decide_invocation_route(
    words: &[String],
    invocation: &Invocation,
    leads: bool,
    config: &DispatcherConfig,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> RouteDecision {
    // gh may run an alias the shim couldn't read (a repeated name), so a command word that could
    // be one is refused, near a forge or not (Sol r6#2). A one-word alias can't take a gh command's
    // name, `help` aside; a two-word one under a command group (`issue fj`) can, and isn't covered
    // yet (#177).
    if let Some(word) = command_word(words).filter(|_| gh_command_word(words).is_none()) {
        if user_gh_aliases_unreadable(env) {
            let file = user_gh_config_dir(env).map_or_else(
                || "gh's config.yml".to_string(),
                |dir| dir.join("config.yml").display().to_string(),
            );
            return RouteDecision::refuse(
                format!(
                    "{file} has aliases gh-forgejo-shim can't read (a repeated name, or YAML it \
                     rejects), so it can't tell what `gh {word}` runs; fix config.yml"
                ),
                None,
            );
        }
    }
    // gh's root command (`gh`, `gh --version`, `gh --help`, or a stray flag it rejects) and
    // `gh version` answer from gh itself and send no request, so whatever repo the call is in,
    // plain gh runs. Root only when cobra finds no command word either: the parser may take one
    // as a flag's value where cobra doesn't (`gh -xR release create v1`).
    // `version` only as the leading word: `gh --notes version release create v1` is
    // `release create` to gh (cobra reads `version` as `--notes`'s value).
    if (invocation.command.is_empty() && command_word(words).is_none())
        || (leads && invocation.command == "version")
    {
        return RouteDecision::delegate("gh's root or version command", None);
    }
    // Only when the command word comes first: with a flag before it, the command gh runs may not
    // be the one the parser found, and these delegate without looking for a Forgejo repo.
    if leads && runs_on_its_own(invocation) {
        return RouteDecision::delegate("a command gh runs on its own", None);
    }
    let decision = route_by_target(invocation, config, env, cwd);
    // Which host gh sends a call to depends on how it parses the call (a word the shim takes for
    // `-R` may be another flag's value: `gh release create v1 --notes -R github.com/o/r` creates
    // the release where the checkout points) and on the command (`gh gist create` ignores
    // `GH_REPO` for `GH_HOST`). The shim can't match gh on either everywhere, so off the delegate
    // list plain gh runs only when nothing in reach names a Forgejo host.
    if decision.kind == RouteKind::Delegate && forgejo_in_reach(words, config, env, cwd) {
        return RouteDecision::refuse(
            "a Forgejo host is in reach (GH_REPO, GH_HOST, the checkout or an argument), and only \
             the delegate list runs plain gh there; run this where no Forgejo host is in reach",
            decision.repo,
        );
    }
    decision
}

/// Whether the call's words, `GH_HOST`, `GH_REPO`, gh's default host from `hosts.yml`, or any of
/// the checkout's remotes name a Forgejo host. Every remote: which one gh picks depends on the
/// command, which is what's uncertain.
fn forgejo_in_reach(
    words: &[String],
    config: &DispatcherConfig,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> bool {
    // Only entries that count as forges: a GitHub host allowlisted by mistake stays GitHub's.
    let hosts = config
        .hosts
        .iter()
        .filter(|host| config.is_forgejo_host(Some(host)))
        .map(|host| gh_hostname(&normalize_host(host)).to_ascii_lowercase())
        .collect::<Vec<_>>();
    let named = words.iter().any(|word| {
        let word = word.to_ascii_lowercase();
        hosts.iter().any(|host| word.contains(host.as_str()))
    });
    // `GH_REPO` with its host resolved as gh would; no remote, flag or argument of the call.
    let bare = Invocation::default();
    let none = Checkout::new(None);
    let gh_repo = |hosts: &GhHosts| {
        detect_repo(&bare, env, &none, hosts)
            .repo
            .is_some_and(|repo| config.forgejo_host(&repo.host).is_some())
    };
    // gh takes its default host from hosts.yml when it's the only host there and GH_HOST is
    // unset; `gh gist create` then posts to it whatever GH_REPO says (Sol r6#1). A hosts.yml the
    // shim can't read might be that, so it counts too.
    let default_forge = GhHosts::user(env)
        .configured_default()
        .is_some_and(|host| config.forgejo_host(host).is_some())
        || user_gh_hosts_unreadable(env);
    named
        || default_forge
        || env
            .get("GH_HOST")
            .is_some_and(|host| config.forgejo_host(host).is_some())
        || gh_repo(&GhHosts::load(env, &config.hosts))
        || gh_repo(&GhHosts::user(env))
        || Checkout::new(cwd)
            .remote_hosts()
            .any(|host| config.forgejo_host(host).is_some())
}

fn route_by_target(
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
    // A forge remote that `ssh -G` sends to a host gh doesn't know, such as a VPN address, counts
    // under its own name, as it would without that `HostName` (#334).
    let detection = detect_repo_by_own_host(
        invocation,
        env,
        &checkout,
        &GhHosts::load(env, &config.hosts),
        &|host| config.forgejo_host(host).is_some(),
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
    let Some(ssh_hostname) = detection.ssh_hostname else {
        return RouteDecision::forgejo(detection.source, Some(repo), Some(host));
    };
    // gh drops this remote, so it gets the repository the wrapper chose in `GH_REPO`, as for
    // `-R HOST/OWNER/REPO`. The host has no port, as gh reads a remote's. `gh api` reads the
    // repository only for its `{owner}` and `{repo}` placeholders, and refuses `{branch}` whenever
    // `GH_REPO` is set (`gh/pkg/cmd/api/api.go` `fillPlaceholders`), so it gets one only then.
    let gh_repo = (invocation.command != "api" || uses_repo_placeholder(invocation))
        .then(|| format!("{}/{}/{}", repo.host, repo.owner, repo.name));
    RouteDecision {
        gh_repo,
        ..RouteDecision::forgejo(
            format!(
                "{}, under its own host: ssh -G resolves {} to {ssh_hostname}",
                detection.source, repo.host
            ),
            Some(repo),
            Some(host),
        )
    }
}

/// Whether any argument or flag value holds one of the placeholders gh's `api` fills from the base
/// repository: `{owner}`, `{repo}`, `:owner` or `:repo` (`placeholderRE` in
/// `gh/pkg/cmd/api/api.go`). A false match only costs the `{branch}` placeholder.
fn uses_repo_placeholder(invocation: &Invocation) -> bool {
    invocation
        .positionals
        .iter()
        .chain(invocation.flags.values().flatten())
        .any(|word| {
            ["{owner}", "{repo}"].iter().any(|name| word.contains(name))
                || word.match_indices(':').any(|(at, _)| {
                    let rest = &word[at + 1..];
                    ["owner", "repo"].iter().any(|name| {
                        rest.strip_prefix(name).is_some_and(|after| {
                            !after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                        })
                    })
                })
        })
}

/// The spec's delegate list (launcher §2): commands that call no API, or that write gh's own
/// state, which a launched gh would write to a throwaway copy. Matched on the command after alias
/// expansion, so `gh sg` with `sg: auth setup-git` delegates too. Help, the list's other entry,
/// delegates in `run_gh`, after routing has recorded the forge. The `help` command isn't here: an
/// alias can take that name (cobra adds `help` after gh's aliases), and gh reads alias files the
/// shim can't, so `gh help …` routes like any other call.
fn runs_on_its_own(invocation: &Invocation) -> bool {
    let command = invocation.command.as_str();
    let group = command.split_whitespace().next().unwrap_or_default();
    matches!(
        group,
        "completion"
            | "config"
            | "alias"
            | "extension"
            | "extensions"
            | "ext"
            | "codespace"
            | "cs"
            | "copilot"
            | "skill"
            | "skills"
            | "agent-task"
            | "agent-tasks"
            | "agent"
            | "agents"
    ) || matches!(
        command,
        "auth login"
            | "auth logout"
            | "auth switch"
            | "auth refresh"
            | "auth setup-git"
            | "auth git-credential"
    )
}

/// Whether an invocation asks for help that gh would print: a command with captured help
/// (`contract/usage-errors.json`), which is either in the shim's grammar, where the parser knows
/// the command's own `-h` and its value flags, or a group such as `pr`, which has no `-h` of its
/// own. A leaf without grammar has no captured help, and the parser's fallback would misread it:
/// `repo edit -h URL` sets a homepage and `release create v1 --notes --help` sets notes, both
/// writes. Those keep the Forgejo path's refusal. A unit test keeps the two lists in step.
fn runs_help(invocation: &Invocation) -> bool {
    invocation.help().is_some()
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

/// `helper_call`: this call started with `FJ_SHIM_CREDENTIALS` set, as the launcher's helper does.
fn run_gh(
    original_argv: Vec<OsString>,
    env: HashMap<String, String>,
    cwd: Option<&Path>,
    mut mode: DelegateMode<'_>,
    helper_call: bool,
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
    let (words, routed, leads) = routing_invocation(&argv, &env);
    // Inside a launched call, git's credentials come only from the launcher's helper (§3). A
    // call its exact match didn't take runs only when cobra's command word is one of gh's own
    // other than `auth`: an `auth` form (`auth git-credential -- get`), an alias or an extension
    // could reach gh's own helper, which answers from the user's logins. The shim can't read
    // every alias gh accepts (an unquoted `true:` key, a repeated name), so it doesn't try.
    if helper_call && gh_command_word(&argv).is_none_or(|word| word == "auth") {
        write_error(
            &mut mode,
            "gh-forgejo-shim: inside a launched gh, git's credentials come only from the launcher's helper",
        );
        return 1;
    }
    let mut decision = decide_invocation_route(&words, &routed, leads, &config, &env, cwd);
    let trace_enabled = trace::tracing_enabled(&env);
    let help = decision.kind == RouteKind::Forgejo && leads && runs_help(&routed);
    if help {
        // The trace still names the forge routing chose, and says plain gh ran.
        // A prefix: callers read the reason's ending (`… user's gh hosts`).
        decision.reason = format!("help, run by plain gh; {}", decision.reason);
    }

    let exit_code = match decision.kind {
        RouteKind::Refuse => {
            write_error(&mut mode, &format!("gh-forgejo-shim: {}", decision.reason));
            1
        }
        RouteKind::Delegate => {
            run_delegate(&original_argv, &env, cwd, &config, &mut mode, trace_enabled)
        }
        // Help is plain gh's (launcher spec §2): routing still records the forge it would have
        // used, and real gh prints the help.
        RouteKind::Forgejo if help => {
            run_delegate(&original_argv, &env, cwd, &config, &mut mode, trace_enabled)
        }
        RouteKind::Forgejo => {
            let host = decision.trace_host().unwrap_or_default().to_string();
            if let Some(message) = push_goes_elsewhere(&routed, &words, &env, cwd, &host) {
                write_error(&mut mode, &format!("gh-forgejo-shim: {message}"));
                1
            } else {
                let may_step_aside = !names_routed_port(&host, &routed, &words, &env);
                launch_listed(
                    &routed,
                    &decision,
                    &original_argv,
                    &env,
                    &config,
                    &mode,
                    may_step_aside,
                )
                .or_else(|| {
                    may_step_aside
                        .then(|| {
                            step_aside_logged_in(
                                &routed,
                                &decision,
                                &words,
                                &original_argv,
                                &env,
                                &config,
                                &mode,
                            )
                        })
                        .flatten()
                })
                .unwrap_or_else(|| run_forgejo(&invocation, &env, cwd, &decision, &mut mode))
            }
        }
    };

    if trace_enabled {
        // A step-aside sends gh to the forge's portless hostname, its default port included.
        #[cfg(unix)]
        if let Some(host) = crate::launcher::start::stepped_aside_to() {
            decision.host = Some(host.to_string());
        }
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

/// Runs real gh on this process's terminal with this process's environment, untouched: no
/// `Command` environment call, so entries without `=`, repeated names and their order all pass
/// through (launcher spec §7). With tracing off the wrapper becomes gh (§2, "Delegate means
/// exec"); with it on, it waits so it can write the trace record.
fn delegate_inherit(real_gh: &Path, argv: &[OsString], trace_enabled: bool) -> i32 {
    let mut command = std::process::Command::new(real_gh);
    command.args(argv);
    #[cfg(unix)]
    if !trace_enabled {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        return exec_failure(real_gh, &error);
    }
    match command.status() {
        Ok(status) => crate::external::inherited_exit_code(status),
        Err(error) => exec_failure(real_gh, &error),
    }
}

fn exec_failure(real_gh: &Path, error: &io::Error) -> i32 {
    eprintln!(
        "gh-forgejo-shim: could not run {}: {error}",
        real_gh.display()
    );
    match error.kind() {
        io::ErrorKind::PermissionDenied => 126,
        _ => 127,
    }
}

fn run_delegate(
    argv: &[OsString],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    config: &DispatcherConfig,
    mode: &mut DelegateMode<'_>,
    trace_enabled: bool,
) -> i32 {
    let Some(real_gh) = find_program("gh", config.real_gh.as_deref(), env) else {
        write_error(
            mode,
            "gh-forgejo-shim: could not find the real gh executable; set FJ_SHIM_REAL_GH",
        );
        return 127;
    };

    match mode {
        DelegateMode::Inherit => delegate_inherit(&real_gh, argv, trace_enabled),
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
    may_step_aside: bool,
) -> Option<i32> {
    #[cfg(unix)]
    if matches!(mode, DelegateMode::Inherit) && crate::launcher::launches(routed) {
        let host = decision.trace_host()?;
        return crate::launcher::start::launch(
            host,
            routed,
            decision,
            argv,
            env,
            config.real_gh.as_deref(),
            may_step_aside,
        );
    }
    let _ = (routed, decision, argv, env, config, mode, may_step_aside);
    None
}

/// Whether the call or `GH_REPO` spells the routed host with its port (`-R forge.test:3443/o/r`,
/// `--hostname forge.test:3443`, a URL). gh keeps that port for some forms (a full repository
/// name, `gh api`'s URL or `--hostname`) and drops it for others (`gh pr view <URL>`, a remote),
/// so a step-aside, which makes the portless hostname gh's default host, can't say which endpoint
/// gh reaches. Such a call doesn't step aside (#231, parallax r2#3). `gh api` and `gh auth` take
/// no host from `GH_REPO`.
fn names_routed_port(
    host: &str,
    routed: &Invocation,
    words: &[String],
    env: &HashMap<String, String>,
) -> bool {
    let host = normalize_host(host);
    if !host.contains(':') || host.starts_with('[') {
        return false;
    }
    let gh_repo = if routed.command == "api" || routed.command.starts_with("auth ") {
        None
    } else {
        env.get("GH_REPO")
    };
    words
        .iter()
        .chain(gh_repo)
        .any(|word| word.to_ascii_lowercase().contains(&host))
}

/// Refuses a forge-routed command that pushes with git when that push could leave the forge,
/// whatever path then runs gh (#231, parallax r1#3). gh's API requests follow `GH_HOST`; git's
/// pushes follow the remote's push URL, git's URL rewrites and SSH's host aliases.
///
/// - `gh pr create` pushes the branch from the current checkout, to its remote or to a fork remote
///   it adds on the forge: refused when one of those could push elsewhere ([`crate::repo::push_leaves`]).
///   A fork remote uses gh's `git_protocol` for the forge, read from gh's files
///   ([`gh_uses_https`]); when that isn't `https`, the SSH destination counts too.
/// - `gh repo create` pushes, with `--push` or when it prompts, from a `--source` checkout the
///   wrapper would have to read as gh does, rewrites and SSH aliases included. It is refused
///   instead; without `--push` it creates the repository, and git pushes after.
fn push_goes_elsewhere(
    routed: &Invocation,
    words: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    host: &str,
) -> Option<String> {
    let forge = gh_hostname(host);
    if creates(routed, words, "repo") && repo_create_pushes(words) {
        return Some(format!(
            "`gh repo create` pushes with git when given --push or when it prompts, and the \
             wrapper can't confine that push to {forge}; create the repository without --push, \
             then push with git"
        ));
    }
    if creates(routed, words, "pr") {
        let ssh_command = git_ssh_command(env, cwd);
        let elsewhere =
            crate::repo::push_leaves(host, cwd, ssh_command.as_deref()).or_else(|| {
                (!gh_uses_https(env, host))
                    .then(|| crate::repo::ssh_push_leaves(host, ssh_command.as_deref()))
                    .flatten()
            })?;
        return Some(format!(
            "{elsewhere}, not {forge}, so `gh pr create` would push there; point the push at {forge}"
        ));
    }
    None
}

/// The SSH command git connects through instead of `ssh` on PATH, by the name that sets it:
/// `GIT_SSH_COMMAND`, `GIT_SSH` or `core.sshCommand`. ssh on PATH doesn't see its configuration.
fn git_ssh_command(env: &HashMap<String, String>, cwd: Option<&Path>) -> Option<String> {
    ["GIT_SSH_COMMAND", "GIT_SSH"]
        .into_iter()
        .find(|name| env.get(*name).is_some_and(|value| !value.trim().is_empty()))
        .map(str::to_string)
        .or_else(|| {
            crate::external::git_output(&["config", "--get", "core.sshCommand"], cwd)
                .map(|_| "core.sshCommand".to_string())
        })
}

/// Whether gh uses `https` for a remote it adds on the forge, read from the user's gh files as
/// gh reads them (`internal/config/config.go`: the host's `git_protocol`, then the top-level one,
/// then `https`). A launched gh gets the user's `hosts.yml` entries and `config.yml` without its
/// `hosts` key; plain gh, after a step-aside, reads a legacy `hosts` key in `config.yml` ahead of
/// `hosts.yml`, which go-gh adds as a second `hosts` entry. Both must say `https`. The forge's key
/// is its portless hostname, as gh's remotes have it. A file that can't be read counts as not
/// `https`.
fn gh_uses_https(env: &HashMap<String, String>, host: &str) -> bool {
    let Some(dir) = user_gh_config_dir(env) else {
        return true;
    };
    let read = |name: &str| -> Option<Option<serde_yaml::Mapping>> {
        match std::fs::read_to_string(dir.join(name)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Some(None),
            Err(_) => None,
            // gh's YAML reader returns an alias's own name (`git_protocol: *ssh` is `ssh`),
            // where serde_yaml returns the anchored value; a file using one counts as unread.
            Ok(text) if text.lines().any(yaml_alias_value) => None,
            Ok(text) => match serde_yaml::from_str::<serde_yaml::Value>(&text) {
                Ok(serde_yaml::Value::Mapping(mapping)) => Some(Some(mapping)),
                Ok(serde_yaml::Value::Null) => Some(None),
                _ => None,
            },
        }
    };
    let (Some(config), Some(hosts)) = (read("config.yml"), read("hosts.yml")) else {
        return false;
    };
    let hostname = gh_hostname(host).to_ascii_lowercase();
    let protocol = |entries: Option<&serde_yaml::Value>| {
        entries?
            .get(hostname.as_str())?
            .get("git_protocol")?
            .as_str()
            .map(str::to_string)
    };
    let global = config
        .as_ref()
        .and_then(|config| config.get("git_protocol"))
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_string);
    let hosts_yml = hosts.map(serde_yaml::Value::Mapping);
    let legacy = config.as_ref().and_then(|config| config.get("hosts"));
    let launched = protocol(hosts_yml.as_ref()).or_else(|| global.clone());
    let plain = match legacy {
        Some(legacy) => protocol(Some(legacy)),
        None => protocol(hosts_yml.as_ref()),
    }
    .or(global);
    [launched, plain]
        .iter()
        .all(|protocol| protocol.as_deref().unwrap_or("https") == "https")
}

/// Whether a YAML line uses an alias as a value: `key: *name`, `- *name`, or a flow form such as
/// `{key: *name}`. A false match only keeps the SSH check.
fn yaml_alias_value(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("- *")
        || line.match_indices(':').any(|(at, _)| {
            line[at + 1..]
                .trim_start_matches([' ', '\t'])
                .starts_with('*')
        })
}

/// gh 2.103.3's subcommands of `repo` and `pr`, with their aliases (`pkg/cmd/repo`, `pkg/cmd/pr`).
const REPO_SUBCOMMANDS: &[&str] = &[
    "archive",
    "autolink",
    "clone",
    "create",
    "new",
    "credits",
    "delete",
    "deploy-key",
    "edit",
    "fork",
    "gitignore",
    "license",
    "list",
    "ls",
    "read-dir",
    "read-file",
    "rename",
    "set-default",
    "sync",
    "unarchive",
    "view",
];
const PR_SUBCOMMANDS: &[&str] = &[
    "checkout",
    "co",
    "checks",
    "close",
    "comment",
    "create",
    "new",
    "diff",
    "edit",
    "list",
    "ls",
    "lock",
    "merge",
    "ready",
    "reopen",
    "review",
    "revert",
    "status",
    "unlock",
    "update-branch",
    "view",
];

/// The indices of the words cobra takes for command names, as `stripFlags` does (cobra v1.10.2,
/// `command.go`): `--` ends them, and a `--flag` without `=`, or a two-letter `-f`, takes the next
/// word as its value unless it's a boolean. At gh's root and group levels the booleans are
/// `--help`, `-h` and `--version`. `skip` is a word already taken for a command.
fn cobra_commands(words: &[String], skip: Option<usize>) -> Vec<usize> {
    let mut commands = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let at = index;
        let word = words[at].as_str();
        index += 1;
        if Some(at) == skip {
            continue;
        }
        if word == "--" {
            break;
        }
        let takes_value = (word.starts_with("--")
            && !word.contains('=')
            && !matches!(word, "--help" | "--version"))
            || (word.starts_with('-') && !word.contains('=') && word.len() == 2 && word != "-h");
        if takes_value {
            if Some(index) == skip {
                index += 1;
            }
            index += 1;
        } else if !word.is_empty() && !word.starts_with('-') {
            commands.push(at);
        }
    }
    commands
}

/// Where `group` (`repo` or `pr`) leads the call, the index of the group and of its subcommand,
/// found as cobra finds them: the first command word, then the first command word of the rest,
/// when it's one of the group's subcommands. So `gh repo --source x create` and
/// `gh --source x repo create` are `repo create`, and `gh pr view new` is `pr view`.
fn subcommand(words: &[String], group: &str) -> Option<(usize, usize)> {
    let at = *cobra_commands(words, None).first()?;
    if words[at] != group {
        return None;
    }
    let names = if group == "repo" {
        REPO_SUBCOMMANDS
    } else {
        PR_SUBCOMMANDS
    };
    let sub = *cobra_commands(words, Some(at)).first()?;
    names.contains(&words[sub].as_str()).then_some((at, sub))
}

/// Whether the call is `<group> create` or its alias `new`, as the parser or [`subcommand`] reads it.
fn creates(routed: &Invocation, words: &[String], group: &str) -> bool {
    routed.command == format!("{group} create")
        || subcommand(words, group)
            .is_some_and(|(_, sub)| words[sub] == "create" || words[sub] == "new")
}

/// Whether `gh repo create` pushes: with `--push` (pflag's bool, so not `--push=false`), or when
/// it prompts, which gh 2.103.3 does only with no argument and no flag (`create.go:126`); cobra
/// drops a `--`, so `gh repo create --` prompts too.
fn repo_create_pushes(words: &[String]) -> bool {
    let push = words.iter().any(|word| {
        word == "--push"
            || word.strip_prefix("--push=").is_some_and(|value| {
                !matches!(value, "0" | "f" | "F" | "false" | "FALSE" | "False")
            })
    });
    let prompts = subcommand(words, "repo").is_some_and(|(at, sub)| {
        words
            .iter()
            .enumerate()
            .all(|(index, word)| index == at || index == sub || word == "--")
    });
    push || prompts
}

/// Plain gh for a forge the user's own gh is logged in to (server mode, spec §2), when the call
/// didn't launch: the built-in path would answer with the shim's own token, or refuse a form
/// plain gh serves there, such as ChatGPT's `gh api graphql --hostname <forge>`. Only on the
/// terminal path, as a launch, and only when nothing in the call names another host.
fn step_aside_logged_in(
    routed: &Invocation,
    decision: &RouteDecision,
    words: &[String],
    argv: &[OsString],
    env: &HashMap<String, String>,
    config: &DispatcherConfig,
    mode: &DelegateMode<'_>,
) -> Option<i32> {
    #[cfg(unix)]
    if matches!(mode, DelegateMode::Inherit) {
        let host = decision.trace_host()?;
        // gh sends `api` and `auth` where routing says (the URL, `--hostname` or `GH_HOST`, or
        // decision 4's), and their other words are data, such as a GraphQL document.
        let targets_only = routed.command == "api" || routed.command.starts_with("auth ");
        let words = if targets_only { &[][..] } else { words };
        if names_another_host(host, words, env, config) {
            return None;
        }
        return crate::launcher::start::step_aside_if_logged_in(
            host,
            decision.gh_repo.as_deref(),
            argv,
            env,
            config.real_gh.as_deref(),
        );
    }
    let _ = (routed, decision, words, argv, env, config, mode);
    None
}

/// Whether plain gh could go somewhere other than `host`, judged from what the call says:
/// `GH_HOST` naming another host, or a word or `GH_REPO` naming one: another allowlisted forge
/// anywhere in it, or a URL, a `HOST/OWNER/REPO` or an SSH address (`host:o/r`, `git@host:o/r`)
/// whose host isn't `host`, or a `--hostname` value that isn't. Routing doesn't model every
/// argument gh takes a host from (`gh repo clone forge-b/o/r` in forge-a's checkout routes to
/// forge-a), so stepping aside on the routed host's login could send the call elsewhere (Sol
/// r1#1). Any `x:` counts as a host, whatever its
/// characters (Sol r3#1): a false match, such as a path `a/b/c` or `--search is:open`, leaves the
/// call to the built-in path, as before.
fn names_another_host(
    host: &str,
    words: &[String],
    env: &HashMap<String, String>,
    config: &DispatcherConfig,
) -> bool {
    let forge = gh_hostname(host).to_ascii_lowercase();
    let other = |name: &str| {
        let name = name.rsplit_once('@').map_or(name, |(_, name)| name);
        gh_hostname(name).to_ascii_lowercase() != forge
    };
    if env
        .get("GH_HOST")
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|value| other(value))
    {
        return true;
    }
    let forges = config
        .hosts
        .iter()
        .filter(|entry| config.is_forgejo_host(Some(entry)))
        .map(|entry| gh_hostname(entry).to_ascii_lowercase())
        .filter(|entry| *entry != forge)
        .collect::<Vec<_>>();
    // `--hostname` names a host on its own (`gh attestation download --hostname github.com`),
    // which none of the shapes below matches.
    let hostname_flag = words.iter().zip(words.iter().skip(1)).any(|(flag, value)| {
        flag == "--hostname" && !value.trim().is_empty() && other(value.trim())
    }) || words.iter().any(|word| {
        word.strip_prefix("--hostname=")
            .is_some_and(|value| !value.trim().is_empty() && other(value.trim()))
    });
    hostname_flag
        || words.iter().chain(env.get("GH_REPO")).any(|word| {
            let word = word.to_ascii_lowercase();
            let value = word
                .rsplit_once('=')
                .map_or(word.as_str(), |(_, value)| value);
            let parts = value.split('/').collect::<Vec<_>>();
            forges.iter().any(|entry| word.contains(entry.as_str()))
            || word
                .split("://")
                .skip(1)
                .any(|rest| other(rest.split(['/', '?', '#']).next().unwrap_or(rest)))
            // `host:o/r` or `git@host:o/r`; `://` is a URL's, above.
            || word.match_indices(':').any(|(colon, _)| {
                !word[colon + 1..].starts_with("//")
                    && other(word[..colon].rsplit(['@', '=']).next().unwrap_or_default())
            })
            || (parts.len() == 3 && parts.iter().all(|part| !part.is_empty()) && other(parts[0]))
        })
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
    fn gh_s_own_version_and_root_help_delegate_even_for_a_forgejo_repo() {
        // `-h pr` is gh's help for `pr`; the shim reads `-h` as a value flag, leaving no command.
        for args in [
            &["--version"][..],
            &["version"][..],
            &["--help"][..],
            &["-h", "pr"][..],
            &[][..],
        ] {
            for forgejo in [
                ("GH_REPO", "git.example.com/owner/repo"),
                ("GH_HOST", "git.example.com"),
            ] {
                let decision = decide_route(
                    &argv(args),
                    &config(&["git.example.com"]),
                    &env(&[forgejo]),
                    Some(Path::new("/nonexistent-gfjs-repo")),
                );
                assert_eq!(decision.kind, RouteKind::Delegate, "{args:?} {forgejo:?}");
                assert_eq!(decision.reason, "gh's root or version command", "{args:?}");
            }
        }
    }

    #[test]
    fn the_delegate_list_runs_plain_gh_even_for_a_forgejo_repo() {
        for args in [
            &["completion", "-s", "bash"][..],
            &["config", "get", "editor"][..],
            &["config", "set", "editor", "vim"][..],
            &["alias", "list"][..],
            &["alias", "set", "co", "pr checkout"][..],
            &["auth", "login"][..],
            &["auth", "login", "--hostname", "git.example.com"][..],
            &["auth", "logout"][..],
            &["auth", "switch"][..],
            &["auth", "refresh"][..],
            &["auth", "setup-git"][..],
            &["extension", "list"][..],
            &["ext", "install", "o/gh-x"][..],
            &["codespace", "list"][..],
            &["copilot", "suggest", "x"][..],
            &["skill", "list"][..],
            &["skills", "list"][..],
            &["agent-task", "list"][..],
            &["agent-tasks", "list"][..],
            &["agent", "list"][..],
            &["agents", "list"][..],
        ] {
            let decision = decide_route(
                &argv(args),
                &config(&["git.example.com"]),
                &env(&[("GH_REPO", "git.example.com/owner/repo")]),
                Some(Path::new("/nonexistent-gfjs-repo")),
            );
            assert_eq!(decision.kind, RouteKind::Delegate, "{args:?}");
            assert_eq!(decision.reason, "a command gh runs on its own", "{args:?}");
        }
        // Ordinary commands keep routing, and so does a help flag, which run_gh hands to plain gh
        // after the route is recorded.
        for args in [
            &["pr", "view", "--help"][..],
            &["pr", "view", "1", "-h"][..],
            &["auth", "token", "-h", "git.example.com"][..],
            &["auth", "status", "-h", "git.example.com"][..],
            &["pr", "view", "1"][..],
            &["issue", "list"][..],
        ] {
            let decision = decide_route(
                &argv(args),
                &config(&["git.example.com"]),
                &env(&[("GH_REPO", "git.example.com/owner/repo")]),
                Some(Path::new("/nonexistent-gfjs-repo")),
            );
            assert_eq!(decision.kind, RouteKind::Forgejo, "{args:?}");
        }
        // `help` routes like any call: an alias can take its name (Sol r5#1). A Forgejo repo keeps
        // the Forgejo path, which refuses it; with a forge only in reach it's refused outright.
        // Plain gh runs it only with no forge in reach.
        let help = |env_pairs: &[(&str, &str)]| {
            decide_route(
                &argv(&["help", "pr"]),
                &config(&["git.example.com"]),
                &env(env_pairs),
                Some(Path::new("/nonexistent-gfjs-repo")),
            )
            .kind
        };
        assert_eq!(
            help(&[("GH_REPO", "git.example.com/owner/repo")]),
            RouteKind::Forgejo
        );
        assert_eq!(
            help(&[
                ("GH_REPO", "github.com/o/r"),
                ("GH_HOST", "git.example.com")
            ]),
            RouteKind::Refuse
        );
        assert_eq!(help(&[]), RouteKind::Delegate);
    }

    #[test]
    fn help_runs_plain_gh_only_where_the_grammar_knows_the_flag() {
        let help = |args: &[&str]| runs_help(&Invocation::parse(&argv(args)));
        for args in [
            &["pr", "view", "--help"][..],
            &["pr", "view", "1", "-h"][..],
            &["issue", "list", "--help"][..],
            // Groups have no -h of their own.
            &["pr", "--help"][..],
            &["repo", "-h"][..],
            &["auth", "--help"][..],
        ] {
            assert!(help(args), "{args:?}");
        }
        for args in [
            // In the auth commands -h is --hostname.
            &["auth", "token", "-h", "git.example.com"][..],
            // A flag's value, and after --.
            &["pr", "create", "--title", "--help"][..],
            &["pr", "view", "--", "--help"][..],
            // No grammar: -h is --homepage there, and --help may be a value. These are writes.
            &["repo", "edit", "-h", "https://x.test"][..],
            &["repo", "create", "foo", "--public", "-h", "https://x.test"][..],
            &["release", "create", "v1", "--notes", "--help"][..],
        ] {
            assert!(!help(args), "{args:?}");
        }
    }

    #[test]
    fn every_command_with_captured_help_is_a_group_or_in_the_grammar() {
        // runs_help trusts captured help to mean the parser read the help flag right; a leaf
        // with captured help but no grammar would bring back repo edit -h URL as "help".
        let usage: serde_json::Value =
            serde_json::from_str(include_str!("../../../contract/usage-errors.json"))
                .expect("usage");
        let grammar = include_str!("../../../contract/tiers.toml");
        for (command, entry) in usage.as_object().expect("object") {
            let group = entry["commands"]
                .as_array()
                .is_some_and(|commands| !commands.is_empty());
            let listed = grammar.contains(&format!("[command.\"{command}\"]"));
            assert!(
                group || listed,
                "{command} has captured help but is neither a group nor in the grammar"
            );
        }
    }

    #[test]
    fn a_captured_call_takes_the_helper_context_from_its_own_environment() {
        let call = |marker: bool| {
            let mut env = env(&[
                ("FJ_SHIM_REAL_GH", "/nonexistent/gh"),
                ("HOME", "/nonexistent-home"),
            ]);
            if marker {
                env.insert(
                    crate::launcher::credentials::CREDENTIALS_VAR.to_string(),
                    String::new(),
                );
            }
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            let code = run_gh_captured(
                argv(&["auth", "git-credential", "--", "erase"]),
                env,
                Some(Path::new("/nonexistent-gfjs-repo")),
                &mut stdout,
                &mut stderr,
            );
            (code, String::from_utf8_lossy(&stderr).into_owned())
        };
        // With the marker, refused; without, never refused because an earlier call had one.
        let (code, stderr) = call(true);
        assert_eq!(code, 1, "{stderr}");
        assert!(
            stderr.contains("come only from the launcher's helper"),
            "{stderr}"
        );
        let (_, stderr) = call(false);
        assert!(
            !stderr.contains("come only from the launcher's helper"),
            "{stderr}"
        );
    }

    #[test]
    fn a_flag_before_the_command_keeps_the_forgejo_route() {
        // gh reads `config` as --notes's value and runs release create, a write: never plain gh.
        for args in [
            &["--notes", "config", "release", "create", "v1"][..],
            &["--notes", "version", "release", "create", "v1"][..],
            &["-R", "o/r", "config", "get", "editor"][..],
            &["--repo", "o/r", "pr", "view", "--help"][..],
        ] {
            let decision = decide_route(
                &argv(args),
                &config(&["git.example.com"]),
                &env(&[("GH_REPO", "git.example.com/owner/repo")]),
                Some(Path::new("/nonexistent-gfjs-repo")),
            );
            assert_eq!(decision.kind, RouteKind::Forgejo, "{args:?}");
        }
    }

    #[test]
    fn gh_s_default_host_and_unreadable_aliases() {
        // Sol r6#1: a forge alone in hosts.yml is gh's default host (go-gh's `defaultHost`), and
        // `gist create` posts there though GH_REPO names GitHub. With github.com there too, the
        // default is github.com and nothing changes. Sol r6#2: aliases the shim can't read make
        // any word that could be an alias refuse, forge or not.
        let dir = std::env::temp_dir().join(format!("gfj-r6-{}", std::process::id()));
        // A failed run leaves the directory; start from an empty one either way.
        let _ = std::fs::remove_dir_all(&dir);
        let route = |argv_words: &[&str], file: &str, text: &str, extra: &[(&str, &str)]| {
            std::fs::create_dir_all(&dir).expect("dir");
            for name in ["hosts.yml", "config.yml"] {
                let _ = std::fs::remove_file(dir.join(name));
            }
            if !file.is_empty() {
                std::fs::write(dir.join(file), text).expect("write");
            }
            let mut pairs = vec![
                ("GH_REPO", "github.com/o/r"),
                ("GH_CONFIG_DIR", dir.to_str().expect("utf-8")),
            ];
            pairs.extend_from_slice(extra);
            decide_route(
                &argv(argv_words),
                &config(&["git.example.com"]),
                &env(&pairs),
                Some(Path::new("/nonexistent-gfjs-repo")),
            )
            .kind
        };
        let gist = ["gist", "create", "-"];
        let forge_only = "git.example.com:\n    git_protocol: https\n";
        let both = "github.com:\n    user: u\ngit.example.com:\n    user: u\n";
        assert_eq!(route(&gist, "", "", &[]), RouteKind::Delegate);
        assert_eq!(
            route(&gist, "hosts.yml", forge_only, &[]),
            RouteKind::Refuse
        );
        assert_eq!(route(&gist, "hosts.yml", both, &[]), RouteKind::Delegate);
        assert_eq!(
            route(&gist, "hosts.yml", forge_only, &[("GH_HOST", "github.com")]),
            RouteKind::Delegate
        );
        assert_eq!(
            route(&gist, "hosts.yml", "[unclosed", &[]),
            RouteKind::Refuse
        );
        let repeated = "aliases:\n    help: release create v1 --notes x -R git.example.com/o/r\n    help: pr view\n";
        assert_eq!(
            route(&["help"], "config.yml", repeated, &[]),
            RouteKind::Refuse
        );
        assert_eq!(
            route(&["gfjs-alias"], "config.yml", repeated, &[]),
            RouteKind::Refuse
        );
        // gh's own commands can't be aliases, so they route as before.
        assert_eq!(
            route(&["gist", "list"], "config.yml", repeated, &[]),
            RouteKind::Delegate
        );
        assert_eq!(
            route(
                &["help"],
                "config.yml",
                "aliases:\n    co: pr checkout\n",
                &[]
            ),
            RouteKind::Delegate
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn an_alias_named_help_replaces_gh_s_help() {
        // cobra adds its help command after gh registers aliases, so `help` here is
        // `release create`, a write (Sol r4#2).
        let dir = std::env::temp_dir().join(format!("gfj-help-alias-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("config.yml"),
            "aliases:\n    help: release create v1\n",
        )
        .expect("config");
        let decision = decide_route(
            &argv(&["help"]),
            &config(&["git.example.com"]),
            &env(&[
                ("GH_REPO", "git.example.com/owner/repo"),
                ("GH_CONFIG_DIR", dir.to_str().expect("utf-8")),
            ]),
            Some(Path::new("/nonexistent-gfjs-repo")),
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
        assert_eq!(decision.kind, RouteKind::Forgejo, "{decision:?}");
    }

    #[test]
    fn an_alias_to_a_delegated_command_delegates() {
        // Launcher spec §11, test 14: `gh sg` with `sg: auth setup-git`.
        let dir = std::env::temp_dir().join(format!("gfj-alias-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("config.yml"),
            "aliases:\n    sg: auth setup-git\n    sl: skill list\n",
        )
        .expect("config");
        // gh's own name is skill; an alias can expand to it.
        let skill = decide_route(
            &argv(&["sl"]),
            &config(&["git.example.com"]),
            &env(&[
                ("GH_REPO", "git.example.com/owner/repo"),
                ("GH_CONFIG_DIR", dir.to_str().expect("utf-8")),
            ]),
            Some(Path::new("/nonexistent-gfjs-repo")),
        );
        assert_eq!(skill.kind, RouteKind::Delegate, "{skill:?}");
        let decision = decide_route(
            &argv(&["sg"]),
            &config(&["git.example.com"]),
            &env(&[
                ("GH_REPO", "git.example.com/owner/repo"),
                ("GH_CONFIG_DIR", dir.to_str().expect("utf-8")),
            ]),
            Some(Path::new("/nonexistent-gfjs-repo")),
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
        assert_eq!(decision.kind, RouteKind::Delegate);
        assert_eq!(decision.reason, "a command gh runs on its own");
    }

    #[test]
    fn a_yaml_alias_value_is_seen() {
        assert!(yaml_alias_value("git_protocol: *ssh"));
        assert!(yaml_alias_value("    git_protocol:  *proto"));
        assert!(yaml_alias_value("<<: *defaults"));
        assert!(yaml_alias_value("- *item"));
        assert!(yaml_alias_value("forge.test: {git_protocol: *ssh}"));
        assert!(!yaml_alias_value("saved: &ssh https"));
        assert!(!yaml_alias_value("git_protocol: https"));
        assert!(!yaml_alias_value("    co: \"!git log && echo *\""));
    }

    #[test]
    fn repo_create_pushes_with_push_or_a_prompt() {
        let pushes = |words: &[&str]| repo_create_pushes(&argv(words));
        assert!(!pushes(&["repo", "create", "o/r", "--private"]));
        assert!(pushes(&["repo", "create", "o/r", "--push", "-s=../src"]));
        assert!(pushes(&["repo", "new", "o/r", "--push=true"]));
        assert!(pushes(&[
            "repo", "--source", "x", "create", "o/r", "--push"
        ]));
        assert!(!pushes(&[
            "repo",
            "create",
            "--source",
            "../src",
            "--push=false"
        ]));
        // gh prompts only with no argument and no flag.
        assert!(pushes(&["repo", "create"]));
        assert!(pushes(&["repo", "new"]));
        assert!(pushes(&["repo", "create", "--"]));
        assert!(!pushes(&["repo", "create", "--private"]));
        assert!(!pushes(&["repo", "create", "--", "o/r"]));
    }

    #[test]
    fn the_subcommand_is_the_first_of_the_group_s() {
        let sub = |words: &[&str], group: &str| {
            let words = argv(words);
            subcommand(&words, group).map(|(_, sub)| words[sub].clone())
        };
        assert_eq!(sub(&["pr", "view", "new"], "pr").as_deref(), Some("view"));
        assert_eq!(
            sub(&["repo", "--source", "x", "create", "o/r"], "repo").as_deref(),
            Some("create")
        );
        assert_eq!(sub(&["issue", "create", "--title", "pr new"], "pr"), None);
        assert_eq!(
            sub(&["pr", "new", "--title", "t"], "pr").as_deref(),
            Some("new")
        );
        // A flag's value is never a command name, before the group or after it (parallax r5#2).
        assert_eq!(
            sub(&["repo", "--source", "view", "create", "o/r"], "repo").as_deref(),
            Some("create")
        );
        assert_eq!(
            sub(&["--source", "../src", "repo", "create", "o/r"], "repo").as_deref(),
            Some("create")
        );
        assert_eq!(
            sub(&["--help", "repo", "create"], "repo").as_deref(),
            Some("create")
        );
        assert_eq!(sub(&["repo", "--", "create"], "repo"), None);
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

    #[test]
    fn api_repo_placeholders_match_as_gh_s_pattern_does() {
        let uses = |args: &[&str]| uses_repo_placeholder(&Invocation::parse(&argv(args)));
        for args in [
            &["api", "repos/{owner}/{repo}/pulls"][..],
            &["api", "repos/:owner/:repo"],
            &["api", "graphql", "-F", "name={repo}"],
            &["api", "repos/:owner/x"],
        ] {
            assert!(uses(args), "{args:?}");
        }
        for args in [
            &["api", "repos/o/r/branches/{branch}"][..],
            &["api", "graphql", "-f", "query=query($owner:String!){x(owner:$owner){id}}"],
            &["api", "repos/:owners/x"],
            &["api", "user"],
        ] {
            assert!(!uses(args), "{args:?}");
        }
    }
}
