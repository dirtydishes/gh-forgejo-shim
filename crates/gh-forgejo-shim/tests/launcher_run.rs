//! The installed wrapper's launch path end to end, as a subprocess with a fake gh: the run
//! directory, the live gateway, exit status, signals, and gh's environment passed through byte
//! for byte. Subprocesses only: the launcher edits its own environment, which is sound only in
//! a single-threaded process, never in this test harness.
#![cfg(unix)]

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const SHIM: &str = env!("CARGO_BIN_EXE_gh-forgejo-shim");

/// One test at a time: a test that writes its fake gh while another forks can see ETXTBSY on
/// Linux (jko.42).
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Fixture {
    dir: PathBuf,
    home: PathBuf,
    /// The run directory's parent, as TMPDIR.
    parent: PathBuf,
    report: PathBuf,
}

impl Fixture {
    /// A configured forge, `forge.test`, whose upstream is `api_url`, with a token.
    fn new(api_url: &str) -> Self {
        let mut bytes = [0u8; 8];
        fs::File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut bytes))
            .expect("random");
        let name: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        // Short, so the socket path fits sun_path on macOS too.
        let dir = PathBuf::from("/tmp").join(format!("lr-{name}"));
        let home = dir.join("home");
        let parent = dir.join("p");
        let config = home.join(".config/gh-forgejo-shim");
        fs::create_dir_all(&config).expect("config dir");
        fs::create_dir_all(&parent).expect("parent");
        fs::write(
            config.join("config.toml"),
            format!(
                "hosts = [\"forge.test\"]\n[host.\"forge.test\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"{api_url}\"\n"
            ),
        )
        .expect("config");
        Self {
            report: dir.join("report"),
            dir,
            home,
            parent,
        }
    }

    fn fake_gh(&self, body: &str) -> PathBuf {
        let path = self.dir.join("fake-gh");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("fake gh");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    /// The wrapper's environment, as `NAME=value` entries.
    fn env(&self, gh: &Path) -> Vec<String> {
        vec![
            format!("HOME={}", self.home.display()),
            "PATH=/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin".to_string(),
            format!("TMPDIR={}", self.parent.display()),
            "FJ_SHIM_AUTH_STORE=file".to_string(),
            format!("FJ_SHIM_REAL_GH={}", gh.display()),
            "HOST_TOKEN=host-secret".to_string(),
            format!("REPORT={}", self.report.display()),
        ]
    }

    fn command(&self, gh: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(SHIM);
        command
            .arg("gh")
            .args(args)
            .current_dir(&self.dir)
            .env_clear()
            .stdin(Stdio::null());
        for entry in self.env(gh) {
            let (name, value) = entry.split_once('=').expect("entry");
            command.env(name, value);
        }
        command
    }

    fn launch(&self, gh: &Path) -> Output {
        self.command(gh, &["pr", "view", "1", "-R", "forge.test/o/r"])
            .output()
            .expect("run wrapper")
    }

    fn report(&self) -> String {
        fs::read_to_string(&self.report).unwrap_or_default()
    }

    fn parent_is_empty(&self) -> bool {
        fs::read_dir(&self.parent).expect("parent").next().is_none()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn wait_bounded(child: &mut Child, what: &str) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("wait") {
            return status;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .status();
            let _ = child.wait();
            panic!("{what}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn exit_code_passes_through_and_the_run_directory_goes() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let gh = fixture.fake_gh(
        r#"ls -A "$GH_CONFIG_DIR" > "$REPORT"
stat -c %a "$GH_CONFIG_DIR" 2>/dev/null >> "$REPORT" || stat -f %Lp "$GH_CONFIG_DIR" >> "$REPORT"
exit 7"#,
    );
    let output = fixture.launch(&gh);
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let report = fixture.report();
    let seen: Vec<&str> = report.lines().collect();
    assert_eq!(
        seen,
        [
            "config.yml",
            "credentials",
            "gh",
            "hosts.yml",
            "lock",
            "s",
            "700"
        ],
        "the files gh saw"
    );
    assert!(fixture.parent_is_empty(), "run directory removed");
}

#[test]
fn a_signal_death_passes_through() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let gh = fixture.fake_gh("kill -TERM $$");
    let output = fixture.launch(&gh);
    assert_eq!(output.status.signal(), Some(15), "{output:?}");
    assert!(fixture.parent_is_empty());
}

#[test]
fn the_live_gateway_refuses_other_hosts_and_wrong_tokens() {
    let _serial = serial();
    // `localhost`, not an address, so the upstream request resolves a name on a runtime thread.
    let fixture = Fixture::new("http://localhost:9");
    let gh = fixture.fake_gh(
        r#"token=$(sed -n 's/^ *oauth_token: //p' "$GH_CONFIG_DIR/hosts.yml" | head -n 1)
ask() { curl -s -o /dev/null -w '%{http_code}\n' --unix-socket "$GH_CONFIG_DIR/s" -H "Authorization: token $1" "http://$2/api/v3/user"; }
ask "$token" github.com > "$REPORT"
ask wrong forge.test >> "$REPORT"
ask "$token" forge.test >> "$REPORT""#,
    );
    let output = fixture.launch(&gh);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    let codes: Vec<&str> = report.lines().collect();
    assert_eq!(codes[..2], ["421", "401"], "foreign host, then wrong token");
    assert!(
        !["421", "401"].contains(&codes[2]),
        "the per-call token is accepted and the gateway goes upstream: {codes:?}"
    );
}

#[test]
fn a_missing_gh_exits_127() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let output = fixture.launch(&fixture.dir.join("absent"));
    assert_eq!(output.status.code(), Some(127), "{output:?}");
    assert!(fixture.parent_is_empty());
}

#[test]
fn a_configured_gh_that_is_not_executable_exits_126() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let file = fixture.home.join("not-executable-gh");
    fs::write(&file, "#!/bin/sh\nexit 0\n").expect("gh");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).expect("chmod");
    // A plain path, the same file through `~/`, and a directory (Sol r2#1 on #82).
    for configured in [
        file.clone(),
        PathBuf::from("~/not-executable-gh"),
        fixture.dir.clone(),
    ] {
        let output = fixture.launch(&configured);
        assert_eq!(
            output.status.code(),
            Some(126),
            "{configured:?}: {output:?}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("is not executable"),
            "{output:?}"
        );
    }
}

