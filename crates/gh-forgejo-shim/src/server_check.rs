//! `gfj server check FORGE`: whether a forge's proxy sends the gateway's routes to fga-server
//! (docs/gateway/server-mode.md). It sends no token, so it runs before anyone logs in, against any
//! forge, and a wrong address costs nothing.

use std::io::Write;
use std::time::Duration;

use reqwest::Method;
use serde_json::Value;

use crate::config::{self, Config, EnvMap};
use crate::forgejo::{Answer, ForgejoClient, CERTIFICATE_REFUSED};
use crate::{Result, ShimError};

/// An owner and repository that can't exist, so Forgejo answers its PR link with `404` whatever
/// the forge holds, and only the gateway redirects it.
const PULL_LINK: &str = "/gfj-check/check/pull/1";

pub(crate) struct Finding {
    name: &'static str,
    ok: bool,
    detail: String,
}

impl Finding {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: true,
            detail: detail.into(),
        }
    }

    fn fix(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: false,
            detail: detail.into(),
        }
    }
}

/// `https://HOST[:PORT]` from `target`, written as `HOST`, `HOST:PORT` or that behind `https://`
/// or `http://`, with nothing after the authority but an optional `/`.
pub(crate) fn origin(target: &str) -> Result<(String, String)> {
    let usage = || {
        ShimError::new(format!(
            "{target} isn't a forge address; write it as HOST, HOST:PORT or https://HOST[:PORT]"
        ))
    };
    let target = target.trim();
    let (scheme, rest) = match target.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => ("https", rest),
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => ("http", rest),
        Some(_) => return Err(usage()),
        None => ("https", target),
    };
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty() || authority.contains(['/', '?', '#', '@', '\\']) {
        return Err(usage());
    }
    let host = config::normalized_forgejo_host(authority).ok_or_else(usage)?;
    Ok((format!("{scheme}://{host}"), host))
}

pub(crate) fn run(
    target: &str,
    config: &Config,
    env: &EnvMap,
    stdout: &mut dyn Write,
) -> Result<i32> {
    let (origin, host) = origin(target)?;
    let client = ForgejoClient::for_host_without_redirects(&host, None, config, env)
        .map_err(|error| ShimError::new(error.message().to_string()))?
        .at_origin(&origin)
        .with_timeout(Duration::from_secs(10));
    let findings = check(&client, &origin, &host);
    writeln!(stdout, "gh-forgejo-shim server check {origin}")?;
    for finding in &findings {
        let mark = if finding.ok { "ok" } else { "fix" };
        writeln!(stdout, "[{mark}] {}: {}", finding.name, finding.detail)?;
    }
    writeln!(stdout)?;
    if findings.iter().all(|finding| finding.ok) {
        writeln!(
            stdout,
            "The gateway answers on {host}. Log gh in with a Forgejo token \
             (gh auth login --hostname {host}), then check it with: gh api --hostname {host} /meta"
        )?;
        Ok(0)
    } else {
        writeln!(
            stdout,
            "See the server mode guide, docs/gateway/server-mode.md, for each step."
        )?;
        Ok(1)
    }
}

pub(crate) fn check(client: &ForgejoClient, origin: &str, host: &str) -> Vec<Finding> {
    let forgejo = "Forgejo";
    let mut findings = vec![match client.answer(host, Method::GET, "/api/v1/version") {
        Err(error) => {
            let hint = if error.message() == CERTIFICATE_REFUSED {
                "; if the forge's certificate comes from a private CA, set the host's ca_file in \
                 the shim's config, or FJ_SHIM_CA_FILE"
            } else {
                ""
            };
            // Nothing else would answer either.
            return vec![Finding::fix(
                forgejo,
                format!("{origin}: {}{hint}", error.message()),
            )];
        }
        Ok(answer) => match answer.body["version"].as_str() {
            Some(version) if answer.status == 200 => {
                Finding::ok(forgejo, format!("version {version}"))
            }
            _ => Finding::fix(
                forgejo,
                format!(
                    "/api/v1/version answered {} without Forgejo's version; is {origin} the forge's address?",
                    answer.status
                ),
            ),
        },
    }];
    findings.push(route(
        "REST API",
        "/api/v3/",
        client.answer(host, Method::GET, "/api/v3/meta"),
        origin,
    ));
    findings.push(route(
        "GraphQL API",
        "/api/graphql",
        client.answer(host, Method::POST, "/api/graphql"),
        origin,
    ));
    findings.push(pull_link(
        client.answer(host, Method::GET, PULL_LINK),
        origin,
    ));
    findings
}

