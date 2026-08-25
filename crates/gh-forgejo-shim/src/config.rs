//! Configuration loading and host allowlisting.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub use crate::provider::{is_known_github_host, normalize_host};
use crate::provider::{normalize_transport_host, HostProfile, HostRegistry};
use crate::{Result, ShimError};

pub type EnvMap = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathsConfig {
    pub gh: Option<String>,
    pub fj: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub hosts: Vec<String>,
    pub paths: PathsConfig,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    pub registry: HostRegistry,
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    hosts: Option<Vec<String>>,
    #[serde(default)]
    host_profiles: Vec<RawHostProfile>,
    paths: Option<RawPathsConfig>,
}

#[derive(Debug, Deserialize)]
struct RawHostProfile {
    canonical_host: String,
    #[serde(default)]
    aliases: Vec<String>,
    api_root: Option<String>,
    credential_host: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawPathsConfig {
    gh: Option<String>,
    fj: Option<String>,
}

impl Config {
    pub fn is_forgejo_host(&self, host: Option<&str>) -> bool {
        let Some(host) = host else {
            return false;
        };
        let normalized = normalize_host(host);
        if is_known_github_host(Some(&normalized)) {
            return false;
        }
        self.hosts
            .iter()
            .any(|configured| normalize_host(configured) == normalized)
    }
}

pub fn current_env() -> EnvMap {
    std::env::vars().collect()
}

pub fn config_dir(home: Option<&Path>) -> PathBuf {
    home_dir(home).join(".config").join("gh-forgejo-shim")
}

pub fn config_path(home: Option<&Path>) -> PathBuf {
    config_dir(home).join("config.toml")
}

pub fn load_config() -> Result<Config> {
    let env = current_env();
    load_config_with_env(None, &env)
}

pub fn load_config_at(path: &Path) -> Result<Config> {
    let env = current_env();
    load_config_with_env(Some(path), &env)
}

pub fn load_config_with_env(path: Option<&Path>, env: &EnvMap) -> Result<Config> {
    Ok(load_runtime_config_with_env(path, env)?.config)
}

pub fn load_host_registry_with_env(path: Option<&Path>, env: &EnvMap) -> Result<HostRegistry> {
    Ok(load_runtime_config_with_env(path, env)?.registry)
}

pub fn load_runtime_config_with_env(path: Option<&Path>, env: &EnvMap) -> Result<LoadedConfig> {
    let config_path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config_path(None));

    let raw = read_raw_config(&config_path)?;

    let mut hosts = raw
        .hosts
        .unwrap_or_default()
        .into_iter()
        .map(|host| normalized_forgejo_host(&host))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    let mut gh = raw
        .paths
        .as_ref()
        .and_then(|paths| optional_string(paths.gh.as_deref()));
    let mut fj = raw
        .paths
        .as_ref()
        .and_then(|paths| optional_string(paths.fj.as_deref()));

    if let Some(value) = env.get("FJ_SHIM_HOSTS") {
        hosts = split_hosts(value)
            .into_iter()
            .map(|host| normalized_forgejo_host(&host))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
    }
    if let Some(value) = env
        .get("FJ_SHIM_REAL_GH")
        .and_then(|value| optional_string(Some(value)))
    {
        gh = Some(value);
    }
    if let Some(value) = env
        .get("FJ_SHIM_REAL_FJ")
        .and_then(|value| optional_string(Some(value)))
    {
        fj = Some(value);
    }

    let config = Config {
        hosts: dedupe(hosts),
        paths: PathsConfig { gh, fj },
        path: config_path,
    };

    let profiles = if env.contains_key("FJ_SHIM_HOSTS") {
        config
            .hosts
            .iter()
            .cloned()
            .map(default_host_profile)
            .collect()
    } else {
        effective_profiles(&config.hosts, explicit_profiles(raw.host_profiles))
    };
    let registry = HostRegistry::new(profiles)?;

    Ok(LoadedConfig { config, registry })
}

