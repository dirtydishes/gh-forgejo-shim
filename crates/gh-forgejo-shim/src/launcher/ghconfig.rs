//! The files a launched gh reads from its private config directory (launcher spec §3), built as
//! text from the user's own files. Nothing here touches the user's config directory.

use std::path::Path;

use serde_yaml::{Mapping, Value};

use crate::repo::gh_hostname;

/// The token every host but the served ones gets. No server accepts it, and a non-empty token in
/// `hosts.yml` keeps gh from falling back to the Keychain (`gh/internal/config/config.go:301-304`).
pub const PLACEHOLDER: &str = "gfj-not-served";

/// A host in *S*: an allowlist entry this call serves, with its real token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedHost {
    /// The allowlist entry, normalized: `forge.test` or `forge.test:3443`.
    pub entry: String,
    pub real_token: String,
    /// The account name, written as `user` in an `auth status` call (§3).
    pub login: Option<String>,
}

impl ServedHost {
    /// The `hosts.yml` keys gh looks this host up by: the portless hostname, which gh's API
    /// client uses, and `host:port` for a ported entry, which `--hostname host:port` uses.
    pub fn keys(&self) -> Vec<String> {
        entry_keys(&self.entry)
    }

    /// The host as git hands it to a credential helper: `host:port` for a ported entry.
    pub fn git_host(&self) -> &str {
        &self.entry
    }
}

fn entry_keys(entry: &str) -> Vec<String> {
    let hostname = gh_hostname(entry);
    if hostname == entry {
        vec![hostname]
    } else {
        vec![hostname, entry.to_string()]
    }
}

/// Which token a served host's keys hold in `hosts.yml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServedToken<'a> {
    /// Every call but the two below: a token only the gateway accepts.
    PerCall(&'a str),
    /// `gh auth token`, and `gh auth status --show-token`, which print it.
    Real,
}

/// `config.yml` for the launched gh: the user's file with the socket set.
pub fn config_yml(user_config: Option<&str>, socket: &Path) -> Result<String, String> {
    let mut config = match user_config {
        None => Mapping::new(),
        Some(text) if text.trim().is_empty() => Mapping::new(),
        Some(text) => match serde_yaml::from_str::<Value>(text) {
            Ok(Value::Mapping(mapping)) => mapping,
            Ok(Value::Null) => Mapping::new(),
            Ok(_) => return Err("it isn't a YAML mapping".to_string()),
            Err(error) => return Err(error.to_string()),
        },
    };
    let socket = socket
        .to_str()
        .ok_or_else(|| "the socket path isn't UTF-8".to_string())?;
    // go-gh reads `hosts.yml` as a second `hosts` entry and takes the first match, so a legacy
    // `hosts:` key left in `config.yml` would hide the launcher's whole `hosts.yml`.
    config.remove("hosts");
    config.insert("http_unix_socket".into(), socket.into());
    if !config.contains_key("version") {
        config.insert("version".into(), "1".into());
    }
    serde_yaml::to_string(&Value::Mapping(config)).map_err(|error| error.to_string())
}

/// The user's `hosts.yml`, parsed. An error for a file gh couldn't read either: every host in it
/// needs a placeholder, so a launch can't go ahead without knowing them (§3, §6).
pub fn user_hosts(text: Option<&str>) -> Result<Mapping, String> {
    match text {
        None => Ok(Mapping::new()),
        Some(text) if text.trim().is_empty() => Ok(Mapping::new()),
        Some(text) => match serde_yaml::from_str::<Value>(text) {
            Ok(Value::Mapping(mapping)) => Ok(mapping),
            Ok(Value::Null) => Ok(Mapping::new()),
            Ok(_) => Err("it isn't a YAML mapping".to_string()),
            Err(error) => Err(error.to_string()),
        },
    }
}

