//! Real gh's version against the oldest one the shim supports (decision D3). Install and
//! bootstrap refuse a missing or older gh, and one whose version can't be read unless the user
//! opts in; doctor reports the same.

use std::fmt;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::EnvMap;
use crate::setup::ToolPath;

/// The oldest gh the shim supports.
pub const MINIMUM: Version = Version {
    major: 2,
    minor: 103,
    patch: 3,
};

/// Set to `1` to go ahead with a gh whose version can't be read, such as one built from source.
/// A gh that reports an older version is refused even then.
pub const ALLOW_UNKNOWN_ENV: &str = "FJ_SHIM_ALLOW_UNKNOWN_GH";

/// Tokens gh reads from the environment. `--version` needs none, so none is passed to it.
const TOKEN_VARS: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

/// Real gh answers in well under a second; this leaves room for a first run that the system
/// checks before starting it.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Far more than gh's two-line answer. A gh that prints more isn't answering `--version`.
const OUTPUT_CAP: usize = 64 * 1024;

const GET_GH: &str = "https://github.com/cli/cli#installation";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// What gh's `--version` said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// A release, or a pre-release of one (`2.104.0-rc.1`), which comes before that release.
    Release {
        text: String,
        version: Version,
        pre_release: bool,
    },
    /// A build from source: `DEV`, or `git describe` output such as `2.103.3-12-gabc1234`, a
    /// build some commits after the release it names.
    Dev {
        text: String,
        after: Option<Version>,
    },
    /// No version could be read, and why.
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Supported,
    /// The version can't be read, and the user opted in with [`ALLOW_UNKNOWN_ENV`].
    AllowedUnknown,
    Refused,
}

/// The verdict on real gh, worded for doctor and bootstrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhCheck {
    pub status: Status,
    pub detail: String,
    pub repair_commands: Vec<String>,
}

/// Checks the gh that discovery found, if any: runs its `--version` and compares the answer with
/// [`MINIMUM`].
pub fn check(real_gh: Option<&ToolPath>, env: &EnvMap, platform: &str) -> GhCheck {
    let Some(real_gh) = real_gh else {
        return GhCheck {
            status: Status::Refused,
            detail: format!(
                "could not find the real GitHub CLI from config, env, current PATH, or common \
                 install dirs; gh-forgejo-shim needs gh {MINIMUM} or newer, from {GET_GH}"
            ),
            repair_commands: repair_commands(platform, "install"),
        };
    };
    let found = format!("{} via {}", real_gh.path.display(), real_gh.source);
    assess(
        &found,
        &probe(&real_gh.path, env),
        allows_unknown(env),
        platform,
    )
}