pub fn add_host(host: &str, path: Option<&Path>) -> Result<Config> {
    let config = load_config_with_env(path, &EnvMap::new())?;
    let explicit = explicit_profiles(read_raw_config(&config.path)?.host_profiles);
    let mut hosts = config.hosts;
    if let Some(normalized) = normalized_forgejo_host(host)? {
        if !hosts.iter().any(|configured| configured == &normalized) {
            hosts.push(normalized);
        }
    }
    hosts.sort();

    let updated = Config {
        hosts,
        paths: config.paths,
        path: config.path,
    };
    HostRegistry::new(effective_profiles(&updated.hosts, explicit.clone()))?;
    write_config_data(&updated, &explicit)?;
    Ok(updated)
}

pub fn add_host_profile(
    host: &str,
    aliases: Vec<String>,
    api_root: Option<&str>,
    credential_host: Option<&str>,
    path: Option<&Path>,
) -> Result<HostRegistry> {
    let config = load_config_with_env(path, &EnvMap::new())?;
    let mut explicit = explicit_profiles(read_raw_config(&config.path)?.host_profiles);
    let canonical_host = normalize_host(host);
    let mut profile = explicit
        .iter()
        .find(|existing| normalize_host(&existing.canonical_host) == canonical_host)
        .cloned()
        .unwrap_or_else(|| default_host_profile(canonical_host.clone()));
    for alias in aliases {
        let alias = normalize_host(&alias);
        if !profile
            .aliases
            .iter()
            .any(|existing| normalize_host(existing) == alias)
        {
            profile.aliases.push(alias);
        }
    }
    if let Some(value) = optional_string(api_root) {
        profile.api_root = value;
    }
    if let Some(value) = optional_string(credential_host) {
        profile.credential_host = normalize_host(&value);
    }
    explicit.retain(|existing| normalize_host(&existing.canonical_host) != canonical_host);
    explicit.push(profile);
    explicit.sort_by(|left, right| left.canonical_host.cmp(&right.canonical_host));

    let mut hosts = config.hosts;
    if !hosts.contains(&canonical_host) {
        hosts.push(canonical_host);
    }
    hosts.sort();
    let updated = Config {
        hosts,
        paths: config.paths,
        path: config.path,
    };
    let registry = HostRegistry::new(effective_profiles(&updated.hosts, explicit.clone()))?;
    write_config_data(&updated, &explicit)?;
    Ok(registry)
}

pub fn remove_host(host: &str, path: Option<&Path>) -> Result<Config> {
    let config = load_config_with_env(path, &EnvMap::new())?;
    let normalized = normalize_host(host);
    let mut explicit = explicit_profiles(read_raw_config(&config.path)?.host_profiles);
    explicit.retain(|profile| normalize_host(&profile.canonical_host) != normalized);
    let hosts = config
        .hosts
        .into_iter()
        .filter(|configured| normalize_host(configured) != normalized)
        .collect();
    let updated = Config {
        hosts,
        paths: config.paths,
        path: config.path,
    };
    HostRegistry::new(effective_profiles(&updated.hosts, explicit.clone()))?;
    write_config_data(&updated, &explicit)?;
    Ok(updated)
}

pub fn write_config(config: &Config) -> Result<()> {
    let explicit = explicit_profiles(read_raw_config(&config.path)?.host_profiles);
    HostRegistry::new(effective_profiles(&config.hosts, explicit.clone()))?;
    write_config_data(config, &explicit)
}

