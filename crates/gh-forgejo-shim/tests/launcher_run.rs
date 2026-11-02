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
        r#"for name in GH_CONFIG_DIR GH_HOST GH_ENTERPRISE_TOKEN FJ_SHIM_ORIG_GH_ENTERPRISE_TOKEN FJ_SHIM_LAUNCHER FJ_SHIM_SAVED; do
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
        assert_eq!(
            reported(&report, "FJ_SHIM_SAVED"),
            "unset",
            "nothing changed"
        );
        assert!(fixture.parent_is_empty(), "no run directory");
    }
}

#[test]
fn stepping_aside_keeps_decision_4_and_drops_enterprise_tokens() {
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
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    assert_eq!(reported(&report, "GH_CONFIG_DIR"), "unset", "{report}");
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
        "ghe_corp"
    );
    assert_eq!(reported(&report, "FJ_SHIM_LAUNCHER"), "unset");
    assert!(reported(&report, "FJ_SHIM_SAVED").contains("GH_HOST"));

    // The user's own GH_HOST naming the forge stays, and so do their enterprise tokens.
    let output = fixture
        .command(&gh, &["api", "user"])
        .env("GH_HOST", "forge.test")
        .env("GH_ENTERPRISE_TOKEN", "ghe_corp")
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = fixture.report();
    assert_eq!(reported(&report, "GH_HOST"), "forge.test");
    assert_eq!(
        reported(&report, "GH_ENTERPRISE_TOKEN"),
        "ghe_corp",
        "{report}"
    );
}

#[test]
fn only_a_login_under_the_portless_key_steps_aside() {
    let _serial = serial();
    let fixture = Fixture::new("http://127.0.0.1:9");
    fixture.allowlist(&["forge.test:3443"]);
    let gh = reporting_gh(&fixture);
    let pr_view = ["pr", "view", "1", "-R", "forge.test:3443/o/r"];

    fixture.user_hosts("forge.test:\n    oauth_token: user-login\n");
    let output = fixture
        .command(&gh, &pr_view)
        .output()
        .expect("run wrapper");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        reported(&fixture.report(), "GH_CONFIG_DIR"),
        "unset",
        "stepped aside"
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
