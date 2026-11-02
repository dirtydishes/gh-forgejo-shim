//! The browser helper a launched gh opens URLs through (launcher spec §9). The gateway's links
//! say `/pull/N`, as GitHub's do; Forgejo serves `/pulls/N`. With no proxy to redirect in client
//! mode, `GH_BROWSER` names this helper, which rewrites a served host's PR page and opens the
//! result with the browser the user would have got.

use url::Url;

use crate::repo::gh_hostname;

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
