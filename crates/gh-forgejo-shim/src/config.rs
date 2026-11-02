//! Configuration loading and host allowlisting.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml_edit::{Array, DocumentMut, Item, Value};

use crate::{shim, Result, ShimError};

pub type EnvMap = BTreeMap<String, String>;

pub const KNOWN_GITHUB_HOSTS: &[&str] = &["github.com", "www.github.com"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathsConfig {
    pub gh: Option<String>,
    pub fj: Option<String>,
}

/// Per-host tables use `host`, singular, so they can coexist with the legacy
/// `hosts = [...]` array. A table also adds its key to the host allowlist.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct HostConfig {
    pub api_url: Option<String>,
    pub token_env: Option<String>,
    pub ca_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub hosts: Vec<String>,
    pub host: BTreeMap<String, HostConfig>,
    pub paths: PathsConfig,
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    host: BTreeMap<String, HostConfig>,
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
        is_forgejo_host(&self.hosts, host)
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
        .unwrap_or_else(|| config_path(env.get("HOME").map(Path::new)));

    let mut raw = RawConfig {
        host: BTreeMap::new(),
        hosts: None,
        paths: None,
    };
    if config_path.exists() {
        let text = fs::read_to_string(&config_path).map_err(|error| {
            ShimError::new(format!("could not read {}: {error}", config_path.display()))
        })?;
        raw = toml::from_str(&text).map_err(|error| parse_error(&config_path, &text, &error))?;
    }

    let mut hosts = raw
        .hosts
        .unwrap_or_default()
        .into_iter()
        .filter_map(|host| normalized_forgejo_host(&host))
        .collect::<Vec<_>>();

    let mut host_settings = BTreeMap::new();
    for (host, mut settings) in raw.host {
        let Some(host) = normalized_forgejo_host(&host) else {
            continue;
        };
        settings.api_url = optional_string(settings.api_url.as_deref());
        settings.token_env = optional_string(settings.token_env.as_deref());
        if let Some(base) = &settings.api_url {
            let url = reqwest::Url::parse(base).map_err(|_| {
                ShimError::new(format!(
                    "invalid api_url for {host}: expected an HTTP or HTTPS base URL"
                ))
            })?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(ShimError::new(format!("invalid api_url for {host}: expected an HTTP or HTTPS base URL without credentials, query, or fragment")));
            }
            settings.api_url = Some(base.trim_end_matches('/').to_string());
        }
        // A relative path would resolve against whatever repo the shim runs in.
        if settings
            .ca_file
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err(ShimError::new(format!(
                "invalid ca_file for {host}: expected an absolute path"
            )));
        }
        hosts.push(host.clone());
        if host_settings.insert(host.clone(), settings).is_some() {
            return Err(ShimError::new(format!(
                "duplicate host settings for {host}"
            )));
        }
    }

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
        host: host_settings,
        paths: PathsConfig { gh, fj },
        path: config_path,
    })
}

/// Names the line but never quotes the source, which can hold a URL password. The message itself
/// can quote it too: serde puts a mistyped value in double quotes, and the parser puts a
/// duplicate key or a table path in backticks. Both are cut.
fn parse_error(path: &Path, text: &str, error: &toml::de::Error) -> ShimError {
    let mut message = error.message().to_string();
    for quote in ['"', '`'] {
        cut_quoted(&mut message, quote);
    }
    match error.span().and_then(|span| text.get(..span.start)) {
        Some(before) => ShimError::new(format!(
            "could not parse {} at line {}: {message}",
            path.display(),
            before.matches('\n').count() + 1
        )),
        None => ShimError::new(format!("could not parse {}: {message}", path.display())),
    }
}

/// Replaces everything between the first and last `quote` with `...`. Pairing quotes up would let
/// a value with quotes of its own slip through. Double quotes always hold a value, so they're
/// always cut. Backticks also hold the parser's expected tokens (`` expected `]` ``), kept when the
/// text is nothing but TOML syntax characters.
fn cut_quoted(message: &mut String, quote: char) {
    let (Some(start), Some(end)) = (message.find(quote), message.rfind(quote)) else {
        return;
    };
    let inner = start + quote.len_utf8()..end;
    let syntax_only = quote == '`'
        && message[inner.clone()]
            .chars()
            .all(|character| "[]{}=,.`'\" ".contains(character));
    if start < end && !syntax_only {
        message.replace_range(inner, "...");
    }
}