/// Sends `signal` to a launched wrapper in its own process group, so no terminal owns it; the
/// fake gh reports which signal reached it and exits 3.
fn forwarded(signal: &str) -> (Option<i32>, String) {
    let fixture = Fixture::new("http://127.0.0.1:9");
    let ready = fixture.dir.join("ready");
    let gh = fixture.fake_gh(&format!(
        r#"for name in TERM HUP INT; do trap "echo $name > \"\$REPORT\"; exit 3" $name; done
: > '{}'
while :; do sleep 0.05; done"#,
        ready.display()
    ));
    let mut child = fixture
        .command(&gh, &["pr", "view", "1", "-R", "forge.test/o/r"])
        .process_group(0)
        .spawn()
        .expect("spawn wrapper");
    let started = Instant::now();
    while !ready.exists() {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("the wrapper exited before gh was ready: {status}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "gh never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    Command::new("/bin/kill")
        .args([&format!("-{signal}"), &child.id().to_string()])
        .status()
        .expect("kill");
    let status = wait_bounded(&mut child, &format!("SIG{signal} never reached gh"));
    (status.code(), fixture.report().trim().to_string())
}

#[test]
fn sigterm_and_sighup_reach_gh_and_its_exit_code_comes_back() {
    let _serial = serial();
    for signal in ["TERM", "HUP"] {
        assert_eq!(forwarded(signal), (Some(3), signal.to_string()), "{signal}");
    }
}

#[test]
fn sigint_without_a_terminal_reaches_gh() {
    let _serial = serial();
    // Agents and `timeout -s INT` signal the wrapper's pid directly, with no terminal (spec §6).
    assert_eq!(forwarded("INT"), (Some(3), "INT".to_string()));
}

/// Starts the wrapper through `execve` with a hand-built environment, which Rust's `Command`
/// can't produce: entries without `=`, an empty entry, `=value`, a repeated name.
fn run_with_raw_env(fixture: &Fixture, gh: &Path, args: &[&str], extra: &[&str]) -> Output {
    const EXECVE: &str = r#"
import ctypes, sys
path = sys.argv[1].encode()
split = sys.argv.index("--")
args = [path] + [a.encode() for a in sys.argv[2:split]]
env = [e.encode("utf-8", "surrogateescape") for e in sys.argv[split + 1:]]
libc = ctypes.CDLL(None)
libc.execve(path, (ctypes.c_char_p * (len(args) + 1))(*args, None),
            (ctypes.c_char_p * (len(env) + 1))(*env, None))
sys.exit(99)
"#;
    let mut command = Command::new("python3");
    command
        .args(["-I", "-c", EXECVE, SHIM, "gh"])
        .args(args)
        .arg("--")
        .args(extra)
        .args(fixture.env(gh))
        .current_dir(&fixture.dir)
        .stdin(Stdio::null());
    command.output().expect("python3 execve")
}

/// A fake gh that reads its own raw `environ`, with no shell in between (a shell rebuilds the
/// environment before its children see it), optionally queries the gateway first, and writes
/// each entry to `$REPORT` on its own line.
fn raw_env_gh(fixture: &Fixture, query_gateway: bool) -> PathBuf {
    let path = fixture.dir.join("fake-gh");
    let query = if query_gateway {
        r#"subprocess.run(["curl", "-s", "-o", "/dev/null", "--unix-socket",
                os.environ["GH_CONFIG_DIR"] + "/s", "http://forge.test/api/v3/user"])"#
    } else {
        ""
    };
    fs::write(
        &path,
        format!(
            r#"#!/usr/bin/env python3
import ctypes, os, subprocess, sys
{query}
libc = ctypes.CDLL(None)
if sys.platform == "darwin":
    libc._NSGetEnviron.restype = ctypes.POINTER(ctypes.POINTER(ctypes.c_char_p))
    environ = libc._NSGetEnviron().contents
else:
    environ = ctypes.POINTER(ctypes.c_char_p).in_dll(libc, "environ")
entries = []
index = 0
while environ[index] is not None:
    entries.append(environ[index])
    index += 1
with open(os.environ["REPORT"], "wb") as report:
    report.write(b"\n".join(entries) + b"\n")
"#
        ),
    )
    .expect("fake gh");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// The odd entries, between markers that fix their order.
const ODD: [&str; 7] = [
    "MARK_A=1",
    "BARE_NAME",
    "",
    "=value",
    "DUP=first",
    "DUP=second",
    "MARK_B=2",
];

fn assert_odd_entries_in_order(report: &str) {
    let lines: Vec<&str> = report.split('\n').collect();
    let positions: Vec<Option<usize>> = ODD
        .iter()
        .map(|entry| lines.iter().position(|line| line == entry))
        .collect();
    assert!(
        positions.iter().all(Option::is_some),
        "an odd entry didn't reach gh: {positions:?} in {report:?}"
    );
    let found: Vec<usize> = positions.into_iter().flatten().collect();
    assert!(
        found.windows(2).all(|pair| pair[0] < pair[1]),
        "order changed: {found:?} in {report:?}"
    );
}

#[test]
fn launched_gh_gets_the_wrappers_environment_byte_for_byte() {
    let _serial = serial();
    let fixture = Fixture::new("http://localhost:9");
    // Query the live gateway first, so its runtime threads run, then dump the raw environment.
    let gh = raw_env_gh(&fixture, true);
    let output = run_with_raw_env(
        &fixture,
        &gh,
        &["pr", "view", "1", "-R", "forge.test/o/r"],
        &[&ODD[..], &["GH_TOKEN=github-secret"]].concat(),
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    assert_odd_entries_in_order(&report);
    let has = |prefix: &str| report.lines().any(|line| line.starts_with(prefix));
    assert!(has("GH_CONFIG_DIR=") && has("GH_PATH=") && has("GIT_CONFIG_PARAMETERS="));
    assert!(
        !has("GH_TOKEN="),
        "a token variable never reaches a launched gh"
    );
    assert!(
        has("FJ_SHIM_SAVED=") && has("FJ_SHIM_LAUNCHER="),
        "bookkeeping for nested calls"
    );
}

#[test]
fn delegated_gh_gets_the_wrappers_environment_byte_for_byte() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let gh = raw_env_gh(&fixture, false);
    // A GitHub repository: the route is delegate, and the wrapper execs real gh.
    let output = run_with_raw_env(
        &fixture,
        &gh,
        &["pr", "view", "1", "-R", "github.com/o/r"],
        &ODD,
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    assert_odd_entries_in_order(&report);
    assert!(
        report.lines().any(|line| line == "FJ_SHIM_DEPTH=1"),
        "every wrapper invocation raises the depth: {report:?}"
    );
}

#[test]
fn a_nested_call_gets_the_users_environment_back() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let gh = fixture.fake_gh(
        r#"printf '%s\n' "GH_TOKEN=${GH_TOKEN-unset}" "GIT_CONFIG_PARAMETERS=${GIT_CONFIG_PARAMETERS-unset}" \
  "FJ_SHIM_DEPTH=${FJ_SHIM_DEPTH-unset}" "FJ_SHIM_CREDENTIALS=${FJ_SHIM_CREDENTIALS-unset}" \
  "FJ_SHIM_SAVED=${FJ_SHIM_SAVED-unset}" "FJ_SHIM_LAUNCHER=${FJ_SHIM_LAUNCHER-unset}" > "$REPORT""#,
    );
    // What a launched call leaves for a nested one: its edits and bookkeeping, and the
    // variable the run directory's script sets. The route here delegates.
    let output = fixture
        .command(&gh, &["pr", "view", "1", "-R", "github.com/o/r"])
        .env("FJ_SHIM_SAVED", "GH_TOKEN,GIT_CONFIG_PARAMETERS")
        .env("FJ_SHIM_ORIG_GH_TOKEN", "usertok")
        .env("FJ_SHIM_ORIG_GIT_CONFIG_PARAMETERS", "'a.b=c'")
        .env("FJ_SHIM_SET_GIT_CONFIG_PARAMETERS", "'credential.helper='")
        .env("GIT_CONFIG_PARAMETERS", "'credential.helper='")
        .env("FJ_SHIM_DEPTH", "3")
        .env("FJ_SHIM_LAUNCHER", u32::MAX.to_string())
        .env("FJ_SHIM_CREDENTIALS", "/nonexistent/credentials")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        fixture.report(),
        "GH_TOKEN=usertok\nGIT_CONFIG_PARAMETERS='a.b=c'\nFJ_SHIM_DEPTH=4\n\
         FJ_SHIM_CREDENTIALS=unset\nFJ_SHIM_SAVED=unset\nFJ_SHIM_LAUNCHER=unset\n"
    );
}

#[test]
fn a_credential_request_is_answered_before_the_guards() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let credentials = fixture.dir.join("credentials");
    fs::write(&credentials, "forge.test real-token\n").expect("credentials");
    // Depth at the cap, and this test as the wrapper's parent and "launcher": both guards would
    // stop any other call.
    let mut child = fixture
        .command(
            &fixture.dir.join("absent"),
            &["auth", "git-credential", "get"],
        )
        .env("FJ_SHIM_CREDENTIALS", &credentials)
        .env("FJ_SHIM_DEPTH", "8")
        .env("FJ_SHIM_LAUNCHER", std::process::id().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wrapper");
    use std::io::Write as _;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"protocol=https\nhost=forge.test\n\n")
        .expect("write");
    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("password=real-token\n"),
        "{output:?}"
    );
}

#[test]
fn other_git_credential_forms_in_a_launch_never_reach_plain_gh() {
    // Inside a launched call only the launcher's helper answers git (§3). A form its exact match
    // doesn't take is refused, not delegated: plain gh would answer from the user's own logins.
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let credentials = fixture.dir.join("credentials");
    fs::write(&credentials, "forge.test real-token\n").expect("credentials");
    let user_gh = fixture.home.join(".config/gh");
    fs::create_dir_all(&user_gh).expect("gh config");
    fs::write(
        user_gh.join("config.yml"),
        // gh accepts what the shim's YAML reader doesn't: an unquoted Boolean name, and a repeated
        // name (gh takes the first). Unreadable, so routing sees no alias at all (Sol r3#2).
        "aliases:\n    gc: auth git-credential\n    true: auth git-credential\n    gc: pr view\n",
    )
    .expect("alias");
    let gh = fixture.fake_gh(r#"echo ran > "$REPORT"; echo password=user-token"#);
    // Set but empty counts too: the marker's presence makes it a launched call's helper context.
    for (args, marker) in [
        (
            &["auth", "git-credential", "--", "get"][..],
            credentials.as_os_str(),
        ),
        (
            &["auth", "git-credential", "--help=false", "get"][..],
            credentials.as_os_str(),
        ),
        (&["gc", "get"][..], credentials.as_os_str()),
        (&["true", "get"][..], credentials.as_os_str()),
        // cobra adds `help` after gh registers aliases, so an alias can take the name (Sol r4#2).
        (&["help", "get"][..], credentials.as_os_str()),
        // An extension, or any word that isn't one of gh's commands.
        (&["gfjs-extension", "get"][..], credentials.as_os_str()),
        (
            &["--repo", "o/r", "auth", "git-credential", "get"][..],
            credentials.as_os_str(),
        ),
        (
            &["auth", "git-credential", "--", "get"][..],
            std::ffi::OsStr::new(""),
        ),
    ] {
        let mut child = fixture
            .command(&gh, args)
            .env("FJ_SHIM_CREDENTIALS", marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn wrapper");
        use std::io::Write as _;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(b"protocol=https\nhost=gist.github.com\n\n")
            .expect("write");
        let output = child.wait_with_output().expect("wait");
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("password="),
            "{args:?}: {output:?}"
        );
        assert_eq!(fixture.report(), "", "{args:?}: plain gh ran");
    }
    // A nested call with one of gh's own commands still runs.
    let output = fixture
        .command(&gh, &["pr", "view", "1", "-R", "github.com/o/r"])
        .env("FJ_SHIM_CREDENTIALS", &credentials)
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fixture.report(), "ran\n");
}

