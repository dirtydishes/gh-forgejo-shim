//! Process regressions for the shared config loader and explicit auth imports.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gh_forgejo_shim::{auth, config};
use serde_json::{json, Value};

use super::support::{CliFixture, TestResult};

#[test]
fn multiline_and_table_hosts_route_pr_list_to_configured_http_api() -> TestResult {
    for hosts in ["hosts = [\n  \"git.example.com\", # comment\n]\n", ""] {
        let fixture = CliFixture::new()?;
        fixture.init_git_repo()?;
        let decoy = fixture.write_executable("gh#decoy", "#!/bin/sh\necho delegated\nexit 99\n")?;
        let (base_url, server) = serve_json_once("[]")?;
        let config_path = config::config_path(Some(fixture.home()));
        fs::create_dir_all(
            config_path
                .parent()
                .ok_or_else(|| io::Error::other("config parent"))?,
        )?;
        fs::write(
            &config_path,
            format!(
                r#"{hosts}
[host."git.example.com"]
api_url = "{base_url}/"
token_env = "HOST_TOKEN"
[paths]
gh = '{}'
"#,
                decoy.display()
            ),
        )?;

        let listed = fixture
            .command("gfj")?
            .env_remove("FJ_SHIM_HOSTS")
            .args(["config", "list"])
            .output()?;
        assert!(listed.status.success(), "{listed:?}");
        assert_eq!(String::from_utf8(listed.stdout)?.trim(), "git.example.com");
        let output = fixture
            .command("gh-forgejo-shim")?
            .env_remove("FJ_SHIM_HOSTS")
            .env_remove("FJ_SHIM_REAL_GH")
            .env("HOST_TOKEN", "host-secret")
            .args(["gh", "pr", "list", "--json", "number"])
            .output()?;

        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert_eq!(String::from_utf8(output.stdout)?.trim(), "[]");
        let request = server.join().expect("HTTP server")?;
        assert!(
            request.starts_with("GET /api/v1/repos/owner/repo/pulls?state=open HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: token host-secret\r\n"),
            "{request}"
        );
    }
    Ok(())
}

#[test]
fn quoted_hash_in_configured_gh_path_survives_dispatcher_loading() -> TestResult {
    let fixture = CliFixture::new()?;
    let decoy = fixture.write_executable("gh#decoy", "#!/bin/sh\necho configured-gh\nexit 99\n")?;
    let path = config::config_path(Some(fixture.home()));
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::other("config parent"))?,
    )?;
    fs::write(path, format!("[paths]\ngh = '{}'\n", decoy.display()))?;
    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_HOSTS")
        .env_remove("FJ_SHIM_REAL_GH")
        .args(["gh", "--version"])
        .output()?;
    assert_eq!(output.status.code(), Some(99), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout)?.trim(), "configured-gh");
    Ok(())
}

#[test]
fn missing_config_has_no_hosts_and_delegates_pr_list() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let decoy = fixture.write_executable("gh", "#!/bin/sh\nexit 99\n")?;
    let output = fixture
        .command("gfj")?
        .env_remove("FJ_SHIM_HOSTS")
        .args(["config", "list"])
        .output()?;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout)?.trim(),
        "no Forgejo hosts configured"
    );
    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_HOSTS")
        .env("FJ_SHIM_REAL_GH", decoy)
        .args(["gh", "pr", "list"])
        .output()?;
    assert_eq!(output.status.code(), Some(99), "{output:?}");
    Ok(())
}

