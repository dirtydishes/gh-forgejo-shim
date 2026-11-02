//! From a routing decision to a running launch (launcher spec §2, §3, §6, §7).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use fga_core::map::WipPrefixes;
use url::Url;

use crate::auth;
use crate::config::{self, Config, EnvMap};
use crate::external::find_program;
use crate::launcher::gateway::GatewayHost;
use crate::launcher::ghconfig::{self, ServedHost, ServedToken};
use crate::launcher::run::{self, Environment, Files, Launch};
use crate::launcher::rundir;
use crate::repo;

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
    args: &[OsString],
    env: &HashMap<String, String>,
    real_gh: Option<&Path>,
) -> Option<i32> {
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
    let forgejo_url = upstream_url(host, &config, env)?;

    let Some(gh) = find_program("gh", real_gh, env) else {
        return Some(fail(
            127,
            "could not find the real gh executable; set FJ_SHIM_REAL_GH",
        ));
    };
    let Some(token) =
        auth::resolve_token(host, &config, &env_map, home.as_deref(), Some(platform()))
    else {
        return Some(fail(
            4,
            &format!("no token for {host}; run: gfj auth login {host}"),
        ));
    };

    let (parent, source) = rundir::parent(env);
    if let Err(message) = rundir::check_parent(&parent, source) {
        return Some(fail(1, &message));
    }

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

    let per_call = match rundir::random_hex(32) {
        Ok(token) => token,
        Err(error) => return Some(fail(1, &format!("could not make a call token: {error}"))),
    };
    let served = vec![ServedHost {
        entry: config::normalize_host(host),
        real_token: token.token.clone(),
        login: None,
    }];
    let allowlist = config.hosts.clone();
    let base_env = env_os();
    let launcher_pid = std::process::id().to_string();
    let depth = (crate::launcher::depth(env) + 1).to_string();

    let gateway = vec![GatewayHost {
        entry: config::normalize_host(host),
        forgejo_url,
        expected: per_call.clone(),
        upstream: token.token,
        answer_portless: true,
        wip_prefixes: WipPrefixes::default(),
    }];

    Some(run::run(Launch {
        gh,
        args: args.to_vec(),
        parent,
        sweep_parents: rundir::sweep_parents(env),
        gateway,
        files: Box::new(move |run_dir| {
            let socket = run_dir.join("s");
            Files {
                hosts_yml: ghconfig::hosts_yml(
                    &user_hosts,
                    &allowlist,
                    &served,
                    ServedToken::PerCall(&per_call),
                ),
                // Git's credentials come with the launcher's helper (L3's second change). Until
                // then nothing reads these, so the real token isn't written at all.
                credentials: String::new(),
                gh_script: String::new(),
                // Checked above, so this only fails on a non-UTF-8 socket path, which §4's
                // path check rules out.
                config_yml: ghconfig::config_yml(user_config.as_deref(), &socket)
                    .unwrap_or_default(),
            }
        }),
        env: Box::new(move |run_dir| launched_env(base_env, run_dir, &launcher_pid, &depth)),
    }))
}

/// The forge's own base URL, or `None` while the gateway can't honour this host's settings.
fn upstream_url(host: &str, config: &Config, env: &HashMap<String, String>) -> Option<Url> {
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

/// This process's environment, byte for byte (R7).
fn env_os() -> Environment {
    std::env::vars_os().collect()
}

/// gh's environment for a launch: the wrapper's own, without the token variables, with the run
/// directory as gh's config directory. The §7 bookkeeping for nested calls comes with the
/// credential helper (L3's second change).
fn launched_env(
    mut env: Environment,
    run_dir: &Path,
    launcher_pid: &str,
    depth: &str,
) -> Environment {
    let set: [(&str, &OsStr); 5] = [
        ("GH_CONFIG_DIR", run_dir.as_os_str()),
        ("GH_NO_UPDATE_NOTIFIER", OsStr::new("1")),
        ("GH_NO_EXTENSION_UPDATE_NOTIFIER", OsStr::new("1")),
        ("FJ_SHIM_LAUNCHER", OsStr::new(launcher_pid)),
        ("FJ_SHIM_DEPTH", OsStr::new(depth)),
    ];
    env.retain(|(name, _)| {
        !TOKEN_VARIABLES.iter().any(|token| name == token)
            && !set.iter().any(|(set_name, _)| name == set_name)
    });
    env.extend(
        set.iter()
            .map(|(name, value)| (OsString::from(name), value.to_os_string())),
    );
    env
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

    #[test]
    fn launched_env_drops_token_variables_and_points_gh_at_the_run_directory() {
        let env = vec![
            (OsString::from("PATH"), OsString::from("/bin")),
            (OsString::from("GH_ENTERPRISE_TOKEN"), OsString::from("ghe")),
            (OsString::from("GH_TOKEN"), OsString::from("gh")),
            (OsString::from("GH_CONFIG_DIR"), OsString::from("/user/gh")),
        ];
        let out = launched_env(env, Path::new("/t/gfj-x"), "42", "3");
        let get = |name: &str| {
            out.iter()
                .filter(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(get("PATH"), [OsString::from("/bin")]);
        assert!(get("GH_ENTERPRISE_TOKEN").is_empty());
        assert!(get("GH_TOKEN").is_empty());
        assert_eq!(get("GH_CONFIG_DIR"), [OsString::from("/t/gfj-x")]);
        assert_eq!(get("FJ_SHIM_LAUNCHER"), [OsString::from("42")]);
        assert_eq!(get("FJ_SHIM_DEPTH"), [OsString::from("3")]);
    }
}
