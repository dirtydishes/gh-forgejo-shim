//! Authentication storage and token discovery.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::config::{current_env, normalize_host, EnvMap};
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

pub fn token_from_process_env() -> Option<String> {
    let env = current_env();
    token_from_env(&env)
}

pub fn token_from_env(env: &EnvMap) -> Option<String> {
    token_from_env_with_source(env).map(|discovery| discovery.token)
}

pub fn token_from_env_with_source(env: &EnvMap) -> Option<TokenDiscovery> {
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

pub fn discover_fj_token(host: Option<&str>, env: &EnvMap, home: Option<&Path>) -> Option<String> {
    discover_token(host, env, home, None, true).map(|discovery| discovery.token)
}

pub fn discover_import_token(
    host: &str,
    env: &EnvMap,
    home: Option<&Path>,
) -> Option<TokenDiscovery> {
    discover_token(Some(host), env, home, None, false)
}

pub fn discover_token(
    host: Option<&str>,
    env: &EnvMap,
    home: Option<&Path>,
    platform: Option<&str>,
    include_stored: bool,
) -> Option<TokenDiscovery> {
    if let Some(discovery) = token_from_env_with_source(env) {
        return Some(discovery);
    }

    let normalized_host = host.map(normalize_host).filter(|host| !host.is_empty());
    if include_stored {
        if let Some(host) = normalized_host.as_deref() {
            if let Some(stored) = read_stored_token(host, home, platform) {
                return Some(TokenDiscovery {
                    token: stored.token,
                    source: stored.storage,
                });
            }
        }
    }

    let base = home_dir(home);
    let candidates = [
        base.join("Library")
            .join("Application Support")
            .join("Cyborus.forgejo-cli")
            .join("keys.json"),
        base.join(".config").join("fj").join("config.json"),
        base.join(".config").join("fj").join("config.yml"),
        base.join(".config").join("fj").join("config.yaml"),
        base.join(".config").join("tea").join("config.yml"),
        base.join(".config").join("tea").join("config.yaml"),
        base.join(".config").join("gitea").join("tea.yml"),
    ];

    for path in candidates {
        if let Some(token) = read_token_file(&path, normalized_host.as_deref()) {
            return Some(TokenDiscovery {
                token,
                source: path.display().to_string(),
            });
        }
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
    let host = normalize_host(host);
    let url = format!("https://{host}/api/v1/user");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();
    let response = agent
        .get(&url)
        .set("Accept", "application/json")
        .set("User-Agent", "gh-forgejo-shim")
        .set("Authorization", &format!("token {token}"))
        .call();

    let body = match response {
        Ok(response) => response
            .into_string()
            .map_err(|error| ShimError::new(format!("Forgejo API request failed: {error}")))?,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(ShimError::new(format!(
                "Forgejo API returned HTTP {code}: {body}"
            )));
        }
        Err(ureq::Error::Transport(error)) => {
            let message = error
                .message()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| error.to_string());
            return Err(ShimError::new(format!(
                "Forgejo API request failed: {}",
                message
            )));
        }
    };

    let value = serde_json::from_str::<Value>(&body)
        .map_err(|error| ShimError::new(format!("Forgejo API returned invalid JSON: {error}")))?;
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

fn read_token_file(path: &Path, host: Option<&str>) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;

    if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
        let data = serde_json::from_str::<Value>(&text).ok()?;
        return find_token(&data, host);
    }

    find_token_in_text(&text, host)
}