fn allows_unknown(env: &EnvMap) -> bool {
    env.get(ALLOW_UNKNOWN_ENV).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

fn assess(found: &str, reading: &Reading, allow_unknown: bool, platform: &str) -> GhCheck {
    let older = |text: &str| GhCheck {
        status: Status::Refused,
        detail: format!(
            "found gh {text} at {found}, but gh-forgejo-shim needs gh {MINIMUM} or newer; \
             upgrade it from {GET_GH}"
        ),
        repair_commands: repair_commands(platform, "upgrade"),
    };
    let unknown = |reason: &str| {
        if allow_unknown {
            GhCheck {
                status: Status::AllowedUnknown,
                detail: format!(
                    "found {found}, but {reason}; going ahead because {ALLOW_UNKNOWN_ENV} is set. \
                     gh-forgejo-shim supports gh {MINIMUM} or newer"
                ),
                repair_commands: Vec::new(),
            }
        } else {
            GhCheck {
                status: Status::Refused,
                detail: format!(
                    "found {found}, but {reason}, and gh-forgejo-shim needs gh {MINIMUM} or \
                     newer, from {GET_GH}. For a gh you built yourself, set \
                     {ALLOW_UNKNOWN_ENV}=1 to go ahead"
                ),
                repair_commands: repair_commands(platform, "upgrade"),
            }
        }
    };
    match reading {
        Reading::Release {
            text,
            version,
            pre_release,
        } => {
            if *version > MINIMUM || (*version == MINIMUM && !pre_release) {
                GhCheck {
                    status: Status::Supported,
                    detail: format!("found gh {text} at {found}"),
                    repair_commands: Vec::new(),
                }
            } else {
                older(text)
            }
        }
        // Some commits after a release older than the minimum is still older: the next release
        // after it can't be the minimum's.
        Reading::Dev {
            text,
            after: Some(after),
        } if *after < MINIMUM => older(text),
        Reading::Dev { text, .. } => unknown(&format!("it's a development build ({text})")),
        Reading::Unknown(reason) => unknown(reason),
    }
}

fn repair_commands(platform: &str, verb: &str) -> Vec<String> {
    let mut commands = Vec::new();
    if platform == "darwin" {
        commands.push(format!("brew {verb} gh"));
    }
    commands.push("FJ_SHIM_REAL_GH=/path/to/gh gh-forgejo-shim doctor".to_string());
    commands
}

/// Reads gh's `gh version X.Y.Z (date)` line. Text after the version on that line, such as a
/// distribution's note, is ignored, and so is semver build metadata (`+…`).
pub fn parse(stdout: &str) -> Reading {
    let line = stdout.lines().next().unwrap_or_default().trim();
    let Some(rest) = line.strip_prefix("gh version ") else {
        return Reading::Unknown(format!(
            "its `--version` output starts {:?}, not `gh version`",
            line.chars().take(60).collect::<String>()
        ));
    };
    let text = rest
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    if text.eq_ignore_ascii_case("dev") {
        return Reading::Dev { text, after: None };
    }
    let bare = text.strip_prefix('v').unwrap_or(&text);
    let bare = bare.split_once('+').map_or(bare, |(core, _build)| core);
    let (core, suffix) = match bare.split_once('-') {
        Some((core, suffix)) => (core, Some(suffix)),
        None => (bare, None),
    };
    let Some(version) = parse_core(core) else {
        return Reading::Unknown(format!("its version, {text:?}, isn't X.Y.Z"));
    };
    match suffix {
        None => Reading::Release {
            text,
            version,
            pre_release: false,
        },
        Some(suffix) if is_describe_suffix(suffix) => Reading::Dev {
            text,
            after: Some(version),
        },
        Some(_) => Reading::Release {
            text,
            version,
            pre_release: true,
        },
    }
}

fn parse_core(core: &str) -> Option<Version> {
    let mut parts = core.split('.').map(|part| {
        (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    });
    let version = Version {
        major: parts.next()??,
        minor: parts.next()??,
        patch: parts.next()??,
    };
    parts.next().is_none().then_some(version)
}

/// `git describe`'s `<commits>-g<hash>`, with `-dirty` after it for uncommitted changes.
fn is_describe_suffix(suffix: &str) -> bool {
    let suffix = suffix.strip_suffix("-dirty").unwrap_or(suffix);
    let Some((commits, hash)) = suffix.split_once("-g") else {
        return false;
    };
    !commits.is_empty()
        && commits.bytes().all(|byte| byte.is_ascii_digit())
        && hash.len() >= 4
        && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Runs `path --version` and reads its answer. The environment is `env` without any token, and
/// with gh's update notices off, so the check never reaches the network. Stdin is closed. A gh
/// that runs past [`TIMEOUT`] or prints more than [`OUTPUT_CAP`] is killed.
pub fn probe(path: &Path, env: &EnvMap) -> Reading {
    probe_with(path, env, TIMEOUT, OUTPUT_CAP)
}

fn probe_with(path: &Path, env: &EnvMap, timeout: Duration, cap: usize) -> Reading {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .env_clear()
        .envs(
            env.iter()
                .filter(|(key, _)| !TOKEN_VARS.contains(&key.as_str())),
        )
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Reading::Unknown(format!("it couldn't be run ({error})")),
    };
    let Some(stdout) = child.stdout.take() else {
        stop(&mut child);
        return Reading::Unknown("its output couldn't be read".to_string());
    };
    // A reader thread, so a gh that never writes or never exits can't hold this one up. It reads
    // one byte past the cap to tell a full answer from too much.
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
        let result = stdout.take(limit).read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });

    let deadline = Instant::now() + timeout;
    let mut output = None;
    let status = loop {
        if output.is_none() {
            output = receiver.try_recv().ok();
        }
        if let Some(Ok(bytes)) = &output {
            if bytes.len() > cap {
                stop(&mut child);
                return too_much(cap);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                stop(&mut child);
                return Reading::Unknown(format!(
                    "its `--version` didn't finish within {} seconds",
                    timeout.as_secs()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                stop(&mut child);
                return Reading::Unknown(format!("waiting for it failed ({error})"));
            }
        }
    };
    // It exited, but whatever it started may still hold its output open.
    let output = match output {
        Some(output) => output,
        None => match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(output) => output,
            Err(_) => {
                return Reading::Unknown(
                    "its `--version` output didn't end when it exited".to_string(),
                )
            }
        },
    };
    let bytes = match output {
        Ok(bytes) => bytes,
        Err(error) => return Reading::Unknown(format!("its output couldn't be read ({error})")),
    };
    if bytes.len() > cap {
        return too_much(cap);
    }
    if !status.success() {
        return Reading::Unknown(format!("its `--version` failed ({status})"));
    }
    parse(&String::from_utf8_lossy(&bytes))
}

fn too_much(cap: usize) -> Reading {
    Reading::Unknown(format!(
        "its `--version` printed more than {} KiB",
        cap / 1024
    ))
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(text: &str, major: u64, minor: u64, patch: u64, pre_release: bool) -> Reading {
        Reading::Release {
            text: text.to_string(),
            version: Version {
                major,
                minor,
                patch,
            },
            pre_release,
        }
    }

    #[test]
    fn reads_gh_version_lines() {
        for (stdout, reading) in [
            (
                "gh version 2.103.3 (2026-05-01)\nhttps://github.com/cli/cli/releases/tag/v2.103.3\n",
                release("2.103.3", 2, 103, 3, false),
            ),
            (
                "gh version 2.104.0-rc.1 (2026-06-01)\n",
                release("2.104.0-rc.1", 2, 104, 0, true),
            ),
            ("gh version 2.103.3 (Debian 2.103.3-1)\n", release("2.103.3", 2, 103, 3, false)),
            ("gh version v2.110.0\n", release("v2.110.0", 2, 110, 0, false)),
            ("gh version 2.103.3+build.7\n", release("2.103.3+build.7", 2, 103, 3, false)),
            (
                "gh version DEV\n",
                Reading::Dev {
                    text: "DEV".to_string(),
                    after: None,
                },
            ),
            (
                "gh version 2.103.3-12-gabc1234 (2026-05-09)\n",
                Reading::Dev {
                    text: "2.103.3-12-gabc1234".to_string(),
                    after: Some(MINIMUM),
                },
            ),
            (
                "gh version 2.90.0-4-g0123abcd-dirty\n",
                Reading::Dev {
                    text: "2.90.0-4-g0123abcd-dirty".to_string(),
                    after: Some(Version {
                        major: 2,
                        minor: 90,
                        patch: 0,
                    }),
                },
            ),
        ] {
            assert_eq!(parse(stdout), reading, "{stdout:?}");
        }
    }

    #[test]
    fn output_that_isnt_a_version_reads_as_unknown() {
        for stdout in [
            "",
            "\n",
            "0.1.2\n",
            "gh-forgejo-shim 0.1.2\n",
            "gh version\n",
            "gh version 2.103\n",
            "gh version 2.103.3.1\n",
            "gh version 2.x.3\n",
            "gh version 2..3\n",
            "gh version +2.103.3\n",
            "gh version 99999999999999999999.0.0\n",
            "warning: something\ngh version 2.103.3\n",
        ] {
            assert!(
                matches!(parse(stdout), Reading::Unknown(_)),
                "{stdout:?} -> {:?}",
                parse(stdout)
            );
        }
    }

    #[test]
    fn the_minimum_and_newer_pass_and_older_is_refused_even_with_the_opt_in() {
        for (stdout, expected) in [
            ("gh version 2.103.3 (2026-05-01)", Status::Supported),
            ("gh version 2.103.4", Status::Supported),
            ("gh version 2.104.0-rc.1", Status::Supported),
            ("gh version 3.0.0", Status::Supported),
            ("gh version 2.103.3-rc.1", Status::Refused),
            ("gh version 2.103.2", Status::Refused),
            ("gh version 2.99.9", Status::Refused),
            ("gh version 1.200.0", Status::Refused),
            ("gh version 2.90.0-4-g0123abc", Status::Refused),
        ] {
            for allow_unknown in [false, true] {
                let check = assess("gh via PATH", &parse(stdout), allow_unknown, "linux");
                assert_eq!(check.status, expected, "{stdout} (opt-in {allow_unknown})");
                if expected == Status::Refused {
                    assert!(check.detail.contains("2.103.3"), "{}", check.detail);
                    assert!(check.detail.contains(GET_GH), "{}", check.detail);
                }
            }
        }
    }

    #[test]
    fn an_unknown_or_dev_version_needs_the_opt_in() {
        for stdout in [
            "gh version DEV",
            "gh version 2.103.3-12-gabc1234",
            "something else",
        ] {
            let refused = assess("gh via PATH", &parse(stdout), false, "darwin");
            assert_eq!(refused.status, Status::Refused, "{stdout}");
            assert!(
                refused.detail.contains(ALLOW_UNKNOWN_ENV),
                "{}",
                refused.detail
            );
            assert!(refused
                .repair_commands
                .contains(&"brew upgrade gh".to_string()));
            let allowed = assess("gh via PATH", &parse(stdout), true, "darwin");
            assert_eq!(allowed.status, Status::AllowedUnknown, "{stdout}");
        }
    }

    #[test]
    fn a_missing_gh_is_refused_with_how_to_get_it() {
        let check = check(None, &EnvMap::new(), "darwin");
        assert_eq!(check.status, Status::Refused);
        assert!(check.detail.contains("2.103.3") && check.detail.contains(GET_GH));
        assert_eq!(check.repair_commands[0], "brew install gh");
    }

    #[test]
    fn only_a_yes_value_opts_in() {
        for (value, allowed) in [
            ("1", true),
            ("true", true),
            ("YES", true),
            ("0", false),
            ("", false),
            ("no", false),
        ] {
            let env = EnvMap::from([(ALLOW_UNKNOWN_ENV.to_string(), value.to_string())]);
            assert_eq!(allows_unknown(&env), allowed, "{value:?}");
        }
    }

    #[cfg(unix)]
    mod spawned {
        use std::io::{self, Write};
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU64, Ordering};

        use super::*;

        static NEXT_ID: AtomicU64 = AtomicU64::new(1);

        /// A private directory of this test's own, removed when it's dropped.
        struct Dir(PathBuf);

        impl Dir {
            fn new() -> io::Result<Self> {
                use std::os::unix::fs::DirBuilderExt;

                loop {
                    let path = std::env::temp_dir().join(format!(
                        "gfj-gh-version-{}-{}",
                        std::process::id(),
                        NEXT_ID.fetch_add(1, Ordering::Relaxed)
                    ));
                    match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                        Ok(()) => return Ok(Self(path)),
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                        Err(error) => return Err(error),
                    }
                }
            }

            fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        /// A fake gh, written from a child `sh` so this process never holds a write descriptor a
        /// sibling test's fork could inherit (ETXTBSY when it runs).
        fn fake_gh(body: &str) -> io::Result<(Dir, PathBuf)> {
            let dir = Dir::new()?;
            let path = dir.path().join("gh");
            let mut child = Command::new("/bin/sh")
                .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
                .arg(&path)
                .stdin(Stdio::piped())
                .spawn()?;
            let written = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("no stdin"))?
                .write_all(body.as_bytes());
            let status = child.wait()?;
            written?;
            if !status.success() {
                return Err(io::Error::other(format!("writing the fake gh: {status}")));
            }
            Ok((dir, path))
        }

        fn env_with(dir: &Path) -> EnvMap {
            EnvMap::from([
                ("PATH".to_string(), "/usr/bin:/bin".to_string()),
                ("HOME".to_string(), dir.display().to_string()),
            ])
        }

        #[test]
        fn reads_the_version_a_gh_prints() -> io::Result<()> {
            let (dir, gh) = fake_gh("#!/bin/sh\nprintf 'gh version 2.104.1 (2026-07-01)\\n'\n")?;
            assert_eq!(
                probe(&gh, &env_with(dir.path())),
                release("2.104.1", 2, 104, 1, false)
            );
            Ok(())
        }

        #[test]
        fn passes_no_token_and_turns_update_notices_off() -> io::Result<()> {
            let (dir, gh) = fake_gh(
                "#!/bin/sh\n\
                 [ \"$1\" = --version ] || exit 3\n\
                 [ -z \"$GH_TOKEN$GITHUB_TOKEN$GH_ENTERPRISE_TOKEN$GITHUB_ENTERPRISE_TOKEN\" ] || exit 4\n\
                 [ \"$GH_NO_UPDATE_NOTIFIER\" = 1 ] || exit 5\n\
                 [ \"$GH_NO_EXTENSION_UPDATE_NOTIFIER\" = 1 ] || exit 6\n\
                 [ \"$KEPT\" = yes ] || exit 7\n\
                 read -r line && exit 8\n\
                 printf 'gh version 2.103.3\\n'\n",
            )?;
            let mut env = env_with(dir.path());
            for key in TOKEN_VARS {
                env.insert((*key).to_string(), "audit-dummy-token".to_string());
            }
            env.insert("KEPT".to_string(), "yes".to_string());
            assert_eq!(probe(&gh, &env), release("2.103.3", 2, 103, 3, false));
            Ok(())
        }

        #[test]
        fn a_gh_that_fails_reads_as_unknown() -> io::Result<()> {
            let (dir, gh) = fake_gh("#!/bin/sh\nprintf 'gh version 2.103.3\\n'\nexit 1\n")?;
            assert!(matches!(
                probe(&gh, &env_with(dir.path())),
                Reading::Unknown(_)
            ));
            Ok(())
        }

        // A hanging or flooding gh is killed: Linux only, per the coordinator, so no macOS
        // process is ever made to misbehave here.
        #[cfg(target_os = "linux")]
        #[test]
        fn a_gh_that_hangs_is_killed_at_the_timeout() -> io::Result<()> {
            let (dir, gh) = fake_gh("#!/bin/sh\nexec sleep 60\n")?;
            let started = Instant::now();
            let reading = probe_with(
                &gh,
                &env_with(dir.path()),
                Duration::from_secs(1),
                OUTPUT_CAP,
            );
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{:?}",
                started.elapsed()
            );
            assert!(
                matches!(&reading, Reading::Unknown(reason) if reason.contains("didn't finish")),
                "{reading:?}"
            );
            Ok(())
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn a_gh_that_prints_megabytes_is_stopped_at_the_cap() -> io::Result<()> {
            let (dir, gh) = fake_gh("#!/bin/sh\nexec yes 'gh version 2.103.3'\n")?;
            let started = Instant::now();
            let reading = probe_with(
                &gh,
                &env_with(dir.path()),
                Duration::from_secs(30),
                OUTPUT_CAP,
            );
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{:?}",
                started.elapsed()
            );
            assert!(
                matches!(&reading, Reading::Unknown(reason) if reason.contains("more than")),
                "{reading:?}"
            );
            Ok(())
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn a_gh_whose_child_keeps_its_output_open_is_not_waited_on_forever() -> io::Result<()> {
            let (dir, gh) =
                fake_gh("#!/bin/sh\nprintf 'gh version 2.103.3\\n'\n(sleep 60) &\nexit 0\n")?;
            let started = Instant::now();
            let reading = probe_with(
                &gh,
                &env_with(dir.path()),
                Duration::from_secs(1),
                OUTPUT_CAP,
            );
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{:?}",
                started.elapsed()
            );
            assert!(matches!(reading, Reading::Unknown(_)), "{reading:?}");
            Ok(())
        }
    }
}
