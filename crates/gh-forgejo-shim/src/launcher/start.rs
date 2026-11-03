//! From a routing decision to a running launch (launcher spec §2, §3, §6, §7).

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use fga_core::map::WipPrefixes;
use url::Url;

use crate::auth;
use crate::config::{self, Config, EnvMap};
use crate::external::find_program;
use crate::invocation::Invocation;
use crate::launcher::credentials::git_config_parameters;
use crate::launcher::env::Edit;
use crate::launcher::gateway::GatewayHost;
use crate::launcher::ghconfig::{self, ServedHost, ServedToken};
use crate::launcher::run::{self, Edits, Files, Launch};
use crate::launcher::rundir;
use crate::repo::{self, gh_hostname};

/// The token variables gh would prefer over `hosts.yml` (spec fact 3). A launched gh never sees
/// them: `GH_ENTERPRISE_TOKEN` would otherwise reach the forge.
const TOKEN_VARIABLES: [&str; 4] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

/// Launches real gh for `host`, or returns `None` when this host can't be launched yet and the
/// built-in path should serve the call: a custom CA, or an `api_url` with a path, until the
/// config bridge (L4) gives the gateway those settings.
pub fn launch(
    host: &str,
    routed: &Invocation,
    reason: &str,
    args: &[OsString],
    env: &HashMap<String, String>,
    real_gh: Option<&Path>,
) -> Option<i32> {
    let Plan {
        real_token,
        expect_real,
        all_hosts,
        look_up_logins,
        mut extra_edits,
    } = Plan::for_invocation(routed, host, env);
    let env_map: EnvMap = env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let home = env
        .get("HOME")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let config =
        match config::load_config_with_env(Some(&config::config_path(home.as_deref())), &env_map) {
            Ok(config) => config,
            Err(error) => return Some(fail(1, &error.to_string())),
        };
    // The user's gh files, read once and never written (§3).
    let user_dir = repo::user_gh_config_dir(env);
    let read = |name: &str| -> Result<Option<String>, String> {
        let Some(dir) = &user_dir else {
            return Ok(None);
        };
        let path = dir.join(name);
        match fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    };
    let user_config = match read("config.yml") {
        Ok(text) => text,
        Err(message) => return Some(fail(1, &message)),
    };
    // A file gh couldn't read either stops the launch, naming it (§3, §6).
    if let Err(error) = ghconfig::config_yml(user_config.as_deref(), Path::new("/s")) {
        let path = user_dir
            .as_ref()
            .map(|dir| dir.join("config.yml"))
            .unwrap_or_default();
        return Some(fail(1, &format!("{}: {error}", path.display())));
    }
    // An unreadable or unparseable hosts.yml stops the launch too: its hosts need placeholders.
    let user_hosts = match read("hosts.yml").and_then(|text| {
        ghconfig::user_hosts(text.as_deref()).map_err(|error| {
            let path = user_dir
                .as_ref()
                .map(|dir| dir.join("hosts.yml"))
                .unwrap_or_default();
            format!("{}: {error}", path.display())
        })
    }) {
        Ok(hosts) => hosts,
        Err(message) => return Some(fail(1, &message)),
    };

    // A forge the user's own gh is logged in to serves gh itself (server mode): plain gh (§2).
    if logged_in(&user_hosts, host) {
        let gh = match find_real_gh(real_gh, env) {
            Ok(gh) => gh,
            Err(code) => return Some(code),
        };
        return Some(step_aside(
            &gh,
            args,
            extra_edits,
            crate::trace::tracing_enabled(env),
        ));
    }
    let forgejo_url = upstream_url(host, &config, env)?;
    let token = auth::resolve_token(host, &config, &env_map, home.as_deref(), Some(platform()));
    // `auth status` keeps its current answer for a host with no token (§2), the built-in path's
    // "Not logged in", unless it's host-less and other hosts have tokens: S is every allowlist
    // host that has one (§3).
    if token.is_none() && routed.command == "auth status" && !all_hosts {
        return None;
    }

    let gh = match find_real_gh(real_gh, env) {
        Ok(gh) => gh,
        Err(code) => return Some(code),
    };
    // Plain gh's default host is this forge, but its entry holds no login gh can use (§3).
    if reason.ends_with("user's gh hosts") {
        let key = user_hosts
            .keys()
            .filter_map(serde_yaml::Value::as_str)
            .next()
            .unwrap_or(host);
        let file = user_dir
            .as_ref()
            .map(|dir| dir.join("hosts.yml"))
            .unwrap_or_default();
        return Some(fail(
            1,
            &format!(
                "{} makes {key} gh's default host, but gh has no login it can use there; delete the {key} entry from that file to use gh-forgejo-shim there",
                file.display()
            ),
        ));
    }

    if token.is_none() && routed.command != "auth status" {
        return Some(fail(
            4,
            &format!("no token for {host}; run: gfj auth login {host}"),
        ));
    }

    let (parent, source) = rundir::parent(env);
    if let Err(message) = rundir::check_parent(&parent, source) {
        return Some(fail(1, &message));
    }

    let per_call = match rundir::random_hex(32) {
        Ok(token) => token,
        Err(error) => return Some(fail(1, &format!("could not make a call token: {error}"))),
    };
    // S, the served hosts (§3): the routed host, or for a host-less `auth status` every
    // allowlist entry with a token, except entries sharing a hostname, forges the user's gh is
    // logged in to, and settings the gateway can't honour yet.
    let routed_entry = config::normalize_host(host);
    let mut served_hosts = Vec::new();
    if let Some(token) = &token {
        served_hosts.push((routed_entry.clone(), token.token.clone(), forgejo_url));
    }
    if all_hosts {
        for entry in config
            .hosts
            .iter()
            .map(|entry| config::normalize_host(entry))
        {
            let shared = config
                .hosts
                .iter()
                .filter(|other| gh_hostname(other) == gh_hostname(&entry))
                .count()
                > 1;
            if entry == routed_entry || shared || logged_in(&user_hosts, &entry) {
                continue;
            }
            let (Some(url), Some(found)) = (
                upstream_url(&entry, &config, env),
                auth::resolve_token(&entry, &config, &env_map, home.as_deref(), Some(platform())),
            ) else {
                continue;
            };
            served_hosts.push((entry, found.token, url));
        }
    }
    // A host-less `auth status` whose hosts all lack tokens keeps the built-in answer.
    if served_hosts.is_empty() {
        return None;
    }
    let served: Vec<ServedHost> = served_hosts
        .iter()
        .map(|(entry, real, _)| ServedHost {
            entry: entry.clone(),
            real_token: real.clone(),
            login: None,
        })
        .collect();
    let gateway: Vec<GatewayHost> = served_hosts
        .into_iter()
        .map(|(entry, real, forgejo_url)| GatewayHost {
            entry,
            forgejo_url,
            // `auth status --show-token` presents the real token, which it prints (§3, §5).
            expected: if expect_real {
                real.clone()
            } else {
                per_call.clone()
            },
            upstream: real,
            answer_portless: true,
            wip_prefixes: WipPrefixes::default(),
        })
        .collect();
    let allowlist = config.hosts.clone();
    let shim = std::env::current_exe().unwrap_or_default();
    // Every `--web` open and `gh browse` goes through the browser helper (§9).
    let served_entries: Vec<String> = served.iter().map(|host| host.entry.clone()).collect();
    extra_edits.push((
        "GH_BROWSER",
        Edit::Set(crate::launcher::browser::gh_browser(&shim, &served_entries)),
    ));
    let credentials = ghconfig::credentials(&served);

    Some(run::run(Launch {
        gh,
        args: args.to_vec(),
        parent,
        sweep_parents: rundir::sweep_parents(env),
        gateway,
        files: Box::new(move |run_dir| {
            let socket = run_dir.join("s");
            Files {
                // Git's credentials: the launcher's own helper answers every git in the call (§3).
                credentials,
                gh_script: ghconfig::gh_script(run_dir, &shim),
                // Checked above, so this only fails on a non-UTF-8 socket path, which §4's
                // path check rules out.
                config_yml: ghconfig::config_yml(user_config.as_deref(), &socket)
                    .unwrap_or_default(),
            }
        }),
        hosts_yml: Box::new(move |logins| {
            let served: Vec<ServedHost> = served
                .into_iter()
                .zip(logins)
                .map(|(host, login)| ServedHost {
                    login: login.clone(),
                    ..host
                })
                .collect();
            ghconfig::hosts_yml(
                &user_hosts,
                &allowlist,
                &served,
                if real_token {
                    ServedToken::Real
                } else {
                    ServedToken::PerCall(&per_call)
                },
            )
        }),
        look_up_logins,
        edits: Box::new(move |run_dir| {
            let mut edits = launched_edits(run_dir);
            edits.extend(extra_edits);
            edits
        }),
    }))
}