#[test]
fn a_repeated_depth_entry_still_counts_toward_the_cap() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let gh = raw_env_gh(&fixture, false);
    // Two entries: routing reads the last, `set_var` alone would change only the first, and the
    // depth would never climb (Sol r1#1 on #87).
    let output = run_with_raw_env(
        &fixture,
        &gh,
        &["pr", "view", "1", "-R", "github.com/o/r"],
        &["FJ_SHIM_DEPTH=7", "FJ_SHIM_DEPTH=7"],
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    let depths: Vec<&str> = report
        .lines()
        .filter(|line| line.starts_with("FJ_SHIM_DEPTH="))
        .collect();
    assert_eq!(depths, ["FJ_SHIM_DEPTH=8"], "{report}");

    // One more level trips guard 3.
    let output = run_with_raw_env(
        &fixture,
        &gh,
        &["pr", "view", "1", "-R", "github.com/o/r"],
        &["FJ_SHIM_DEPTH=0", "FJ_SHIM_DEPTH=8"],
    );
    assert_eq!(output.status.code(), Some(127), "{output:?}");
}

#[test]
fn a_host_less_auth_token_gets_the_real_token_and_the_forge_as_gh_host() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    // A forge on a custom port: GH_HOST is its portless hostname (decision 4, §2).
    let config = fixture.home.join(".config/gh-forgejo-shim/config.toml");
    fs::write(
        &config,
        "hosts = [\"forge.test:3443\"]\n[host.\"forge.test:3443\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"http://127.0.0.1:9\"\n",
    )
    .expect("config");
    let gh = fixture.fake_gh(
        r#"{ echo "GH_HOST=${GH_HOST-unset}"; cat "$GH_CONFIG_DIR/hosts.yml"; } > "$REPORT""#,
    );
    let output = fixture
        .command(&gh, &["auth", "token"])
        .env("GH_REPO", "forge.test:3443/o/r")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    assert!(report.starts_with("GH_HOST=forge.test\n"), "{report}");
    assert!(report.contains("oauth_token: host-secret"), "{report}");
}

#[test]
fn auth_token_for_a_named_user_finds_the_real_token_under_that_login() {
    let _serial = serial();
    let api = stub_forgejo(&[("host-secret", "login-a")]);
    let fixture = Fixture::new(&api);
    fs::write(
        fixture.home.join(".config/gh-forgejo-shim/config.toml"),
        format!("hosts = [\"forge.test\"]\n[host.\"forge.test\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"{api}\"\n"),
    )
    .expect("config");
    let gh = fixture.fake_gh(r#"cat "$GH_CONFIG_DIR/hosts.yml" > "$REPORT""#);
    let output = fixture
        .command(&gh, &["auth", "token", "--user", "login-a"])
        .env("GH_REPO", "forge.test/o/r")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let hosts: serde_yaml::Value = serde_yaml::from_str(&fixture.report()).expect("hosts.yml");
    // gh's `auth token --user` reads only `users.<login>.oauth_token` (§3).
    assert_eq!(
        hosts["forge.test"]["users"]["login-a"]["oauth_token"].as_str(),
        Some("host-secret"),
        "{hosts:?}"
    );
    assert_eq!(hosts["forge.test"]["user"].as_str(), Some("login-a"));
}

impl Fixture {
    /// The shim's allowlist, each entry with the token from `HOST_TOKEN`.
    fn allowlist(&self, entries: &[&str]) {
        let mut text = format!(
            "hosts = [{}]\n",
            entries
                .iter()
                .map(|entry| format!("\"{entry}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for entry in entries {
            text.push_str(&format!(
                "[host.\"{entry}\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"http://127.0.0.1:9\"\n"
            ));
        }
        fs::write(self.home.join(".config/gh-forgejo-shim/config.toml"), text).expect("config");
    }

    /// The user's own gh `hosts.yml`.
    fn user_hosts(&self, text: &str) {
        let dir = self.home.join(".config/gh");
        fs::create_dir_all(&dir).expect("gh config dir");
        fs::write(dir.join("hosts.yml"), text).expect("hosts.yml");
    }
}

/// A fake gh that reports what a step-aside or a launch changed.
fn reporting_gh(fixture: &Fixture) -> PathBuf {
    fixture.fake_gh(
        r#"for name in GH_CONFIG_DIR GH_HOST GH_TOKEN GH_ENTERPRISE_TOKEN FJ_SHIM_ORIG_GH_TOKEN FJ_SHIM_ORIG_GH_ENTERPRISE_TOKEN FJ_SHIM_LAUNCHER FJ_SHIM_SAVED; do
  eval "printf '%s=%s\n' $name \"\${$name-unset}\""
done > "$REPORT""#,
    )
}

fn reported(report: &str, name: &str) -> String {
    report
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
        .unwrap_or("missing")
        .to_string()
}

#[test]
fn a_forge_the_users_gh_is_logged_in_to_steps_aside() {
    let _serial = serial();
    for login in [
        "forge.test:\n    oauth_token: user-login\n",
        "forge.test:\n    user: kell\n",
    ] {
        let fixture = Fixture::new("http://127.0.0.1:9");
        fixture.user_hosts(login);
        let gh = reporting_gh(&fixture);
        let output = fixture.launch(&gh);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let report = fixture.report();
        assert_eq!(
            reported(&report, "GH_CONFIG_DIR"),
            "unset",
            "{login}: {report}"
        );
        assert_eq!(reported(&report, "FJ_SHIM_LAUNCHER"), "unset", "{login}");
        // The forge becomes gh's default host (#231), the one change, recorded for nested calls.
        assert_eq!(reported(&report, "GH_HOST"), "forge.test", "{login}");
        assert_eq!(reported(&report, "FJ_SHIM_SAVED"), "GH_HOST", "{login}");
        assert!(fixture.parent_is_empty(), "no run directory");
    }
}

/// In a checkout whose remote is the forge, with gh logged in to github.com as well, gh's default
/// host would be github.com: a host-less `OWNER/REPO` and the commands that only use the default
/// host went there while the trace named the forge (#231). Stepping aside makes the forge gh's
/// default host. `step_aside_hosts.rs` checks the requests real gh then sends.
#[test]
fn stepping_aside_makes_the_forge_gh_s_default_host() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    fixture.user_hosts(
        "forge.test:\n    oauth_token: forge-login\ngithub.com:\n    oauth_token: github-login\n",
    );
    for args in [
        &["init", "-q"][..],
        &["remote", "add", "origin", "https://forge.test/o/r.git"][..],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&fixture.dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &fixture.home)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }
    let gh = fixture.fake_gh(
        r#"for name in GH_HOST FJ_SHIM_ORIG_GH_HOST FJ_SHIM_SET_GH_HOST FJ_SHIM_SAVED GH_CONFIG_DIR; do
  eval "printf '%s=%s\n' $name \"\${$name-unset}\""
done > "$REPORT""#,
    );
    let commands: [&[&str]; 5] = [
        &["repo", "delete", "o/target", "--yes"],
        &["repo", "clone", "o/target"],
        &["search", "prs", "foo"],
        &["gist", "list"],
        // A launched form, which steps aside on the launch path.
        &["pr", "view", "1"],
    ];
    for args in commands {
        let output = fixture.command(&gh, args).output().expect("run wrapper");
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        let report = fixture.report();
        assert_eq!(
            reported(&report, "GH_CONFIG_DIR"),
            "unset",
            "{args:?}: stepped aside"
        );
        assert_eq!(
            reported(&report, "GH_HOST"),
            "forge.test",
            "{args:?}: {report}"
        );
        assert_eq!(reported(&report, "FJ_SHIM_SAVED"), "GH_HOST", "{args:?}");
        assert_eq!(
            reported(&report, "FJ_SHIM_SET_GH_HOST"),
            "forge.test",
            "{args:?}"
        );
        assert_eq!(
            reported(&report, "FJ_SHIM_ORIG_GH_HOST"),
            "unset",
            "{args:?}"
        );

        // The user's own GH_HOST naming the forge stays, unrecorded.
        let output = fixture
            .command(&gh, args)
            .env("GH_HOST", "forge.test")
            .output()
            .expect("run wrapper");
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        let report = fixture.report();
        assert_eq!(
            reported(&report, "GH_HOST"),
            "forge.test",
            "{args:?}: {report}"
        );
        assert_eq!(reported(&report, "FJ_SHIM_SAVED"), "unset", "{args:?}");
    }
    // A host-less `-R` or `GH_REPO` means gh's default host, github.com: refused, never run.
    for (args, gh_repo) in [
        (&["repo", "delete", "-R", "o/target", "--yes"][..], None),
        (&["repo", "delete", "--yes"][..], Some("o/target")),
    ] {
        let mut command = fixture.command(&gh, args);
        if let Some(gh_repo) = gh_repo {
            command.env("GH_REPO", gh_repo);
        }
        let _ = fs::remove_file(fixture.dir.join("report"));
        let output = command.output().expect("run wrapper");
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert_eq!(fixture.report(), "", "{args:?}: gh never ran");
    }
    // A user GH_HOST naming the forge with its port gives way to the portless name gh looks the
    // login up under, and a nested call gets the user's value back.
    let output = fixture
        .command(&gh, &["pr", "view", "1", "-R", "forge.test/o/r"])
        .env("GH_HOST", "forge.test:3443")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    assert_eq!(reported(&report, "GH_HOST"), "forge.test", "{report}");
    assert_eq!(reported(&report, "FJ_SHIM_ORIG_GH_HOST"), "forge.test:3443");
}

/// ChatGPT's PR card and pane send these (command words and flags from its gh calls): GraphQL
/// documents through `gh api graphql --hostname <host>`, plus `api user`, `auth status --active`,
/// `pr list` and `pr view`.
const CHATGPT_QUERY: &str = "query=query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){number state mergeable}}}";

#[test]
fn chatgpt_s_calls_step_aside_when_logged_in_and_api_graphql_launches_otherwise() {
    let _serial = serial();
    let graphql: &[&str] = &[
        "api",
        "graphql",
        "--hostname",
        "forge.test",
        "-f",
        CHATGPT_QUERY,
        "-f",
        "owner=o",
        "-f",
        "name=r",
        "-F",
        "number=1",
    ];
    // Server mode: the user's gh is logged in to the forge, so every form runs plain gh with the
    // user's login, the built-in path's forms too (they used to answer with the shim's token, or
    // refuse `api graphql`).
    for args in [
        graphql,
        &["api", "user", "--hostname", "forge.test"][..],
        &["auth", "status", "--active", "--hostname", "forge.test"][..],
        &[
            "pr",
            "list",
            "--author",
            "@me",
            "--head",
            "h",
            "--json",
            "number",
            "--repo",
            "forge.test/o/r",
            "--state",
            "open",
        ][..],
        &[
            "pr",
            "view",
            "1",
            "--json",
            "number",
            "-R",
            "forge.test/o/r",
        ][..],
        &["pr", "diff", "1", "-R", "forge.test/o/r"][..],
        &["repo", "view", "forge.test/o/r"][..],
    ] {
        let fixture = Fixture::new("http://127.0.0.1:9");
        fixture.user_hosts("forge.test:\n    user: kell\n");
        let gh = reporting_gh(&fixture);
        let output = fixture.command(&gh, args).output().expect("run wrapper");
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        let report = fixture.report();
        assert_eq!(
            reported(&report, "GH_CONFIG_DIR"),
            "unset",
            "{args:?}: {report}"
        );
        assert_eq!(reported(&report, "FJ_SHIM_LAUNCHER"), "unset", "{args:?}");
        assert!(fixture.parent_is_empty(), "{args:?}: no run directory");
    }
    // Client-only: no login there, so ChatGPT's query launches through the in-process gateway.
    let fixture = Fixture::new("http://127.0.0.1:9");
    let gh = reporting_gh(&fixture);
    let output = fixture.command(&gh, graphql).output().expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    assert!(
        reported(&report, "GH_CONFIG_DIR").starts_with(&fixture.parent.display().to_string()),
        "launched: {report}"
    );
    // A mutation keeps the built-in path, which refuses `gh api` for the forge.
    let fixture = Fixture::new("http://127.0.0.1:9");
    let gh = reporting_gh(&fixture);
    let mut mutation = graphql.to_vec();
    mutation[5] =
        "query=mutation{addComment(input:{subjectId:\"x\",body:\"b\"}){clientMutationId}}";
    let output = fixture
        .command(&gh, &mutation)
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fixture.report(), "", "gh never ran");
}