pub fn add_host(host: &str, path: Option<&Path>) -> Result<Config> {
    let mut config = load_config_with_env(path, &EnvMap::new())?;
    if let Some(normalized) = normalized_forgejo_host(host) {
        if !config.hosts.contains(&normalized) {
            config.hosts.push(normalized);
        }
    }
    config.hosts.sort();
    write_hosts(&config, None)?;
    Ok(config)
}

pub fn remove_host(host: &str, path: Option<&Path>) -> Result<Config> {
    let mut config = load_config_with_env(path, &EnvMap::new())?;
    let normalized = normalize_host(host);
    config.host.remove(&normalized);
    config
        .hosts
        .retain(|configured| normalize_host(configured) != normalized);
    write_hosts(&config, Some(&normalized))?;
    Ok(config)
}

/// Rewrites the `hosts` allowlist in the file at `config.path`, and drops the `[host.…]` table
/// for `removed`. Everything else stays as the user wrote it: comments, `[paths]`, and keys and
/// tables this version doesn't know.
fn write_hosts(config: &Config, removed: Option<&str>) -> Result<()> {
    let path = &config.path;
    let parent = path.parent().ok_or_else(|| {
        ShimError::new(format!(
            "could not determine parent directory for {}",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        ShimError::new(format!("could not create {}: {error}", parent.display()))
    })?;

    let mut document = match fs::read_to_string(path) {
        // The caller has already loaded this file, so its parse errors were reported there.
        Ok(text) => text
            .parse::<DocumentMut>()
            .map_err(|_| ShimError::new(format!("could not parse {}", path.display())))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => DocumentMut::new(),
        Err(error) => {
            return Err(ShimError::new(format!(
                "could not read {}: {error}",
                path.display()
            )))
        }
    };
    set_hosts(&mut document, &config.hosts);
    if let (Some(removed), Some(tables)) = (
        removed,
        document.get_mut("host").and_then(Item::as_table_like_mut),
    ) {
        let keys = tables
            .iter()
            .map(|(key, _)| key.to_string())
            .filter(|key| normalize_host(key) == removed)
            .collect::<Vec<_>>();
        for key in keys {
            tables.remove(&key);
        }
    }

    replace_config_file(path, document.to_string().as_bytes())
}

/// Makes the `hosts` array hold `hosts`. An existing array is edited in place, so its comments,
/// spellings and order stay with the hosts they describe: dropped hosts are removed, new ones
/// appended.
fn set_hosts(document: &mut DocumentMut, hosts: &[String]) {
    let Some(array) = document.get_mut("hosts").and_then(Item::as_array_mut) else {
        document["hosts"] = toml_edit::value(hosts.iter().collect::<Array>());
        return;
    };
    let mut kept = Vec::<String>::new();
    let mut dropped = Vec::new();
    for (index, value) in array.iter().enumerate() {
        match value.as_str().and_then(normalized_forgejo_host) {
            Some(host) if hosts.contains(&host) && !kept.contains(&host) => kept.push(host),
            _ => dropped.push(index),
        }
    }
    // Last first, so a run of removals hands each line's comment along in turn.
    for index in dropped.into_iter().rev() {
        remove_keeping_line_comments(array, index);
    }
    for host in hosts.iter().filter(|host| !kept.contains(host)) {
        let trailing = array.trailing().as_str().unwrap_or_default().to_string();
        if !trailing.contains('\n') {
            array.push(host.as_str());
            continue;
        }
        // One host per line: indent like the last one, after any comment that ended its line.
        let indent = array
            .iter()
            .last()
            .and_then(|value| value.decor().prefix())
            .and_then(|prefix| prefix.as_str())
            .and_then(|prefix| prefix.rsplit('\n').next())
            .unwrap_or("  ")
            .to_string();
        let mut value = Value::from(host.as_str());
        value.decor_mut().set_prefix(format!(
            "{}{indent}",
            trailing.trim_end_matches([' ', '\t'])
        ));
        array.push_formatted(value);
        array.set_trailing("\n");
    }
}

/// Removes element `index`, keeping the line-end comment of the element before it. In a
/// multi-line array, the text up to the first line break of an element's prefix ends the line
/// above: it holds that element's `# comment`. Removing the element would lose it, and leave the
/// removed element's own comment, held by whatever follows, on the line above instead.
fn remove_keeping_line_comments(array: &mut Array, index: usize) {
    let removed = array.remove(index);
    let prefix = removed
        .decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default();
    let Some(line_end) = prefix.find('\n') else {
        return;
    };
    let carried = &prefix[..=line_end];
    let replace_head = |text: &str| match text.find('\n') {
        Some(end) => format!("{carried}{}", &text[end + 1..]),
        None => text.to_string(),
    };
    if let Some(next) = array.get_mut(index) {
        let text = next
            .decor()
            .prefix()
            .and_then(|prefix| prefix.as_str())
            .unwrap_or_default()
            .to_string();
        next.decor_mut().set_prefix(replace_head(&text));
    } else {
        let text = array.trailing().as_str().unwrap_or_default().to_string();
        array.set_trailing(replace_head(&text));
    }
}

/// Writes a new file and renames it into place, so a failed write leaves the old config whole.
/// A symlinked config is replaced at its target, so a dotfiles link survives, and the file keeps
/// its mode.
fn replace_config_file(path: &Path, contents: &[u8]) -> Result<()> {
    let could_not_write = |error: &dyn std::fmt::Display| {
        ShimError::new(format!("could not write {}: {error}", path.display()))
    };
    let target = config_file_target(path);
    let permissions = fs::metadata(&target)
        .ok()
        .map(|metadata| metadata.permissions());
    shim::replace_file(&target, contents, false).map_err(|error| could_not_write(&error))?;
    if let Some(permissions) = permissions {
        fs::set_permissions(&target, permissions).map_err(|error| could_not_write(&error))?;
    }
    Ok(())
}

/// The file a config write replaces: the end of any symlink chain, so a dotfiles link survives.
/// A dangling chain is followed to its missing end, which is created, rather than a link in it
/// being replaced. Like the kernel, it gives up after 40 links.
fn config_file_target(path: &Path) -> PathBuf {
    if let Ok(target) = fs::canonicalize(path) {
        return target;
    }
    let mut target = path.to_path_buf();
    for _ in 0..40 {
        let Ok(link) = fs::read_link(&target) else {
            break;
        };
        target = target
            .parent()
            .map_or(link.clone(), |parent| parent.join(&link));
    }
    target
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

pub fn is_forgejo_host(hosts: &[String], host: Option<&str>) -> bool {
    let Some(host) = host else { return false };
    let normalized = normalize_host(host);
    !is_known_github_host(Some(&normalized))
        && hosts
            .iter()
            .any(|configured| normalize_host(configured) == normalized)
}

pub fn is_known_github_host(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let normalized = normalize_host(host);
    // Userinfo, a port or a trailing dot still reach GitHub, so none of them may make it a
    // Forgejo host that receives the Forgejo token.
    let name = normalized
        .rsplit_once('@')
        .map_or(normalized.as_str(), |(_, name)| name);
    let name = name.rsplit_once(':').map_or(name, |(name, _)| name);
    let name = name.trim_end_matches('.');
    // Every github.com subdomain is GitHub's (api., gist., uploads. and so on), so none is a
    // Forgejo host either.
    KNOWN_GITHUB_HOSTS.contains(&name) || name.ends_with(".github.com")
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
    fn multiline_hosts_and_quoted_hash_use_toml_syntax() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(
            &path,
            r#"
hosts = [
    "Git.Example.com",
    "git.example.com", # duplicate
    "code.example.org",
]
[paths]
gh = "/bin/gh#real"
"#,
        )?;

        let config = load_config_with_env(Some(&path), &EnvMap::new())?;
        assert_eq!(config.hosts, ["git.example.com", "code.example.org"]);
        assert_eq!(config.paths.gh.as_deref(), Some("/bin/gh#real"));
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn host_tables_allowlist_and_survive_config_edits() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(
            &path,
            r#"
hosts = ["legacy.example.com"]
[host."https://Git.Example.com/path"]
api_url = "http://127.0.0.1:3000/forgejo/"
token_env = "EXAMPLE_TOKEN"
ca_file = "/tmp/ca#root.pem"
[host."table-only.example.com"]
[paths]
gh = "/bin/gh#real"
"#,
        )?;
        let config = load_config_with_env(Some(&path), &EnvMap::new())?;
        assert_eq!(
            config.hosts,
            [
                "legacy.example.com",
                "git.example.com",
                "table-only.example.com"
            ]
        );
        let settings = &config.host["git.example.com"];
        assert_eq!(
            settings.api_url.as_deref(),
            Some("http://127.0.0.1:3000/forgejo")
        );
        assert_eq!(settings.token_env.as_deref(), Some("EXAMPLE_TOKEN"));
        assert_eq!(
            settings.ca_file.as_deref(),
            Some(Path::new("/tmp/ca#root.pem"))
        );

        add_host("new.example.com", Some(&path))?;
        let updated = load_config_with_env(Some(&path), &EnvMap::new())?;
        assert_eq!(updated.host, config.host);
        assert_eq!(updated.paths, config.paths);
        let removed = remove_host("git.example.com", Some(&path))?;
        assert!(!removed.host.contains_key("git.example.com"));
        assert!(!load_config_with_env(Some(&path), &EnvMap::new())?
            .is_forgejo_host(Some("git.example.com")));
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn host_edits_keep_comments_and_unknown_settings() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(
            &path,
            r#"# my forges
hosts = ["git.example.com"] # the allowlist
future = "keep"

[host."Old.Example.com"]
future_setting = "goes with its host"

[host."git.example.com"]
future_setting = "keep" # why
"#,
        )?;

        add_host("new.example.com", Some(&path))?;
        remove_host("old.example.com", Some(&path))?;

        assert_eq!(
            fs::read_to_string(&path)?,
            r#"# my forges
hosts = ["git.example.com", "new.example.com"] # the allowlist
future = "keep"

[host."git.example.com"]
future_setting = "keep" # why
"#
        );
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn host_edits_keep_comments_inside_a_multiline_hosts_array() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(
            &path,
            "hosts = [\n  # keep this forge\n  \"git.example.com\", # important reason\n]\n",
        )?;

        add_host("new.example.com", Some(&path))?;

        assert_eq!(
            fs::read_to_string(&path)?,
            "hosts = [\n  # keep this forge\n  \"git.example.com\", # important reason\n  \"new.example.com\",\n]\n"
        );
        assert_eq!(
            load_config_with_env(Some(&path), &EnvMap::new())?.hosts,
            ["git.example.com", "new.example.com"]
        );
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn removing_a_host_keeps_the_comments_of_the_hosts_that_stay() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        fs::write(
            &path,
            "hosts = [ # forges\n  \"a.example.com\", # a reason\n  # about b\n  \"b.example.com\", # b reason\n  \"c.example.com\", # c reason\n  \"d.example.com\", # d reason\n]\n",
        )?;

        remove_host("b.example.com", Some(&path))?;
        remove_host("d.example.com", Some(&path))?;

        assert_eq!(
            fs::read_to_string(&path)?,
            "hosts = [ # forges\n  \"a.example.com\", # a reason\n  \"c.example.com\", # c reason\n]\n"
        );
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn rejects_invalid_base_urls_and_duplicate_normalized_tables() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        for base in [
            "ftp://git.example.com",
            "https://user:secret@git.example.com",
            "https://git.example.com/?token=secret",
            "https://git.example.com/#secret",
            "not a URL",
        ] {
            fs::write(
                &path,
                format!("[host.\"git.example.com\"]\napi_url = {base:?}\n"),
            )?;
            let error = load_config_with_env(Some(&path), &EnvMap::new()).expect_err("invalid URL");
            assert!(error.to_string().contains("invalid api_url"));
            assert!(!error.to_string().contains("secret"));
        }
        fs::write(
            &path,
            "[host.\"Git.Example.com\"]\n[host.\"git.example.com\"]\n",
        )?;
        assert!(load_config_with_env(Some(&path), &EnvMap::new())
            .expect_err("duplicate host")
            .to_string()
            .contains("duplicate host settings"));
        fs::write(&path, "[host.\"git.example.com\"]\nca_file = \"ca.pem\"\n")?;
        assert!(load_config_with_env(Some(&path), &EnvMap::new())
            .expect_err("relative ca_file")
            .to_string()
            .contains("expected an absolute path"));
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn parse_errors_name_the_line_without_quoting_values() -> Result<()> {
        let root = temp_root()?;
        let path = root.join("config.toml");
        for (text, line) in [
            (
                "hosts = []\nhost = \"https://user:secret@git.example.com\"\n",
                2,
            ),
            ("hosts = []\n[paths]\ngh = \"/bin/secret\n", 3),
            // A mistyped value made only of punctuation is still a value.
            ("hosts = []\nhost = \"!@#$%^&*\"\n", 2),
            // The parser puts duplicate keys in backticks, and a key can hold backticks too.
            (
                "'`https://u:secret@x.com`' = 1\n'`https://u:secret@x.com`' = 1\n",
                2,
            ),
            (
                "[host]\n'https://u:secret@x.com' = {}\n'https://u:secret@x.com' = {}\n",
                3,
            ),
        ] {
            fs::write(&path, text)?;
            let error = load_config_with_env(Some(&path), &EnvMap::new())
                .expect_err("invalid config")
                .to_string();
            assert!(error.contains(&format!("at line {line}:")), "{error}");
            assert!(!error.contains("secret"), "{error}");
        }
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
    fn github_with_userinfo_a_port_or_a_trailing_dot_is_still_github() {
        for host in [
            "github.com:443",
            "GitHub.com:8443",
            "github.com.",
            "www.github.com.:443",
            "user@github.com",
            "https://github.com:443/owner/repo",
            "api.github.com",
            "GIST.GitHub.com.:443",
            "https://uploads.github.com/repos/o/r",
        ] {
            assert!(is_known_github_host(Some(host)), "{host}");
        }
        for host in [
            "github.com.example.org",
            "notgithub.com",
            "evilgithub.com",
            "github.com@git.example.com",
            "git.example.com:443",
        ] {
            assert!(!is_known_github_host(Some(host)), "{host}");
        }
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

    #[cfg(unix)]
    #[test]
    fn config_edits_keep_a_symlinked_config_and_its_mode() -> Result<()> {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = temp_root()?;
        let target = root.join("dotfiles-config.toml");
        fs::write(&target, "hosts = [\"git.example.com\"]\n")?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;
        let path = root.join("config.toml");
        symlink(&target, &path)?;

        add_host("new.example.com", Some(&path))?;

        assert!(fs::symlink_metadata(&path)?.file_type().is_symlink());
        assert_eq!(fs::metadata(&target)?.permissions().mode() & 0o777, 0o600);
        assert!(fs::read_to_string(&target)?.contains("new.example.com"));
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn config_edits_through_a_dangling_symlink_create_its_target() -> Result<()> {
        use std::os::unix::fs::symlink;

        let root = temp_root()?;
        let path = root.join("config.toml");
        symlink("middle.toml", &path)?;
        symlink("dotfiles-config.toml", root.join("middle.toml"))?;

        add_host("new.example.com", Some(&path))?;

        for link in [&path, &root.join("middle.toml")] {
            assert!(
                fs::symlink_metadata(link)?.file_type().is_symlink(),
                "{}",
                link.display()
            );
        }
        assert!(fs::read_to_string(root.join("dotfiles-config.toml"))?.contains("new.example.com"));
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
