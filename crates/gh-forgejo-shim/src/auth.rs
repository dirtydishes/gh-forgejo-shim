//! Host-keyed authentication storage and token resolution.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::config::{self, normalize_host, Config, EnvMap};
use crate::{Result, ShimError};

pub const TOKEN_ENV_VARS: &[&str] = &["FJ_SHIM_TOKEN", "FORGEJO_TOKEN", "GITEA_TOKEN", "FJ_TOKEN"];
pub const KEYCHAIN_SERVICE: &str = "gh-forgejo-shim";
pub const AUTH_FILE_NAME: &str = "auth.json";

static NEXT_AUTH_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenDiscovery {
    pub token: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredToken {
    pub token: String,
    pub storage: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub host: String,
    pub exists: bool,
    pub storage: Option<String>,
}

fn token_from_env_with_source(env: &EnvMap) -> Option<TokenDiscovery> {
    for name in TOKEN_ENV_VARS {
        if let Some(value) = env
            .get(*name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            return Some(TokenDiscovery {
                token: value.to_string(),
                source: (*name).to_string(),
            });
        }
    }
    None
}

/// Resolve credentials only for this host. Generic env tokens are safe only
/// when the effective allowlist contains exactly one Forgejo host.
pub fn resolve_token(
    host: &str,
    config: &Config,
    env: &EnvMap,
    home: Option<&Path>,
    platform: Option<&str>,
) -> Option<TokenDiscovery> {
    let host = normalize_host(host);
    if let Some(stored) = read_stored_token(&host, home, platform) {
        return Some(TokenDiscovery {
            token: stored.token,
            source: stored.storage,
        });
    }
    if !config.is_forgejo_host(Some(&host)) {
        return None;
    }
    if let Some(name) = config
        .host
        .get(&host)
        .and_then(|settings| settings.token_env.as_ref())
    {
        if let Some(value) = env
            .get(name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            return Some(TokenDiscovery {
                token: value.to_string(),
                source: name.clone(),
            });
        }
    }
    if config.hosts.len() == 1 {
        return token_from_env_with_source(env);
    }
    None
}

pub fn read_stored_token(
    host: &str,
    home: Option<&Path>,
    platform: Option<&str>,
) -> Option<StoredToken> {
    let normalized = normalize_host(host);
    if use_keychain(platform) {
        if let Some(token) = read_keychain_token(&normalized) {
            return Some(StoredToken {
                token,
                storage: "macOS Keychain".to_string(),
            });
        }
    }

    read_file_token(&normalized, home).map(|token| StoredToken {
        token,
        storage: auth_file_path(home).display().to_string(),
    })
}

pub fn write_stored_token(
    host: &str,
    token: &str,
    home: Option<&Path>,
    platform: Option<&str>,
) -> Result<String> {
    let normalized = normalize_host(host);
    if normalized.is_empty() {
        return Err(ShimError::new("Forgejo host is required"));
    }
    if token.trim().is_empty() {
        return Err(ShimError::new("Forgejo token is required"));
    }

    if use_keychain(platform) && write_keychain_token(&normalized, token).is_ok() {
        delete_file_token(&normalized, home)?;
        return Ok("macOS Keychain".to_string());
    }

    write_file_token(&normalized, token, home)?;
    Ok(auth_file_path(home).display().to_string())
}

pub fn delete_stored_token(
    host: &str,
    home: Option<&Path>,
    platform: Option<&str>,
) -> Result<bool> {
    let normalized = normalize_host(host);
    let mut deleted = false;
    if use_keychain(platform) {
        deleted = delete_keychain_token(&normalized) || deleted;
    }
    Ok(delete_file_token(&normalized, home)? || deleted)
}

pub fn stored_auth_status(host: &str, home: Option<&Path>, platform: Option<&str>) -> AuthStatus {
    let normalized = normalize_host(host);
    if let Some(stored) = read_stored_token(&normalized, home, platform) {
        return AuthStatus {
            host: normalized,
            exists: true,
            storage: Some(stored.storage),
        };
    }
    AuthStatus {
        host: normalized,
        exists: false,
        storage: None,
    }
}

pub fn auth_file_path(home: Option<&Path>) -> PathBuf {
    home_dir(home)
        .join(".config")
        .join("gh-forgejo-shim")
        .join(AUTH_FILE_NAME)
}

pub fn validate_token(host: &str, token: &str) -> Result<Option<String>> {
    let env = config::current_env();
    let config = config::load_config_with_env(None, &env)?;
    let client =
        crate::forgejo::ForgejoClient::for_host(host, Some(token.to_string()), &config, &env)?
            .with_timeout(Duration::from_secs(10));
    let value = client.get_current_user(&normalize_host(host))?;
    let object = value
        .as_object()
        .ok_or_else(|| ShimError::new("Forgejo API returned unexpected user data"))?;
    for key in ["login", "username", "name"] {
        if let Some(login) = object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|login| !login.is_empty())
        {
            return Ok(Some(login.to_string()));
        }
    }
    Ok(None)
}