fn find_token(data: &Value, host: Option<&str>) -> Option<String> {
    match data {
        Value::Object(object) => {
            if let Some(host) = host {
                for (key, value) in object {
                    if normalize_host(key) == host {
                        if let Some(found) = find_token(value, None) {
                            return Some(found);
                        }
                    }
                }
                if dict_matches_host(object, host) {
                    if let Some(found) = token_from_object(object) {
                        return Some(found);
                    }
                }
                for value in object.values() {
                    if let Some(found) = find_token(value, Some(host)) {
                        return Some(found);
                    }
                }
                None
            } else {
                if let Some(found) = token_from_object(object) {
                    return Some(found);
                }
                for value in object.values() {
                    if let Some(found) = find_token(value, None) {
                        return Some(found);
                    }
                }
                None
            }
        }
        Value::Array(items) => {
            for item in items {
                if let Some(found) = find_token(item, host) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn token_from_object(object: &Map<String, Value>) -> Option<String> {
    for key in ["token", "access_token"] {
        if let Some(value) = object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
    }
    None
}

fn dict_matches_host(object: &Map<String, Value>, host: &str) -> bool {
    for key in [
        "host",
        "hostname",
        "url",
        "server",
        "server_url",
        "base_url",
    ] {
        if object
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| normalize_host(value) == host)
        {
            return true;
        }
    }
    false
}

fn find_token_in_text(text: &str, host: Option<&str>) -> Option<String> {
    if let Some(host) = host {
        let lower_text = text.to_ascii_lowercase();
        let lower_host = host.to_ascii_lowercase();
        let host_index = lower_text.find(&lower_host)?;
        let nearby = text[host_index..].chars().take(2000).collect::<String>();
        return token_line(&nearby);
    }
    token_line(text)
}

fn token_line(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        for key in ["token", "access_token"] {
            if lower.starts_with(key) {
                if let Some(token) = parse_token_tail(&trimmed[key.len()..]) {
                    return Some(token);
                }
            }
        }
    }
    None
}

fn parse_token_tail(value: &str) -> Option<String> {
    let value = value.trim_start();
    let value = value
        .strip_prefix(':')
        .or_else(|| value.strip_prefix('='))?
        .trim_start();
    let value = value
        .strip_prefix('"')
        .or_else(|| value.strip_prefix('\''))
        .unwrap_or(value);
    let token = value
        .chars()
        .take_while(|character| !matches!(character, '"' | '\'' | '#' | ' ' | '\t' | '\r' | '\n'))
        .collect::<String>();
    (!token.is_empty()).then_some(token)
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
    platform.unwrap_or(current_platform()) == "darwin" && executable_in_path("security")
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

        assert_eq!(token_from_env(&env).as_deref(), Some("shim"));
    }

    #[test]
    fn discovers_macos_fj_keys_json_token_for_host() -> Result<()> {
        let home = temp_root()?;
        let path = home
            .join("Library")
            .join("Application Support")
            .join("Cyborus.forgejo-cli");
        fs::create_dir_all(&path).map_err(|error| ShimError::new(error.to_string()))?;
        fs::write(
            path.join("keys.json"),
            r#"{"hosts":{"git.deltaisland.io":{"type":"token","name":"dirtydishes","token":"secret-token"},"git.example.com":{"token":"other-token"}}}"#,
        )
        .map_err(|error| ShimError::new(error.to_string()))?;

        let token = discover_fj_token(Some("git.deltaisland.io"), &EnvMap::new(), Some(&home));

        assert_eq!(token.as_deref(), Some("secret-token"));
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn discovers_yaml_token_for_matching_host() -> Result<()> {
        let home = temp_root()?;
        let path = home.join(".config").join("tea");
        fs::create_dir_all(&path).map_err(|error| ShimError::new(error.to_string()))?;
        fs::write(
            path.join("config.yml"),
            "servers:\n  - host: https://git.example.com/path\n    token: yaml-token\n",
        )
        .map_err(|error| ShimError::new(error.to_string()))?;

        let token = discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home));

        assert_eq!(token.as_deref(), Some("yaml-token"));
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn env_token_wins_over_external_config() -> Result<()> {
        let home = temp_root()?;
        let path = home
            .join("Library")
            .join("Application Support")
            .join("Cyborus.forgejo-cli");
        fs::create_dir_all(&path).map_err(|error| ShimError::new(error.to_string()))?;
        fs::write(
            path.join("keys.json"),
            r#"{"hosts":{"git.deltaisland.io":{"token":"file-token"}}}"#,
        )
        .map_err(|error| ShimError::new(error.to_string()))?;
        let env = env([("FJ_SHIM_TOKEN", "env-token")]);

        let token = discover_fj_token(Some("git.deltaisland.io"), &env, Some(&home));

        assert_eq!(token.as_deref(), Some("env-token"));
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn host_specific_discovery_ignores_unrelated_file_tokens() -> Result<()> {
        let home = temp_root()?;
        let path = home
            .join("Library")
            .join("Application Support")
            .join("Cyborus.forgejo-cli");
        fs::create_dir_all(&path).map_err(|error| ShimError::new(error.to_string()))?;
        fs::write(
            path.join("keys.json"),
            r#"{"hosts":{"git.other.test":{"token":"other-token"}}}"#,
        )
        .map_err(|error| ShimError::new(error.to_string()))?;

        let token = discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home));

        assert!(token.is_none());
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn stores_and_discovers_shim_auth_file_token() -> Result<()> {
        let home = temp_root()?;
        let storage = write_stored_token(
            "https://Git.Example.com/path",
            "stored-token",
            Some(&home),
            Some("linux"),
        )?;

        let token = discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home));

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

        let token = discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home));

        assert_eq!(token.as_deref(), Some("old-token"));
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn env_token_wins_over_stored_shim_auth() -> Result<()> {
        let home = temp_root()?;
        write_stored_token(
            "git.example.com",
            "stored-token",
            Some(&home),
            Some("linux"),
        )?;
        let env = env([("FJ_SHIM_TOKEN", "env-token")]);

        let token = discover_fj_token(Some("git.example.com"), &env, Some(&home));

        assert_eq!(token.as_deref(), Some("env-token"));
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn import_discovery_excludes_existing_shim_auth() -> Result<()> {
        let home = temp_root()?;
        write_stored_token(
            "git.example.com",
            "stored-token",
            Some(&home),
            Some("linux"),
        )?;

        let discovery = discover_import_token("git.example.com", &EnvMap::new(), Some(&home));

        assert!(discovery.is_none());
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
        let token = discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home));

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
        assert!(discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home)).is_none());
        assert_eq!(
            discover_fj_token(Some("git.other.test"), &EnvMap::new(), Some(&home)).as_deref(),
            Some("other-token")
        );
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn text_token_scan_handles_utf8_near_scan_boundary() {
        let padding = "a".repeat(2000 - "git.example.com".len() - 1);
        let text = format!("git.example.com{padding}é\ntoken: secret");

        assert_eq!(find_token_in_text(&text, Some("git.example.com")), None);
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
