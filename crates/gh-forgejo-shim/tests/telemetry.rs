//! The gh wrapper's daily usage report (docs/telemetry.md), against a stand-in collector and forge
//! on loopback. A debug build sends only to `GFJ_TELEMETRY_URL`, which every test here names.
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

const SHIM: &str = env!("CARGO_BIN_EXE_gh-forgejo-shim");
const GH_VERSION: &str = "gh version 2.103.3 (2026-05-01)";

/// Every request a stand-in server got, as (path, body).
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// A loopback HTTP server answering each request with `answer(path)`, or never answering when
/// that's `None`.
fn serve(answer: fn(&str) -> Option<(u16, String)>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                let _ = reader.read_line(&mut request);
                let mut length = 0;
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let path = request.split(' ').nth(1).unwrap_or_default().to_string();
                log.lock()
                    .expect("log")
                    .push((path.clone(), String::from_utf8_lossy(&body).into_owned()));
                match answer(&path) {
                    Some((status, reply)) => {
                        let _ = write!(
                            reader.get_mut(),
                            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                            reply.len()
                        );
                    }
                    None => std::thread::sleep(Duration::from_secs(30)),
                }
            });
        }
    });
    (format!("http://{address}"), seen)
}

fn collector(path: &str) -> Option<(u16, String)> {
    (path == "/v1/report").then(|| (204, String::new()))
}

fn silent_collector(_: &str) -> Option<(u16, String)> {
    None
}

