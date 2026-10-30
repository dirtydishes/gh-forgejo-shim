//! Explicit, one-shot imports. Normal authentication never reads these files.
//!
//! Schemas and paths:
//! - https://codeberg.org/forgejo-contrib/forgejo-cli/src/commit/9cd11936a3c60bc4b15c7c7d4f171eff00f153a2/src/keys.rs
//! - https://gitea.com/gitea/tea/src/commit/397e6300ec93937b7745ebcc19b95ca9cb30568b/modules/config/config.go
//! - https://gitea.com/gitea/tea/src/commit/397e6300ec93937b7745ebcc19b95ca9cb30568b/modules/config/login.go

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::{is_known_github_host, normalize_host, EnvMap};
use crate::{Result, ShimError};

#[derive(Clone, Copy)]
pub enum ImportSource {
    Fj,
    Tea,
}

impl ImportSource {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "fj" => Ok(Self::Fj),
            "tea" => Ok(Self::Tea),
            _ => Err(ShimError::new("auth import --from must be fj or tea")),
        }
    }
}

pub struct ImportedToken {
    pub host: String,
    pub token: String,
    pub path: PathBuf,
}

#[derive(Deserialize)]
struct FjKeys {
    hosts: BTreeMap<String, FjLogin>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum FjLogin {
    Application { token: String },
    OAuth { token: String },
}

#[derive(Deserialize)]
struct TeaConfig {
    #[serde(default)]
    logins: Vec<TeaLogin>,
}

#[derive(Deserialize)]
struct TeaLogin {
    url: String,
    #[serde(default)]
    token: String,
}

pub fn read_import(
    source: ImportSource,
    hostname: Option<&str>,
    home: &Path,
    env: &EnvMap,
    platform: &str,
) -> Result<ImportedToken> {
    let paths = source_paths(source, home, env, platform);
    let mut loaded = None;
    for path in &paths {
        match fs::read_to_string(path) {
            Ok(text) => {
                loaded = Some((path.clone(), text));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(ShimError::new(format!(
                    "could not read import source {}",
                    path.display()
                )))
            }
        }
    }
    let (path, text) = loaded.ok_or_else(|| {
        ShimError::new(format!(
            "no import source file found; checked {}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    // Parser errors can quote input lines containing tokens. Keep them private.
    let invalid = || ShimError::new(format!("could not parse import source {}", path.display()));
    let entries: Vec<(String, String)> = match source {
        ImportSource::Fj => {
            let keys: FjKeys = serde_json::from_str(&text).map_err(|_| invalid())?;
            keys.hosts
                .into_iter()
                .map(|(host, login)| {
                    let (FjLogin::Application { token } | FjLogin::OAuth { token }) = login;
                    (host, token)
                })
                .collect()
        }
        ImportSource::Tea => {
            let config: TeaConfig = serde_yaml::from_str(&text).map_err(|_| invalid())?;
            config
                .logins
                .into_iter()
                .map(|login| (login.url, login.token))
                .collect()
        }
    };
    let requested = hostname.map(normalize_host);
    if requested.as_deref().is_some_and(str::is_empty) {
        return Err(ShimError::new("Forgejo host is required"));
    }
    let mut matches = entries.into_iter().filter_map(|(host, token)| {
        let host = normalize_host(&host);
        if host.is_empty()
            || is_known_github_host(Some(&host))
            || requested
                .as_ref()
                .is_some_and(|requested| requested != &host)
        {
            return None;
        }
        Some((host, token.trim().to_string()))
    });
    let first = matches.next().ok_or_else(|| ShimError::new(format!(
        "no matching token entry in {}; choose a host with --hostname HOST or use auth login HOST", path.display()
    )))?;
    if matches.next().is_some() {
        return Err(ShimError::new(format!(
            "multiple matching token entries in {}; use --hostname HOST to select one entry",
            path.display()
        )));
    }
    if first.1.is_empty() {
        return Err(ShimError::new(format!(
            "matching entry in {} has no token in the source file; use auth login HOST",
            path.display()
        )));
    }
    Ok(ImportedToken {
        host: first.0,
        token: first.1,
        path,
    })
}

fn env_dir(env: &EnvMap, name: &str) -> Option<PathBuf> {
    env.get(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn source_paths(source: ImportSource, home: &Path, env: &EnvMap, platform: &str) -> Vec<PathBuf> {
    let macos = matches!(platform, "darwin" | "macos");
    let app_support = home.join("Library/Application Support");
    match source {
        ImportSource::Fj if macos => vec![
            app_support.join("forgejo-cli.forgejo-cli/keys.json"),
            app_support.join("Cyborus.forgejo-cli/keys.json"),
        ],
        ImportSource::Fj => vec![env_dir(env, "XDG_DATA_HOME")
            .unwrap_or_else(|| home.join(".local/share"))
            .join("forgejo-cli/keys.json")],
        ImportSource::Tea => vec![
            env_dir(env, "XDG_CONFIG_HOME")
                .unwrap_or_else(|| {
                    if macos {
                        app_support
                    } else {
                        home.join(".config")
                    }
                })
                .join("tea/config.yml"),
            home.join(".tea/tea.yml"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_paths_follow_upstream_platform_defaults_and_xdg_overrides() {
        let home = Path::new("/synthetic/home");
        let env = EnvMap::new();
        assert_eq!(
            source_paths(ImportSource::Fj, home, &env, "darwin"),
            [
                home.join("Library/Application Support/forgejo-cli.forgejo-cli/keys.json"),
                home.join("Library/Application Support/Cyborus.forgejo-cli/keys.json"),
            ]
        );
        assert_eq!(
            source_paths(ImportSource::Fj, home, &env, "linux"),
            [home.join(".local/share/forgejo-cli/keys.json")]
        );
        for platform in ["darwin", "linux"] {
            let expected = if platform == "darwin" {
                "Library/Application Support/tea/config.yml"
            } else {
                ".config/tea/config.yml"
            };
            assert_eq!(
                source_paths(ImportSource::Tea, home, &env, platform),
                [home.join(expected), home.join(".tea/tea.yml")]
            );
            let env = EnvMap::from([
                (
                    "XDG_CONFIG_HOME".to_string(),
                    "/synthetic/config".to_string(),
                ),
                ("XDG_DATA_HOME".to_string(), "/synthetic/data".to_string()),
            ]);
            assert_eq!(
                source_paths(ImportSource::Tea, home, &env, platform)[0],
                Path::new("/synthetic/config/tea/config.yml")
            );
            if platform == "linux" {
                assert_eq!(
                    source_paths(ImportSource::Fj, home, &env, platform)[0],
                    Path::new("/synthetic/data/forgejo-cli/keys.json")
                );
            }
        }
        let relative = EnvMap::from([
            ("XDG_DATA_HOME".to_string(), "relative".to_string()),
            ("XDG_CONFIG_HOME".to_string(), String::new()),
        ]);
        assert_eq!(
            source_paths(ImportSource::Fj, home, &relative, "linux"),
            source_paths(ImportSource::Fj, home, &env, "linux")
        );
        assert_eq!(
            source_paths(ImportSource::Tea, home, &relative, "linux"),
            source_paths(ImportSource::Tea, home, &env, "linux")
        );
    }
}
