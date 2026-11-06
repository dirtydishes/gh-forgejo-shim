//! `doctor` asks each configured host's Forgejo which required token scopes its token has.
// Shared with cli_scaffold; this file uses only the fixture.
#[allow(dead_code)]
mod support;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use support::{CliFixture, TestResult};

/// Every request the stand-in saw, as `METHOD path`.
type Seen = Arc<Mutex<Vec<String>>>;

/// A stand-in Forgejo 15 that judges each token by the scopes `tokens` gives it, as Forgejo does:
/// a route's scope first (`read:` for GET, `write:` for anything else, a write scope covering its
/// read), then the owner or repository it names. Nothing exists but the token's user, so a
/// granted request for any repository or organization gets the lookup's 404. With
/// `refuse_writes`, every non-GET answers 405, as a proxy that blocks writes would.
fn stand_in(
    tokens: &'static [(&'static str, &'static [&'static str])],
    refuse_writes: bool,
) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            let _ = reader.read_line(&mut request);
            let mut token = String::new();
            let mut length = 0;
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("authorization: token ") {
                    token = line.trim()["authorization: token ".len()..].to_string();
                }
                if let Some(value) = lower.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                line.clear();
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let mut parts = request.split(' ');
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or_default().to_string();
            log.lock()
                .expect("request log")
                .push(format!("{method} {path}"));

            let category = if path == "/api/v1/user" {
                "user"
            } else if path.starts_with("/api/v1/orgs/") {
                "organization"
            } else if path.starts_with("/api/v1/repos/") && path.contains("/issues") {
                "issue"
            } else {
                "repository"
            };
            let level = if method == "GET" { "read" } else { "write" };
            let (status, reply) = match tokens.iter().find(|(known, _)| *known == token) {
                None => (
                    "401 Unauthorized",
                    r#"{"message":"user does not exist"}"#.to_string(),
                ),
                Some(_) if refuse_writes && method != "GET" => {
                    ("405 Method Not Allowed", String::new())
                }
                Some((_, scopes)) => {
                    let has = scopes.contains(&format!("{level}:{category}").as_str())
                        || scopes.contains(&format!("write:{category}").as_str());
                    if !has {
                        (
                            "403 Forbidden",
                            format!(
                                r#"{{"message":"token does not have at least one of required scope(s): [{level}:{category}]","url":"http://forge.test/api/swagger"}}"#
                            ),
                        )
                    } else if category == "user" {
                        ("200 OK", r#"{"id":1,"login":"me"}"#.to_string())
                    } else if category == "organization" {
                        (
                            "404 Not Found",
                            r#"{"message":"GetOrgByName","errors":[]}"#.to_string(),
                        )
                    } else {
                        (
                            "404 Not Found",
                            r#"{"message":"GetUserByName","errors":[]}"#.to_string(),
                        )
                    }
                }
            };
            let _ = write!(
                reader.get_mut(),
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    (format!("http://{address}"), seen)
}

const ALL: &[&str] = &[
    "read:user",
    "write:repository",
    "write:issue",
    "read:organization",
];

const TOKENS: &[(&str, &[&str])] = &[
    ("full-secret", ALL),
    (
        "no-user-secret",
        &["write:repository", "write:issue", "read:organization"],
    ),
    (
        "no-repo-secret",
        &["read:user", "write:issue", "read:organization"],
    ),
    (
        "read-repo-secret",
        &[
            "read:user",
            "read:repository",
            "write:issue",
            "read:organization",
        ],
    ),
    (
        "no-issue-secret",
        &["read:user", "write:repository", "read:organization"],
    ),
    (
        "read-issue-secret",
        &[
            "read:user",
            "write:repository",
            "read:issue",
            "read:organization",
        ],
    ),
    (
        "no-org-secret",
        &["read:user", "write:repository", "write:issue"],
    ),
];

/// `doctor`'s output for one host, `forge.test`, served by `api` with `token`.
fn doctor(fixture: &CliFixture, api: &str, token: &str) -> std::io::Result<String> {
    let config = fixture.home().join(".config/gh-forgejo-shim");
    fs::create_dir_all(&config)?;
    fs::write(
        config.join("config.toml"),
        format!("hosts = [\"forge.test\"]\n[host.\"forge.test\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"{api}\"\n"),
    )?;
    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_HOSTS")
        .env("HOST_TOKEN", token)
        .arg("doctor")
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn scope_line(output: &str) -> &str {
    output
        .lines()
        .find(|line| line.contains("token scopes (forge.test)"))
        .unwrap_or_else(|| panic!("no scope line: {output}"))
}

#[test]
fn doctor_names_every_scope_a_full_token_has() -> TestResult {
    let (api, seen) = stand_in(TOKENS, false);
    let fixture = CliFixture::new()?;
    let output = doctor(&fixture, &api, "full-secret")?;
    assert_eq!(
        scope_line(&output),
        "[ok] token scopes (forge.test): read:user, write:repository, write:issue, read:organization"
    );
    // The probes read nothing and can change nothing: every one but `/user` names an owner
    // Forgejo can't have, and each write fails at that owner's lookup.
    let seen = seen.lock().expect("request log").clone();
    assert_eq!(
        seen,
        [
            "GET /api/v1/user",
            "POST /api/v1/repos/gfj--scope-probe/gfj--scope-probe/pulls",
            "POST /api/v1/repos/gfj--scope-probe/gfj--scope-probe/issues",
            "GET /api/v1/orgs/gfj--scope-probe",
        ]
    );
    Ok(())
}

#[test]
fn doctor_names_each_missing_scope_and_what_breaks() -> TestResult {
    let (api, _) = stand_in(TOKENS, false);
    let cases = [
        (
            "no-user-secret",
            "read:user is missing, so every gh call fails, since the gateway can't tell whose token it is",
        ),
        (
            "no-repo-secret",
            "write:repository is missing, so pull requests, branches and checks can't be read",
        ),
        (
            "read-repo-secret",
            "read:repository without write:repository, so pull requests can't be created, edited or merged",
        ),
        (
            "no-issue-secret",
            "write:issue is missing, so issues can't be read, and a pull request's comments and scheduled auto-merge read as none",
        ),
        (
            "read-issue-secret",
            "read:issue without write:issue, so issues and comments can't be created or edited",
        ),
        (
            "no-org-secret",
            "read:organization is missing, so a repository an organization owns reads as owned by a user",
        ),
    ];
    for (token, problem) in cases {
        let fixture = CliFixture::new()?;
        let output = doctor(&fixture, &api, token)?;
        assert_eq!(
            scope_line(&output),
            format!(
                "[fix] token scopes (forge.test): {problem}; create a token with read:user, \
                 write:repository, write:issue and read:organization (README: Token scopes)"
            ),
            "{token}"
        );
        assert!(
            output.contains("  gh-forgejo-shim auth login forge.test"),
            "{token}: {output}"
        );
        assert!(!output.contains(token), "{token} printed: {output}");
    }
    Ok(())
}

#[test]
fn doctor_checks_each_host_with_its_own_token() -> TestResult {
    let (api, _) = stand_in(TOKENS, false);
    let fixture = CliFixture::new()?;
    let config = fixture.home().join(".config/gh-forgejo-shim");
    fs::create_dir_all(&config)?;
    fs::write(
        config.join("config.toml"),
        format!(
            "hosts = [\"forge.test\", \"other.test\"]\n\
             [host.\"forge.test\"]\ntoken_env = \"FORGE_TOKEN\"\napi_url = \"{api}\"\n\
             [host.\"other.test\"]\ntoken_env = \"OTHER_TOKEN\"\napi_url = \"{api}\"\n"
        ),
    )?;
    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_HOSTS")
        .env("FORGE_TOKEN", "full-secret")
        .env("OTHER_TOKEN", "no-org-secret")
        .arg("doctor")
        .output()?;
    let output = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.contains("[ok] token scopes (forge.test): read:user,"),
        "{output}"
    );
    assert!(
        output.contains("[fix] token scopes (other.test): read:organization is missing"),
        "{output}"
    );
    Ok(())
}

#[test]
fn doctor_reports_a_rejected_token() -> TestResult {
    let (api, _) = stand_in(TOKENS, false);
    let fixture = CliFixture::new()?;
    let output = doctor(&fixture, &api, "revoked-secret")?;
    assert_eq!(
        scope_line(&output),
        "[fix] token scopes (forge.test): Forgejo rejected the token; log in again with \
         gh-forgejo-shim auth login forge.test"
    );
    assert!(!output.contains("revoked-secret"), "{output}");
    Ok(())
}

/// A write that doesn't fail with one of Forgejo's two answers can't say whether the scope is
/// there, so doctor says so and why, and still reports what it could check.
#[test]
fn doctor_says_when_a_write_scope_cant_be_checked() -> TestResult {
    let (api, _) = stand_in(TOKENS, true);
    let fixture = CliFixture::new()?;
    let output = doctor(&fixture, &api, "full-secret")?;
    assert_eq!(
        scope_line(&output),
        "[warn] token scopes (forge.test): has read:user, read:organization; write:repository not \
         checked: Forgejo answered HTTP 405; write:issue not checked: Forgejo answered HTTP 405"
    );
    Ok(())
}

#[test]
fn doctor_says_when_the_forge_cant_be_reached() -> TestResult {
    let fixture = CliFixture::new()?;
    // Port 9 (discard) on loopback refuses the connection.
    let output = doctor(&fixture, "http://127.0.0.1:9", "full-secret")?;
    assert_eq!(
        scope_line(&output),
        "[warn] token scopes (forge.test): not checked: Forgejo at forge.test: it couldn't be reached"
    );
    Ok(())
}
