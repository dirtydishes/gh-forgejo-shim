//! `launcher::run` end to end with a fake gh: files, socket, exit status and cleanup, and the
//! installed wrapper forwarding signals. Its own test binary, because the launcher installs
//! process-wide signal handlers.
#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use fga_core::map::WipPrefixes;
use gh_forgejo_shim::launcher::gateway::GatewayHost;
use gh_forgejo_shim::launcher::run::{run, Files, Launch};

/// One test at a time: a test that writes its fake gh while another forks can see ETXTBSY on
/// Linux (jko.42), which `run` reports as a failed spawn.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn scratch() -> PathBuf {
    let mut bytes = [0u8; 8];
    fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .expect("random");
    let name: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    // Short, so the socket path fits sun_path on macOS too.
    let dir = PathBuf::from("/tmp").join(format!("lr-{name}"));
    fs::create_dir(&dir).expect("scratch");
    dir
}

fn fake_gh(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-gh");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("fake gh");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

fn launch(gh: PathBuf, parent: &Path, report: &Path) -> Launch<'static> {
    let report = report.to_path_buf();
    Launch {
        gh,
        args: vec![OsString::from("pr"), OsString::from("view")],
        parent: parent.to_path_buf(),
        sweep_parents: vec![parent.to_path_buf()],
        gateway: vec![GatewayHost {
            entry: "forge.test".to_string(),
            // Nothing listens there: these tests never let a request reach the upstream.
            forgejo_url: "http://127.0.0.1:9/".parse().expect("url"),
            expected: "per-call".to_string(),
            upstream: "real".to_string(),
            answer_portless: true,
            wip_prefixes: WipPrefixes::default(),
        }],
        files: Box::new(|_| Files {
            hosts_yml: "forge.test:\n    oauth_token: per-call\n".to_string(),
            credentials: "forge.test real\n".to_string(),
            gh_script: "#!/bin/sh\n".to_string(),
            config_yml: "version: \"1\"\n".to_string(),
        }),
        env: Box::new(move |run_dir| {
            vec![
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (
                    OsString::from("GH_CONFIG_DIR"),
                    run_dir.as_os_str().to_owned(),
                ),
                (OsString::from("REPORT"), report.as_os_str().to_owned()),
            ]
        }),
    }
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("list")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn exit_code_passes_through_and_the_run_directory_goes() {
    let _serial = serial();
    let dir = scratch();
    let parent = dir.join("p");
    fs::create_dir(&parent).expect("parent");
    let report = dir.join("report");
    let gh = fake_gh(
        &dir,
        r#"ls -A "$GH_CONFIG_DIR" > "$REPORT"; stat -c %a "$GH_CONFIG_DIR" 2>/dev/null >> "$REPORT" || stat -f %Lp "$GH_CONFIG_DIR" >> "$REPORT"; exit 7"#,
    );
    assert_eq!(run(launch(gh, &parent, &report)), 7);
    let seen = fs::read_to_string(&report).expect("report");
    let seen: Vec<&str> = seen.lines().collect();
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
    assert!(
        entries(&parent).is_empty(),
        "run directory removed: {:?}",
        entries(&parent)
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn a_signal_death_becomes_128_plus_the_signal() {
    let _serial = serial();
    let dir = scratch();
    let parent = dir.join("p");
    fs::create_dir(&parent).expect("parent");
    let gh = fake_gh(&dir, "kill -TERM $$");
    assert_eq!(run(launch(gh, &parent, &dir.join("report"))), 128 + 15);
    assert!(entries(&parent).is_empty());
    fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn the_gateway_answers_on_the_socket_and_refuses_other_hosts() {
    let _serial = serial();
    let dir = scratch();
    let parent = dir.join("p");
    fs::create_dir(&parent).expect("parent");
    let report = dir.join("report");
    let gh = fake_gh(
        &dir,
        r#"curl -s -o /dev/null -w '%{http_code}\n' --unix-socket "$GH_CONFIG_DIR/s" -H 'Authorization: token per-call' http://github.com/api/v3/user > "$REPORT"
curl -s -o /dev/null -w '%{http_code}\n' --unix-socket "$GH_CONFIG_DIR/s" -H 'Authorization: token wrong' http://forge.test/api/v3/user >> "$REPORT""#,
    );
    assert_eq!(run(launch(gh, &parent, &report)), 0);
    assert_eq!(
        fs::read_to_string(&report).expect("report"),
        "421\n401\n",
        "a foreign host gets 421; a wrong token gets 401, before any upstream contact"
    );
    fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn a_missing_gh_exits_127_and_cleans_up() {
    let _serial = serial();
    let dir = scratch();
    let parent = dir.join("p");
    fs::create_dir(&parent).expect("parent");
    assert_eq!(
        run(launch(dir.join("absent"), &parent, &dir.join("report"))),
        127
    );
    assert!(entries(&parent).is_empty());
    fs::remove_dir_all(&dir).expect("cleanup");
}

/// Runs the installed wrapper as a launched `gh pr view` against a configured forge, with a fake
/// gh that reports which signal reached it, in its own process group so no terminal owns it.
fn forwarded(signal: &str) -> (Option<i32>, String) {
    let dir = scratch();
    let home = dir.join("home");
    let config = home.join(".config/gh-forgejo-shim");
    fs::create_dir_all(&config).expect("config dir");
    fs::write(
        config.join("config.toml"),
        "hosts = [\"forge.test\"]\n[host.\"forge.test\"]\ntoken_env = \"HOST_TOKEN\"\n",
    )
    .expect("config");
    let report = dir.join("report");
    let ready = dir.join("ready");
    let gh = fake_gh(
        &dir,
        r#"for name in TERM HUP INT; do trap "echo $name > \"\$REPORT\"; exit 3" $name; done
: > "$READY"
while :; do sleep 0.05; done"#,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_gh-forgejo-shim"))
        .args(["gh", "pr", "view", "1", "-R", "forge.test/o/r"])
        .current_dir(&dir)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", "/tmp")
        .env("FJ_SHIM_AUTH_STORE", "file")
        .env("FJ_SHIM_REAL_GH", &gh)
        .env("HOST_TOKEN", "host-secret")
        .env("REPORT", &report)
        .env("READY", &ready)
        .stdin(Stdio::null())
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
    // A launcher that didn't forward would leave gh looping: fail instead of hanging.
    let signalled = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        if signalled.elapsed() > Duration::from_secs(10) {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .status();
            let _ = child.wait();
            panic!("SIG{signal} never reached gh");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let seen = fs::read_to_string(&report).unwrap_or_default();
    fs::remove_dir_all(&dir).expect("cleanup");
    (status.code(), seen.trim().to_string())
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

#[test]
fn a_configured_gh_that_is_not_executable_exits_126() {
    let _serial = serial();
    let dir = scratch();
    let home = dir.join("home");
    let config = home.join(".config/gh-forgejo-shim");
    fs::create_dir_all(&config).expect("config dir");
    fs::write(
        config.join("config.toml"),
        "hosts = [\"forge.test\"]\n[host.\"forge.test\"]\ntoken_env = \"HOST_TOKEN\"\n",
    )
    .expect("config");
    let gh = dir.join("not-executable-gh");
    fs::write(&gh, "#!/bin/sh\nexit 0\n").expect("gh");
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o644)).expect("chmod");
    let output = Command::new(env!("CARGO_BIN_EXE_gh-forgejo-shim"))
        .args(["gh", "pr", "view", "1", "-R", "forge.test/o/r"])
        .current_dir(&dir)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/nonexistent")
        .env("TMPDIR", "/tmp")
        .env("FJ_SHIM_AUTH_STORE", "file")
        .env("FJ_SHIM_REAL_GH", &gh)
        .env("HOST_TOKEN", "host-secret")
        .stdin(Stdio::null())
        .output()
        .expect("run wrapper");
    fs::remove_dir_all(&dir).expect("cleanup");
    assert_eq!(output.status.code(), Some(126), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("is not executable"),
        "{output:?}"
    );
}