#[test]
fn stepping_aside_keeps_decision_4_and_drops_token_variables() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    fixture.allowlist(&["forge.test:3443"]);
    fixture.user_hosts(
        "forge.test:\n    oauth_token: user-login\ngithub.com:\n    oauth_token: gh-login\n",
    );
    let gh = reporting_gh(&fixture);
    let output = fixture
        .command(&gh, &["api", "user"])
        .env("GH_REPO", "forge.test:3443/o/r")
        .env("GH_ENTERPRISE_TOKEN", "ghe_corp")
        .env("GH_TOKEN", "ghp_github")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    assert_eq!(reported(&report, "GH_CONFIG_DIR"), "unset", "{report}");
    // gh would send GH_TOKEN to a forge named github.localhost or *.ghe.com (§2).
    assert_eq!(reported(&report, "GH_TOKEN"), "unset", "{report}");
    assert_eq!(reported(&report, "FJ_SHIM_ORIG_GH_TOKEN"), "unset");
    assert_eq!(
        reported(&report, "GH_HOST"),
        "forge.test",
        "portless: {report}"
    );
    assert_eq!(
        reported(&report, "GH_ENTERPRISE_TOKEN"),
        "unset",
        "{report}"
    );
    assert_eq!(
        reported(&report, "FJ_SHIM_ORIG_GH_ENTERPRISE_TOKEN"),
        "unset",
        "not recorded"
    );
    assert_eq!(reported(&report, "FJ_SHIM_LAUNCHER"), "unset");
    assert!(reported(&report, "FJ_SHIM_SAVED").contains("GH_HOST"));

    // The user's own GH_HOST naming the forge stays, but their token variables still go: gh has
    // the user's stored login there, and a call naming another host would carry them there.
    let output = fixture
        .command(&gh, &["api", "user"])
        .env("GH_HOST", "forge.test")
        .env("GH_ENTERPRISE_TOKEN", "ghe_corp")
        .env("GH_TOKEN", "ghp_github")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    assert_eq!(reported(&report, "GH_HOST"), "forge.test");
    assert_eq!(reported(&report, "GH_TOKEN"), "unset", "{report}");
    assert_eq!(
        reported(&report, "GH_ENTERPRISE_TOKEN"),
        "unset",
        "{report}"
    );
    assert_eq!(
        reported(&report, "FJ_SHIM_ORIG_GH_ENTERPRISE_TOKEN"),
        "unset",
        "not recorded"
    );

    // Other forms drop them too, launched or not: gh would otherwise send another host's
    // enterprise token to the forge ahead of the user's login there (Sol r1#2). In a checkout on
    // the ported entry, as a call spelling the port doesn't step aside (parallax r2#3).
    for args in [
        &["init", "-q"][..],
        &["remote", "add", "origin", "https://forge.test:3443/o/r.git"][..],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&fixture.dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &fixture.home)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }
    for args in [
        &["pr", "diff", "1"][..],
        &["pr", "view", "1", "--json", "number"][..],
    ] {
        let output = fixture
            .command(&gh, args)
            .env("GH_ENTERPRISE_TOKEN", "ghe_corp")
            .output()
            .expect("run wrapper");
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        let report = fixture.report();
        // Portless, as gh looks up the login under that key (#231).
        assert_eq!(
            reported(&report, "GH_HOST"),
            "forge.test",
            "{args:?}: {report}"
        );
        assert_eq!(
            reported(&report, "GH_ENTERPRISE_TOKEN"),
            "unset",
            "{args:?}: {report}"
        );
        assert_eq!(
            reported(&report, "FJ_SHIM_ORIG_GH_ENTERPRISE_TOKEN"),
            "unset",
            "{args:?}: not recorded"
        );
    }
}

