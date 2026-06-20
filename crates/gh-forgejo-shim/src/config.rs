//! Configuration loading and host allowlisting.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Result, ShimError};

pub type EnvMap = BTreeMap<String, String>;

pub const KNOWN_GITHUB_HOSTS: &[&str] = &["github.com", "www.github.com"];

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

#[derive(Debug, Deserialize)]
struct RawConfig {
    hosts: Option<Vec<String>>,
    paths: Option<RawPathsConfig>,
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
    let config_path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config_path(None));

    let mut raw = RawConfig {
        hosts: None,
        paths: None,
    };
    if config_path.exists() {
        let text = fs::read_to_string(&config_path).map_err(|error| {
            ShimError::new(format!("could not read {}: {error}", config_path.display()))
        })?;
        raw = toml::from_str(&text).map_err(|error| {
            ShimError::new(format!(
                "could not parse {}: {error}",
                config_path.display()
            ))
        })?;
    }

    let mut hosts = raw
        .hosts
        .unwrap_or_default()
        .into_iter()
        .filter_map(|host| normalized_forgejo_host(&host))
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
            .filter_map(|host| normalized_forgejo_host(&host))
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

    Ok(Config {
        hosts: dedupe(hosts),
        paths: PathsConfig { gh, fj },
        path: config_path,
    })
}

pub fn add_host(host: &str, path: Option<&Path>) -> Result<Config> {
    let env = current_env();
    add_host_with_env(host, path, &env)
}

pub fn add_host_with_env(host: &str, path: Option<&Path>, env: &EnvMap) -> Result<Config> {
    let config = load_config_with_env(path, env)?;
    let mut hosts = config.hosts;
    if let Some(normalized) = normalized_forgejo_host(host) {
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
    write_config(&updated)?;
    Ok(updated)
}

pub fn remove_host(host: &str, path: Option<&Path>) -> Result<Config> {
    let env = current_env();
    remove_host_with_env(host, path, &env)
}

pub fn remove_host_with_env(host: &str, path: Option<&Path>, env: &EnvMap) -> Result<Config> {
    let config = load_config_with_env(path, env)?;
    let normalized = normalize_host(host);
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
    write_config(&updated)?;
    Ok(updated)
}

pub fn write_config(config: &Config) -> Result<()> {
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

pub fn normalize_host(host: &str) -> String {
    let mut value = host.trim();
    if let Some((_, rest)) = value.split_once("://") {
        value = rest;
    }
    if let Some((host, _)) = value.split_once('/') {
        value = host;
    }
    value.to_ascii_lowercase()
}

pub fn is_known_github_host(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let normalized = normalize_host(host);
    KNOWN_GITHUB_HOSTS.contains(&normalized.as_str())
}

pub fn split_hosts(value: &str) -> Vec<String> {
    value
        .split([',', ';', ' '])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalized_forgejo_host(host: &str) -> Option<String> {
    let normalized = normalize_host(host);
    if normalized.is_empty() || is_known_github_host(Some(&normalized)) {
        None
    } else {
        Some(normalized)
    }
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
    fn add_and_remove_host_round_trips_python_readable_toml() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");

        add_host_with_env("https://Git.Example.com/path", Some(&path), &EnvMap::new())?;
        let config = load_config_with_env(Some(&path), &EnvMap::new())?;
        assert_eq!(config.hosts, vec!["git.example.com"]);

        remove_host_with_env("git.example.com", Some(&path), &EnvMap::new())?;
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

        add_host_with_env("github.com", Some(&path), &EnvMap::new())?;
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