/// Real gh, or §6's exit code: 126 for a configured path that exists but can't run (`~` expanded
/// as discovery does), 127 when there is none.
fn find_real_gh(real_gh: Option<&Path>, env: &HashMap<String, String>) -> Result<PathBuf, i32> {
    if let Some(gh) = find_program("gh", real_gh, env) {
        return Ok(gh);
    }
    if let Some(path) = real_gh.map(|path| expand_home(path, env)) {
        if path.exists() && !is_executable_file(&path) {
            return Err(fail(
                126,
                &format!("the real gh at {} is not executable", path.display()),
            ));
        }
    }
    Err(fail(
        127,
        "could not find the real gh executable; set FJ_SHIM_REAL_GH",
    ))
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// `~` and `~/…` against `HOME`, as `external::find_program` reads a configured path.
fn expand_home(path: &Path, env: &HashMap<String, String>) -> PathBuf {
    let value = path.to_string_lossy();
    match (value.strip_prefix('~'), env.get("HOME")) {
        (Some(""), Some(home)) => PathBuf::from(home),
        (Some(rest), Some(home)) if rest.starts_with('/') => Path::new(home).join(&rest[1..]),
        _ => path.to_path_buf(),
    }
}

/// The forge's own base URL, or `None` while the gateway can't honour this host's settings.
fn upstream_url(host: &str, config: &Config, env: &HashMap<String, String>) -> Option<Url> {
    // gh can't use an IPv6 host, and routing declines them (L2); never launch one.
    if config::normalize_host(host).starts_with('[') {
        return None;
    }
    let settings = config.host.get(&config::normalize_host(host));
    let custom_ca = settings.is_some_and(|settings| settings.ca_file.is_some())
        || env
            .get("FJ_SHIM_CA_FILE")
            .is_some_and(|path| !path.is_empty());
    if custom_ca {
        return None;
    }
    let base = settings
        .and_then(|settings| settings.api_url.clone())
        .unwrap_or_else(|| format!("https://{}", config::normalize_host(host)));
    let url = Url::parse(&format!("{}/", base.trim_end_matches('/'))).ok()?;
    // fga-core joins absolute `/api/v1/...` paths, which would drop a path prefix.
    (url.path() == "/").then_some(url)
}

/// Whether the user's own gh is logged in to `host`: an `oauth_token` or a `user` under the key
/// gh's API client looks up, the hostname without its port, spelled exactly (§2). A login stored
/// only under `host:port` doesn't count, since plain gh's requests would never find it.
fn logged_in(user_hosts: &serde_yaml::Mapping, host: &str) -> bool {
    let has = |entry: &serde_yaml::Value, field: &str| {
        entry
            .get(field)
            .and_then(serde_yaml::Value::as_str)
            .is_some_and(|value| !value.is_empty())
    };
    user_hosts
        .get(gh_hostname(host).as_str())
        .is_some_and(|entry| has(entry, "oauth_token") || has(entry, "user"))
}

/// Runs plain gh for a forge that serves gh itself (§2): the user's environment, but for a
/// host-less `api` or `auth token` the forge as `GH_HOST`, without the token variables, which
/// would otherwise beat the user's login there: the enterprise ones for most forges, `GH_TOKEN`
/// and `GITHUB_TOKEN` for `github.localhost` and `*.ghe.com` (§2). Recorded for nested calls
/// (§7), with no `FJ_SHIM_LAUNCHER`. As on the delegate path, gh replaces this process, unless
/// tracing is on, when the wrapper waits so it can write the trace record.
fn step_aside(gh: &Path, args: &[OsString], mut edits: Edits, trace_enabled: bool) -> i32 {
    if edits.iter().any(|(name, _)| *name == "GH_HOST") {
        edits.extend(TOKEN_VARIABLES.iter().map(|name| (*name, Edit::Remove)));
    }
    // Still single-threaded: nothing in the wrapper has started a thread yet.
    crate::launcher::env::apply(&edits);
    let mut command = std::process::Command::new(gh);
    command.args(args);
    let error = if trace_enabled {
        match command.status() {
            Ok(status) => return crate::external::inherited_exit_code(status),
            Err(error) => error,
        }
    } else {
        use std::os::unix::process::CommandExt;
        command.exec()
    };
    let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
        126
    } else {
        127
    };
    fail(code, &format!("could not run {}: {error}", gh.display()))
}