#[test]
fn a_nested_call_never_gets_the_token_variables_back() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    fixture.user_hosts("forge.test:\n    user: kell\n");
    // Plain gh runs git, whose credential helper calls the wrapper again (Sol r4#1).
    let gh = fixture.fake_gh(&format!(
        r#"if [ "$1" = auth ]; then
  for name in GH_TOKEN GH_ENTERPRISE_TOKEN FJ_SHIM_SAVED; do
    eval "printf '%s=%s\n' $name \"\${{$name-unset}}\""
  done > "$REPORT"
else
  "{SHIM}" gh auth git-credential get < /dev/null
fi"#
    ));
    for gh_host in [None, Some("forge.test")] {
        let mut command = fixture.command(&gh, &["pr", "diff", "1", "-R", "forge.test/o/r"]);
        command
            .env("GH_ENTERPRISE_TOKEN", "ghe_corp")
            .env("GH_TOKEN", "ghp_github");
        if let Some(gh_host) = gh_host {
            command.env("GH_HOST", gh_host);
        }
        let output = command.output().expect("run wrapper");
        assert_eq!(output.status.code(), Some(0), "{gh_host:?}: {output:?}");
        let report = fixture.report();
        assert_eq!(
            reported(&report, "GH_ENTERPRISE_TOKEN"),
            "unset",
            "{gh_host:?}: {report}"
        );
        assert_eq!(reported(&report, "GH_TOKEN"), "unset", "{gh_host:?}");
    }
}

#[test]
fn a_call_naming_another_host_keeps_the_built_in_path() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    fixture.allowlist(&["forge-a.test", "forge-b.test"]);
    fixture.user_hosts("forge-a.test:\n    user: kell\n");
    let gh = reporting_gh(&fixture);
    // Routing picks forge-a from GH_REPO, but gh would clone from forge-b, where the user has no
    // login, or reach another host GH_HOST names (Sol r1#1).
    for (args, gh_host) in [
        (&["repo", "clone", "forge-b.test/o/r"][..], None),
        (&["repo", "clone", "https://forge-b.test/o/r"][..], None),
        (&["repo", "clone", "git@forge-b.test:o/r"][..], None),
        // With the user's GH_HOST naming forge-a, gh would keep their token variables (Sol r2#1).
        (
            &["repo", "clone", "other.example:o/r"][..],
            Some("forge-a.test"),
        ),
        (&["repo", "clone", "--", "other.example:o/r"][..], None),
        // Any characters (Sol r3#1).
        (&["repo", "clone", "forge_b:o/r"][..], Some("forge-a.test")),
        (
            &["repo", "clone", "git@other_host.example:o/r"][..],
            Some("forge-a.test"),
        ),
        (&["repo", "clone", "other.example/o/r"][..], None),
        (&["pr", "diff", "1"][..], Some("other.example")),
        // `--hostname`'s value names a host on its own (parallax r1#1, #231).
        (
            &[
                "attestation",
                "download",
                "artifact.bin",
                "--owner",
                "o",
                "--hostname",
                "github.com",
            ][..],
            None,
        ),
        (
            &[
                "attestation",
                "download",
                "artifact.bin",
                "--owner",
                "o",
                "--hostname=github.com",
            ][..],
            None,
        ),
    ] {
        let mut command = fixture.command(&gh, args);
        command.env("GH_REPO", "forge-a.test/o/r");
        if let Some(gh_host) = gh_host {
            command.env("GH_HOST", gh_host);
        }
        let output = command.output().expect("run wrapper");
        assert_ne!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert_eq!(fixture.report(), "", "{args:?}: gh never ran");
    }
    // Naming the forge itself still steps aside.
    let output = fixture
        .command(&gh, &["pr", "diff", "1", "-R", "forge-a.test/o/r"])
        .env("GH_HOST", "forge-a.test")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(reported(&fixture.report(), "GH_CONFIG_DIR"), "unset");
}

/// `gh pr create` and `gh repo create --push` push with git, which follows the remote's push URL
/// and git's URL rewrites, not `GH_HOST` (#231). A `pr create` push that could leave the forge is
/// refused before gh runs (parallax r1#3, r2#1); a `repo create` that pushes always is, however
/// it's spelled (r2#2, r3#1, r3#2, r4#1, r5#2), and one that can't push steps aside (r2#4, r3#4).
#[test]
fn a_push_git_would_send_off_the_forge_is_refused() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    fixture.user_hosts(
        "forge.test:\n    oauth_token: forge-login\n    git_protocol: https\n\
         github.com:\n    oauth_token: github-login\n",
    );
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&fixture.dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &fixture.home)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["remote", "add", "origin", "https://forge.test/o/r.git"]);
    let gh = reporting_gh(&fixture);
    let refused = |args: &[&str], why: &str| {
        let _ = fs::remove_file(fixture.dir.join("report"));
        let output = fixture.command(&gh, args).output().expect("run wrapper");
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(why),
            "{args:?}: {output:?}"
        );
        assert_eq!(fixture.report(), "", "{args:?}: gh never ran");
    };
    let steps_aside = |args: &[&str]| {
        let output = fixture.command(&gh, args).output().expect("run wrapper");
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert_eq!(
            reported(&fixture.report(), "GH_HOST"),
            "forge.test",
            "{args:?}"
        );
    };

    // A `repo create` that pushes, in any spelling, whatever git's configuration says.
    for args in [
        &[
            "repo",
            "create",
            "acme/new",
            "--private",
            "--source",
            ".",
            "--push",
        ][..],
        &[
            "repo",
            "create",
            "acme/new",
            "--private",
            "-s=../source",
            "--push",
        ][..],
        &[
            "repo",
            "new",
            "acme/new",
            "--private",
            "--source",
            ".",
            "--push",
        ][..],
        &[
            "repo",
            "--source",
            ".",
            "create",
            "acme/new",
            "--private",
            "--push",
        ][..],
        &["repo", "create"][..],
        // cobra drops `--`, so gh prompts (r4#1).
        &["repo", "create", "--"][..],
        &["repo", "new", "--"][..],
        // A flag's value isn't a command name, wherever the flag sits (r5#2).
        &[
            "repo",
            "--source",
            "view",
            "create",
            "acme/new",
            "--private",
            "--push",
        ][..],
        &[
            "--source",
            ".",
            "repo",
            "create",
            "acme/new",
            "--private",
            "--push",
        ][..],
    ] {
        refused(args, "create the repository without --push");
    }
    // One that can't push steps aside, even when a push would leave the forge.
    git(&[
        "config",
        "remote.origin.pushurl",
        "https://github.com/o/r.git",
    ]);
    steps_aside(&["repo", "create", "acme/new", "--private"]);
    steps_aside(&[
        "repo",
        "create",
        "--private",
        "--source",
        ".",
        "--push=false",
    ]);
    // `new` here is an argument, not `pr create`'s alias: nothing pushes (r4#4).
    steps_aside(&["pr", "view", "new"]);

    // `pr create` pushes the checkout's remote: refused when its push URL leaves the forge.
    for args in [
        &["pr", "create", "--title", "t", "--body", "b"][..],
        &["pr", "new", "--title", "t", "--body", "b"][..],
    ] {
        refused(args, "remote origin pushes to github.com");
    }
    git(&["config", "--unset", "remote.origin.pushurl"]);
    // A rewrite for one owner's repositories still counts.
    git(&[
        "config",
        "url.https://github.com/acme/.pushInsteadOf",
        "https://forge.test/acme/",
    ]);
    refused(
        &["pr", "create", "--title", "t", "--body", "b"],
        "git rewrites URLs on forge.test to github.com",
    );
    git(&["config", "--remove-section", "url.https://github.com/acme/"]);

    // Pushes that stay on the forge step aside as before.
    git(&["config", "remote.origin.pushurl", "git@forge.test:o/r.git"]);
    steps_aside(&["pr", "create", "--title", "t", "--body", "b"]);
}

/// A forge checkout over https, with a fake `ssh` first on PATH that resolves `git@forge.test` to
/// github.com and any other host to itself, and a fake gh that fails `gh config` and otherwise
/// reports GH_HOST. `pr create`'s push check runs before gh, so a refusal leaves no report.
struct PushFixture {
    fixture: Fixture,
    gh: PathBuf,
    ssh_path: String,
}

impl PushFixture {
    fn new() -> Self {
        let fixture = Fixture::new("http://127.0.0.1:9");
        let bin = fixture.dir.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let ssh = bin.join("ssh");
        fs::write(
            &ssh,
            "#!/bin/sh\ncase \"$3\" in git@forge.test) echo 'hostname github.com' ;; \
             *) echo \"hostname $3\" ;; esac\n",
        )
        .expect("fake ssh");
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).expect("chmod");
        let gh = fixture.fake_gh(
            r#"if [ "$1" = config ]; then exit 1; fi
printf 'GH_HOST=%s\n' "${GH_HOST-unset}" > "$REPORT""#,
        );
        let push = Self {
            ssh_path: format!("{}:/usr/bin:/bin", bin.display()),
            fixture,
            gh,
        };
        push.git(&["init", "-q"]);
        push.git(&["remote", "add", "origin", "https://forge.test/o/r.git"]);
        push
    }

    fn git(&self, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(&self.fixture.dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &self.fixture.home)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    fn gh_files(&self, hosts: &str, config: Option<&str>) {
        self.fixture.user_hosts(hosts);
        let path = self.fixture.home.join(".config/gh/config.yml");
        match config {
            Some(config) => fs::write(&path, config).expect("config.yml"),
            None => {
                let _ = fs::remove_file(&path);
            }
        }
    }

    fn pr_create(&self, path: &str, extra: &[(&str, &str)]) -> Output {
        let _ = fs::remove_file(self.fixture.dir.join("report"));
        let mut command = self
            .fixture
            .command(&self.gh, &["pr", "create", "--title", "t", "--body", "b"]);
        command.env("PATH", path);
        for (name, value) in extra {
            command.env(name, value);
        }
        command.output().expect("run wrapper")
    }

    fn refused(&self, output: &Output, why: &str) {
        assert_eq!(output.status.code(), Some(1), "{why}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(why),
            "{why}: {output:?}"
        );
        assert_eq!(self.fixture.report(), "", "{why}: gh never ran");
    }

    fn ran(&self, output: &Output, case: &str) {
        assert_eq!(output.status.code(), Some(0), "{case}: {output:?}");
        assert!(
            self.fixture.report().starts_with("GH_HOST="),
            "{case}: gh ran"
        );
    }
}