#[test]
fn fj_and_tea_import_select_exactly_one_entry_without_printing_tokens() -> TestResult {
    for source in ["fj", "tea"] {
        for (hosts, requested, expected) in [
            (vec!["a.example.com"], None, Some("a.example.com")),
            (
                vec!["a.example.com", "b.example.com"],
                Some("b.example.com"),
                Some("b.example.com"),
            ),
            (vec!["a.example.com", "b.example.com"], None, None),
            (vec!["a.example.com"], Some("missing.example.com"), None),
            (vec![], None, None),
            (
                vec!["a.example.com", "A.Example.com"],
                Some("a.example.com"),
                None,
            ),
        ] {
            let fixture = CliFixture::new()?;
            let entries = hosts
                .iter()
                .map(|host| (*host, format!("secret-{host}")))
                .collect::<Vec<_>>();
            let path = write_source(&fixture, source, &entries)?;
            auth::write_stored_token(
                "kept.example.com",
                "keep-secret",
                Some(fixture.home()),
                Some("linux"),
            )?;
            let auth_path = auth::auth_file_path(Some(fixture.home()));
            let before = fs::read(&auth_path)?;
            let mut command = fixture.command("gfj")?;
            command.args(["auth", "import", "--from", source]);
            if let Some(host) = requested {
                command.args(["--hostname", host]);
            }
            let output = command.output()?;
            let stdout = String::from_utf8(output.stdout)?;
            let stderr = String::from_utf8(output.stderr)?;
            for (_, token) in &entries {
                assert!(!stdout.contains(token), "{stdout}");
                assert!(!stderr.contains(token), "{stderr}");
            }
            assert!(!stdout.contains("keep-secret"));
            assert!(!stderr.contains("keep-secret"));
            if let Some(host) = expected {
                assert!(output.status.success(), "{stderr}");
                assert!(
                    stdout.contains(&format!("imported auth for {host} from {}", path.display())),
                    "{stdout}"
                );
                assert_eq!(
                    auth::read_stored_token(host, Some(fixture.home()), Some("linux"))
                        .map(|token| token.token),
                    Some(format!("secret-{host}"))
                );
                let loaded = config::load_config_with_env(
                    Some(&config::config_path(Some(fixture.home()))),
                    &config::EnvMap::new(),
                )?;
                assert_eq!(loaded.hosts, [host]);
                let stored: Value = serde_json::from_slice(&fs::read(&auth_path)?)?;
                assert_eq!(
                    stored["hosts"].as_object().map(|hosts| hosts.len()),
                    Some(2)
                );
            } else {
                assert_eq!(output.status.code(), Some(1), "{stdout}");
                assert!(
                    stderr.contains(if hosts.len() > 1 {
                        "multiple matching"
                    } else {
                        "no matching"
                    }),
                    "{stderr}"
                );
                assert_eq!(fs::read(&auth_path)?, before);
                assert!(!config::config_path(Some(fixture.home())).exists());
            }
        }
    }
    Ok(())
}

#[test]
fn import_errors_never_echo_source_lines_or_write_auth() -> TestResult {
    for source in ["fj", "tea"] {
        for invalid in [false, true] {
            let fixture = CliFixture::new()?;
            let path = write_source(&fixture, source, &[("a.example.com", String::new())])?;
            if invalid {
                fs::write(path, "{token: secret-on-invalid-line, [")?;
            }
            let output = fixture
                .command("gfj")?
                .args(["auth", "import", "--from", source])
                .output()?;
            assert_eq!(output.status.code(), Some(1));
            let stderr = String::from_utf8(output.stderr)?;
            assert!(
                stderr.contains(if invalid {
                    "could not parse import source"
                } else {
                    "has no token"
                }),
                "{stderr}"
            );
            assert!(!stderr.contains("secret-on-invalid-line"), "{stderr}");
            assert!(output.stdout.is_empty());
            assert!(!auth::auth_file_path(Some(fixture.home())).exists());
            assert!(!config::config_path(Some(fixture.home())).exists());
        }
    }
    Ok(())
}

#[test]
fn tokenless_entries_still_count_when_rejecting_ambiguous_imports() -> TestResult {
    for source in ["fj", "tea"] {
        let fixture = CliFixture::new()?;
        write_source(
            &fixture,
            source,
            &[
                ("a.example.com", String::new()),
                ("b.example.com", "b-secret".to_string()),
            ],
        )?;
        let output = fixture
            .command("gfj")?
            .args(["auth", "import", "--from", source])
            .output()?;
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8(output.stderr)?;
        assert!(stderr.contains("multiple matching"), "{stderr}");
        assert!(!stderr.contains("b-secret"));
        assert!(!auth::auth_file_path(Some(fixture.home())).exists());
        assert!(!config::config_path(Some(fixture.home())).exists());
    }
    Ok(())
}