fn read_file_token(host: &str, home: Option<&Path>) -> Option<String> {
    let data = read_auth_file(home);
    let item = data.get("hosts")?.as_object()?.get(host)?;
    if let Some(object) = item.as_object() {
        return object
            .get("token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(ToOwned::to_owned);
    }
    item.as_str()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
}

fn write_file_token(host: &str, token: &str, home: Option<&Path>) -> Result<()> {
    let mut data = read_auth_file(home);
    let object = ensure_object(&mut data);
    let hosts = object
        .entry("hosts".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !hosts.is_object() {
        *hosts = Value::Object(Map::new());
    }
    let host_object = json!({ "token": token });
    hosts
        .as_object_mut()
        .ok_or_else(|| ShimError::new("auth hosts must be an object"))?
        .insert(host.to_string(), host_object);
    write_auth_file(&data, home)
}

fn delete_file_token(host: &str, home: Option<&Path>) -> Result<bool> {
    let path = auth_file_path(home);
    let mut data = read_auth_file(home);
    let Some(hosts) = data.get_mut("hosts").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    if hosts.remove(host).is_none() {
        return Ok(false);
    }

    if hosts.is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ShimError::new(format!(
                    "could not remove {}: {error}",
                    path.display()
                )));
            }
        }
    } else {
        write_auth_file(&data, home)?;
    }
    Ok(true)
}

fn read_auth_file(home: Option<&Path>) -> Value {
    let path = auth_file_path(home);
    let Ok(text) = fs::read_to_string(path) else {
        return json!({ "hosts": {} });
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(data) if data.is_object() => data,
        _ => json!({ "hosts": {} }),
    }
}