const FORGE_LOGIN: &str = "forge.test:\n    oauth_token: forge-login\n";

/// `gh pr create` may add a fork remote with gh's `git_protocol` for the forge, read from gh's
/// files as gh reads them: the forge's entry, then the top-level value, then `https`. When that
/// isn't `https`, SSH's destination for `git@<forge>`, the remote gh writes, counts: a
/// `Match user git` rule can send it elsewhere while the bare host resolves to the forge. A
/// legacy `hosts` key in config.yml hides hosts.yml for plain gh, so it counts too.
#[test]
fn a_fork_remote_s_ssh_destination_counts_unless_gh_uses_https() {
    let _serial = serial();
    let push = PushFixture::new();
    for (hosts, config, ssh) in [
        (format!("{FORGE_LOGIN}    git_protocol: ssh\n"), None, true),
        (
            format!("{FORGE_LOGIN}    git_protocol: https\n"),
            None,
            false,
        ),
        (
            format!("{FORGE_LOGIN}    git_protocol: https\n"),
            Some("hosts:\n    forge.test:\n        git_protocol: ssh\n"),
            true,
        ),
        (FORGE_LOGIN.to_string(), Some("git_protocol: ssh\n"), true),
    ] {
        push.gh_files(&hosts, config);
        let output = push.pr_create(&push.ssh_path, &[]);
        if ssh {
            push.refused(&output, "ssh resolves git@forge.test to github.com");
        } else {
            push.ran(&output, &hosts);
        }
    }
    // A push URL that's already SSH resolves with its own user, whatever the protocol.
    push.gh_files(&format!("{FORGE_LOGIN}    git_protocol: https\n"), None);
    push.git(&["config", "remote.origin.pushurl", "git@forge.test:o/r.git"]);
    push.refused(
        &push.pr_create(&push.ssh_path, &[]),
        "remote origin pushes to github.com",
    );
}

/// With no entry of its own for the forge, gh takes the top-level `git_protocol`, then `https`.
/// A ported key doesn't count: gh looks the forge up by its portless name. Over https, an SSH
/// alias for the forge doesn't matter.
#[test]
fn a_forge_without_its_own_entry_uses_gh_s_top_level_protocol() {
    let _serial = serial();
    let push = PushFixture::new();
    for (hosts, config) in [
        (
            "github.com:\n    oauth_token: github-login\n",
            Some("git_protocol: https\n"),
        ),
        ("forge.test:3443:\n    oauth_token: forge-login\n", None),
    ] {
        push.gh_files(hosts, config);
        push.ran(&push.pr_create(&push.ssh_path, &[]), hosts);
    }
}

#[test]
fn only_a_login_under_the_portless_key_steps_aside() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    fixture.allowlist(&["forge.test:3443"]);
    let gh = reporting_gh(&fixture);
    let pr_view = ["pr", "view", "1", "-R", "forge.test:3443/o/r"];

    fixture.user_hosts("forge.test:\n    oauth_token: user-login\n");
    // A checkout on the ported entry: gh drops the port from remotes and goes to the portless
    // host on port 443, and the trace says so (parallax r1#2, #231).
    for args in [
        &["init", "-q"][..],
        &["remote", "add", "origin", "https://forge.test:3443/o/r.git"][..],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&fixture.dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &fixture.home)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }
    let trace = fixture.dir.join("trace.jsonl");
    let output = fixture
        .command(&gh, &["pr", "view", "1"])
        .env("FJ_SHIM_TRACE", &trace)
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        reported(&fixture.report(), "GH_CONFIG_DIR"),
        "unset",
        "stepped aside"
    );
    let record = fs::read_to_string(&trace).expect("trace");
    let record: serde_json::Value =
        serde_json::from_str(record.lines().last().expect("a record")).expect("json");
    assert_eq!(record["host"], "forge.test", "{record}");
    // A call spelling the port: gh would keep it for a full name, so it doesn't step aside
    // (parallax r2#3).
    let _ = fs::remove_file(fixture.dir.join("report"));
    let output = fixture
        .command(&gh, &pr_view)
        .output()
        .expect("run wrapper");
    assert_ne!(
        reported(&fixture.report(), "GH_CONFIG_DIR"),
        "unset",
        "didn't step aside: {output:?}"
    );

    fixture.user_hosts(
        "forge.test:3443:\n    oauth_token: user-login\nother.test:\n    git_protocol: ssh\n",
    );
    let output = fixture
        .command(&gh, &pr_view)
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        reported(&fixture.report(), "GH_CONFIG_DIR").contains("/gfj-"),
        "launched: {}",
        fixture.report()
    );
}

#[test]
fn a_users_gh_hosts_route_with_no_usable_login_is_refused() {
    let _serial = serial();
    for (allowlist, hosts, key) in [
        (
            &["forge.test", "other.test"][..],
            "forge.test:\n    git_protocol: ssh\n",
            "forge.test",
        ),
        (
            &["forge.test", "other.test"][..],
            "Forge.Test:\n    git_protocol: ssh\n",
            "Forge.Test",
        ),
        (
            &["forge.test:3443", "other.test"][..],
            "forge.test:3443:\n    oauth_token: user-login\n",
            "forge.test:3443",
        ),
    ] {
        let fixture = Fixture::new("http://127.0.0.1:9");
        fixture.allowlist(allowlist);
        fixture.user_hosts(hosts);
        let before = fs::read(fixture.home.join(".config/gh/hosts.yml")).expect("hosts.yml");
        let gh = reporting_gh(&fixture);
        let output = fixture
            .command(&gh, &["pr", "view", "1", "-R", "other/target"])
            .output()
            .expect("run wrapper");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{key}: {output:?}");
        assert!(
            stderr.contains(&format!("makes {key} gh's default host"))
                && stderr.contains(&format!("delete the {key} entry"))
                && stderr.contains(".config/gh/hosts.yml"),
            "{key}: {stderr}"
        );
        assert_eq!(fixture.report(), "", "gh never ran");
        assert_eq!(
            fs::read(fixture.home.join(".config/gh/hosts.yml")).expect("hosts.yml"),
            before,
            "the user's config is left alone"
        );
    }
}

