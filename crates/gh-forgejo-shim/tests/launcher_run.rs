//! `launcher::run` end to end with a fake gh: files, socket, exit status and cleanup. Its own
//! test binary, because the launcher installs process-wide signal handlers.
#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fga_core::map::WipPrefixes;
use gh_forgejo_shim::launcher::gateway::GatewayHost;
use gh_forgejo_shim::launcher::run::{run, Files, Launch};

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
