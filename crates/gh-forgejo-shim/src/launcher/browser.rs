//! The browser helper a launched gh opens URLs through (launcher spec §9). The gateway's links
//! say `/pull/N`, as GitHub's do; Forgejo serves `/pulls/N`. With no proxy to redirect in client
//! mode, `GH_BROWSER` names this helper, which rewrites a served host's PR page and opens the
//! result with the browser the user would have got.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use url::Url;

use crate::launcher::ghconfig::single_quote;
use crate::repo::{gh_hostname, shell_split};

/// The hidden subcommand `GH_BROWSER` names.
pub const SUBCOMMAND: &str = "__gfj-browser";

/// `GH_BROWSER` for a launched gh: the shim, single-quoted for gh's shlex split, the hidden
/// subcommand, and the served entries. gh appends the URL.
pub fn gh_browser(shim: &Path, served: &[String]) -> OsString {
    let mut value = OsString::from(single_quote(&shim.to_string_lossy()));
    value.push(format!(" {SUBCOMMAND}"));
    for entry in served {
        value.push(format!(" {entry}"));
    }
    value
}

/// The helper itself: `args` are the served entries, then the URL gh appended. Opens the URL,
/// rewritten if it is a served PR page, with the browser the user would have got, and exits
/// with that program's status.
pub fn run(args: &[OsString]) -> i32 {
    let Some((url, entries)) = args.split_last() else {
        eprintln!("gh-forgejo-shim: {SUBCOMMAND} needs a URL");
        return 1;
    };
    let served: Vec<String> = entries
        .iter()
        .map(|entry| entry.to_string_lossy().into_owned())
        .collect();
    let target = rewrite(&url.to_string_lossy(), &served);
    let shim = std::env::current_exe().ok();
    let not_self =
        |command: Option<String>| command.filter(|command| !is_self(command, shim.as_deref()));
    let command = browser_command(
        not_self(std::env::var("FJ_SHIM_ORIG_GH_BROWSER").ok()).as_deref(),
        not_self(config_browser()).as_deref(),
        not_self(std::env::var("BROWSER").ok()).as_deref(),
    );
    let Some(words) = shell_split(&command).filter(|words| !words.is_empty()) else {
        eprintln!("gh-forgejo-shim: can't read the browser command {command:?}");
        return 1;
    };
    match std::process::Command::new(&words[0])
        .args(&words[1..])
        .arg(&target)
        .status()
    {
        Ok(status) => crate::external::inherited_exit_code(status),
        Err(error) => {
            eprintln!("gh-forgejo-shim: could not run {}: {error}", words[0]);
            1
        }
    }
}

/// The `browser` key in gh's `config.yml`: the run directory's copy of the user's.
fn config_browser() -> Option<String> {
    let dir = std::env::var_os("GH_CONFIG_DIR")?;
    let text = std::fs::read_to_string(Path::new(&dir).join("config.yml")).ok()?;
    let config: serde_yaml::Value = serde_yaml::from_str(&text).ok()?;
    config.get("browser")?.as_str().map(str::to_string)
}

/// A browser command that would run this helper again: the shim's own binary, or anything
/// naming the hidden subcommand. The helper skips it, never calling itself.
pub fn is_self(command: &str, shim: Option<&Path>) -> bool {
    let Some(words) = shell_split(command) else {
        return false;
    };
    if words.iter().any(|word| word == SUBCOMMAND) {
        return true;
    }
    let (Some(first), Some(shim)) = (words.first(), shim) else {
        return false;
    };
    let canonical = |path: &Path| std::fs::canonicalize(path).ok();
    canonical(Path::new(first)).is_some_and(|first| Some(first) == canonical(shim))
        || OsStr::new(first) == shim.as_os_str()
}

/// The URL to open: `/<owner>/<repo>/pull/<N>`, optionally followed by `/…`, a query or a
/// fragment, becomes `/pulls/<N>` when the host is a served one; anything else is unchanged.
/// `served` holds allowlist entries; a ported entry matches its URL with or without the port,
/// the two spellings the gateway admits (§5).
pub fn rewrite(url: &str, served: &[String]) -> String {
    let Ok(mut parsed) = Url::parse(url) else {
        return url.to_string();
    };
    if !matches!(parsed.scheme(), "https" | "http") || !served_host(&parsed, served) {
        return url.to_string();
    }
    let segments: Vec<&str> = parsed.path().trim_start_matches('/').split('/').collect();
    let is_pr = segments.len() >= 4
        && !segments[0].is_empty()
        && !segments[1].is_empty()
        && segments[2] == "pull"
        && !segments[3].is_empty()
        && segments[3].bytes().all(|byte| byte.is_ascii_digit());
    if !is_pr {
        return url.to_string();
    }
    let mut rewritten = segments.clone();
    rewritten[2] = "pulls";
    parsed.set_path(&format!("/{}", rewritten.join("/")));
    parsed.to_string()
}