/// A stub Forgejo whose `/api/v1/user` answers with a login chosen by the request's token, for
/// as long as the test runs.
fn stub_forgejo(logins: &'static [(&'static str, &'static str)]) -> String {
    use std::io::{BufRead, BufReader, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut token = String::new();
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                if let Some(value) = line
                    .to_ascii_lowercase()
                    .strip_prefix("authorization: token ")
                {
                    token = value.trim().to_string();
                }
                line.clear();
            }
            let login = logins
                .iter()
                .find(|(known, _)| *known == token)
                .map_or("nobody", |(_, login)| login);
            let body = format!(r#"{{"id":1,"login":"{login}","avatar_url":""}}"#);
            let _ = write!(
                reader.get_mut(),
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    format!("http://{address}")
}

#[test]
fn host_less_auth_status_serves_every_eligible_host_with_its_login() {
    let _serial = serial();
    let api = stub_forgejo(&[("host-secret", "login-a"), ("other-secret", "login-b")]);
    let fixture = Fixture::new(&api);
    let host = |entry: &str, token_env: &str| {
        format!("[host.\"{entry}\"]\ntoken_env = \"{token_env}\"\napi_url = \"{api}\"\n")
    };
    fs::write(
        fixture.home.join(".config/gh-forgejo-shim/config.toml"),
        format!(
            "hosts = [\"forge.test\", \"other.test\", \"a.test:3443\", \"a.test:8443\", \"logged.test\"]\n{}{}{}{}{}",
            host("forge.test", "HOST_TOKEN"),
            host("other.test", "OTHER_TOKEN"),
            host("a.test:3443", "HOST_TOKEN"),
            host("a.test:8443", "HOST_TOKEN"),
            host("logged.test", "HOST_TOKEN"),
        ),
    )
    .expect("config");
    fixture.user_hosts("logged.test:\n    oauth_token: user-login\n");
    let gh = fixture.fake_gh(r#"cat "$GH_CONFIG_DIR/hosts.yml" > "$REPORT""#);
    let output = fixture
        .command(&gh, &["auth", "status"])
        .env("GH_REPO", "forge.test/o/r")
        .env("OTHER_TOKEN", "other-secret")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let hosts: serde_yaml::Mapping = serde_yaml::from_str(&fixture.report()).expect("hosts.yml");
    let field = |key: &str, name: &str| {
        hosts
            .get(key)
            .and_then(|entry| entry.get(name))
            .and_then(serde_yaml::Value::as_str)
            .map(str::to_string)
    };
    let per_call = field("forge.test", "oauth_token").expect("forge.test token");
    assert_eq!(per_call.len(), 64, "a per-call token, not the real one");
    assert_eq!(field("forge.test", "user").as_deref(), Some("login-a"));
    assert_eq!(field("other.test", "oauth_token"), Some(per_call.clone()));
    assert_eq!(field("other.test", "user").as_deref(), Some("login-b"));
    for unserved in ["a.test", "a.test:3443", "a.test:8443", "logged.test"] {
        assert_eq!(
            field(unserved, "oauth_token").as_deref(),
            Some("gfj-not-served"),
            "{unserved}"
        );
        assert_eq!(field(unserved, "user"), None, "{unserved}");
    }
}

#[test]
fn host_less_auth_status_serves_the_rest_when_the_routed_host_cant_be_served() {
    let _serial = serial();
    let api = stub_forgejo(&[("other-secret", "login-b")]);
    let fixture = Fixture::new(&api);
    fs::write(
        fixture.home.join(".config/gh-forgejo-shim/config.toml"),
        format!(
            "hosts = [\"forge.test\", \"other.test\"]\n[host.\"forge.test\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"{api}\"\nca_file = \"/nonexistent/ca.pem\"\n[host.\"other.test\"]\ntoken_env = \"OTHER_TOKEN\"\napi_url = \"{api}\"\n"
        ),
    )
    .expect("config");
    let gh = fixture.fake_gh(r#"cat "$GH_CONFIG_DIR/hosts.yml" > "$REPORT""#);
    let output = fixture
        .command(&gh, &["auth", "status"])
        .env("GH_REPO", "forge.test/o/r")
        .env("OTHER_TOKEN", "other-secret")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let hosts: serde_yaml::Mapping = serde_yaml::from_str(&fixture.report()).expect("hosts.yml");
    let field = |key: &str, name: &str| {
        hosts
            .get(key)
            .and_then(|entry| entry.get(name))
            .and_then(serde_yaml::Value::as_str)
            .map(str::to_string)
    };
    assert_eq!(field("other.test", "user").as_deref(), Some("login-b"));
    assert_eq!(
        field("other.test", "oauth_token").map(|token| token.len()),
        Some(64)
    );
    assert_eq!(
        field("forge.test", "oauth_token").as_deref(),
        Some("gfj-not-served"),
        "a CA file the built-in client would refuse"
    );
}

#[test]
fn show_token_puts_the_real_token_in_hosts_yml_and_the_gateway_accepts_it() {
    let _serial = serial();
    let api = stub_forgejo(&[("host-secret", "login-a")]);
    let fixture = Fixture::new(&api);
    let gh = fixture.fake_gh(
        r#"token=$(sed -n 's/^ *oauth_token: //p' "$GH_CONFIG_DIR/hosts.yml" | head -n 1)
code=$(curl -s -o /dev/null -w '%{http_code}' --unix-socket "$GH_CONFIG_DIR/s" -H "Authorization: token $token" http://forge.test/api/v3/user)
printf '%s %s\n' "$token" "$code" > "$REPORT""#,
    );
    for args in [
        &["auth", "status", "-t"][..],
        &["auth", "status", "--show-token=false"][..],
    ] {
        let output = fixture
            .command(&gh, args)
            .env("GH_REPO", "forge.test/o/r")
            .output()
            .expect("run wrapper");
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let report = fixture.report();
        let (token, code) = report.trim().split_once(' ').expect("report");
        assert_eq!(
            code, "200",
            "{args:?}: the gateway accepts gh's token: {report}"
        );
        assert_eq!(
            token == "host-secret",
            args[2] == "-t",
            "{args:?}: {report}"
        );
    }
}

#[test]
fn the_browser_helper_rewrites_served_pr_pages_and_never_calls_itself() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let recorder = fixture.fake_gh(r#"printf '%s\n' "$@" > "$REPORT""#);
    let open = |url: &str, original: &str, browser: Option<&Path>| {
        let mut command = Command::new(SHIM);
        command
            .args(["__gfj-browser", "forge.test", "port.test:3443", url])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("REPORT", &fixture.report)
            .env("FJ_SHIM_ORIG_GH_BROWSER", original);
        if let Some(browser) = browser {
            command.env("BROWSER", browser);
        }
        let output = command.output().expect("helper");
        assert_eq!(output.status.code(), Some(0), "{url}: {output:?}");
        fixture.report().trim().to_string()
    };
    let recorder_text = recorder.to_string_lossy().into_owned();
    assert_eq!(
        open("https://forge.test/o/r/pull/3", &recorder_text, None),
        "https://forge.test/o/r/pulls/3"
    );
    assert_eq!(
        open(
            "https://port.test:3443/o/r/pull/3/files",
            &recorder_text,
            None
        ),
        "https://port.test:3443/o/r/pulls/3/files"
    );
    // The user's GH_BROWSER names this helper: skip it, use BROWSER; a GitHub URL is unchanged.
    let itself = format!("'{SHIM}' __gfj-browser forge.test");
    assert_eq!(
        open("https://github.com/o/r/pull/3", &itself, Some(&recorder)),
        "https://github.com/o/r/pull/3"
    );
    assert_eq!(
        open("https://forge.test/o/r/pull/4", SHIM, Some(&recorder)),
        "https://forge.test/o/r/pulls/4"
    );
}

#[test]
fn auth_status_steps_aside_or_serves_tokened_hosts_without_a_routed_token() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    let host = |entry: &str, token_env: &str| {
        format!(
            "[host.\"{entry}\"]\ntoken_env = \"{token_env}\"\napi_url = \"http://127.0.0.1:9\"\n"
        )
    };
    fs::write(
        fixture.home.join(".config/gh-forgejo-shim/config.toml"),
        format!(
            "hosts = [\"forge.test\", \"other.test\"]\n{}{}",
            host("forge.test", "NO_SUCH_TOKEN"),
            host("other.test", "HOST_TOKEN"),
        ),
    )
    .expect("config");
    let gh = fixture.fake_gh(
        r#"{ echo "GH_CONFIG_DIR=${GH_CONFIG_DIR-unset}"; [ -n "$GH_CONFIG_DIR" ] && cat "$GH_CONFIG_DIR/hosts.yml"; } > "$REPORT"; exit 0"#,
    );

    // Host-less, in a checkout of the forge without a token: the forge that has one is served.
    let output = fixture
        .command(&gh, &["auth", "status"])
        .env("GH_REPO", "forge.test/o/r")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    let hosts: serde_yaml::Mapping =
        serde_yaml::from_str(report.split_once('\n').expect("report").1).expect("hosts.yml");
    let token = |key: &str| {
        hosts
            .get(key)
            .and_then(|entry| entry.get("oauth_token"))
            .and_then(serde_yaml::Value::as_str)
            .map(str::to_string)
    };
    assert_eq!(
        token("forge.test").as_deref(),
        Some("gfj-not-served"),
        "{report}"
    );
    assert_eq!(
        token("other.test").map(|token| token.len()),
        Some(64),
        "{report}"
    );

    // The user's gh is logged in to the forge: plain gh, even with no shim token for it.
    fixture.user_hosts("forge.test:\n    oauth_token: user-login\n");
    let output = fixture
        .command(&gh, &["auth", "status", "--hostname", "forge.test"])
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        fixture.report().starts_with("GH_CONFIG_DIR=unset"),
        "{}",
        fixture.report()
    );
}

#[test]
fn a_signal_during_the_login_lookup_ends_the_launch_at_once() {
    let _serial = serial();
    // An upstream that accepts and never answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let api = format!("http://{}", listener.local_addr().expect("address"));
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    });
    let fixture = Fixture::new(&api);
    let gh = fixture.fake_gh("exit 0");
    let mut child = fixture
        .command(&gh, &["auth", "status"])
        .env("GH_REPO", "forge.test/o/r")
        .process_group(0)
        .spawn()
        .expect("spawn wrapper");
    std::thread::sleep(Duration::from_millis(700));
    let sent = Instant::now();
    Command::new("/bin/kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill");
    let status = wait_bounded(&mut child, "the lookup held off SIGINT");
    assert!(
        sent.elapsed() < Duration::from_secs(3),
        "{:?}",
        sent.elapsed()
    );
    assert_eq!(status.signal(), Some(2), "{status}");
    assert!(fixture.parent_is_empty(), "run directory removed");
}

/// A Forgejo that knows one user and one draft pull request, `o/r#1`, titled `DRAFT: x`, with
/// its API under `prefix`. Over TLS, it serves `localhost` with a certificate from the test CA.
fn draft_forgejo(prefix: &'static str, tls: bool) -> String {
    use std::io::{BufRead, BufReader, Write as _};
    fn answer(stream: impl std::io::Read + std::io::Write, prefix: &str) {
        let mut reader = BufReader::new(stream);
        let mut request = String::new();
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
            if request.is_empty() {
                request = line.clone();
            }
            line.clear();
        }
        let user = r#"{"id":1,"login":"o","avatar_url":""}"#;
        let body = if request.starts_with(&format!("GET {prefix}/api/v1/user ")) {
            user.to_string()
        } else if request.starts_with(&format!("GET {prefix}/api/v1/repos/o/r/pulls/1 ")) {
            format!(
                r#"{{"number":1,"title":"DRAFT: x","state":"open","merged":false,"draft":true,"html_url":"https://forge.test/o/r/pulls/1","head":{{"label":"topic","ref":"topic","sha":"abc","repo":null}},"base":{{"label":"main","ref":"main","sha":"def","repo":null}},"user":{user},"merged_at":null}}"#
            )
        } else {
            String::new()
        };
        let status = if body.is_empty() {
            "404 Not Found"
        } else {
            "200 OK"
        };
        let _ = write!(
            reader.get_mut(),
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = reader.get_mut().flush();
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let server = tls.then(|| {
        let cert = rustls::pki_types::CertificateDer::from(
            include_bytes!("fixtures/test-server.der").to_vec(),
        );
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            include_bytes!("fixtures/test-server-key.der")
                .to_vec()
                .into(),
        );
        std::sync::Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .expect("server config"),
        )
    });
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            match &server {
                Some(config) => {
                    let Ok(connection) = rustls::ServerConnection::new(config.clone()) else {
                        continue;
                    };
                    let mut tls = rustls::StreamOwned::new(connection, stream);
                    answer(&mut tls, prefix);
                    tls.conn.send_close_notify();
                    let _ = tls.flush();
                }
                None => answer(stream, prefix),
            }
        }
    });
    if tls {
        format!("https://localhost:{port}{prefix}")
    } else {
        format!("http://127.0.0.1:{port}{prefix}")
    }
}