#[test]
fn import_reads_documented_legacy_paths_when_primary_is_missing() -> TestResult {
    for source in ["fj", "tea"] {
        if source == "fj" && !cfg!(target_os = "macos") {
            continue;
        }
        let fixture = CliFixture::new()?;
        let primary = write_source(
            &fixture,
            source,
            &[("a.example.com", "legacy-secret".to_string())],
        )?;
        let legacy = fixture.home().join(if source == "fj" {
            "Library/Application Support/Cyborus.forgejo-cli/keys.json"
        } else {
            ".tea/tea.yml"
        });
        fs::create_dir_all(
            legacy
                .parent()
                .ok_or_else(|| io::Error::other("source parent"))?,
        )?;
        fs::rename(primary, &legacy)?;
        let output = fixture
            .command("gfj")?
            .args(["auth", "import", "--from", source])
            .output()?;
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8(output.stdout)?.contains(&legacy.display().to_string()));
        assert_eq!(
            auth::read_stored_token("a.example.com", Some(fixture.home()), Some("linux"))
                .map(|token| token.token)
                .as_deref(),
            Some("legacy-secret")
        );
    }
    Ok(())
}

#[test]
fn normal_commands_ignore_external_credential_files() -> TestResult {
    let fixture = CliFixture::new()?;
    write_source(
        &fixture,
        "fj",
        &[("git.example.com", "fj-secret".to_string())],
    )?;
    write_source(
        &fixture,
        "tea",
        &[("git.example.com", "tea-secret".to_string())],
    )?;
    let output = fixture
        .command("gh-forgejo-shim")?
        .args(["gh", "auth", "status", "--hostname", "git.example.com"])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("Not logged in to git.example.com"),
        "{stderr}"
    );
    assert!(!stderr.contains("fj-secret"));
    assert!(!stderr.contains("tea-secret"));
    assert!(!auth::auth_file_path(Some(fixture.home())).exists());
    Ok(())
}

fn write_source(
    fixture: &CliFixture,
    source: &str,
    entries: &[(&str, String)],
) -> io::Result<PathBuf> {
    let path = if source == "fj" {
        if cfg!(target_os = "macos") {
            fixture
                .home()
                .join("Library/Application Support/forgejo-cli.forgejo-cli/keys.json")
        } else {
            fixture.home().join(".local/share/forgejo-cli/keys.json")
        }
    } else if cfg!(target_os = "macos") {
        fixture
            .home()
            .join("Library/Application Support/tea/config.yml")
    } else {
        fixture.home().join(".config/tea/config.yml")
    };
    let text = if source == "fj" {
        let hosts = entries
            .iter()
            .map(|(host, token)| {
                (
                    (*host).to_string(),
                    json!({"type": "Application", "token": token}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        serde_json::to_string(&json!({"hosts": hosts, "aliases": {}, "default_ssh": []}))?
    } else {
        entries
            .iter()
            .map(|(host, token)| {
                format!("  - name: {host}\n    url: https://{host}/\n    token: {token:?}\n")
            })
            .fold("logins:\n".to_string(), |mut text, login| {
                text.push_str(&login);
                text
            })
    };
    // An empty YAML sequence must be explicit; `logins:` alone is null.
    let text = if source == "tea" && entries.is_empty() {
        "logins: []\n".to_string()
    } else {
        text
    };
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::other("source parent"))?,
    )?;
    fs::write(&path, text)?;
    Ok(path)
}

fn serve_json_once(body: &'static str) -> io::Result<(String, JoinHandle<io::Result<String>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let base_url = format!("http://{}", listener.local_addr()?);
    listener.set_nonblocking(true)?;
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) => return Err(error),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(&mut stream);
        let mut request = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                return Err(io::Error::other("incomplete HTTP request"));
            }
            request.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
        Ok(request)
    });
    Ok((base_url, handle))
}
