//! Credential and config audit regressions. All tokens are synthetic fixtures.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use gh_forgejo_shim::{auth, auth_import, config};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> io::Result<Self> {
        let root = loop {
            let id = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gh-forgejo-shim-audit-credentials-{}-{id}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&root) {
                Ok(()) => break root,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let fixture = Self {
            home: root.join("home"),
            bin: root.join("bin"),
            root,
        };
        fs::create_dir(&fixture.home)?;
        fs::create_dir(&fixture.bin)?;
        // File storage is also forced explicitly. This is a second guard for
        // any future PATH-based security invocation in the audited commands.
        let security = fixture.bin.join("security");
        fs::write(&security, "#!/bin/sh\nexit 99\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(security, fs::Permissions::from_mode(0o700))?;
        }
        Ok(fixture)
    }

    fn config_path(&self) -> PathBuf {
        config::config_path(Some(&self.home))
    }

    fn write(&self, path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| io::Error::other("file parent"))?,
        )?;
        fs::write(path, contents)
    }

    fn fj_source(&self, host: &str, token: &str) -> io::Result<()> {
        let path = self.home.join(if cfg!(target_os = "macos") {
            "Library/Application Support/forgejo-cli.forgejo-cli/keys.json"
        } else {
            ".local/share/forgejo-cli/keys.json"
        });
        self.write(
            &path,
            json!({ "hosts": { host: { "type": "Application", "token": token } } }).to_string(),
        )
    }

    fn isolate(&self, command: &mut Command) {
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.bin)
            .env("FJ_SHIM_AUTH_STORE", "file")
            .current_dir(&self.root)
            .stdin(Stdio::null());
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gfj"));
        self.isolate(&mut command);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn audit_cred_malformed_hosts_does_not_reset_store() -> TestResult {
    let fixture = Fixture::new()?;
    let path = auth::auth_file_path(Some(&fixture.home));
    let before = r#"{"hosts":[{"host":"kept.example.com","token":"audit-kept-token"}]}"#;
    fixture.write(&path, before)?;

    let result = auth::write_stored_token(
        "new.example.com",
        "audit-new-token",
        Some(&fixture.home),
        Some("linux"),
    );

    let after = fs::read_to_string(&path)?;
    assert!(
        after == before,
        "malformed hosts was silently replaced and its existing token was lost"
    );
    assert!(
        result.is_err(),
        "malformed auth hosts must reject the write"
    );
    Ok(())
}

#[test]
fn audit_cred_github_default_port_does_not_receive_forgejo_token() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write(&fixture.config_path(), "hosts = [\"github.com:443\"]\n")?;
    let env = config::EnvMap::from([("FJ_SHIM_TOKEN".into(), "audit-forgejo-token".into())]);
    let loaded = config::load_config_with_env(Some(&fixture.config_path()), &env)?;
    let destination = reqwest::Url::parse("https://github.com:443/api/v1/user")?;
    assert_eq!(destination.host_str(), Some("github.com"));
    assert_eq!(destination.port_or_known_default(), Some(443));

    let found = auth::resolve_token(
        "github.com:443",
        &loaded,
        &env,
        Some(&fixture.home),
        Some("linux"),
    );

    assert!(
        found.is_none(),
        "Forgejo token resolved for github.com:443, the same HTTPS destination as github.com"
    );
    Ok(())
}

#[test]
fn audit_cred_import_rejects_github_default_port() -> TestResult {
    let fixture = Fixture::new()?;
    let path = fixture.home.join(".config/tea/config.yml");
    fixture.write(
        &path,
        "logins:\n  - url: https://github.com:443\n    token: audit-github-token\n",
    )?;

    let result = auth_import::read_import(
        auth_import::ImportSource::Tea,
        Some("github.com:443"),
        &fixture.home,
        &config::EnvMap::new(),
        "linux",
    );

    assert!(
        result.is_err(),
        "GitHub token accepted as a Forgejo import through the default HTTPS port"
    );
    Ok(())
}

#[test]
fn audit_cred_config_parse_error_redacts_url_password() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write(
        &fixture.config_path(),
        "[host.\"git.example.com\"]\napi_url = [\"https://user:audit-url-password@git.example.com\"]\n",
    )?;

    let output = fixture.command().args(["config", "list"]).output()?;
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("could not parse"), "{stderr}");
    assert!(
        !stderr.contains("audit-url-password"),
        "config parser echoed URL password to stderr: {stderr}"
    );
    Ok(())
}

#[test]
#[ignore = "audit C4: fails until gh-forgejo-shim-jko.20 is fixed"]
fn audit_cred_failed_import_preserves_previous_token() -> TestResult {
    let fixture = Fixture::new()?;
    auth::write_stored_token(
        "git.example.com",
        "audit-previous-token",
        Some(&fixture.home),
        Some("linux"),
    )?;
    fixture.write(&fixture.config_path(), "hosts = [\"git.example.com\"\n")?;
    fixture.fj_source("git.example.com", "audit-replacement-token")?;

    let output = fixture
        .command()
        .args([
            "auth",
            "import",
            "--from",
            "fj",
            "--hostname",
            "git.example.com",
        ])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stderr)?.contains("could not parse"));
    let stored = auth::read_stored_token("git.example.com", Some(&fixture.home), Some("linux"))
        .expect("previously stored token");

    assert!(
        stored.token == "audit-previous-token",
        "failed auth import replaced the previous stored token before rejecting corrupt config"
    );
    Ok(())
}

#[test]
#[ignore = "audit C6: fails until gh-forgejo-shim-jko.22 is fixed"]
fn audit_cred_config_update_preserves_unknown_settings() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write(
        &fixture.config_path(),
        "hosts = [\"git.example.com\"]\n[host.\"git.example.com\"]\nfuture_setting = \"keep-me\"\n[future]\nsetting = \"also-keep-me\"\n",
    )?;

    let result = config::add_host("new.example.com", Some(&fixture.config_path()));
    let after = fs::read_to_string(fixture.config_path())?;
    assert!(
        result.is_err() || (after.contains("future_setting") && after.contains("also-keep-me")),
        "successful config update silently discarded unknown host and top-level settings"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn audit_cred_failed_config_write_preserves_original_file() -> TestResult {
    let fixture = Fixture::new()?;
    let before = "hosts = [\"git.example.com\"]\n";
    fixture.write(&fixture.config_path(), before)?;
    let mut command = Command::new("/bin/sh");
    fixture.isolate(&mut command);
    // Ignore SIGXFSZ in this child only, so a file-size limit produces EFBIG
    // rather than killing the process. Reads of the old file still succeed.
    let output = command
        .args([
            "-c",
            "trap '' XFSZ; ulimit -f 0; exec \"$@\"",
            "audit-file-limit",
            env!("CARGO_BIN_EXE_gfj"),
            "config",
            "add-host",
            "new.example.com",
        ])
        .output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("could not write"), "{stderr}");
    assert!(
        fs::read_to_string(fixture.config_path())? == before,
        "failed config write truncated the original config and left an empty file"
    );
    Ok(())
}
