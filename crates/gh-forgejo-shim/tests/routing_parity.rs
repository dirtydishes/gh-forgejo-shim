//! The wrapper must pick the same repository real gh would (finding F3).
//!
//! `wrapper_matches_table` runs the dispatcher and reads its route trace: the repository, and the
//! allowlist entry it routes to. `real_gh_matches_table` runs real gh v2.103.3 on the same checkout
//! and reads the request URL from `GH_DEBUG=api`. It is ignored unless `FJ_SHIM_PARITY_GH` names
//! that binary. It checks two things:
//! - a routed case's repository, with gh given the `hosts.yml` key set a launched gh would get:
//!   the user's hosts plus the allowlist hosts, or the user's hosts alone when the wrapper chose
//!   plain gh's own key set;
//! - for a delegated case, that plain gh, given only the user's hosts, doesn't target a host the
//!   allowlist serves.
//!
//! gh's `http_unix_socket` points at a path nothing listens on, so every request fails at once and
//! nothing leaves the machine. A stub `ssh` on PATH answers `ssh -G`, so the user's own ssh config
//! never decides a row.
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

/// How gh picks its base repository for the command the case runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Base {
    /// `SmartBaseRepoFunc`, which honors `gh repo set-default`: `pr`, `browse` and most others.
    Smart,
    /// `BaseRepoFunc`, the first ranked remote: `secret`, `variable` and a few others.
    First,
}

/// The `hosts.yml` key set a launched gh gets.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Keys {
    /// The user's hosts plus every allowlist host.
    Shim,
    /// The user's hosts alone, because plain gh's own choice was a Forgejo host.
    User,
}

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    /// `(remote name, fetch URL, push URL)`, added in this order.
    remotes: &'static [(&'static str, &'static str, Option<&'static str>)],
    /// `(remote name, value)` lines for `remote.<name>.gh-resolved`, added in this order.
    resolved: &'static [(&'static str, &'static str)],
    /// Hosts in the user's own gh `hosts.yml`.
    gh_hosts: &'static [&'static str],
    /// `FJ_SHIM_HOSTS`, the shim's Forgejo allowlist.
    allowlist: &'static str,
    env: &'static [(&'static str, &'static str)],
    args: &'static [&'static str],
    base: Base,
    /// Replaces the command words `base` picks, for both the wrapper and real gh.
    command: Option<&'static [&'static str]>,
    /// `(name, expansion)` aliases in the user's gh `config.yml`.
    aliases: &'static [(&'static str, &'static str)],
    /// Compare only the host with real gh: its GraphQL request names no repository in the URL.
    host_only: bool,
    /// `(host, owner, name)` the wrapper detects, or `None` when there's no repository.
    expected: Option<(&'static str, &'static str, &'static str)>,
    /// The allowlist entry the wrapper routes to, or `None` when it delegates.
    route: Option<&'static str>,
    keys: Keys,
}

const BASE: Case = Case {
    name: "",
    remotes: &[],
    resolved: &[],
    gh_hosts: &[],
    allowlist: FORGE,
    env: &[],
    args: &[],
    base: Base::Smart,
    command: None,
    aliases: &[],
    host_only: false,
    expected: None,
    route: None,
    keys: Keys::Shim,
};

const ORIGIN_FORGE: (&str, &str, Option<&str>) =
    ("origin", "https://forge.test/local/project.git", None);
const UPSTREAM_GITHUB: (&str, &str, Option<&str>) =
    ("upstream", "https://github.com/up/stream.git", None);