/// A Forgejo 15 that answers its version routes and the user lookup.
fn forge(path: &str) -> Option<(u16, String)> {
    Some(match path {
        "/api/forgejo/v1/version" | "/api/v1/version" => {
            (200, r#"{"version":"15.0.11+gitea-1.22.0"}"#.to_string())
        }
        "/api/v1/user" => (200, r#"{"id":1,"login":"me","avatar_url":""}"#.to_string()),
        _ => (404, r#"{"message":"Not Found"}"#.to_string()),
    })
}

/// The same forge, three seconds slow to say its version.
fn slow_forge(path: &str) -> Option<(u16, String)> {
    if path.ends_with("/version") {
        std::thread::sleep(Duration::from_secs(3));
    }
    forge(path)
}

struct Fixture {
    dir: PathBuf,
    home: PathBuf,
    gh: PathBuf,
    collector: String,
    reports: Seen,
}

impl Fixture {
    fn new(answer: fn(&str) -> Option<(u16, String)>) -> Self {
        Self::with_forge(answer, forge)
    }

    fn with_forge(
        answer: fn(&str) -> Option<(u16, String)>,
        forge_answer: fn(&str) -> Option<(u16, String)>,
    ) -> Self {
        let mut bytes = [0u8; 6];
        fs::File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut bytes))
            .expect("random");
        let name: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        // Short, so a launched call's socket path fits sun_path on macOS.
        let dir = PathBuf::from("/tmp").join(format!("tm-{name}"));
        let home = dir.join("home");
        fs::create_dir_all(home.join(".config/gh-forgejo-shim")).expect("config dir");
        fs::create_dir_all(dir.join("p")).expect("run dir parent");
        let (forge, _) = serve(forge_answer);
        fs::write(
            home.join(".config/gh-forgejo-shim/config.toml"),
            format!(
                "hosts = [\"forge.test\"]\n[host.\"forge.test\"]\ntoken_env = \"HOST_TOKEN\"\napi_url = \"{forge}\"\n"
            ),
        )
        .expect("config");
        let gh = dir.join("gh");
        fs::write(
            &gh,
            format!("#!/bin/sh\n[ \"$1\" = --version ] && {{ echo '{GH_VERSION}'; exit 0; }}\neval \"${{GH_BODY:-:}}\"\n"),
        )
        .expect("fake gh");
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).expect("chmod");
        let (collector, reports) = serve(answer);
        Self {
            dir,
            home,
            gh,
            collector,
            reports,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(SHIM);
        command
            .args(args)
            .current_dir(&self.dir)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .env("TMPDIR", self.dir.join("p"))
            .env("FJ_SHIM_AUTH_STORE", "file")
            .env("FJ_SHIM_REAL_GH", &self.gh)
            .env("HOST_TOKEN", "host-secret")
            .env("GFJ_TELEMETRY_URL", format!("{}/v1/report", self.collector))
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str], extra: &[(&str, &str)]) -> Output {
        self.command(args)
            .envs(extra.iter().copied())
            .output()
            .expect("run shim")
    }

    fn state(&self) -> PathBuf {
        self.home.join(".local/state/gh-forgejo-shim/telemetry")
    }

    /// The report bodies the collector got, waiting up to `wait` for `count` of them.
    fn reports(&self, count: usize, wait: Duration) -> Vec<Value> {
        let deadline = Instant::now() + wait;
        loop {
            let got = self.reports.lock().expect("log").len();
            if got >= count || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.reports
            .lock()
            .expect("log")
            .iter()
            .map(|(path, body)| {
                assert_eq!(path, "/v1/report");
                serde_json::from_str(body).expect("a JSON report")
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn this_week() -> String {
    String::from_utf8(
        Command::new("date")
            .args(["-u", "+%G-W%V"])
            .output()
            .expect("date")
            .stdout,
    )
    .expect("utf-8")
    .trim()
    .to_string()
}

#[test]
fn one_report_a_day_holds_these_fields_and_the_gh_call_is_untouched() {
    let fixture = Fixture::new(collector);
    for _ in 0..2 {
        let output = fixture.run(&["gh", "--version"], &[]);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(stdout(&output), format!("{GH_VERSION}\n"));
        assert_eq!(output.stderr, b"", "{output:?}");
    }
    let reports = fixture.reports(1, Duration::from_secs(10));
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = &reports[0];
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let client = report["client"].as_str().expect("a client");
    assert!(
        [
            "codex",
            "chatgpt",
            "claude-code",
            "claude-desktop",
            "cursor",
            "other"
        ]
        .contains(&client),
        "{report}"
    );
    let mut expected = serde_json::json!({
        "schema": 1,
        "version": gh_forgejo_shim::VERSION,
        "os": os,
        "arch": arch,
        "mode": "client",
        "client": client,
        "forge": "forgejo",
        "forge_major": 15,
        "gh": "2.103",
        "first_week": this_week(),
        "unsupported": {},
    });
    assert_eq!(report, &expected);
    // A later call the same day sends nothing more.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(fixture.reports(2, Duration::ZERO).len(), 1);
    // `show` prints exactly what was sent, its client found from show's own callers.
    let shown: Value = serde_json::from_str(&stdout(&fixture.run(&["telemetry", "show"], &[])))
        .expect("show prints JSON");
    expected["client"] = shown["client"].clone();
    assert_eq!(shown, expected);
    let text = serde_json::to_string(report).expect("text");
    for private in ["forge.test", "host-secret", "127.0.0.1", "me"] {
        assert!(!text.contains(&format!("\"{private}\"")), "{text}");
    }
}

#[test]
fn every_off_switch_sends_nothing_and_keeps_nothing() {
    for (name, value) in [
        ("GFJ_TELEMETRY", "0"),
        ("DO_NOT_TRACK", "1"),
        ("CI", "true"),
        ("CI", ""),
        ("config", ""),
    ] {
        let fixture = Fixture::new(collector);
        let extra: Vec<(&str, &str)> = if name == "config" {
            let off = fixture.run(&["telemetry", "off"], &[]);
            assert_eq!(
                stdout(&off),
                "off: turned off with `gfj telemetry off`\n",
                "{off:?}"
            );
            Vec::new()
        } else {
            vec![(name, value)]
        };
        let output = fixture.run(&["gh", "--version"], &extra);
        assert_eq!(stdout(&output), format!("{GH_VERSION}\n"), "{name}");
        let status = stdout(&fixture.run(&["telemetry", "status"], &extra));
        assert!(status.starts_with("off: "), "{name}: {status}");
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(
            fixture.reports(1, Duration::ZERO),
            Vec::<Value>::new(),
            "{name}"
        );
        assert!(!fixture.state().exists(), "{name} kept state");
    }
}

#[test]
fn turning_reports_back_on_says_where_they_go() {
    let fixture = Fixture::new(collector);
    fixture.run(&["telemetry", "off"], &[]);
    let on = stdout(&fixture.run(&["telemetry", "on"], &[]));
    assert_eq!(
        on,
        format!(
            "on: one anonymous usage report a day to {}/v1/report; `gfj telemetry show` prints it\n",
            fixture.collector
        )
    );
}

/// A collector that takes the connection and never answers holds up nothing: the wrapper doesn't
/// wait for its sender.
#[test]
fn a_collector_that_never_answers_never_slows_gh() {
    let fixture = Fixture::new(silent_collector);
    // A fixture's first call can take a second while macOS first looks at its new files.
    fixture.run(&["gh", "--version"], &[("GFJ_TELEMETRY", "0")]);
    let started = Instant::now();
    let output = fixture.run(&["gh", "--version"], &[]);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(stdout(&output), format!("{GH_VERSION}\n"));
    assert_eq!(output.stderr, b"");
    // The sender reached it, and gave up on its own.
    assert_eq!(fixture.reports(1, Duration::from_secs(10)).len(), 1);
}

#[test]
fn the_notice_shows_once_from_setup_commands_and_never_from_gh() {
    let fixture = Fixture::new(collector);
    let gh = fixture.run(&["gh", "--version"], &[]);
    assert!(!stdout(&gh).contains("usage report") && gh.stderr.is_empty());
    let first = stdout(&fixture.run(&["doctor"], &[]));
    assert!(
        first.contains("\ngh-forgejo-shim sends one anonymous usage report a day"),
        "{first}"
    );
    assert!(first.contains("gfj telemetry off") && first.contains("docs/telemetry.md"));
    let second = stdout(&fixture.run(&["doctor"], &[]));
    assert!(!second.contains("usage report"), "{second}");
    // Off, it never shows.
    let quiet = Fixture::new(collector);
    let output = stdout(&quiet.run(&["doctor"], &[("GFJ_TELEMETRY", "0")]));
    assert!(!output.contains("usage report"), "{output}");
}

/// A launched call's gateway counts what it lacks; the next report carries the counts, by GitHub's
/// name only, and only once.
#[test]
fn unsupported_requests_reach_the_next_report() {
    let fixture = Fixture::new(collector);
    // Today's report goes first, so the call's counts wait for the next one.
    fixture.run(&["gh", "--version"], &[]);
    assert_eq!(fixture.reports(1, Duration::from_secs(10)).len(), 1);
    let body = r#"curl -s -o /dev/null --unix-socket "$GH_CONFIG_DIR/s" -H "Authorization: token $(sed -n 's/^ *oauth_token: //p' "$GH_CONFIG_DIR/hosts.yml" | head -n 1)" "http://forge.test/api/v3/repos/acme/secret-repo/releases/1""#;
    let output = fixture.run(
        &["gh", "pr", "view", "1", "-R", "forge.test/acme/secret-repo"],
        &[("GH_BODY", body)],
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let shown: Value =
        serde_json::from_str(&stdout(&fixture.run(&["telemetry", "show"], &[]))).expect("JSON");
    assert_eq!(
        shown["unsupported"],
        serde_json::json!({"rest:repos.releases": 1})
    );
    let send = fixture.run(&["telemetry", "send", "--caller", "1"], &[]);
    assert_eq!((send.stdout.len(), send.stderr.len()), (0, 0), "{send:?}");
    let reports = fixture.reports(2, Duration::from_secs(5));
    assert_eq!(
        reports[1]["unsupported"],
        serde_json::json!({"rest:repos.releases": 1})
    );
    let shown: Value =
        serde_json::from_str(&stdout(&fixture.run(&["telemetry", "show"], &[]))).expect("JSON");
    assert_eq!(shown["unsupported"], serde_json::json!({}), "sent once");
    let state = fs::read_dir(fixture.state())
        .expect("state")
        .map(|entry| fs::read(entry.expect("entry").path()).unwrap_or_default())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .collect::<String>();
    assert!(
        !state.contains("acme") && !state.contains("secret"),
        "{state}"
    );
}

/// Turning reports off, by command or by variable, deletes what was kept for them.
#[test]
fn turning_reports_off_deletes_what_was_kept() {
    let fixture = Fixture::new(collector);
    fixture.run(&["gh", "--version"], &[]);
    fixture.reports(1, Duration::from_secs(10));
    std::thread::sleep(Duration::from_millis(500));
    fs::write(
        fixture.state().join("unsupported"),
        "rest:repos.releases 1\n",
    )
    .expect("counts");
    assert!(fixture.state().join("first-week").exists());
    fixture.run(&["telemetry", "off"], &[]);
    assert!(!fixture.state().exists(), "off keeps nothing");

    let fixture = Fixture::new(collector);
    fixture.run(&["gh", "--version"], &[]);
    fixture.reports(1, Duration::from_secs(10));
    std::thread::sleep(Duration::from_millis(500));
    assert!(fixture.state().exists());
    fixture.run(&["gh", "--version"], &[("DO_NOT_TRACK", "1")]);
    assert!(!fixture.state().exists(), "DO_NOT_TRACK keeps nothing");
}

/// A config the shim can't parse never undoes `gfj telemetry off`.
#[test]
fn a_broken_config_keeps_reports_off() {
    let fixture = Fixture::new(collector);
    fixture.run(&["telemetry", "off"], &[]);
    let config = fixture.home.join(".config/gh-forgejo-shim/config.toml");
    let mut text = fs::read_to_string(&config).expect("config");
    text.push_str("\n[broken\n");
    fs::write(&config, text).expect("config");
    let status = stdout(&fixture.run(&["telemetry", "status"], &[]));
    assert!(status.starts_with("off: "), "{status}");
    fixture.run(&["gh", "--version"], &[]);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(fixture.reports(1, Duration::ZERO), Vec::<Value>::new());
}

/// A `gfj telemetry off` while a report is being built wins: nothing goes and nothing stays.
#[test]
fn turning_reports_off_while_one_is_built_sends_nothing() {
    let fixture = Fixture::with_forge(collector, slow_forge);
    fixture.run(&["gh", "--version"], &[]);
    // The sender now waits on the forge's version answer.
    std::thread::sleep(Duration::from_millis(500));
    fixture.run(&["telemetry", "off"], &[]);
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(fixture.reports(1, Duration::ZERO), Vec::<Value>::new());
    assert!(
        !fixture.state().exists(),
        "{:?}",
        fs::read_dir(fixture.state()).map(|d| d.count())
    );
}

/// No file in the state directory can hold up a gh call: a pipe with no reader where the counts go
/// is passed over.
#[test]
fn a_pipe_in_the_state_directory_never_holds_up_gh() {
    let fixture = Fixture::new(collector);
    fixture.run(&["gh", "--version"], &[]);
    fixture.reports(1, Duration::from_secs(10));
    let pipe = fixture.state().join("unsupported");
    let made = Command::new("mkfifo").arg(&pipe).status().expect("mkfifo");
    assert!(made.success());
    let body = r#"curl -s -o /dev/null --unix-socket "$GH_CONFIG_DIR/s" -H "Authorization: token $(sed -n 's/^ *oauth_token: //p' "$GH_CONFIG_DIR/hosts.yml" | head -n 1)" "http://forge.test/api/v3/repos/acme/secret-repo/releases/1""#;
    let mut child = fixture
        .command(&["gh", "pr", "view", "1", "-R", "forge.test/acme/secret-repo"])
        .env("GH_BODY", body)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run shim");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(
        status.and_then(|status| status.code()),
        Some(0),
        "the call finished"
    );
}

/// An opt-out in another call's environment stops a sender already running: that call deletes
/// today's marker, and the sender checks for it before it sends.
#[test]
fn an_opt_out_elsewhere_stops_a_report_being_built() {
    let fixture = Fixture::with_forge(collector, slow_forge);
    fixture.run(&["gh", "--version"], &[]);
    std::thread::sleep(Duration::from_millis(500));
    fixture.run(&["gh", "--version"], &[("DO_NOT_TRACK", "1")]);
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(fixture.reports(1, Duration::ZERO), Vec::<Value>::new());
    assert!(!fixture.state().exists());
}

/// A config set to `telemetry = false` by hand clears what was kept on the next call, even on a day
/// whose report already went.
#[test]
fn a_config_opt_out_clears_state_the_same_day() {
    let fixture = Fixture::new(collector);
    fixture.run(&["gh", "--version"], &[]);
    fixture.reports(1, Duration::from_secs(10));
    std::thread::sleep(Duration::from_millis(500));
    fs::write(
        fixture.state().join("unsupported"),
        "rest:repos.releases 1\n",
    )
    .expect("counts");
    let config = fixture.home.join(".config/gh-forgejo-shim/config.toml");
    let mut text = fs::read_to_string(&config).expect("config");
    text.insert_str(0, "telemetry = false\n");
    fs::write(&config, text).expect("config");
    fixture.run(&["gh", "--version"], &[]);
    assert!(!fixture.state().exists());
}