/// What one invocation needs beyond every launch's settings.
struct Plan {
    /// `gh auth token` and `gh auth status --show-token` print the token from `hosts.yml`, so
    /// it holds the real one (§3).
    real_token: bool,
    /// `--show-token`, which also calls the API: the gateway expects the real token (§5).
    expect_real: bool,
    /// A host-less `auth status`: every eligible allowlist host is served (§3).
    all_hosts: bool,
    /// `auth status` shows each account, from `user` in `hosts.yml` (§3).
    look_up_logins: bool,
    extra_edits: Edits,
}

impl Plan {
    fn for_invocation(routed: &Invocation, host: &str, env: &HashMap<String, String>) -> Self {
        let command = routed.command.as_str();
        let mut extra_edits = Edits::new();
        // Decision 4 (§2): host-less `gh api` and `gh auth token` use gh's default host, so the
        // forge becomes it. Portless, because gh drops the port from remotes before comparing
        // them with GH_HOST. A user GH_HOST with that value already stays as it is.
        if matches!(command, "api" | "auth token") && routed.flag("hostname").is_none() {
            let hostname = gh_hostname(host);
            if env.get("GH_HOST") != Some(&hostname) {
                extra_edits.push(("GH_HOST", Edit::Set(hostname.into())));
            }
        }
        let status = command == "auth status";
        // pflag's ParseBool, last occurrence, after alias expansion (§3).
        let show_token = status && routed.flag("show-token") == Some("true");
        Self {
            real_token: command == "auth token" || show_token,
            expect_real: show_token,
            all_hosts: status && routed.flag("hostname").is_none(),
            look_up_logins: status,
            extra_edits,
        }
    }
}

