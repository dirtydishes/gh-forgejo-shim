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
use crate::launcher::credentials::git_config_parameters;
use crate::launcher::env::Edit;
use crate::launcher::gateway::GatewayHost;
use crate::launcher::ghconfig::{self, ServedHost, ServedToken};
use crate::launcher::run::{self, Edits, Files, Launch};
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
        // Discovery skips a file it can't run; a configured one that exists is 126, not 127 (§6).
        if let Some(path) = real_gh.filter(|path| path.is_file() && !is_executable(path)) {
            return Some(fail(
                126,
                &format!("the real gh at {} is not executable", path.display()),
            ));
        }
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
    let shim = std::env::current_exe().unwrap_or_default();

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
                // Git's credentials: the launcher's own helper answers every git in the call (§3).
                credentials: ghconfig::credentials(&served),
                gh_script: ghconfig::gh_script(run_dir, &shim),
                // Checked above, so this only fails on a non-UTF-8 socket path, which §4's
                // path check rules out.
                config_yml: ghconfig::config_yml(user_config.as_deref(), &socket)
                    .unwrap_or_default(),
            }
        }),
        edits: Box::new(launched_edits),
    }))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
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