fn write_config_data(config: &Config, profiles: &[HostProfile]) -> Result<()> {
    let parent = config.path.parent().ok_or_else(|| {
        ShimError::new(format!(
            "could not determine parent directory for {}",
            config.path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        ShimError::new(format!("could not create {}: {error}", parent.display()))
    })?;

    let mut lines = Vec::new();
    let hosts = config
        .hosts
        .iter()
        .map(|host| toml_string(host))
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!("hosts = [{hosts}]"));
    for profile in profiles {
        lines.push(String::new());
        lines.push("[[host_profiles]]".to_string());
        lines.push(format!(
            "canonical_host = {}",
            toml_string(&profile.canonical_host)
        ));
        let aliases = profile
            .aliases
            .iter()
            .map(|alias| toml_string(alias))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("aliases = [{aliases}]"));
        lines.push(format!("api_root = {}", toml_string(&profile.api_root)));
        lines.push(format!(
            "credential_host = {}",
            toml_string(&profile.credential_host)
        ));
    }
    if config.paths.gh.is_some() || config.paths.fj.is_some() {
        lines.push(String::new());
        lines.push("[paths]".to_string());
        if let Some(gh) = &config.paths.gh {
            lines.push(format!("gh = {}", toml_string(gh)));
        }
        if let Some(fj) = &config.paths.fj {
            lines.push(format!("fj = {}", toml_string(fj)));
        }
    }
    lines.push(String::new());

    fs::write(&config.path, lines.join("\n")).map_err(|error| {
        ShimError::new(format!(
            "could not write {}: {error}",
            config.path.display()
        ))
    })
}

pub fn split_hosts(value: &str) -> Vec<String> {
    value
        .split([',', ';', ' '])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalized_forgejo_host(host: &str) -> Result<Option<String>> {
    let normalized = normalize_transport_host(host)?;
    if is_known_github_host(Some(&normalized)) {
        Ok(None)
    } else {
        Ok(Some(normalized))
    }
}

fn default_host_profile(canonical_host: String) -> HostProfile {
    HostProfile {
        api_root: default_api_root(&canonical_host),
        credential_host: canonical_host.clone(),
        canonical_host,
        aliases: Vec::new(),
    }
}

fn explicit_profiles(profiles: Vec<RawHostProfile>) -> Vec<HostProfile> {
    profiles
        .into_iter()
        .map(|profile| {
            let canonical_host = normalize_host(&profile.canonical_host);
            HostProfile {
                api_root: optional_string(profile.api_root.as_deref())
                    .unwrap_or_else(|| default_api_root(&canonical_host)),
                credential_host: optional_string(profile.credential_host.as_deref())
                    .map_or_else(|| canonical_host.clone(), |host| normalize_host(&host)),
                canonical_host,
                aliases: profile.aliases,
            }
        })
        .collect()
}

fn effective_profiles(hosts: &[String], mut explicit: Vec<HostProfile>) -> Vec<HostProfile> {
    let allowed_hosts = hosts
        .iter()
        .map(|host| normalize_host(host))
        .collect::<Vec<_>>();
    explicit.retain(|profile| allowed_hosts.contains(&normalize_host(&profile.canonical_host)));
    let explicit_hosts = explicit
        .iter()
        .map(|profile| normalize_host(&profile.canonical_host))
        .collect::<Vec<_>>();
    for host in allowed_hosts {
        if !explicit_hosts.contains(&host) {
            explicit.push(default_host_profile(host));
        }
    }
    explicit
}

fn default_api_root(canonical_host: &str) -> String {
    format!("https://{canonical_host}/api/v1")
}

fn read_raw_config(path: &Path) -> Result<RawConfig> {
    if !path.exists() {
        return Ok(RawConfig::default());
    }
    let text = fs::read_to_string(path)
        .map_err(|error| ShimError::new(format!("could not read {}: {error}", path.display())))?;
    toml::from_str(&text)
        .map_err(|error| ShimError::new(format!("could not parse {}: {error}", path.display())))
}

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeMap::<String, ()>::new();
    let mut result = Vec::new();
    for value in values {
        if !value.is_empty() && !seen.contains_key(&value) {
            seen.insert(value.clone(), ());
            result.push(value);
        }
    }
    result
}

