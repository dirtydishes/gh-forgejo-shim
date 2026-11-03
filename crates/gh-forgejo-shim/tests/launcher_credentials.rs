//! The launcher's git credential helper with real git (launcher spec §3, "Git's credentials"):
//! with `GIT_CONFIG_PARAMETERS` as a launch sets it, every helper configured anywhere else is
//! cleared, and the run directory's script answers exact served hosts only.
#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use gh_forgejo_shim::launcher::credentials::git_config_parameters;
use gh_forgejo_shim::launcher::ghconfig::{credentials, gh_script, ServedHost};

struct Fixture {
    root: PathBuf,
    run_dir: PathBuf,
    home: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let mut bytes = [0u8; 8];
        fs::File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut bytes))
            .expect("random");
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let root = PathBuf::from("/tmp").join(format!("lc-{hex}"));
        let run_dir = root.join(format!("gfj-{hex}"));
        let home = root.join("home");
        fs::create_dir_all(&run_dir).expect("run dir");
        fs::create_dir_all(&home).expect("home");
        let served = [
            ServedHost {
                entry: "forge.test".to_string(),
                real_token: "real-a".to_string(),
                login: None,
            },
            ServedHost {
                entry: "port.test:3443".to_string(),
                real_token: "real-b".to_string(),
                login: None,
            },
        ];
        fs::write(run_dir.join("credentials"), credentials(&served)).expect("credentials");
        let script = run_dir.join("gh");
        fs::write(
            &script,
            gh_script(&run_dir, Path::new(env!("CARGO_BIN_EXE_gh-forgejo-shim"))),
        )
        .expect("gh script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).expect("chmod");

        // A helper that only records that it ran, set in every place git reads helpers from.
        let log = root.join("other-helpers");
        let logger = root.join("log-helper");
        fs::write(
            &logger,
            format!("#!/bin/sh\necho \"$0 $*\" >> '{}'\n", log.display()),
        )
        .expect("logger");
        fs::set_permissions(&logger, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::write(
            home.join(".gitconfig"),
            format!(
                "[credential]\n\thelper = {0}\n[credential \"https://github.com\"]\n\thelper = {0}\n",
                logger.display()
            ),
        )
        .expect("gitconfig");
        Self {
            root,
            run_dir,
            home,
            log,
        }
    }

    fn fill(&self, host: &str) -> Output {
        let logger = self.root.join("log-helper");
        let inherited = OsString::from(format!("'credential.helper={}'", logger.display()));
        // Outside any checkout: `git credential fill` needs no repository.
        let mut child = Command::new("git")
            .args(["credential", "fill"])
            .current_dir(&self.root)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "credential.helper")
            .env("GIT_CONFIG_VALUE_0", &logger)
            .env(
                "GIT_CONFIG_PARAMETERS",
                git_config_parameters(Some(&inherited), &self.run_dir),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("git");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(format!("protocol=https\nhost={host}\n\n").as_bytes())
            .expect("write");
        child.wait_with_output().expect("wait")
    }

    fn other_helpers(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn only_the_launchers_helper_answers_and_only_for_served_hosts() {
    let fixture = Fixture::new();
    for (host, token) in [("forge.test", "real-a"), ("port.test:3443", "real-b")] {
        let output = fixture.fill(host);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{host}: {output:?}");
        assert!(
            stdout.contains(&format!("password={token}\n"))
                && stdout.contains("username=x-access-token\n"),
            "{host}: {stdout}"
        );
    }
    for host in ["gist.forge.test", "github.com", "Forge.Test", "port.test"] {
        let output = fixture.fill(host);
        assert!(
            !output.status.success(),
            "{host} got a credential: {output:?}"
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("password="),
            "{host}"
        );
    }
    assert_eq!(
        fixture.other_helpers(),
        "",
        "a helper from the user's config, GIT_CONFIG_COUNT or the inherited GIT_CONFIG_PARAMETERS ran"
    );
}

#[test]
fn git_parses_the_launchers_config_with_the_users_unset_empty_or_set() {
    // Launcher spec §11, test 14: whatever the user's GIT_CONFIG_PARAMETERS, every git in the
    // call reads the launcher's value, plus a -c of its own, with no config parse error.
    let fixture = Fixture::new();
    for user in [None, Some(""), Some("'a.b'='c'")] {
        let value = git_config_parameters(user.map(OsString::from).as_deref(), &fixture.run_dir);
        let output = Command::new("git")
            .args([
                "-c",
                "x.y=z",
                "config",
                "--get-regexp",
                r"^(a\.b|x\.y|credential\.helper)$",
            ])
            .current_dir(&fixture.root)
            .env_clear()
            .env("HOME", &fixture.home)
            .env("PATH", "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_PARAMETERS", &value)
            .output()
            .expect("git");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{user:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{user:?}: {output:?}");
        assert!(stdout.contains("x.y z\n"), "{user:?}: {stdout}");
        assert_eq!(
            stdout.contains("a.b c\n"),
            user.is_some_and(|value| !value.is_empty()),
            "{user:?}: {stdout}"
        );
        // The clearing entry comes before the launcher's helper, after anything the user set.
        let helpers: Vec<&str> = stdout
            .lines()
            .filter(|line| line.starts_with("credential.helper"))
            .collect();
        assert_eq!(
            helpers.len(),
            3,
            "{user:?}: the user's .gitconfig, the clear, the launcher's: {stdout}"
        );
        assert!(
            helpers[2].ends_with("gh auth git-credential"),
            "{user:?}: {stdout}"
        );
    }
}