fn write_auth_file(data: &Value, home: Option<&Path>) -> Result<()> {
    let path = auth_file_path(home);
    let parent = path.parent().ok_or_else(|| {
        ShimError::new(format!(
            "could not determine parent directory for {}",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        ShimError::new(format!("could not create {}: {error}", parent.display()))
    })?;
    set_permissions(parent, 0o700).ok();

    let text = serde_json::to_string_pretty(data)
        .map_err(|error| ShimError::new(format!("could not serialize auth file: {error}")))?
        + "\n";
    let (mut file, temp_path) = create_temp_file(parent)
        .map_err(|error| ShimError::new(format!("could not create temp auth file: {error}")))?;

    let result = (|| -> Result<()> {
        file.write_all(text.as_bytes()).map_err(|error| {
            ShimError::new(format!("could not write {}: {error}", temp_path.display()))
        })?;
        file.flush().map_err(|error| {
            ShimError::new(format!("could not flush {}: {error}", temp_path.display()))
        })?;
        set_permissions(&temp_path, 0o600).map_err(|error| {
            ShimError::new(format!(
                "could not set permissions on {}: {error}",
                temp_path.display()
            ))
        })?;
        drop(file);
        fs::rename(&temp_path, &path).map_err(|error| {
            ShimError::new(format!("could not write {}: {error}", path.display()))
        })?;
        set_permissions(&path, 0o600).map_err(|error| {
            ShimError::new(format!(
                "could not set permissions on {}: {error}",
                path.display()
            ))
        })?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

fn ensure_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    match value {
        Value::Object(object) => object,
        _ => unreachable!("value was just set to an object"),
    }
}

fn create_temp_file(parent: &Path) -> io::Result<(File, PathBuf)> {
    for _ in 0..1000 {
        let id = NEXT_AUTH_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let temp_path = parent.join(format!(
            ".{AUTH_FILE_NAME}.{}-{id}-{nanos}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options.mode(0o600);
        }

        match options.open(&temp_path) {
            Ok(file) => return Ok((file, temp_path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "unable to allocate auth temp file",
    ))
}

fn use_keychain(platform: Option<&str>) -> bool {
    // Unit tests may simulate macOS, but must never access the user's Keychain.
    !cfg!(test)
        && platform.unwrap_or(current_platform()) == "darwin"
        && executable_in_path("security")
}

fn current_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::consts::OS
    }
}

fn executable_in_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(name).is_file())
}

fn read_keychain_token(host: &str) -> Option<String> {
    let output = run_security([
        "find-generic-password",
        "-a",
        host,
        "-s",
        KEYCHAIN_SERVICE,
        "-w",
    ])
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

fn write_keychain_token(host: &str, token: &str) -> Result<()> {
    let output = run_security([
        "add-generic-password",
        "-a",
        host,
        "-s",
        KEYCHAIN_SERVICE,
        "-w",
        token,
        "-U",
    ])
    .map_err(|error| ShimError::new(format!("security add-generic-password failed: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(ShimError::new(if message.is_empty() {
        "security add-generic-password failed".to_string()
    } else {
        message
    }))
}

fn delete_keychain_token(host: &str) -> bool {
    run_security([
        "delete-generic-password",
        "-a",
        host,
        "-s",
        KEYCHAIN_SERVICE,
    ])
    .is_ok_and(|output| output.status.success())
}

fn run_security<const N: usize>(args: [&str; N]) -> io::Result<std::process::Output> {
    Command::new("security").args(args).output()
}

fn home_dir(home: Option<&Path>) -> PathBuf {
    home.map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(unix)]
fn set_permissions(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn set_permissions(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn env_token_precedence() {
        let env = env([
            ("FJ_SHIM_TOKEN", "shim"),
            ("FORGEJO_TOKEN", "forgejo"),
            ("GITEA_TOKEN", "gitea"),
            ("FJ_TOKEN", "fj"),
        ]);

        assert_eq!(
            token_from_env_with_source(&env)
                .map(|found| found.token)
                .as_deref(),
            Some("shim")
        );
    }

    #[test]
    fn stores_and_reads_shim_auth_file_token() -> Result<()> {
        let home = temp_root()?;
        let storage = write_stored_token(
            "https://Git.Example.com/path",
            "stored-token",
            Some(&home),
            Some("linux"),
        )?;

        let token = read_stored_token("git.example.com", Some(&home), Some("linux"))
            .map(|stored| stored.token);

        assert_eq!(storage, auth_file_path(Some(&home)).display().to_string());
        assert_eq!(token.as_deref(), Some("stored-token"));

        #[cfg(unix)]
        {
            let mode = fs::metadata(auth_file_path(Some(&home)))
                .map_err(|error| ShimError::new(error.to_string()))?
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn host_token_precedence_covers_each_layer_and_combination() -> Result<()> {
        let home = temp_root()?;
        let path = home.join("config.toml");
        fs::write(
            &path,
            "[host.\"git.example.com\"]\ntoken_env = \"HOST_TOKEN\"\n",
        )?;
        let config = config::load_config_with_env(Some(&path), &EnvMap::new())?;

        for stored in [false, true] {
            for per_host in [false, true] {
                for generic in [false, true] {
                    delete_stored_token("git.example.com", Some(&home), Some("linux"))?;
                    if stored {
                        write_stored_token(
                            "git.example.com",
                            "stored",
                            Some(&home),
                            Some("linux"),
                        )?;
                    }
                    let env = env([
                        ("HOST_TOKEN", if per_host { "host-env" } else { " \t" }),
                        ("FORGEJO_TOKEN", if generic { "generic" } else { "" }),
                    ]);
                    let found = resolve_token(
                        "https://Git.Example.com/path",
                        &config,
                        &env,
                        Some(&home),
                        Some("linux"),
                    );
                    let expected = if stored {
                        Some("stored")
                    } else if per_host {
                        Some("host-env")
                    } else if generic {
                        Some("generic")
                    } else {
                        None
                    };
                    assert_eq!(
                        found.as_ref().map(|found| found.token.as_str()),
                        expected,
                        "stored={stored}, per_host={per_host}, generic={generic}"
                    );
                    if per_host && !stored {
                        assert_eq!(
                            found.map(|found| found.source).as_deref(),
                            Some("HOST_TOKEN")
                        );
                    }
                }
            }
        }
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn two_hosts_never_share_generic_or_host_specific_tokens() -> Result<()> {
        let home = temp_root()?;
        let path = home.join("config.toml");
        fs::write(
            &path,
            "[host.\"a.example.com\"]\ntoken_env = \"A_TOKEN\"\n[host.\"b.example.com\"]\n",
        )?;
        let config = config::load_config_with_env(Some(&path), &EnvMap::new())?;
        let mut env = env([("FORGEJO_TOKEN", "generic")]);
        for host in ["a.example.com", "b.example.com", "other.example.com"] {
            assert!(resolve_token(host, &config, &env, Some(&home), Some("linux")).is_none());
        }
        env.insert("A_TOKEN".to_string(), "a-token".to_string());
        assert_eq!(
            resolve_token("a.example.com", &config, &env, Some(&home), Some("linux"))
                .map(|found| found.token)
                .as_deref(),
            Some("a-token")
        );
        assert!(
            resolve_token("b.example.com", &config, &env, Some(&home), Some("linux")).is_none()
        );
        write_stored_token("b.example.com", "b-stored", Some(&home), Some("linux"))?;
        assert_eq!(
            resolve_token("b.example.com", &config, &env, Some(&home), Some("linux"))
                .map(|found| found.token)
                .as_deref(),
            Some("b-stored")
        );
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn generic_env_requires_one_matching_configured_host() -> Result<()> {
        let home = temp_root()?;
        let path = home.join("config.toml");
        let env = env([("FORGEJO_TOKEN", "generic")]);
        let empty = config::load_config_with_env(Some(&path), &env)?;
        assert!(
            resolve_token("git.example.com", &empty, &env, Some(&home), Some("linux")).is_none()
        );
        fs::write(
            &path,
            "hosts = [\"Git.Example.com\", \"git.example.com\", \"github.com\"]\n",
        )?;
        let config = config::load_config_with_env(Some(&path), &env)?;
        assert_eq!(
            resolve_token("git.example.com", &config, &env, Some(&home), Some("linux"))
                .map(|found| found.token)
                .as_deref(),
            Some("generic")
        );
        assert!(resolve_token(
            "other.example.com",
            &config,
            &env,
            Some(&home),
            Some("linux")
        )
        .is_none());
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn reads_python_compatible_string_auth_file_token() -> Result<()> {
        let home = temp_root()?;
        let path = auth_file_path(Some(&home));
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| ShimError::new("missing parent"))?,
        )
        .map_err(|error| ShimError::new(error.to_string()))?;
        fs::write(&path, r#"{"hosts":{"git.example.com":"old-token"}}"#)
            .map_err(|error| ShimError::new(error.to_string()))?;

        let token = read_stored_token("git.example.com", Some(&home), Some("linux"))
            .map(|stored| stored.token);

        assert_eq!(token.as_deref(), Some("old-token"));
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn delete_stored_token_removes_auth_file_entry() -> Result<()> {
        let home = temp_root()?;
        write_stored_token(
            "git.example.com",
            "stored-token",
            Some(&home),
            Some("linux"),
        )?;

        let deleted = delete_stored_token("git.example.com", Some(&home), Some("linux"))?;
        let token = read_stored_token("git.example.com", Some(&home), Some("linux"))
            .map(|stored| stored.token);

        assert!(deleted);
        assert!(token.is_none());
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn delete_stored_token_preserves_other_auth_file_entries() -> Result<()> {
        let home = temp_root()?;
        write_stored_token(
            "git.example.com",
            "stored-token",
            Some(&home),
            Some("linux"),
        )?;
        write_stored_token("git.other.test", "other-token", Some(&home), Some("linux"))?;

        let deleted = delete_stored_token("git.example.com", Some(&home), Some("linux"))?;

        assert!(deleted);
        assert!(
            read_stored_token("git.example.com", Some(&home), Some("linux"))
                .map(|stored| stored.token)
                .is_none()
        );
        assert_eq!(
            read_stored_token("git.other.test", Some(&home), Some("linux"))
                .map(|stored| stored.token)
                .as_deref(),
            Some("other-token")
        );
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn auth_temp_file_starts_private() -> Result<()> {
        let home = temp_root()?;
        let (file, temp_path) = create_temp_file(&home)
            .map_err(|error| ShimError::new(format!("could not create temp file: {error}")))?;
        drop(file);

        let mode = fs::metadata(&temp_path)
            .map_err(|error| ShimError::new(error.to_string()))?
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(mode, 0o600);
        fs::remove_file(temp_path).ok();
        fs::remove_dir_all(home).ok();
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
            "gh-forgejo-shim-auth-test-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(|error| ShimError::new(error.to_string()))?;
        Ok(path)
    }
}