/// `hosts.yml` for the launched gh. Keys: the user's own, as written, then each allowlist
/// entry's keys not already there. Served keys get `token`, every other key the placeholder.
pub fn hosts_yml(
    user: &Mapping,
    allowlist: &[String],
    served: &[ServedHost],
    token: ServedToken<'_>,
) -> String {
    let mut keys: Vec<String> = user
        .keys()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    for entry in allowlist {
        for key in entry_keys(entry) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }

    let mut hosts = Mapping::new();
    for key in keys {
        let served_host = served.iter().find(|host| host.keys().contains(&key));
        let mut fields = Mapping::new();
        let value = match (served_host, token) {
            (Some(_), ServedToken::PerCall(per_call)) => per_call,
            (Some(host), ServedToken::Real) => host.real_token.as_str(),
            (None, _) => PLACEHOLDER,
        };
        fields.insert("oauth_token".into(), value.into());
        if let Some(protocol) = user
            .get(key.as_str())
            .and_then(|entry| entry.get("git_protocol"))
            .and_then(Value::as_str)
        {
            fields.insert("git_protocol".into(), protocol.into());
        }
        if let Some(login) = served_host.and_then(|host| host.login.as_deref()) {
            fields.insert("user".into(), login.into());
        }
        hosts.insert(key.into(), Value::Mapping(fields));
    }
    serde_yaml::to_string(&Value::Mapping(hosts)).unwrap_or_default()
}

/// `credentials`: one `<host as git reaches it> <real token>` line per served host, for the
/// launcher's git credential helper (§3, "Git's credentials").
pub fn credentials(served: &[ServedHost]) -> String {
    served
        .iter()
        .map(|host| format!("{} {}\n", host.git_host(), host.real_token))
        .collect()
}

/// Looks a git host up in a `credentials` file's text: exact match only.
pub fn credential_for<'a>(credentials: &'a str, host: &str) -> Option<&'a str> {
    credentials
        .lines()
        .filter_map(|line| line.split_once(' '))
        .find(|(entry, _)| *entry == host)
        .map(|(_, token)| token)
}

/// The `gh` script `GH_PATH` names. The run directory's path is shell-safe (§4); the shim's
/// path is single-quoted.
pub fn gh_script(run_dir: &Path, shim: &Path) -> String {
    format!(
        "#!/bin/sh\nFJ_SHIM_CREDENTIALS='{}/credentials' exec {} gh \"$@\"\n",
        run_dir.display(),
        single_quote(&shim.to_string_lossy())
    )
}

