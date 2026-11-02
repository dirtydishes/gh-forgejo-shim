//! The wrapper must pick the same repository real gh would (finding F3).
//!
//! Each case is checked twice. `wrapper_matches_table` runs the dispatcher and reads its route
//! trace. `real_gh_matches_table` runs real gh v2.103.3 on the same checkout and reads the request
//! URL from `GH_DEBUG=api`; it is ignored unless `FJ_SHIM_PARITY_GH` names that binary. gh gets a
//! `hosts.yml` listing the user's gh hosts plus the Forgejo host, which is the configuration the
//! wrapper models: to gh, a Forgejo host the shim serves is a known host. Its `http_unix_socket`
//! points at a path nothing listens on, so every request fails at once and nothing leaves the
//! machine.
#![cfg(unix)]

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
const FORGE: &str = "forge.test";
const GITHUB: &str = "github.com";

struct Case {
    name: &'static str,
    /// `(remote name, fetch URL, push URL)`, added in this order.
    remotes: &'static [(&'static str, &'static str, Option<&'static str>)],
    /// `(remote name, value)` for `remote.<name>.gh-resolved`, as `gh repo set-default` writes.
    resolved: Option<(&'static str, &'static str)>,
    /// Hosts in the user's own gh `hosts.yml`.
    gh_hosts: &'static [&'static str],
    /// The shim's Forgejo allowlist entry.
    allowlist: &'static str,
    env: &'static [(&'static str, &'static str)],
    args: &'static [&'static str],
    /// `(host, owner, name)` gh resolves, or `None` when gh finds no repository.
    expected: Option<(&'static str, &'static str, &'static str)>,
}

const ORIGIN_FORGE: (&str, &str, Option<&str>) =
    ("origin", "https://forge.test/local/project.git", None);
const UPSTREAM_GITHUB: (&str, &str, Option<&str>) =
    ("upstream", "https://github.com/up/stream.git", None);

const CASES: &[Case] = &[
    Case {
        name: "single Forgejo origin",
        remotes: &[ORIGIN_FORGE],
        resolved: None,
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: Some((FORGE, "local", "project")),
    },
    Case {
        name: "upstream outranks origin when gh knows two hosts",
        remotes: &[ORIGIN_FORGE, UPSTREAM_GITHUB],
        resolved: None,
        gh_hosts: &[GITHUB],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: Some((GITHUB, "up", "stream")),
    },
    Case {
        name: "a single known host filters the remotes to that host",
        remotes: &[ORIGIN_FORGE, UPSTREAM_GITHUB],
        resolved: None,
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: Some((FORGE, "local", "project")),
    },
    Case {
        name: "Forgejo upstream outranks GitHub origin",
        remotes: &[
            ("origin", "https://github.com/me/fork.git", None),
            ("upstream", "git@forge.test:up/stream.git", None),
        ],
        resolved: None,
        gh_hosts: &[GITHUB],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: Some((FORGE, "up", "stream")),
    },
    Case {
        name: "GH_HOST keeps only its own remotes",
        remotes: &[
            ORIGIN_FORGE,
            ("github", "https://github.com/gh/repo.git", None),
        ],
        resolved: None,
        gh_hosts: &[GITHUB],
        allowlist: FORGE,
        env: &[("GH_HOST", GITHUB)],
        args: &[],
        expected: Some((GITHUB, "gh", "repo")),
    },
    Case {
        name: "GH_HOST with no matching remote finds nothing",
        remotes: &[ORIGIN_FORGE],
        resolved: None,
        gh_hosts: &[GITHUB],
        allowlist: FORGE,
        env: &[("GH_HOST", GITHUB)],
        args: &[],
        expected: None,
    },
    Case {
        name: "a saved default remote wins over ranking",
        remotes: &[ORIGIN_FORGE, UPSTREAM_GITHUB],
        resolved: Some(("origin", "base")),
        gh_hosts: &[GITHUB],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: Some((FORGE, "local", "project")),
    },
    Case {
        name: "a saved default repository keeps its remote's host",
        remotes: &[ORIGIN_FORGE],
        resolved: Some(("origin", "fork/thing")),
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: Some((FORGE, "fork", "thing")),
    },
    Case {
        name: "-R owner/repo takes the only known host",
        remotes: &[ORIGIN_FORGE],
        resolved: None,
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[],
        args: &["-R", "other/target"],
        expected: Some((FORGE, "other", "target")),
    },
    Case {
        name: "-R owner/repo falls back to github.com with two known hosts",
        remotes: &[ORIGIN_FORGE],
        resolved: None,
        gh_hosts: &[GITHUB],
        allowlist: FORGE,
        env: &[],
        args: &["-R", "other/target"],
        expected: Some((GITHUB, "other", "target")),
    },
    Case {
        name: "GH_REPO owner/repo takes the only known host",
        remotes: &[ORIGIN_FORGE],
        resolved: None,
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[("GH_REPO", "other/target")],
        args: &[],
        expected: Some((FORGE, "other", "target")),
    },
    Case {
        name: "-R with a host",
        remotes: &[UPSTREAM_GITHUB],
        resolved: None,
        gh_hosts: &[GITHUB],
        allowlist: FORGE,
        env: &[],
        args: &["-R", "forge.test/a/b"],
        expected: Some((FORGE, "a", "b")),
    },
    Case {
        name: "an HTTPS port is not part of gh's host",
        remotes: &[("origin", "https://forge.test:3443/owner/repo.git", None)],
        resolved: None,
        gh_hosts: &[],
        allowlist: "forge.test:3443",
        env: &[],
        args: &[],
        expected: Some((FORGE, "owner", "repo")),
    },
    Case {
        name: "a remote below a subpath is not a repository to gh",
        remotes: &[("origin", "https://forge.test/sub/owner/repo.git", None)],
        resolved: None,
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: None,
    },
    Case {
        name: "the remote host is lowercased",
        remotes: &[("origin", "https://Forge.Test/owner/repo", None)],
        resolved: None,
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: Some((FORGE, "owner", "repo")),
    },
    Case {
        name: "a leading www. is dropped",
        remotes: &[("origin", "https://www.forge.test/owner/repo", None)],
        resolved: None,
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: Some((FORGE, "owner", "repo")),
    },
    Case {
        // gh trims a literal `www.` before it lowercases (ghrepo/repo.go:74-76).
        name: "an upper-case WWW. is kept",
        remotes: &[("origin", "https://WWW.Forge.Test/owner/repo", None)],
        resolved: None,
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: None,
    },
    Case {
        name: "other remote names keep git's order",
        remotes: &[
            ("b", "https://forge.test/second/repo.git", None),
            ("a", "https://forge.test/first/repo.git", None),
        ],
        resolved: None,
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: Some((FORGE, "first", "repo")),
    },
    Case {
        name: "a push URL stands in for a local fetch URL",
        remotes: &[(
            "origin",
            "/srv/mirror.git",
            Some("https://forge.test/owner/repo.git"),
        )],
        resolved: None,
        gh_hosts: &[],
        allowlist: FORGE,
        env: &[],
        args: &[],
        expected: Some((FORGE, "owner", "repo")),
    },
];