/// What a launch changes in gh's environment (§7). Everything else passes through untouched.
fn launched_edits(run_dir: &Path) -> Edits {
    let inherited_git_config = std::env::var_os("GIT_CONFIG_PARAMETERS");
    let mut edits: Edits = vec![
        ("GH_CONFIG_DIR", Edit::Set(run_dir.as_os_str().to_owned())),
        ("GH_PATH", Edit::Set(run_dir.join("gh").into_os_string())),
        (
            "GIT_CONFIG_PARAMETERS",
            Edit::Set(git_config_parameters(
                inherited_git_config.as_deref(),
                run_dir,
            )),
        ),
        ("GH_NO_UPDATE_NOTIFIER", Edit::Set("1".into())),
        ("GH_NO_EXTENSION_UPDATE_NOTIFIER", Edit::Set("1".into())),
    ];
    edits.extend(TOKEN_VARIABLES.iter().map(|name| (*name, Edit::Remove)));
    edits
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    }
}

fn fail(code: i32, message: &str) -> i32 {
    eprintln!("gh-forgejo-shim: {message}");
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml: &str) -> Config {
        let dir = std::env::temp_dir().join(format!(
            "gfj-start-{}",
            rundir::random_hex(8).expect("random")
        ));
        fs::create_dir(&dir).expect("dir");
        let path = dir.join("config.toml");
        fs::write(&path, toml).expect("config");
        let config = config::load_config_with_env(Some(&path), &EnvMap::new()).expect("load");
        fs::remove_dir_all(&dir).expect("cleanup");
        config
    }

    #[test]
    fn upstream_is_the_forge_or_its_root_api_url() {
        let none = HashMap::new();
        let plain = config("hosts = [\"forge.test\"]\n");
        assert_eq!(
            upstream_url("forge.test", &plain, &none).map(String::from),
            Some("https://forge.test/".to_string())
        );
        let api = config("hosts = [\"forge.test\"]\n[host.\"forge.test\"]\napi_url = \"http://127.0.0.1:3000\"\n");
        assert_eq!(
            upstream_url("forge.test", &api, &none).map(String::from),
            Some("http://127.0.0.1:3000/".to_string())
        );
    }

    #[test]
    fn settings_the_gateway_lacks_keep_the_built_in_path() {
        let none = HashMap::new();
        let prefixed = config("hosts = [\"forge.test\"]\n[host.\"forge.test\"]\napi_url = \"http://127.0.0.1:3000/forgejo\"\n");
        assert_eq!(upstream_url("forge.test", &prefixed, &none), None);
        let ca =
            config("hosts = [\"forge.test\"]\n[host.\"forge.test\"]\nca_file = \"/etc/ca.pem\"\n");
        assert_eq!(upstream_url("forge.test", &ca, &none), None);
        let plain = config("hosts = [\"forge.test\"]\n");
        let env = HashMap::from([("FJ_SHIM_CA_FILE".to_string(), "/etc/ca.pem".to_string())]);
        assert_eq!(upstream_url("forge.test", &plain, &env), None);
    }

    fn plan(args: &[&str], env: &[(&str, &str)]) -> Plan {
        let argv: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let env: HashMap<String, String> = env
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        Plan::for_invocation(&Invocation::parse(&argv), "forge.test:3443", &env)
    }

    #[test]
    fn host_less_api_and_auth_token_get_the_forges_portless_gh_host() {
        let gh_host = Edit::Set("forge.test".into());
        for args in [&["api", "user"][..], &["auth", "token"][..]] {
            assert_eq!(
                plan(args, &[]).extra_edits,
                vec![("GH_HOST", gh_host.clone())],
                "{args:?}"
            );
            assert!(
                plan(args, &[("GH_HOST", "forge.test")])
                    .extra_edits
                    .is_empty(),
                "kept"
            );
            assert_eq!(
                plan(args, &[("GH_HOST", "other.test")]).extra_edits,
                vec![("GH_HOST", gh_host.clone())]
            );
        }
        assert!(plan(&["api", "user", "--hostname", "forge.test"], &[])
            .extra_edits
            .is_empty());
        assert!(plan(&["auth", "token", "-h", "forge.test"], &[])
            .extra_edits
            .is_empty());
        assert!(plan(&["pr", "view", "1"], &[]).extra_edits.is_empty());
    }

    #[test]
    fn only_auth_token_and_show_token_put_the_real_token_in_hosts_yml() {
        assert!(plan(&["auth", "token"], &[]).real_token);
        assert!(!plan(&["api", "user"], &[]).real_token);
        assert!(!plan(&["pr", "view", "1"], &[]).real_token);
        assert!(!plan(&["auth", "status"], &[]).real_token);
    }

    #[test]
    fn show_token_is_read_as_pflag_reads_it() {
        for shown in [
            &["auth", "status", "-t"][..],
            &["auth", "status", "-at"][..],
            &["auth", "status", "-ta"][..],
            &["auth", "status", "--show-token"][..],
            &["auth", "status", "--show-token=1"][..],
            &["auth", "status", "--show-token=t"][..],
        ] {
            let plan = plan(shown, &[]);
            assert!(plan.real_token && plan.expect_real, "{shown:?}");
        }
        for hidden in [
            &["auth", "status"][..],
            &["auth", "status", "--show-token=false"][..],
            &["auth", "status", "--show-token=0"][..],
            &["auth", "status", "-t", "--show-token=false"][..],
        ] {
            let plan = plan(hidden, &[]);
            assert!(!plan.real_token && !plan.expect_real, "{hidden:?}");
        }
    }

    #[test]
    fn host_less_auth_status_serves_every_host_and_looks_up_logins() {
        let host_less = plan(&["auth", "status"], &[]);
        assert!(host_less.all_hosts && host_less.look_up_logins);
        let one = plan(&["auth", "status", "--hostname", "forge.test"], &[]);
        assert!(!one.all_hosts && one.look_up_logins);
        assert!(!plan(&["pr", "view", "1"], &[]).look_up_logins);
    }

    #[test]
    fn an_ipv6_host_keeps_the_built_in_path() {
        let plain = config("hosts = [\"forge.test\"]\n");
        assert_eq!(upstream_url("[::1]:3000", &plain, &HashMap::new()), None);
    }

    #[test]
    fn launched_edits_point_gh_at_the_run_directory_and_drop_token_variables() {
        let edits = launched_edits(Path::new("/t/gfj-x"));
        let edit = |name: &str| {
            edits
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, edit)| edit.clone())
        };
        assert_eq!(edit("GH_CONFIG_DIR"), Some(Edit::Set("/t/gfj-x".into())));
        assert_eq!(edit("GH_PATH"), Some(Edit::Set("/t/gfj-x/gh".into())));
        for token in TOKEN_VARIABLES {
            assert_eq!(edit(token), Some(Edit::Remove), "{token}");
        }
        assert!(
            matches!(edit("GIT_CONFIG_PARAMETERS"), Some(Edit::Set(value))
            if value.to_string_lossy().ends_with("'credential.helper=!/t/gfj-x/gh auth git-credential'"))
        );
        assert_eq!(edit("PATH"), None, "nothing else is touched");
    }
}