fn single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn served(entry: &str, token: &str) -> ServedHost {
        ServedHost {
            entry: entry.to_string(),
            real_token: token.to_string(),
            login: None,
        }
    }

    fn parse(text: &str) -> Mapping {
        match serde_yaml::from_str::<Value>(text).expect("yaml") {
            Value::Mapping(mapping) => mapping,
            other => panic!("not a mapping: {other:?}"),
        }
    }

    fn token_of(hosts: &Mapping, key: &str) -> String {
        hosts[key]["oauth_token"]
            .as_str()
            .expect("token")
            .to_string()
    }

    #[test]
    fn config_keeps_the_users_settings_and_sets_the_socket() {
        let user = "aliases:\n    co: pr checkout\ngit_protocol: ssh\nversion: \"1\"\n";
        let config = parse(&config_yml(Some(user), Path::new("/t/gfj-x/s")).expect("config"));
        assert_eq!(config["http_unix_socket"].as_str(), Some("/t/gfj-x/s"));
        assert_eq!(config["aliases"]["co"].as_str(), Some("pr checkout"));
        assert_eq!(config["git_protocol"].as_str(), Some("ssh"));
        assert_eq!(config["version"].as_str(), Some("1"));
    }

    #[test]
    fn config_without_a_user_file_has_two_keys() {
        for user in [None, Some(""), Some("# only a comment\n")] {
            let config = parse(&config_yml(user, Path::new("/s")).expect("config"));
            assert_eq!(config.len(), 2, "{user:?}");
            assert_eq!(config["version"].as_str(), Some("1"));
        }
    }

    #[test]
    fn config_drops_a_legacy_hosts_key() {
        let user = "hosts:\n    other.test:\n        oauth_token: legacy\neditor: vim\n";
        let config = parse(&config_yml(Some(user), Path::new("/s")).expect("config"));
        assert!(!config.contains_key("hosts"), "{config:?}");
        assert_eq!(config["editor"].as_str(), Some("vim"));
    }

    #[test]
    fn user_hosts_that_do_not_parse_are_an_error() {
        assert!(user_hosts(Some("github.com: [unclosed\n")).is_err());
        assert!(user_hosts(Some("- a list\n")).is_err());
        assert_eq!(user_hosts(None).expect("none").len(), 0);
        assert_eq!(user_hosts(Some("")).expect("empty").len(), 0);
    }

    #[test]
    fn config_that_does_not_parse_is_an_error() {
        assert!(config_yml(Some("aliases: [unclosed\n"), Path::new("/s")).is_err());
        assert!(config_yml(Some("- a list\n"), Path::new("/s")).is_err());
    }

    #[test]
    fn hosts_keep_user_keys_as_written_and_add_allowlist_keys() {
        let user = "github.com:\n    oauth_token: real-gh\n    git_protocol: ssh\nForge.Test:\n    git_protocol: https\n";
        let allowlist = vec!["forge.test".to_string(), "port.test:3443".to_string()];
        let s = vec![
            served("forge.test", "forge-real"),
            served("port.test:3443", "port-real"),
        ];
        let hosts = parse(&hosts_yml(
            &user_hosts(Some(user)).expect("hosts"),
            &allowlist,
            &s,
            ServedToken::PerCall("pc"),
        ));
        let keys: Vec<_> = hosts.keys().filter_map(Value::as_str).collect();
        assert_eq!(
            keys,
            [
                "github.com",
                "Forge.Test",
                "forge.test",
                "port.test",
                "port.test:3443"
            ]
        );
        assert_eq!(token_of(&hosts, "github.com"), PLACEHOLDER);
        assert_eq!(hosts["github.com"]["git_protocol"].as_str(), Some("ssh"));
        assert_eq!(
            token_of(&hosts, "Forge.Test"),
            PLACEHOLDER,
            "a different spelling isn't served"
        );
        assert_eq!(hosts["Forge.Test"]["git_protocol"].as_str(), Some("https"));
        for key in ["forge.test", "port.test", "port.test:3443"] {
            assert_eq!(token_of(&hosts, key), "pc", "{key}");
        }
        assert!(!hosts_yml(
            &user_hosts(Some(user)).expect("hosts"),
            &allowlist,
            &s,
            ServedToken::PerCall("pc")
        )
        .contains("real"));
    }

    #[test]
    fn real_tokens_only_when_asked() {
        let allowlist = vec!["forge.test".to_string(), "port.test:3443".to_string()];
        let s = vec![
            served("forge.test", "forge-real"),
            served("port.test:3443", "port-real"),
        ];
        let hosts = parse(&hosts_yml(
            &Mapping::new(),
            &allowlist,
            &s,
            ServedToken::Real,
        ));
        assert_eq!(token_of(&hosts, "forge.test"), "forge-real");
        assert_eq!(token_of(&hosts, "port.test"), "port-real");
        assert_eq!(token_of(&hosts, "port.test:3443"), "port-real");
    }

    #[test]
    fn unserved_allowlist_entries_get_the_placeholder() {
        // Two entries sharing a hostname are never in S (§3), but their keys still exist.
        let allowlist = vec!["forge.test:3443".to_string(), "forge.test:8443".to_string()];
        let hosts = parse(&hosts_yml(
            &Mapping::new(),
            &allowlist,
            &[],
            ServedToken::PerCall("pc"),
        ));
        let keys: Vec<_> = hosts.keys().filter_map(Value::as_str).collect();
        assert_eq!(keys, ["forge.test", "forge.test:3443", "forge.test:8443"]);
        for key in keys {
            assert_eq!(token_of(&hosts, key), PLACEHOLDER);
        }
    }

    #[test]
    fn auth_status_logins_become_user() {
        let mut host = served("forge.test", "real");
        host.login = Some("kell".to_string());
        let hosts = parse(&hosts_yml(
            &Mapping::new(),
            &["forge.test".to_string()],
            &[host],
            ServedToken::PerCall("pc"),
        ));
        assert_eq!(hosts["forge.test"]["user"].as_str(), Some("kell"));
    }

    #[test]
    fn credentials_match_hosts_exactly_as_git_reaches_them() {
        let text = credentials(&[served("forge.test", "a"), served("port.test:3443", "b")]);
        assert_eq!(credential_for(&text, "forge.test"), Some("a"));
        assert_eq!(credential_for(&text, "port.test:3443"), Some("b"));
        for other in [
            "port.test",
            "gist.forge.test",
            "Forge.Test",
            "forge.test:443",
            "",
        ] {
            assert_eq!(credential_for(&text, other), None, "{other}");
        }
    }

    #[test]
    fn gh_script_quotes_the_shim_path() {
        let script = gh_script(
            Path::new("/t/gfj-0123456789abcdef"),
            Path::new("/opt/it's/gfj"),
        );
        assert_eq!(
            script,
            "#!/bin/sh\nFJ_SHIM_CREDENTIALS='/t/gfj-0123456789abcdef/credentials' exec '/opt/it'\\''s/gfj' gh \"$@\"\n"
        );
    }
}