const CASES: &[Case] = &[
    Case {
        name: "single Forgejo origin",
        remotes: &[ORIGIN_FORGE],
        expected: Some((FORGE, "local", "project")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "upstream outranks origin when gh knows two hosts",
        remotes: &[ORIGIN_FORGE, UPSTREAM_GITHUB],
        gh_hosts: &[GITHUB],
        expected: Some((GITHUB, "up", "stream")),
        ..BASE
    },
    Case {
        name: "a single known host filters the remotes to that host",
        remotes: &[ORIGIN_FORGE, UPSTREAM_GITHUB],
        expected: Some((FORGE, "local", "project")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "Forgejo upstream outranks GitHub origin",
        remotes: &[
            ("origin", "https://github.com/me/fork.git", None),
            ("upstream", "git@forge.test:up/stream.git", None),
        ],
        gh_hosts: &[GITHUB],
        expected: Some((FORGE, "up", "stream")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "GH_HOST keeps only its own remotes",
        remotes: &[
            ORIGIN_FORGE,
            ("github", "https://github.com/gh/repo.git", None),
        ],
        gh_hosts: &[GITHUB],
        env: &[("GH_HOST", GITHUB)],
        expected: Some((GITHUB, "gh", "repo")),
        ..BASE
    },
    Case {
        name: "GH_HOST with no matching remote finds nothing",
        remotes: &[ORIGIN_FORGE],
        gh_hosts: &[GITHUB],
        env: &[("GH_HOST", GITHUB)],
        ..BASE
    },
    Case {
        name: "a saved default remote wins over ranking",
        remotes: &[ORIGIN_FORGE, UPSTREAM_GITHUB],
        resolved: &[("origin", "base")],
        gh_hosts: &[GITHUB],
        expected: Some((FORGE, "local", "project")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "a saved default repository keeps its remote's host",
        remotes: &[ORIGIN_FORGE],
        resolved: &[("origin", "fork/thing")],
        expected: Some((FORGE, "fork", "thing")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "the last saved default line wins",
        remotes: &[ORIGIN_FORGE],
        resolved: &[("origin", "a/first"), ("origin", "b/second")],
        expected: Some((FORGE, "b", "second")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "a dotted remote name never matches its saved default",
        remotes: &[
            ("my.fork", "https://forge.test/me/fork.git", None),
            UPSTREAM_GITHUB,
        ],
        resolved: &[("my.fork", "base")],
        gh_hosts: &[GITHUB],
        expected: Some((GITHUB, "up", "stream")),
        ..BASE
    },
    Case {
        name: "variable commands ignore the saved default",
        remotes: &[ORIGIN_FORGE, UPSTREAM_GITHUB],
        resolved: &[("origin", "base")],
        gh_hosts: &[GITHUB],
        base: Base::First,
        expected: Some((GITHUB, "up", "stream")),
        ..BASE
    },
    Case {
        name: "-R owner/repo takes the only known host",
        remotes: &[ORIGIN_FORGE],
        args: &["-R", "other/target"],
        expected: Some((FORGE, "other", "target")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "-R owner/repo falls back to github.com with two known hosts",
        remotes: &[ORIGIN_FORGE],
        gh_hosts: &[GITHUB],
        args: &["-R", "other/target"],
        expected: Some((GITHUB, "other", "target")),
        ..BASE
    },
    Case {
        name: "GH_REPO owner/repo takes the only known host",
        remotes: &[ORIGIN_FORGE],
        env: &[("GH_REPO", "other/target")],
        expected: Some((FORGE, "other", "target")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "-R with a host",
        remotes: &[UPSTREAM_GITHUB],
        gh_hosts: &[GITHUB],
        args: &["-R", "forge.test/a/b"],
        expected: Some((FORGE, "a", "b")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "a -R URL loses an upper-case WWW.",
        args: &["-R", "https://WWW.forge.test/o/r"],
        expected: Some((FORGE, "o", "r")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "plain gh's single Forgejo host routes when the allowlist names two",
        remotes: &[ORIGIN_FORGE, UPSTREAM_GITHUB],
        gh_hosts: &[FORGE],
        allowlist: "forge.test,other.test",
        expected: Some((FORGE, "local", "project")),
        route: Some(FORGE),
        keys: Keys::User,
        ..BASE
    },
    Case {
        name: "-R owner/repo takes plain gh's single Forgejo host",
        remotes: &[ORIGIN_FORGE],
        gh_hosts: &[FORGE],
        allowlist: "forge.test,other.test",
        args: &["-R", "other/target"],
        expected: Some((FORGE, "other", "target")),
        route: Some(FORGE),
        keys: Keys::User,
        ..BASE
    },
    Case {
        name: "an HTTPS port is not part of gh's host",
        remotes: &[("origin", "https://forge.test:3443/owner/repo.git", None)],
        allowlist: "forge.test:3443",
        expected: Some((FORGE, "owner", "repo")),
        route: Some("forge.test:3443"),
        ..BASE
    },
    Case {
        name: "a custom-port forge counts as two hosts, so -R owner/repo defaults to github.com",
        remotes: &[("origin", "https://forge.test:3443/owner/repo.git", None)],
        allowlist: "forge.test:3443",
        args: &["-R", "o/r"],
        expected: Some((GITHUB, "o", "r")),
        ..BASE
    },
    Case {
        name: "two entries on one hostname leave a portless remote unrouted",
        remotes: &[("origin", "https://forge.test:4443/owner/repo.git", None)],
        allowlist: "forge.test:3443,forge.test:4443",
        expected: Some((FORGE, "owner", "repo")),
        ..BASE
    },
    Case {
        name: "a port no entry has is not routed",
        args: &["-R", "forge.test:9999/o/r"],
        allowlist: "forge.test:3443",
        expected: Some(("forge.test:9999", "o", "r")),
        ..BASE
    },
    Case {
        name: "a remote below a subpath is not a repository to gh",
        remotes: &[("origin", "https://forge.test/sub/owner/repo.git", None)],
        ..BASE
    },
    Case {
        name: "the remote host is lowercased",
        remotes: &[("origin", "https://Forge.Test/owner/repo", None)],
        expected: Some((FORGE, "owner", "repo")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "a leading www. is dropped",
        remotes: &[("origin", "https://www.forge.test/owner/repo", None)],
        expected: Some((FORGE, "owner", "repo")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        // gh trims a literal `www.` from a remote before it lowercases (ghrepo/repo.go:74-76).
        name: "an upper-case WWW. remote is kept",
        remotes: &[("origin", "https://WWW.Forge.Test/owner/repo", None)],
        ..BASE
    },
    Case {
        name: "an ssh alias resolves through ssh -G",
        remotes: &[("origin", "git@forge-alias:owner/repo.git", None)],
        expected: Some((FORGE, "owner", "repo")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "other remote names keep git's order",
        remotes: &[
            ("b", "https://forge.test/second/repo.git", None),
            ("a", "https://forge.test/first/repo.git", None),
        ],
        expected: Some((FORGE, "first", "repo")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "a URL's query and fragment aren't part of its path",
        remotes: &[("origin", "https://forge.test/o/r?ref=a/b#c/d", None)],
        expected: Some((FORGE, "o", "r")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "an alias counts as the command it expands to",
        remotes: &[
            ("origin", "https://github.com/me/fork.git", None),
            ("upstream", "https://forge.test/up/stream.git", None),
        ],
        resolved: &[("origin", "base")],
        gh_hosts: &[GITHUB],
        command: Some(&["vars"]),
        aliases: &[("vars", "variable list")],
        expected: Some((FORGE, "up", "stream")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "a PR URL argument wins over -R",
        gh_hosts: &[GITHUB],
        command: Some(&[
            "pr",
            "view",
            "https://forge.test/o/r/pull/1",
            "-R",
            "github.com/a/b",
        ]),
        host_only: true,
        expected: Some((FORGE, "o", "r")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "an empty -R counts as none",
        remotes: &[ORIGIN_FORGE],
        args: &["-R", ""],
        expected: Some((FORGE, "local", "project")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "a portless entry and a ported entry on one hostname are ambiguous",
        remotes: &[("origin", "https://forge.test:3443/owner/repo.git", None)],
        allowlist: "forge.test,forge.test:3443",
        expected: Some((FORGE, "owner", "repo")),
        ..BASE
    },
    Case {
        name: "GH_HOST keeps its spelling until gh normalizes it",
        env: &[("GH_HOST", "WWW.forge.test")],
        args: &["-R", "o/r"],
        expected: Some(("www.forge.test", "o", "r")),
        ..BASE
    },
    Case {
        name: "a saved default normalizes its remote's host again",
        remotes: &[("origin", "https://WWW.forge.test/o/r", None)],
        resolved: &[("origin", "fork/target")],
        gh_hosts: &["www.forge.test", GITHUB],
        expected: Some((FORGE, "fork", "target")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "user host keys count as written",
        gh_hosts: &["Forge.Test"],
        args: &["-R", "o/r"],
        expected: Some((FORGE, "o", "r")),
        route: Some(FORGE),
        keys: Keys::User,
        ..BASE
    },
    Case {
        name: "-R owner/repo.git keeps .git",
        args: &["-R", "forge.test/o/r.git"],
        expected: Some((FORGE, "o", "r.git")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        // 13 remotes: gh's sort.Sort is no longer an insertion sort, and isn't stable.
        name: "thirteen equally ranked remotes come out in Go's sort order",
        remotes: &[
            ("a00", "https://github.com/o00/r.git", None),
            ("a01", "https://github.com/o01/r.git", None),
            ("a02", "https://github.com/o02/r.git", None),
            ("a03", "https://github.com/o03/r.git", None),
            ("a04", "https://github.com/o04/r.git", None),
            ("a05", "https://github.com/o05/r.git", None),
            ("a06", "https://forge.test/o06/r.git", None),
            ("a07", "https://github.com/o07/r.git", None),
            ("a08", "https://github.com/o08/r.git", None),
            ("a09", "https://github.com/o09/r.git", None),
            ("a10", "https://github.com/o10/r.git", None),
            ("a11", "https://github.com/o11/r.git", None),
            ("upstream", "https://unknown.test/x/y.git", None),
        ],
        gh_hosts: &[GITHUB, FORGE],
        expected: Some((FORGE, "o06", "r")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "nineteen equally ranked remotes come out in Go's sort order",
        remotes: &[
            ("b00", "https://github.com/o00/r.git", None),
            ("b01", "https://github.com/o01/r.git", None),
            ("b02", "https://github.com/o02/r.git", None),
            ("b03", "https://github.com/o03/r.git", None),
            ("b04", "https://github.com/o04/r.git", None),
            ("b05", "https://forge.test/o05/r.git", None),
            ("b06", "https://github.com/o06/r.git", None),
            ("b07", "https://github.com/o07/r.git", None),
            ("b08", "https://github.com/o08/r.git", None),
            ("b09", "https://github.com/o09/r.git", None),
            ("b10", "https://github.com/o10/r.git", None),
            ("b11", "https://forge.test/o11/r.git", None),
            ("b12", "https://github.com/o12/r.git", None),
            ("b13", "https://github.com/o13/r.git", None),
            ("b14", "https://github.com/o14/r.git", None),
            ("b15", "https://github.com/o15/r.git", None),
            ("b16", "https://github.com/o16/r.git", None),
            ("b17", "https://github.com/o17/r.git", None),
            ("b18", "https://github.com/o18/r.git", None),
            ("upstream", "https://unknown.test/x/y.git", None),
        ],
        gh_hosts: &[GITHUB, FORGE],
        expected: Some((GITHUB, "o10", "r")),
        ..BASE
    },
    Case {
        name: "a release URL argument is data, and -R wins",
        gh_hosts: &[GITHUB, FORGE],
        command: Some(&[
            "release",
            "view",
            "https://github.com/o/r",
            "-R",
            "forge.test/a/b",
        ]),
        expected: Some((FORGE, "a", "b")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "a remote URL Go's parser refuses is dropped",
        remotes: &[
            ("origin", "https://forge.test/o/r.git", None),
            ("upstream", "https://bad%ZZ@github.com/up/stream.git", None),
        ],
        gh_hosts: &[GITHUB, FORGE],
        expected: Some((FORGE, "o", "r")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "an alias expansion splits as a shell would",
        remotes: &[
            ("origin", "https://github.com/me/fork.git", None),
            ("upstream", "https://forge.test/up/stream.git", None),
        ],
        resolved: &[("origin", "base")],
        gh_hosts: &[GITHUB, FORGE],
        command: Some(&["vars"]),
        aliases: &[("vars", "\"variable\" list")],
        expected: Some((FORGE, "up", "stream")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "an alias named like a gh command is ignored",
        remotes: &[ORIGIN_FORGE, UPSTREAM_GITHUB],
        resolved: &[("origin", "base")],
        gh_hosts: &[GITHUB, FORGE],
        command: Some(&["pr", "view", "1"]),
        aliases: &[("pr", "variable list")],
        host_only: true,
        expected: Some((FORGE, "local", "project")),
        route: Some(FORGE),
        ..BASE
    },
    Case {
        name: "gh api with an absolute URL goes where the URL says",
        remotes: &[ORIGIN_FORGE],
        command: Some(&["api", "https://api.github.com/user"]),
        ..BASE
    },
    Case {
        name: "a push URL stands in for a local fetch URL",
        remotes: &[(
            "origin",
            "/srv/mirror.git",
            Some("https://forge.test/owner/repo.git"),
        )],
        expected: Some((FORGE, "owner", "repo")),
        route: Some(FORGE),
        ..BASE
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
        let route = (trace["route"]["kind"] == "forgejo")
            .then(|| trace["host"].as_str().unwrap_or_default().to_string());
        // Routing marks a launch decided by plain gh's own hosts in its reason.
        let user_keys = trace["route"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.ends_with("user's gh hosts"));
        let keys_match = case.route.is_none() || user_keys == (case.keys == Keys::User);
        if actual != owned(case.expected) || route.as_deref() != case.route || !keys_match {
            failures.push(format!(
                "{}: expected {:?} routed to {:?}, wrapper chose {actual:?} routed to {route:?} \
                 (reason {})",
                case.name, case.expected, case.route, trace["route"]["reason"]
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
        if case.route.is_some() {
            let actual = fixture.real_gh_target(&gh, case, case.keys)?;
            let matches = if case.host_only {
                actual.as_ref().map(|(host, _, _)| host.as_str())
                    == case.expected.map(|(host, _, _)| host)
            } else {
                actual == owned(case.expected)
            };
            if !matches {
                failures.push(format!(
                    "{}: table says {:?}, gh chose {actual:?}",
                    case.name, case.expected
                ));
            }
        } else {
            // The model the wrapper delegated on must still be gh's own choice...
            let modelled = fixture.real_gh_target(&gh, case, Keys::Shim)?;
            if modelled != owned(case.expected) {
                failures.push(format!(
                    "{}: table says {:?}, gh with the shim's hosts chose {modelled:?}",
                    case.name, case.expected
                ));
            }
            // ...and plain gh, which the call goes to, must not land on a served Forgejo host.
            let plain = fixture.real_gh_target(&gh, case, Keys::User)?;
            if let Some((host, _, _)) = &plain {
                if served(host, case.allowlist) {
                    failures.push(format!(
                        "{}: delegated, but plain gh chose served host {host}",
                        case.name
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

fn owned(value: Option<(&str, &str, &str)>) -> Option<(String, String, String)> {
    value.map(|(host, owner, name)| (host.to_string(), owner.to_string(), name.to_string()))
}

/// Whether the allowlist serves `host`: the same entry, or the one entry with its hostname when
/// `host` has no port.
fn served(host: &str, allowlist: &str) -> bool {
    let entries = allowlist.split(',').collect::<Vec<_>>();
    entries.contains(&host)
        || (!host.contains(':')
            && entries
                .iter()
                .filter(|entry| entry.split(':').next() == Some(host))
                .count()
                == 1)
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
        for dir in [&home, &repo, &bin] {
            fs::create_dir_all(dir)?;
        }
        executable(&bin.join("real-gh"), "#!/bin/sh\nexit 0\n")?;
        // `ssh -G [--] HOST`: `forge-alias` resolves to forge.test, anything else to itself.
        executable(
            &bin.join("ssh"),
            "#!/bin/sh\n[ \"$1\" = -G ] || exit 1\nshift\n[ \"$1\" = -- ] && shift\n\
             case \"$1\" in forge-alias) host=forge.test ;; *) host=\"$1\" ;; esac\n\
             printf 'user git\\nhostname %s\\nport 22\\n' \"$host\"\n",
        )?;
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

        fixture.git(["init", "-q"])?;
        for (name, fetch, push) in case.remotes {
            fixture.git(["remote", "add", name, fetch])?;
            if let Some(push) = push {
                fixture.git(["remote", "set-url", "--push", name, push])?;
            }
        }
        for (name, value) in case.resolved {
            fixture.git([
                "config",
                "--add",
                &format!("remote.{name}.gh-resolved"),
                value,
            ])?;
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

    /// A gh config directory holding `hosts`, with a socket nobody listens on.
    fn gh_config(&self, label: &str, hosts: &[&str], case: &Case) -> io::Result<PathBuf> {
        let dir = self.root.join(label);
        fs::create_dir_all(&dir)?;
        if !hosts.is_empty() {
            let body = hosts
                .iter()
                .map(|host| format!("{host}:\n    oauth_token: fixture-token\n"))
                .collect::<String>();
            fs::write(dir.join("hosts.yml"), body)?;
        }
        fs::write(
            dir.join("config.yml"),
            format!(
                "version: \"1\"\nhttp_unix_socket: {}\naliases:\n{}",
                self.root.join("no-socket").display(),
                case.aliases
                    .iter()
                    .map(|(name, expansion)| {
                        format!("    {name}: '{}'\n", expansion.replace('\'', "''"))
                    })
                    .collect::<String>()
            ),
        )?;
        Ok(dir)
    }

    fn wrapper_trace(&self, case: &Case) -> Result<Value, Box<dyn std::error::Error>> {
        let trace = self.root.join("trace.jsonl");
        let mut command = Command::new(env!("CARGO_BIN_EXE_gh-forgejo-shim"));
        self.isolate(&mut command);
        let words: &[&str] = match (case.command, case.base) {
            (Some(words), _) => words,
            (None, Base::Smart) => &["pr", "view", "--help"],
            (None, Base::First) => &["variable", "list", "--help"],
        };
        command
            .arg("gh")
            .args(words)
            .args(case.args)
            .env(
                "GH_CONFIG_DIR",
                self.gh_config("gh-user", case.gh_hosts, case)?,
            )
            .env("FJ_SHIM_HOSTS", case.allowlist)
            .env("FJ_SHIM_REAL_GH", self.bin.join("real-gh"))
            .env("FJ_SHIM_AUTH_STORE", "file")
            .env("FJ_SHIM_TRACE", &trace)
            .envs(case.env.iter().copied());
        // The exit status doesn't matter here: an unsupported command still writes its route.
        run(&mut command)?;
        let text = fs::read_to_string(trace)?;
        Ok(serde_json::from_str(
            text.lines().last().ok_or("missing route trace")?,
        )?)
    }

    fn real_gh_target(
        &self,
        gh: &Path,
        case: &Case,
        keys: Keys,
    ) -> io::Result<Option<(String, String, String)>> {
        let mut hosts = case.gh_hosts.to_vec();
        if keys == Keys::Shim {
            for entry in case.allowlist.split(',') {
                let hostname = entry.split(':').next().unwrap_or(entry);
                for key in [hostname, entry] {
                    if !hosts.contains(&key) {
                        hosts.push(key);
                    }
                }
            }
        }
        let label = match keys {
            Keys::Shim => "gh-shim-keys",
            Keys::User => "gh-user-keys",
        };
        let words: &[&str] = match (case.command, case.base) {
            (Some(words), _) => words,
            (None, Base::Smart) => &["browse", "-n"],
            (None, Base::First) => &["variable", "list"],
        };
        let mut command = Command::new(gh);
        self.isolate(&mut command);
        command
            .args(words)
            .args(case.args)
            .env("GH_CONFIG_DIR", self.gh_config(label, &hosts, case)?)
            .env("GH_DEBUG", "api")
            .env("GH_TELEMETRY", "0")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .envs(case.env.iter().copied());
        let output = run(&mut command)?;
        let log = String::from_utf8_lossy(&output.stderr);
        let mut requests = log
            .lines()
            .filter_map(|line| line.strip_prefix("* Request to https://"));
        if case.host_only {
            // The first request names the host, even when it is gh's `/api/v3/meta` probe.
            return Ok(requests.next().and_then(|rest| {
                let host = rest.split('/').next()?;
                let host = if host == "api.github.com" {
                    GITHUB
                } else {
                    host
                };
                Some((host.to_string(), "?".into(), "?".into()))
            }));
        }
        Ok(requests.find_map(parse_request_target))
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

fn executable(path: &Path, contents: &str) -> io::Result<()> {
    fs::write(path, contents)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
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