#[test]
fn wrapper_matches_table() -> TestResult {
    let mut failures = Vec::new();
    for case in CASES {
        let fixture = Fixture::new(case)?;
        let trace = fixture.wrapper_trace(case)?;
        let actual = trace
            .get("repo")
            .filter(|repo| !repo.is_null())
            .map(|repo| {
                (
                    repo["host"].as_str().unwrap_or_default().to_string(),
                    repo["owner"].as_str().unwrap_or_default().to_string(),
                    repo["name"].as_str().unwrap_or_default().to_string(),
                )
            });
        let expected_kind = match case.expected {
            Some((host, _, _)) if host == FORGE => "forgejo",
            _ => "delegate",
        };
        if actual != owned(case.expected) || trace["route"]["kind"] != expected_kind {
            failures.push(format!(
                "{}: expected {:?} ({expected_kind}), wrapper chose {actual:?} ({})",
                case.name, case.expected, trace["route"]["kind"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
#[ignore = "needs real gh 2.103.3: set FJ_SHIM_PARITY_GH to its path and run with --ignored"]
fn real_gh_matches_table() -> TestResult {
    let gh = PathBuf::from(std::env::var_os("FJ_SHIM_PARITY_GH").ok_or("set FJ_SHIM_PARITY_GH")?);
    // Isolated like every other gh run here: without HOME, gh writes its state below the cwd.
    let probe = Fixture::new(&CASES[0])?;
    let mut version = Command::new(&gh);
    probe.isolate(&mut version);
    let version = version.arg("--version").output()?;
    assert!(
        String::from_utf8_lossy(&version.stdout).starts_with("gh version 2.103.3 "),
        "the table is pinned to gh 2.103.3"
    );
    let mut failures = Vec::new();
    for case in CASES {
        let fixture = Fixture::new(case)?;
        let actual = fixture.real_gh_target(&gh, case)?;
        if actual != owned(case.expected) {
            failures.push(format!(
                "{}: table says {:?}, gh chose {actual:?}",
                case.name, case.expected
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

fn owned(value: Option<(&str, &str, &str)>) -> Option<(String, String, String)> {
    value.map(|(host, owner, name)| (host.to_string(), owner.to_string(), name.to_string()))
}

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new(case: &Case) -> io::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "gfj-routing-parity-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let repo = root.join("repo");
        let bin = root.join("bin");
        for dir in [
            &home,
            &repo,
            &bin,
            &root.join("gh-user"),
            &root.join("gh-oracle"),
        ] {
            fs::create_dir_all(dir)?;
        }
        let real_gh = bin.join("real-gh");
        fs::write(&real_gh, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&real_gh, fs::Permissions::from_mode(0o755))?;
        // Reuse the installed git binary, but never a wrapper script found on PATH.
        let git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .filter_map(|dir| dir.join("git").canonicalize().ok())
            .find(|path| path.is_file() && !is_script(path))
            .ok_or_else(|| io::Error::other("a git binary is required"))?;
        symlink(git, bin.join("git"))?;
        let fixture = Self {
            root,
            home,
            repo,
            bin,
        };

        write_hosts(&fixture.root.join("gh-user"), case.gh_hosts)?;
        let mut oracle_hosts = case.gh_hosts.to_vec();
        oracle_hosts.push(FORGE);
        write_hosts(&fixture.root.join("gh-oracle"), &oracle_hosts)?;
        fs::write(
            fixture.root.join("gh-oracle/config.yml"),
            format!(
                "version: \"1\"\nhttp_unix_socket: {}\n",
                fixture.root.join("no-socket").display()
            ),
        )?;

        fixture.git(["init", "-q"])?;
        for (name, fetch, push) in case.remotes {
            fixture.git(["remote", "add", name, fetch])?;
            if let Some(push) = push {
                fixture.git(["remote", "set-url", "--push", name, push])?;
            }
        }
        if let Some((name, value)) = case.resolved {
            fixture.git(["config", &format!("remote.{name}.gh-resolved"), value])?;
        }
        Ok(fixture)
    }

    fn isolate(&self, command: &mut Command) {
        command
            .env_clear()
            .env("HOME", &self.home)
            .env(
                "PATH",
                std::env::join_paths([&self.bin, Path::new("/usr/bin"), Path::new("/bin")])
                    .unwrap_or_default(),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .current_dir(&self.repo)
            .stdin(Stdio::null());
    }

    fn git<I, S>(&self, args: I) -> io::Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(self.bin.join("git"));
        self.isolate(&mut command);
        let output = command.args(args).output()?;
        assert!(output.status.success(), "git fixture failed: {output:?}");
        Ok(())
    }

    fn wrapper_trace(&self, case: &Case) -> Result<Value, Box<dyn std::error::Error>> {
        let trace = self.root.join("trace.jsonl");
        let mut command = Command::new(env!("CARGO_BIN_EXE_gh-forgejo-shim"));
        self.isolate(&mut command);
        command
            .arg("gh")
            .args(["pr", "view", "--help"])
            .args(case.args)
            .env("GH_CONFIG_DIR", self.root.join("gh-user"))
            .env("FJ_SHIM_HOSTS", case.allowlist)
            .env("FJ_SHIM_REAL_GH", self.bin.join("real-gh"))
            .env("FJ_SHIM_AUTH_STORE", "file")
            .env("FJ_SHIM_TRACE", &trace)
            .envs(case.env.iter().copied());
        let output = run(&mut command)?;
        assert!(
            output.status.success(),
            "{}: wrapper failed: {output:?}",
            case.name
        );
        let text = fs::read_to_string(trace)?;
        Ok(serde_json::from_str(
            text.lines().last().ok_or("missing route trace")?,
        )?)
    }

    fn real_gh_target(
        &self,
        gh: &Path,
        case: &Case,
    ) -> io::Result<Option<(String, String, String)>> {
        let mut command = Command::new(gh);
        self.isolate(&mut command);
        command
            .args(["browse", "-n"])
            .args(case.args)
            .env("GH_CONFIG_DIR", self.root.join("gh-oracle"))
            .env("GH_DEBUG", "api")
            .env("GH_TELEMETRY", "0")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .envs(case.env.iter().copied());
        let output = run(&mut command)?;
        let log = String::from_utf8_lossy(&output.stderr);
        Ok(log
            .lines()
            .find_map(|line| line.strip_prefix("* Request to https://"))
            .and_then(parse_request_target))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// `api.github.com/repos/o/r` or `<host>/api/v3/repos/o/r`, as gh's debug log prints them.
fn parse_request_target(rest: &str) -> Option<(String, String, String)> {
    let (host, path) = rest.split_once('/')?;
    let (host, path) = match host {
        "api.github.com" => (GITHUB, path),
        host => (host, path.strip_prefix("api/v3/")?),
    };
    let mut parts = path.strip_prefix("repos/")?.split('/');
    Some((
        host.to_string(),
        parts.next()?.to_string(),
        parts.next()?.to_string(),
    ))
}

fn write_hosts(dir: &Path, hosts: &[&str]) -> io::Result<()> {
    if hosts.is_empty() {
        return Ok(());
    }
    let body = hosts
        .iter()
        .map(|host| format!("{host}:\n    oauth_token: fixture-token\n"))
        .collect::<String>();
    fs::write(dir.join("hosts.yml"), body)
}

fn run(command: &mut Command) -> io::Result<Output> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
}

fn is_script(path: &Path) -> bool {
    let mut start = [0_u8; 2];
    File::open(path)
        .and_then(|mut file| io::Read::read_exact(&mut file, &mut start))
        .map_or(true, |()| &start == b"#!")
}