/// An API route answers a request without a token with the gateway's `401`, which carries
/// GitHub's `documentation_url`. Forgejo serves neither path, so it answers `404`.
fn route(
    name: &'static str,
    path: &str,
    answer: crate::forgejo::ForgejoResult<Answer>,
    origin: &str,
) -> Finding {
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => return Finding::fix(name, format!("{path}: {}", error.message())),
    };
    match answer.status {
        401 if gateway_shaped(&answer.body) => {
            Finding::ok(name, format!("{path} reaches the gateway"))
        }
        401 => Finding::fix(
            name,
            format!(
                "{path} answered 401, but not as the gateway does; something in front of the forge \
                 asks for a login first"
            ),
        ),
        _ => Finding::fix(name, misrouted(path, answer.status, origin)),
    }
}

fn pull_link(answer: crate::forgejo::ForgejoResult<Answer>, origin: &str) -> Finding {
    let name = "PR links";
    let path = "/OWNER/REPO/pull/N";
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => return Finding::fix(name, format!("{path}: {}", error.message())),
    };
    let expected = "/gfj-check/check/pulls/1";
    match (answer.status, answer.location.as_deref()) {
        (301, Some(location)) if location == expected => Finding::ok(
            name,
            format!("{path} reaches the gateway, which redirects it to Forgejo's /pulls/N"),
        ),
        (status @ 300..=399, location) => Finding::fix(
            name,
            format!(
                "{path} answered {status} to {}, not the gateway's redirect to {expected}",
                location.unwrap_or("nowhere")
            ),
        ),
        (status, _) => Finding::fix(name, misrouted(path, status, origin)),
    }
}

fn gateway_shaped(body: &Value) -> bool {
    body["message"].is_string() && body["documentation_url"].is_string()
}