fn optional_string(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn toml_string(value: &str) -> String {
    let mut escaped = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

fn home_dir(home: Option<&Path>) -> PathBuf {
    home.map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::provider::ProviderResolution;
    use crate::repo::RepoRef;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn no_default_hosts() -> Result<()> {
        let root = temp_root()?;
        let config = load_config_with_env(Some(&root.join("config.toml")), &EnvMap::new())?;

        assert_eq!(config.hosts, Vec::<String>::new());
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn legacy_and_extended_config_load_as_host_profiles() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(&path, "hosts = [\"codeberg.org\"]\n")
            .map_err(|error| ShimError::new(error.to_string()))?;

        let legacy = load_host_registry_with_env(Some(&path), &EnvMap::new())?;
        let ProviderResolution::Forgejo { profile, .. } =
            legacy.resolve(&RepoRef::new("codeberg.org", "forgejo", "forgejo"))
        else {
            panic!("legacy canonical host must create a default profile");
        };
        assert_eq!(profile.api_root, "https://codeberg.org/api/v1");
        assert_eq!(profile.credential_host, "codeberg.org");
        assert!(profile.aliases.is_empty());

        fs::write(
            &path,
            r#"hosts = ["git.dirtydishes.dev"]

[[host_profiles]]
canonical_host = "git.dirtydishes.dev"
aliases = ["127.0.0.1", "127.0.0.1:2222"]
api_root = "http://127.0.0.1:3000/api/v1"
credential_host = "git.dirtydishes.dev"
"#,
        )
        .map_err(|error| ShimError::new(error.to_string()))?;

        let extended = load_host_registry_with_env(Some(&path), &EnvMap::new())?;
        let ProviderResolution::Forgejo { repo, profile } =
            extended.resolve(&RepoRef::new("127.0.0.1:2222", "dirtydishes", "dirtypages"))
        else {
            panic!("configured transport alias must resolve");
        };
        assert_eq!(repo.host, "git.dirtydishes.dev");
        assert_eq!(profile.api_root, "http://127.0.0.1:3000/api/v1");
        assert_eq!(profile.credential_host, "git.dirtydishes.dev");
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn env_hosts_replace_profiles_and_dedupe_after_normalization() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(
            &path,
            r#"hosts = ["git.example.com"]

[[host_profiles]]
canonical_host = "git.example.com"
aliases = ["git.local"]
"#,
        )
        .map_err(|error| ShimError::new(error.to_string()))?;
        let env = env([("FJ_SHIM_HOSTS", "ENV.EXAMPLE.COM,env.example.com")]);

        let registry = load_host_registry_with_env(Some(&path), &env)?;

        assert_eq!(registry.profiles().len(), 1);
        assert_eq!(registry.profiles()[0].canonical_host, "env.example.com");
        assert!(matches!(
            registry.resolve(&RepoRef::new("git.local", "owner", "repo")),
            ProviderResolution::Unconfigured { .. }
        ));
        assert!(matches!(
            registry.resolve(&RepoRef::new("env.example.com", "owner", "repo")),
            ProviderResolution::Forgejo { .. }
        ));
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn profile_table_without_legacy_allowlist_entry_stays_inert() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(
            &path,
            r#"hosts = []

[[host_profiles]]
canonical_host = "git.example.com"
aliases = ["git.local"]
"#,
        )
        .map_err(|error| ShimError::new(error.to_string()))?;

        let registry = load_host_registry_with_env(Some(&path), &EnvMap::new())?;

        assert!(registry.profiles().is_empty());
        assert!(matches!(
            registry.resolve(&RepoRef::new("git.local", "owner", "repo")),
            ProviderResolution::Unconfigured { .. }
        ));
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn duplicate_legacy_host_spellings_create_one_default_profile() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(
            &path,
            "hosts = [\"Git.Example.com\", \"https://git.example.com/path\"]\n",
        )
        .map_err(|error| ShimError::new(error.to_string()))?;

        let registry = load_host_registry_with_env(Some(&path), &EnvMap::new())?;

        assert_eq!(registry.profiles().len(), 1);
        assert_eq!(registry.profiles()[0].canonical_host, "git.example.com");
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn env_hosts_replace_config_hosts() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(&path, "hosts = [\"git.example.com\"]\n")
            .map_err(|error| ShimError::new(error.to_string()))?;
        let env = env([("FJ_SHIM_HOSTS", "code.example.org, git.local")]);

        let config = load_config_with_env(Some(&path), &env)?;

        assert_eq!(config.hosts, vec!["code.example.org", "git.local"]);
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn github_hosts_are_ignored_even_if_configured() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(&path, "hosts = [\"github.com\", \"git.example.com\"]\n")
            .map_err(|error| ShimError::new(error.to_string()))?;

        let config = load_config_with_env(Some(&path), &EnvMap::new())?;

        assert_eq!(config.hosts, vec!["git.example.com"]);
        assert!(!config.is_forgejo_host(Some("github.com")));
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn malformed_canonical_hosts_fail_configuration_loading() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(&path, "hosts = [\"https://user@git.example.com/path\"]\n")
            .map_err(|error| ShimError::new(error.to_string()))?;

        assert!(load_runtime_config_with_env(Some(&path), &EnvMap::new()).is_err());

        let env = env([("FJ_SHIM_HOSTS", "https://user@git.example.com/path")]);
        assert!(load_runtime_config_with_env(Some(&path), &env).is_err());

        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn add_and_remove_host_round_trips_python_readable_toml() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");

        add_host("https://Git.Example.com/path", Some(&path))?;
        let config = load_config_with_env(Some(&path), &EnvMap::new())?;
        assert_eq!(config.hosts, vec!["git.example.com"]);

        remove_host("git.example.com", Some(&path))?;
        let config = load_config_with_env(Some(&path), &EnvMap::new())?;
        assert!(config.hosts.is_empty());
        assert_eq!(
            fs::read_to_string(&path).unwrap_or_default(),
            "hosts = []\n"
        );
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn add_host_does_not_persist_github() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");

        add_host("github.com", Some(&path))?;
        let config = load_config_with_env(Some(&path), &EnvMap::new())?;

        assert!(config.hosts.is_empty());
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn env_paths_override_config() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(&path, "[paths]\ngh = \"/bin/gh\"\nfj = \"/bin/fj\"\n")
            .map_err(|error| ShimError::new(error.to_string()))?;
        let env = env([
            ("FJ_SHIM_REAL_GH", "/custom/gh"),
            ("FJ_SHIM_REAL_FJ", "/custom/fj"),
        ]);

        let config = load_config_with_env(Some(&path), &env)?;

        assert_eq!(config.paths.gh.as_deref(), Some("/custom/gh"));
        assert_eq!(config.paths.fj.as_deref(), Some("/custom/fj"));
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn add_host_does_not_persist_env_overrides() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(
            &path,
            "hosts = [\"git.example.com\"]\n\n[paths]\ngh = \"/bin/gh\"\n",
        )
        .map_err(|error| ShimError::new(error.to_string()))?;
        let env = env([
            ("FJ_SHIM_HOSTS", "env.example.com"),
            ("FJ_SHIM_REAL_GH", "/env/gh"),
            ("FJ_SHIM_REAL_FJ", "/env/fj"),
        ]);

        let effective = load_config_with_env(Some(&path), &env)?;
        assert_eq!(effective.hosts, vec!["env.example.com"]);
        assert_eq!(effective.paths.gh.as_deref(), Some("/env/gh"));
        assert_eq!(effective.paths.fj.as_deref(), Some("/env/fj"));

        let config = add_host("new.example.com", Some(&path))?;

        assert_eq!(config.hosts, vec!["git.example.com", "new.example.com"]);
        assert_eq!(config.paths.gh.as_deref(), Some("/bin/gh"));
        assert_eq!(config.paths.fj, None);
        assert_eq!(
            fs::read_to_string(&path).unwrap_or_default(),
            "hosts = [\"git.example.com\", \"new.example.com\"]\n\n[paths]\ngh = \"/bin/gh\"\n"
        );
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    fn env(values: impl IntoIterator<Item = (&'static str, &'static str)>) -> EnvMap {
        values
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn temp_root() -> Result<PathBuf> {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "gh-forgejo-shim-config-test-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(|error| ShimError::new(error.to_string()))?;
        Ok(path)
    }
}