fn served_host(url: &Url, served: &[String]) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let with_port = url.port().map(|port| format!("{host}:{port}"));
    served.iter().any(|entry| {
        let entry = entry.to_ascii_lowercase();
        match &with_port {
            Some(spelled) => *spelled == entry,
            // No port in the URL: the entry's hostname, ported or not.
            None => gh_hostname(&entry) == host,
        }
    })
}

/// The browser command the user would have got, in gh's order: their own `GH_BROWSER` (saved by
/// the launcher), the `browser` key in their gh `config.yml`, `BROWSER`, then the platform's
/// opener.
pub fn browser_command(
    original_gh_browser: Option<&str>,
    config_browser: Option<&str>,
    browser_env: Option<&str>,
) -> String {
    [original_gh_browser, config_browser, browser_env]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|command| !command.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "open".to_string()
            } else {
                "xdg-open".to_string()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn served() -> Vec<String> {
        vec!["forge.test".to_string(), "port.test:3443".to_string()]
    }

    #[test]
    fn a_served_pr_page_becomes_forgejos() {
        for (url, expected) in [
            (
                "https://forge.test/o/r/pull/7",
                "https://forge.test/o/r/pulls/7",
            ),
            (
                "https://forge.test/o/r/pull/7/files",
                "https://forge.test/o/r/pulls/7/files",
            ),
            (
                "https://forge.test/o/r/pull/7?x=1#c",
                "https://forge.test/o/r/pulls/7?x=1#c",
            ),
            (
                "https://Forge.Test/o/r/pull/7",
                "https://forge.test/o/r/pulls/7",
            ),
            (
                "https://port.test:3443/o/r/pull/7",
                "https://port.test:3443/o/r/pulls/7",
            ),
            (
                "https://port.test/o/r/pull/7",
                "https://port.test/o/r/pulls/7",
            ),
        ] {
            assert_eq!(rewrite(url, &served()), expected, "{url}");
        }
    }

    #[test]
    fn everything_else_passes_through() {
        for url in [
            "https://github.com/o/r/pull/7",
            "https://forge.test/o/r/pulls/7",
            "https://forge.test/o/r/issues/7",
            "https://forge.test/o/r/pull/new",
            "https://forge.test/o/r/pull/",
            "https://forge.test/o/r",
            "https://port.test:8443/o/r/pull/7",
            "https://forge.test.evil/o/r/pull/7",
            "file:///forge.test/o/r/pull/7",
            "not a url",
        ] {
            assert_eq!(rewrite(url, &served()), url, "{url}");
        }
    }

    #[test]
    fn the_helper_recognises_itself() {
        let shim = std::env::current_exe().expect("exe");
        let quoted = format!("'{}' {SUBCOMMAND} forge.test", shim.display());
        assert!(is_self(&quoted, Some(&shim)));
        assert!(is_self(&shim.to_string_lossy(), Some(&shim)));
        assert!(is_self(
            &format!("/elsewhere/gfj {SUBCOMMAND}"),
            Some(&shim)
        ));
        assert!(!is_self("firefox --new-tab", Some(&shim)));
        assert!(!is_self("open", None));
    }

    #[test]
    fn gh_browser_names_the_shim_the_helper_and_the_served_hosts() {
        let value = gh_browser(
            Path::new("/opt/it's/gfj"),
            &["forge.test".to_string(), "port.test:3443".to_string()],
        );
        assert_eq!(
            value,
            OsString::from(format!(
                "'/opt/it'\\''s/gfj' {SUBCOMMAND} forge.test port.test:3443"
            ))
        );
        let words = shell_split(&value.to_string_lossy()).expect("split");
        assert_eq!(
            words,
            ["/opt/it's/gfj", SUBCOMMAND, "forge.test", "port.test:3443"]
        );
    }

    #[test]
    fn the_browser_follows_ghs_order() {
        let default = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(
            browser_command(Some("firefox"), Some("chrome"), Some("lynx")),
            "firefox"
        );
        assert_eq!(
            browser_command(None, Some("chrome"), Some("lynx")),
            "chrome"
        );
        assert_eq!(browser_command(Some(" "), None, Some("lynx")), "lynx");
        assert_eq!(browser_command(None, None, None), default);
    }
}