/// What a status other than the gateway's own says about the setup.
fn misrouted(path: &str, status: u16, origin: &str) -> String {
    match status {
        // Forgejo answers a POST to a path it doesn't serve with 405.
        404 | 405 => format!(
            "{path} answered {status}, so it reaches Forgejo, not the gateway; add the gateway's \
             routes to the forge's site in the proxy"
        ),
        421 => format!(
            "{path} reaches a gateway that serves another address; set its FGA_PUBLIC_URL to \
             {origin}, and have the proxy pass the client's Host header unchanged"
        ),
        502..=504 => format!(
            "{path} answered {status}, so the proxy routes it but can't reach the gateway; check \
             that the gateway runs, on a network the proxy shares, at the address its routes name"
        ),
        _ => format!("{path} answered {status}, which the gateway never does without a token"),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Every request the stand-in saw, as `METHOD path` and whether it carried a token.
    type Seen = Arc<Mutex<Vec<(String, bool)>>>;

    /// A stand-in forge answering `routes`' `(method path, status, headers, body)`, and 404 to
    /// anything else, one request per connection.
    fn stand_in(routes: Vec<(&'static str, u16, &'static str, &'static str)>) -> (String, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let seen: Seen = Arc::default();
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                let _ = reader.read_line(&mut request);
                let mut authorized = false;
                let mut length = 0;
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    let lower = line.to_ascii_lowercase();
                    authorized |= lower.starts_with("authorization:");
                    if let Some(value) = lower.strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let mut parts = request.split(' ');
                let key = format!(
                    "{} {}",
                    parts.next().unwrap_or_default(),
                    parts.next().unwrap_or_default()
                );
                log.lock().expect("log").push((key.clone(), authorized));
                let (status, headers, body) = routes
                    .iter()
                    .find(|(route, ..)| *route == key)
                    .map_or((404, "", r#"{"message":"Not Found"}"#), |(_, s, h, b)| {
                        (*s, *h, *b)
                    });
                let mut stream = reader.into_inner();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{headers}\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (format!("http://{address}"), seen)
    }

    const UNAUTHORIZED: &str = r#"{"message":"Requires authentication","documentation_url":"https://docs.github.com/rest/authentication/authenticating-to-the-rest-api"}"#;

    fn forgejo() -> (&'static str, u16, &'static str, &'static str) {
        ("GET /api/v1/version", 200, "", r#"{"version":"15.0.1"}"#)
    }

    fn gateway() -> Vec<(&'static str, u16, &'static str, &'static str)> {
        vec![
            forgejo(),
            ("GET /api/v3/meta", 401, "", UNAUTHORIZED),
            ("POST /api/graphql", 401, "", UNAUTHORIZED),
            (
                "GET /gfj-check/check/pull/1",
                301,
                "Location: /gfj-check/check/pulls/1\r\n",
                "",
            ),
        ]
    }

    fn empty_config() -> Config {
        Config {
            hosts: Vec::new(),
            host: Default::default(),
            paths: config::PathsConfig { gh: None, fj: None },
            path: Default::default(),
        }
    }

    fn run_with(origin: &str, config: &Config) -> (i32, String) {
        let mut stdout = Vec::new();
        let code = run(origin, config, &EnvMap::new(), &mut stdout).expect("run");
        (code, String::from_utf8(stdout).expect("utf-8"))
    }

    fn run_against(origin: &str) -> (i32, String) {
        run_with(origin, &empty_config())
    }

    #[test]
    fn a_forge_with_the_gateway_mounted_passes_every_check() {
        let (origin, seen) = stand_in(gateway());

        let (code, output) = run_against(&origin);

        assert_eq!(code, 0, "{output}");
        assert!(output.contains("[ok] Forgejo: version 15.0.1"), "{output}");
        assert!(
            output.contains("[ok] REST API: /api/v3/ reaches the gateway"),
            "{output}"
        );
        assert!(
            output.contains("[ok] GraphQL API: /api/graphql reaches the gateway"),
            "{output}"
        );
        assert!(output.contains("[ok] PR links:"), "{output}");
        assert!(!output.contains("[fix]"), "{output}");
        assert!(
            output.contains("gh auth login --hostname 127.0.0.1:"),
            "{output}"
        );
        let seen = seen.lock().expect("log");
        assert_eq!(seen.len(), 4, "{seen:?}");
        assert!(seen.iter().all(|(_, authorized)| !authorized), "{seen:?}");
    }

    #[test]
    fn a_configured_api_address_doesnt_redirect_the_check() {
        let (origin, seen) = stand_in(gateway());
        let host = origin.trim_start_matches("http://").to_string();
        let mut config = empty_config();
        config.hosts.push(host.clone());
        config.host.insert(
            host,
            config::HostConfig {
                api_url: Some("http://127.0.0.1:1".to_string()),
                ..Default::default()
            },
        );

        let (code, output) = run_with(&origin, &config);

        assert_eq!(code, 0, "{output}");
        assert_eq!(seen.lock().expect("log").len(), 4);
    }

    #[test]
    fn a_forge_without_the_routes_says_they_reach_forgejo() {
        let (origin, _) = stand_in(vec![forgejo(), ("POST /api/graphql", 405, "", "")]);

        let (code, output) = run_against(&origin);

        assert_eq!(code, 1, "{output}");
        assert!(output.contains("[ok] Forgejo"), "{output}");
        for answer in [
            "/api/v3/ answered 404",
            "/api/graphql answered 405",
            "/OWNER/REPO/pull/N answered 404",
        ] {
            assert!(
                output.contains(&format!("{answer}, so it reaches Forgejo")),
                "{output}"
            );
        }
        assert!(output.contains("docs/gateway/server-mode.md"), "{output}");
    }

    #[test]
    fn a_gateway_for_another_address_names_its_public_url() {
        let refused = r#"{"message":"This gateway serves only other.example.com"}"#;
        let (origin, _) = stand_in(vec![
            forgejo(),
            ("GET /api/v3/meta", 421, "", refused),
            ("POST /api/graphql", 421, "", refused),
            ("GET /gfj-check/check/pull/1", 421, "", refused),
        ]);

        let (code, output) = run_against(&origin);

        assert_eq!(code, 1, "{output}");
        assert!(
            output.contains(&format!("set its FGA_PUBLIC_URL to {origin}")),
            "{output}"
        );
    }

    #[test]
    fn a_gateway_the_proxy_cant_reach_says_so() {
        let (origin, _) = stand_in(vec![
            forgejo(),
            ("GET /api/v3/meta", 502, "", ""),
            ("POST /api/graphql", 502, "", ""),
            ("GET /gfj-check/check/pull/1", 502, "", ""),
        ]);

        let (code, output) = run_against(&origin);

        assert_eq!(code, 1, "{output}");
        assert!(
            output.contains(
                "/api/v3/ answered 502, so the proxy routes it but can't reach the gateway"
            ),
            "{output}"
        );
    }

    #[test]
    fn a_login_in_front_of_the_forge_isnt_taken_for_the_gateway() {
        let (origin, _) = stand_in(vec![
            forgejo(),
            ("GET /api/v3/meta", 401, "", r#"{"message":"login"}"#),
            ("POST /api/graphql", 401, "", ""),
            (
                "GET /gfj-check/check/pull/1",
                302,
                "Location: /user/login\r\n",
                "",
            ),
        ]);

        let (code, output) = run_against(&origin);

        assert_eq!(code, 1, "{output}");
        assert!(output.contains("not as the gateway does"), "{output}");
        assert!(output.contains("answered 302 to /user/login"), "{output}");
        assert!(!output.contains("[ok] REST API"), "{output}");
    }

    #[test]
    fn an_unreachable_forge_stops_at_the_first_check() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        drop(listener);

        let (code, output) = run_against(&format!("http://{address}"));

        assert_eq!(code, 1, "{output}");
        assert!(output.contains("[fix] Forgejo:"), "{output}");
        assert!(!output.contains("REST API"), "{output}");
    }

    /// A TLS stand-in whose certificate, for `localhost`, comes from the test CA: Forgejo's
    /// version route and nothing else.
    fn tls_stand_in() -> String {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let cert =
            CertificateDer::from(include_bytes!("../tests/fixtures/test-server.der").to_vec());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            include_bytes!("../tests/fixtures/test-server-key.der").to_vec(),
        ));
        let config = Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .expect("server config"),
        );
        std::thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                let connection =
                    rustls::ServerConnection::new(Arc::clone(&config)).expect("connection");
                let mut stream = BufReader::new(rustls::StreamOwned::new(connection, socket));
                let mut request = String::new();
                if stream.read_line(&mut request).is_err() {
                    continue;
                }
                let mut line = String::new();
                while stream.read_line(&mut line).is_ok_and(|read| read > 2) {
                    line.clear();
                }
                let (status, body) = if request.starts_with("GET /api/v1/version ") {
                    (200, r#"{"version":"15.0.1"}"#)
                } else {
                    (404, "")
                };
                let mut stream = stream.into_inner();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.flush();
            }
        });
        format!("https://localhost:{port}")
    }

    #[test]
    fn a_certificate_from_an_unknown_ca_names_the_ca_settings() {
        let origin = tls_stand_in();

        let (code, output) = run_against(&origin);

        assert_eq!(code, 1, "{output}");
        assert!(
            output.contains("its TLS certificate couldn't be verified"),
            "{output}"
        );
        assert!(output.contains("FJ_SHIM_CA_FILE"), "{output}");

        let ca =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-ca.pem");
        let env = EnvMap::from([("FJ_SHIM_CA_FILE".to_string(), ca.display().to_string())]);
        let mut stdout = Vec::new();
        run(&origin, &empty_config(), &env, &mut stdout).expect("run");
        let output = String::from_utf8(stdout).expect("utf-8");
        assert!(output.contains("[ok] Forgejo: version 15.0.1"), "{output}");
    }

    #[test]
    fn the_address_is_a_bare_origin() {
        for (target, expected) in [
            ("forge.example.com", "https://forge.example.com"),
            ("Forge.Example.com:8443/", "https://forge.example.com:8443"),
            ("HTTPS://forge.example.com", "https://forge.example.com"),
            ("http://127.0.0.1:3000", "http://127.0.0.1:3000"),
        ] {
            assert_eq!(origin(target).expect(target).0, expected);
        }
        for target in [
            "",
            "https://",
            "https://forge.example.com/owner/repo",
            "https://forge.example.com?x=1",
            "https://user@forge.example.com",
            "ftp://forge.example.com",
            "github.com",
        ] {
            assert!(origin(target).is_err(), "{target}");
        }
    }
}
