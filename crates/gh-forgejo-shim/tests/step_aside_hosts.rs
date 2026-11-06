//! Where real gh sends a call the wrapper steps aside for (#231). gh is logged in to the forge
//! and to github.com, with dummy tokens, in a checkout whose remote is the forge. Every request
//! goes through a proxy address nothing listens on, so nothing leaves the machine; gh's
//! `GH_DEBUG=api` log names each request's host before the connection fails.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const SHIM: &str = env!("CARGO_BIN_EXE_gh-forgejo-shim");

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    trace: PathBuf,
    gh: PathBuf,
}

impl Fixture {
    fn new(gh: PathBuf) -> io::Result<Self> {
        let root = std::env::temp_dir().join(format!("gfjs-hosts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let repo = root.join("repo");
        fs::create_dir_all(home.join(".config/gh"))?;
        fs::create_dir_all(home.join(".config/gh-forgejo-shim"))?;
        fs::create_dir_all(&repo)?;
        fs::write(
            home.join(".config/gh/hosts.yml"),
            "forge.test:\n    oauth_token: dummy-forge-token\n    user: u\n    git_protocol: https\n\
             github.com:\n    oauth_token: dummy-github-token\n    user: u\n    git_protocol: https\n",
        )?;
        fs::write(
            home.join(".config/gh-forgejo-shim/config.toml"),
            format!(
                "hosts = [\"forge.test\"]\n[paths]\ngh = {:?}\n",
                gh.display().to_string()
            ),
        )?;
        let fixture = Self {
            trace: root.join("trace.jsonl"),
            root,
            home,
            repo,
            gh,
        };
        fixture.git(&["init", "-q"])?;
        fixture.git(&["remote", "add", "origin", "https://forge.test/o/r.git"])?;
        Ok(fixture)
    }

    /// HOME, the XDG directories and git's config in the fixture, so nothing the programs write
    /// lands in the repository or the user's home.
    fn isolate(&self, command: &mut Command) {
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .current_dir(&self.repo)
            .stdin(Stdio::null());
    }

    fn git(&self, args: &[&str]) -> io::Result<()> {
        let mut command = Command::new("git");
        self.isolate(&mut command);
        let output = command.args(args).output()?;
        assert!(output.status.success(), "git {args:?}: {output:?}");
        Ok(())
    }

    /// Runs `gh <args>` through the wrapper and returns the hosts gh sent requests to, in order,
    /// and the host the wrapper's trace names.
    fn requests(&self, args: &[&str]) -> io::Result<(Vec<String>, Option<String>, Output)> {
        let _ = fs::remove_file(&self.trace);
        let mut command = Command::new(SHIM);
        self.isolate(&mut command);
        let output = command
            .arg("gh")
            .args(args)
            .env("FJ_SHIM_TRACE", &self.trace)
            .env("FJ_SHIM_AUTH_STORE", "file")
            .env("GH_DEBUG", "api")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            // Nothing listens on port 9: every request fails here, after gh has logged it.
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .output()?;
        let log = String::from_utf8_lossy(&output.stderr);
        let hosts = log
            .lines()
            .filter_map(|line| line.strip_prefix("* Request to https://"))
            .filter_map(|rest| rest.split('/').next())
            .map(str::to_string)
            .collect();
        let trace = fs::read_to_string(&self.trace).unwrap_or_default();
        let traced = trace
            .lines()
            .last()
            .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .and_then(|record| record["host"].as_str().map(str::to_string));
        Ok((hosts, traced, output))
    }

    fn check_version(&self) -> io::Result<()> {
        let mut command = Command::new(&self.gh);
        self.isolate(&mut command);
        let version = command.arg("--version").output()?;
        assert!(
            String::from_utf8_lossy(&version.stdout).starts_with("gh version 2.103.3 "),
            "pinned to gh 2.103.3: {version:?}"
        );
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
#[ignore = "needs real gh 2.103.3: set FJ_SHIM_PARITY_GH to its path and run with --ignored"]
fn stepped_aside_calls_reach_only_the_routed_forge() -> io::Result<()> {
    let gh = PathBuf::from(
        std::env::var_os("FJ_SHIM_PARITY_GH").expect("set FJ_SHIM_PARITY_GH to gh 2.103.3"),
    );
    let fixture = Fixture::new(gh)?;
    fixture.check_version()?;
    let mut failures = Vec::new();
    // Host-less `OWNER/REPO` arguments, and commands that only use gh's default host. github.com
    // has no meaning here: `gh search` searches the forge, and `gh gist` gets the forge's answer.
    let cases: [&[&str]; 6] = [
        &["repo", "delete", "o/target", "--yes"],
        &["repo", "clone", "o/target"],
        &["search", "prs", "foo"],
        &["gist", "list"],
        // A launched form, which steps aside on the launch path, and one naming the forge.
        &["pr", "view", "1"],
        &["api", "--hostname", "forge.test", "user"],
    ];
    for args in cases {
        let (hosts, traced, output) = fixture.requests(args)?;
        if hosts.is_empty() || hosts.iter().any(|host| host != "forge.test") {
            failures.push(format!("{args:?}: requests to {hosts:?}, {output:?}"));
        }
        if traced.as_deref() != Some("forge.test") {
            failures.push(format!("{args:?}: the trace names {traced:?}"));
        }
    }
    // A call naming another host doesn't step aside: nothing reaches github.com.
    for args in [
        &["repo", "clone", "github.com/o/target"][..],
        &["repo", "delete", "github.com/o/target", "--yes"][..],
    ] {
        let (hosts, _, output) = fixture.requests(args)?;
        if hosts.iter().any(|host| !host.ends_with("forge.test")) {
            failures.push(format!("{args:?}: requests to {hosts:?}, {output:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}