/// A fake gh that asks the launched gateway for `o/r#1` and reports the status, then the body.
const GH_READS_THE_PR: &str = r#"token=$(sed -n 's/^ *oauth_token: //p' "$GH_CONFIG_DIR/hosts.yml" | head -n 1)
curl -s -w '%{http_code}\n' -o "$REPORT.body" --unix-socket "$GH_CONFIG_DIR/s" -H "Authorization: token $token" "http://forge.test/api/v3/repos/o/r/pulls/1" > "$REPORT"
cat "$REPORT.body" >> "$REPORT""#;

/// The gateway's answer to [`GH_READS_THE_PR`]: the status, and the pull request as JSON.
fn gateways_pr(fixture: &Fixture) -> (String, serde_json::Value) {
    let report = fixture.report();
    let (status, body) = report.split_once('\n').unwrap_or((&report, ""));
    (
        status.to_string(),
        serde_json::from_str(body).unwrap_or(serde_json::Value::Null),
    )
}

#[test]
fn a_hosts_wip_prefixes_reach_the_launched_gateway() {
    let _serial = serial();
    let api = draft_forgejo("", false);
    for (prefixes, title) in [("wip_prefixes = [\"DRAFT:\"]\n", "x"), ("", "DRAFT: x")] {
        let fixture = Fixture::new(&api);
        fs::write(
            fixture.home.join(".config/gh-forgejo-shim/config.toml"),
            format!("hosts = [\"forge.test\"]\n[host.\"forge.test\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"{api}\"\n{prefixes}"),
        )
        .expect("config");
        let gh = fixture.fake_gh(GH_READS_THE_PR);
        let output = fixture.launch(&gh);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let (status, pr) = gateways_pr(&fixture);
        assert_eq!(status, "200", "{}", fixture.report());
        assert_eq!(pr["draft"], true, "{pr}");
        assert_eq!(pr["title"], title, "{prefixes:?}: {pr}");
    }
}

#[test]
fn a_private_ca_forge_under_a_path_is_served_with_verification_on() {
    let _serial = serial();
    let api = draft_forgejo("/forgejo", true);
    // The test server's certificate names `localhost`, not `127.0.0.1`.
    let by_address = api.replace("localhost", "127.0.0.1");
    let ca = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-ca.pem");
    let ca_file = format!("ca_file = \"{}\"\n", ca.display());
    for (what, url, settings, env_ca, status) in [
        ("ca_file", &api, ca_file.as_str(), false, "200"),
        ("FJ_SHIM_CA_FILE", &api, "", true, "200"),
        ("no extra root", &api, "", false, "502"),
        (
            "a hostname the certificate doesn't name",
            &by_address,
            ca_file.as_str(),
            false,
            "502",
        ),
    ] {
        let fixture = Fixture::new(url);
        fs::write(
            fixture.home.join(".config/gh-forgejo-shim/config.toml"),
            format!("hosts = [\"forge.test\"]\n[host.\"forge.test\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"{url}\"\nwip_prefixes = [\"DRAFT:\"]\n{settings}"),
        )
        .expect("config");
        let gh = fixture.fake_gh(GH_READS_THE_PR);
        let mut command = fixture.command(&gh, &["pr", "view", "1", "-R", "forge.test/o/r"]);
        if env_ca {
            command.env("FJ_SHIM_CA_FILE", &ca);
        }
        let output = command.output().expect("run wrapper");
        assert_eq!(output.status.code(), Some(0), "{what}: {output:?}");
        let (got, pr) = gateways_pr(&fixture);
        // A failed verification is an upstream failure: the gateway's 502.
        assert_eq!(got, status, "{what}: {}", fixture.report());
        if status == "200" {
            // The API lives under `/forgejo`, and the host's draft prefix is stripped.
            assert_eq!(pr["title"], "x", "{what}: {pr}");
        }
    }
}

#[test]
fn host_less_auth_status_serves_a_private_ca_host_beside_one_it_refuses() {
    let _serial = serial();
    let api = draft_forgejo("/forgejo", true);
    let fixture = Fixture::new(&api);
    // A PEM block that isn't a certificate: reqwest refuses it only when the client is built.
    let bad = fixture.dir.join("bad-ca.pem");
    fs::write(
        &bad,
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    )
    .expect("bad ca");
    let ca = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-ca.pem");
    fs::write(
        fixture.home.join(".config/gh-forgejo-shim/config.toml"),
        format!(
            "hosts = [\"forge.test\", \"other.test\"]\n[host.\"forge.test\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"{api}\"\nca_file = \"{}\"\n[host.\"other.test\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"{api}\"\nca_file = \"{}\"\n",
            bad.display(),
            ca.display()
        ),
    )
    .expect("config");
    let gh = fixture.fake_gh(r#"cat "$GH_CONFIG_DIR/hosts.yml" > "$REPORT""#);
    let output = fixture
        .command(&gh, &["auth", "status"])
        .env("GH_REPO", "forge.test/o/r")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let hosts: serde_yaml::Value = serde_yaml::from_str(&fixture.report()).expect("hosts.yml");
    assert_eq!(
        hosts["forge.test"]["oauth_token"].as_str(),
        Some("gfj-not-served"),
        "{hosts:?}"
    );
    // The login lookup went through the gateway's client, trusting the test CA.
    assert_eq!(hosts["other.test"]["user"].as_str(), Some("o"), "{hosts:?}");
}

#[test]
fn a_launched_web_open_reaches_a_forge_under_a_path() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9/forgejo");
    let browser = fixture.dir.join("browser");
    fs::write(&browser, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$REPORT\"\n").expect("browser");
    fs::set_permissions(&browser, fs::Permissions::from_mode(0o755)).expect("chmod");
    // gh shlex-splits GH_BROWSER and appends the URL; the gateway's link keeps the prefix.
    let gh = fixture.fake_gh(r#"eval "$GH_BROWSER 'https://forge.test/forgejo/o/r/pull/1'""#);
    let output = fixture
        .command(&gh, &["pr", "view", "1", "--web", "-R", "forge.test/o/r"])
        .env("BROWSER", &browser)
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        fixture.report().trim(),
        "https://forge.test/forgejo/o/r/pulls/1"
    );
}
