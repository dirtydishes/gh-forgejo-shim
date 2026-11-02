//! Adversarial regression tests. All credentials and tools are disposable fakes.
#![cfg(unix)]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use gh_forgejo_shim::{config, doctor, git_recorder, shim, trace, trace_summary};

type TestResult = Result<(), Box<dyn std::error::Error>>;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> io::Result<Self> {
        let root = loop {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let candidate = std::env::temp_dir()
                .join(format!("gfj-audit-diagnostics-{}-{id}", std::process::id()));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        let fixture = Self {
            home: root.join("home"),
            bin: root.join("tools"),
            root,
        };
        for dir in [
            fixture.home.clone(),
            fixture.bin.clone(),
            fixture.root.join("tmp"),
            fixture.home.join(".config/gh-forgejo-shim"),
            fixture.home.join(".cache"),
            fixture.home.join(".local/share"),
            fixture.home.join(".local/state"),
        ] {
            fs::create_dir_all(dir)?;
        }
        fixture.tool("gh", "#!/bin/sh\nexit 0\n")?;
        fixture.tool("git", "#!/bin/sh\nexit 1\n")?;
        fixture.tool("security", "#!/bin/sh\nexit 98\n")?;
        fixture.tool("launchctl", "#!/bin/sh\nexit 1\n")?;
        Ok(fixture)
    }

    fn tool(&self, name: &str, body: &str) -> io::Result<PathBuf> {
        let path = self.bin.join(name);
        fs::write(&path, body)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Ok(path)
    }

    fn env(&self) -> HashMap<String, String> {
        HashMap::from([
            ("HOME".into(), self.home.display().to_string()),
            ("PATH".into(), self.bin.display().to_string()),
            (
                "XDG_CONFIG_HOME".into(),
                self.home.join(".config").display().to_string(),
            ),
            (
                "XDG_CACHE_HOME".into(),
                self.home.join(".cache").display().to_string(),
            ),
            (
                "XDG_DATA_HOME".into(),
                self.home.join(".local/share").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".into(),
                self.home.join(".local/state").display().to_string(),
            ),
            ("TMPDIR".into(), self.root.join("tmp").display().to_string()),
            ("TMP".into(), self.root.join("tmp").display().to_string()),
            ("TEMP".into(), self.root.join("tmp").display().to_string()),
            ("FJ_SHIM_AUTH_STORE".into(), "file".into()),
            ("FJ_SHIM_HOSTS".into(), "forge.example.test".into()),
            (
                "FJ_SHIM_REAL_GH".into(),
                self.bin.join("gh").display().to_string(),
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
            ("LC_ALL".into(), "C".into()),
        ])
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gfj"));
        command
            .env_clear()
            .envs(self.env())
            .current_dir(&self.root)
            .stdin(Stdio::null());
        command
    }

    fn gh_trace(&self, path: &Path, argv: &[String]) -> io::Result<String> {
        let mut env = self.env();
        env.insert("FJ_SHIM_TRACE".into(), path.display().to_string());
        assert!(trace::append_gh_record(
            &env,
            argv,
            Some(&self.root),
            None,
            1.0,
            0
        ));
        fs::read_to_string(path)
    }

    /// Runs `command` and fails the test if it takes longer than ten seconds.
    #[cfg(target_os = "linux")]
    fn run_bounded(&self, command: &mut Command) -> io::Result<Output> {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let started = std::time::Instant::now();
        while child.try_wait()?.is_none() {
            if started.elapsed() >= std::time::Duration::from_secs(10) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("audit child exceeded 10s and was killed");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.wait_with_output()
    }

    fn git_trace(&self, path: &Path, argv: &[&str], body: bool) -> io::Result<Output> {
        self.command()
            .args(["trace", "git-recorder", "run", "--real-git"])
            .arg(self.bin.join("git"))
            .arg("--trace")
            .arg(path)
            .arg("--")
            .args(argv)
            .env("FJ_SHIM_TRACE_BODY", if body { "1" } else { "0" })
            .output()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
#[ignore = "audit D10: fails until gh-forgejo-shim-jko.27 is fixed"]
fn audit_diag_gh_trace_json_escaped_secret() -> TestResult {
    let f = Fixture::new()?;
    let text = f.gh_trace(
        &f.root.join("gh.jsonl"),
        &[
            "api".into(),
            "user".into(),
            "--field".into(),
            r#"payload={"password":"first\"audit-password-tail"}"#.into(),
        ],
    )?;
    assert!(
        !text.contains("audit-password-tail"),
        "escaped JSON password suffix leaked into gh trace argv"
    );
    Ok(())
}

#[test]
#[ignore = "audit D9: fails until gh-forgejo-shim-jko.26 is fixed"]
fn audit_diag_gh_trace_url_password_at_sign() -> TestResult {
    let f = Fixture::new()?;
    let url = "https://alice:prefix@audit-password-tail@forge.example.test/api/v1/user";
    assert_eq!(
        reqwest::Url::parse(url)?.host_str(),
        Some("forge.example.test")
    );
    let text = f.gh_trace(&f.root.join("gh.jsonl"), &["api".into(), url.into()])?;
    assert!(
        !text.contains("audit-password-tail"),
        "URL password suffix leaked into gh trace argv"
    );
    Ok(())
}

#[test]
#[ignore = "audit D3: fails until gh-forgejo-shim-jko.9 is fixed"]
fn audit_diag_git_trace_json_secret() -> TestResult {
    let f = Fixture::new()?;
    f.tool(
        "git",
        "#!/bin/sh\nprintf '%s\\n' '{\"token\": \"audit-forgejo-token\"}'\n",
    )?;
    let path = f.root.join("git.jsonl");
    let output = f.git_trace(&path, &["log", "-1", "--format=%B"], true)?;
    assert!(output.status.success());
    let text = fs::read_to_string(path)?;
    assert!(
        !text.contains("audit-forgejo-token"),
        "JSON token leaked in git excerpts"
    );
    Ok(())
}

#[test]
#[ignore = "audit D2: fails until gh-forgejo-shim-jko.23 is fixed"]
fn audit_diag_git_trace_query_token() -> TestResult {
    let f = Fixture::new()?;
    f.tool("git", "#!/bin/sh\nexit 0\n")?;
    let path = f.root.join("git.jsonl");
    let output = f.git_trace(
        &path,
        &[
            "fetch",
            "https://forge.example.test/owner/repo.git?mode=x&size=1&access_token=audit-query-token",
        ],
        false,
    )?;
    assert!(output.status.success());
    let text = fs::read_to_string(path)?;
    assert!(
        !text.contains("audit-query-token"),
        "query token leaked in git argv"
    );
    Ok(())
}

#[test]
#[ignore = "audit D4: fails until gh-forgejo-shim-jko.10 is fixed"]
fn audit_diag_git_trace_multiple_authorization_headers() -> TestResult {
    let f = Fixture::new()?;
    f.tool(
        "git",
        "#!/bin/sh\nprintf '%s\\n' 'Authorization: token audit-first' 'aUtHoRiZaTiOn: Bearer audit-second' >&2\nexit 1\n",
    )?;
    let path = f.root.join("git.jsonl");
    let output = f.git_trace(&path, &["fetch"], true)?;
    assert_eq!(output.status.code(), Some(1));
    let text = fs::read_to_string(path)?;
    assert!(
        !text.contains("audit-second"),
        "second Authorization header leaked in git excerpts"
    );
    Ok(())
}

#[test]
#[ignore = "audit D11: fails until gh-forgejo-shim-jko.28 is fixed"]
fn audit_diag_smoke_redacts_stderr() -> TestResult {
    let f = Fixture::new()?;
    f.tool(
        "gh",
        "#!/bin/sh\nprintf '%s\\n' 'Authorization: token audit-smoke-token' >&2\nexit 1\n",
    )?;
    let output = f.command().args(["trace", "smoke"]).output()?;
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout)?;
    assert!(
        !text.contains("audit-smoke-token"),
        "smoke report exposed an Authorization credential"
    );
    Ok(())
}

#[test]
#[ignore = "audit D12: fails until gh-forgejo-shim-jko.29 is fixed"]
fn audit_diag_smoke_utf8_stderr() -> TestResult {
    let f = Fixture::new()?;
    f.tool(
        "gh",
        &format!("#!/bin/sh\nprintf '%s' '{}' >&2\nexit 1\n", "é".repeat(130)),
    )?;
    let output = f.command().args(["trace", "smoke"]).output()?;
    assert_eq!(
        output.status.code(),
        Some(1),
        "smoke should report failed probes, not panic on UTF-8: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
#[ignore = "audit D7: fails until gh-forgejo-shim-jko.25 is fixed"]
fn audit_diag_trace_existing_file_permissions() -> TestResult {
    let f = Fixture::new()?;
    f.tool("git", "#!/bin/sh\nexit 0\n")?;
    let gh_path = f.root.join("gh.jsonl");
    let git_path = f.root.join("git.jsonl");
    for path in [&gh_path, &git_path] {
        fs::write(path, "")?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
    }
    f.gh_trace(&gh_path, &["--version".into()])?;
    assert!(f.git_trace(&git_path, &["status"], false)?.status.success());
    let exposed = [&gh_path, &git_path]
        .map(|path| fs::metadata(path).map(|m| m.permissions().mode() & 0o077))
        .into_iter()
        .collect::<io::Result<Vec<_>>>()?;
    assert_eq!(
        exposed,
        vec![0, 0],
        "gh and git trace writers must restrict pre-existing files to the owner"
    );
    Ok(())
}

#[test]
#[ignore = "audit D6: fails until gh-forgejo-shim-jko.11 is fixed"]
fn audit_diag_trace_concurrent_jsonl() -> TestResult {
    let f = Fixture::new()?;
    let path = f.root.join("concurrent.jsonl");
    let mut env = f.env();
    env.insert("FJ_SHIM_TRACE".into(), path.display().to_string());
    let barrier = Arc::new(Barrier::new(12));
    std::thread::scope(|scope| {
        for worker in 0..12 {
            let barrier = Arc::clone(&barrier);
            let env = &env;
            let root = &f.root;
            scope.spawn(move || {
                let argv = vec![format!("worker-{worker}"), "x".repeat(1024)];
                barrier.wait();
                for _ in 0..20 {
                    assert!(trace::append_gh_record(
                        env,
                        &argv,
                        Some(root),
                        None,
                        1.0,
                        0
                    ));
                }
            });
        }
    });
    let (records, errors) = trace_summary::load_trace(&path)?;
    assert_eq!(
        records.len(),
        240,
        "concurrent appends corrupted JSONL: {} parse errors, first: {:?}",
        errors.len(),
        errors.first()
    );
    Ok(())
}

#[test]
#[ignore = "audit D19: fails until gh-forgejo-shim-jko.30 is fixed"]
fn audit_diag_summary_invalid_utf8_tail() -> TestResult {
    let f = Fixture::new()?;
    let path = f.root.join("partial.jsonl");
    fs::write(
        &path,
        b"{\"kind\":\"gh\",\"exit_code\":1}\n{\"argv\":[\"\xc3",
    )?;
    let result = trace_summary::summarize_trace_file(&path, 1000.0);
    assert!(
        result.is_ok(),
        "an interrupted UTF-8 tail must not hide complete earlier records: {result:?}"
    );
    let summary = result?;
    assert_eq!(summary.total, 1);
    assert_eq!(summary.parse_errors.len(), 1);
    Ok(())
}

#[test]
fn audit_diag_recorder_remove_preserves_unrelated_files() -> TestResult {
    let f = Fixture::new()?;
    let directory = f.root.join("existing-bin");
    fs::create_dir(&directory)?;
    let unrelated = directory.join("user-tool");
    fs::write(&unrelated, "user-owned tool\n")?;
    git_recorder::create_git_recorder(
        &f.root.join("git.jsonl"),
        Some(&f.bin.join("git")),
        None,
        Some(&directory),
        &f.env(),
        Path::new(env!("CARGO_BIN_EXE_gfj")),
    )?;
    let output = f
        .command()
        .args(["trace", "git-recorder", "remove"])
        .arg(directory.join("git"))
        .output()?;
    assert!(output.status.success());
    assert!(
        unrelated.exists(),
        "removing one git recorder recursively deleted the unrelated user-tool"
    );
    Ok(())
}

#[test]
fn audit_diag_recorder_preserves_signal_exit() -> TestResult {
    let f = Fixture::new()?;
    let git = f.tool("git", "#!/bin/sh\nkill -TERM $$\n")?;
    let bare = Command::new(&git)
        .env_clear()
        .envs(f.env())
        .current_dir(&f.root)
        .stdin(Stdio::null())
        .output()?;
    assert_eq!(bare.status.signal(), Some(15));
    let recorded = f.git_trace(&f.root.join("git.jsonl"), &["fetch"], false)?;
    let shell_status = recorded
        .status
        .code()
        .unwrap_or_else(|| 128 + recorded.status.signal().unwrap_or(0));
    assert_eq!(
        shell_status, 143,
        "recorder must preserve SIGTERM's shell exit status instead of returning generic failure"
    );
    let trace = fs::read_to_string(f.root.join("git.jsonl"))?;
    let record: serde_json::Value =
        serde_json::from_str(trace.lines().last().ok_or("no git trace record")?)?;
    assert_eq!(
        record["exit_code"], 143,
        "the recorder must write its trace record before dying of the signal"
    );
    Ok(())
}

#[test]
#[ignore = "audit D13: fails until gh-forgejo-shim-jko.12 is fixed"]
fn audit_diag_doctor_checks_wrapper_target() -> TestResult {
    let f = Fixture::new()?;
    let managed_dir = f.home.join(".local/bin");
    shim::install_shim(
        Some(&managed_dir),
        Some(&f.home),
        &f.root.join("deleted-release/gfj"),
        false,
    )?;
    let mut env = BTreeMap::from_iter(f.env());
    env.insert(
        "PATH".into(),
        std::env::join_paths([&managed_dir, &f.bin])?
            .to_string_lossy()
            .into_owned(),
    );
    env.insert("FJ_SHIM_TOKEN".into(), "audit-dummy-token".into());
    let mut options = doctor::CheckOptions::new(env.clone());
    options.home = Some(f.home.clone());
    options.config = Some(config::load_config_with_env(
        Some(&f.home.join(".config/gh-forgejo-shim/config.toml")),
        &env,
    )?);
    options.cwd = Some(f.root.clone());
    options.fallback_dirs = Some(Vec::new());
    options.check_current_repo = Some(false);
    options.check_gui_path = Some(false);
    options.platform = "linux".into();
    let checks = doctor::run_checks(options)?;
    let broken = Command::new(managed_dir.join("gh"))
        .env_clear()
        .envs(f.env())
        .current_dir(&f.root)
        .arg("--version")
        .output()?;
    assert!(!broken.status.success());
    assert!(
        doctor::has_failures(&checks),
        "doctor reported a broken wrapper as healthy"
    );
    Ok(())
}

#[test]
// Linux only: on macOS the CLI reads the real launchd PATH through /bin/launchctl, even with
// --no-gui-path, and the library call inherits the runner's own environment.
#[cfg(target_os = "linux")]
#[ignore = "audit D14: fails until gh-forgejo-shim-jko.13 is fixed"]
fn audit_diag_uninstall_reverses_default_bootstrap() -> TestResult {
    let f = Fixture::new()?;
    let selected = f.home.join(".cargo/bin");
    fs::create_dir_all(&selected)?;
    let path = std::env::join_paths([&selected, &f.bin])?;
    let bootstrap = f.run_bounded(
        f.command()
            .env("PATH", &path)
            .args(["bootstrap", "--no-gui-path"]),
    )?;
    assert!(
        shim::is_managed_shim(&selected.join("gh")),
        "bootstrap did not create fixture wrapper: {}",
        String::from_utf8_lossy(&bootstrap.stdout)
    );
    let uninstall = f.run_bounded(f.command().env("PATH", path).arg("uninstall-shim"))?;
    assert!(uninstall.status.success());
    assert!(
        !selected.join("gh").exists(),
        "documented rollback left bootstrap's active ~/.cargo/bin/gh in place"
    );
    Ok(())
}

#[test]
fn audit_diag_install_refuses_dangling_symlink() -> TestResult {
    let f = Fixture::new()?;
    let target_dir = f.home.join(".local/bin");
    fs::create_dir_all(&target_dir)?;
    let victim = f.home.join("unrelated-file");
    symlink(&victim, target_dir.join("gh"))?;
    let output = f.command().arg("install-shim").output()?;
    assert!(
        !output.status.success() && !victim.exists(),
        "install without --force followed an unmanaged dangling symlink and created its target: exit={:?}, victim_exists={}",
        output.status.code(),
        victim.exists()
    );
    Ok(())
}

#[test]
fn audit_diag_install_drops_other_write_permissions() -> TestResult {
    let f = Fixture::new()?;
    let target_dir = f.home.join(".local/bin");
    fs::create_dir_all(&target_dir)?;
    let target = target_dir.join("gh");
    fs::write(&target, shim::shim_script(Path::new("/old/gfj")))?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o666))?;
    assert!(f.command().arg("install-shim").output()?.status.success());
    let writable_by_others = fs::metadata(target)?.permissions().mode() & 0o022;
    assert_eq!(
        writable_by_others, 0,
        "install made the wrapper executable while preserving group/other write permission"
    );
    Ok(())
}

#[test]
fn audit_diag_recorder_recreate_prevents_self_recursion() -> TestResult {
    let f = Fixture::new()?;
    let directory = f.root.join("recorder");
    let first = git_recorder::create_git_recorder(
        &f.root.join("git.jsonl"),
        Some(&f.bin.join("git")),
        None,
        Some(&directory),
        &f.env(),
        Path::new(env!("CARGO_BIN_EXE_gfj")),
    )?;
    let mut env = f.env();
    env.insert("PATH".into(), first.path);
    let second = git_recorder::create_git_recorder(
        &f.root.join("git.jsonl"),
        None,
        None,
        Some(&directory),
        &env,
        Path::new(env!("CARGO_BIN_EXE_gfj")),
    )?;
    assert!(
        second.real_git != second.wrapper_path,
        "recreating the recorder selected itself as real git and rewrote itself to recurse forever"
    );
    Ok(())
}

#[test]
#[cfg(target_os = "linux")]
fn audit_diag_release_documented_extract() -> TestResult {
    let f = Fixture::new()?;
    for name in ["awk", "mktemp", "install", "tar", "gzip", "rm"] {
        let source = [
            Path::new("/usr/bin").join(name),
            Path::new("/bin").join(name),
        ]
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| format!("required local test tool missing: {name}"))?;
        symlink(source, f.bin.join(name))?;
    }
    let checksum = if Path::new("/usr/bin/sha256sum").is_file() {
        "sha256sum"
    } else {
        "shasum"
    };
    symlink(Path::new("/usr/bin").join(checksum), f.bin.join(checksum))?;
    f.tool("git", "#!/bin/sh\nprintf '%s\\n' \"$AUDIT_PACKAGE_ROOT\"\n")?;
    f.tool(
        "cargo",
        "#!/bin/sh\ncase \"$1\" in pkgid) printf '%s\\n' 'path+file:///audit#gh-forgejo-shim@0.1.2';; build) exit 0;; *) exit 98;; esac\n",
    )?;
    let target = "x86_64-unknown-linux-gnu";
    let release = f.root.join("build").join(target).join("release");
    fs::create_dir_all(&release)?;
    for binary in ["gh-forgejo-shim", "gfj"] {
        fs::write(release.join(binary), "#!/bin/sh\nexit 0\n")?;
    }
    fs::create_dir(f.root.join("docs"))?;
    for file in [
        "README.md",
        "LICENSE",
        "docs/installation.md",
        "docs/pipx-to-rust-migration.md",
        "docs/rollback.md",
    ] {
        fs::write(f.root.join(file), "audit fixture\n")?;
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/package-release.sh");
    let packaged = Command::new("/bin/bash")
        .arg(script)
        .arg(target)
        .env_clear()
        .envs(f.env())
        .env("AUDIT_PACKAGE_ROOT", &f.root)
        .env("CARGO_TARGET_DIR", f.root.join("build"))
        .env("DIST_DIR", f.root.join("dist"))
        .current_dir(&f.root)
        .output()?;
    assert!(
        packaged.status.success(),
        "packaging fixture failed: {}",
        String::from_utf8_lossy(&packaged.stderr)
    );
    // Extract the members exactly as docs/installation.md documents them.
    let docs = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/installation.md"),
    )?;
    let documented = docs
        .lines()
        .find(|line| line.trim_start().starts_with("tar -xzf"))
        .ok_or("docs/installation.md documents no tar -xzf command")?;
    let words = documented.split_whitespace().collect::<Vec<_>>();
    let members = words
        .iter()
        .position(|word| *word == "-C")
        .and_then(|index| words.get(index + 2..))
        .ok_or("the documented tar command has no -C DIR")?;
    let destination = f.home.join(".local/bin");
    fs::create_dir_all(&destination)?;
    let extracted = Command::new("tar")
        .arg("-xzf")
        .arg(
            f.root
                .join(format!("dist/gh-forgejo-shim-0.1.2-{target}.tar.gz")),
        )
        .arg("-C")
        .arg(destination)
        .args(members)
        .env_clear()
        .envs(f.env())
        .current_dir(&f.root)
        .output()?;
    assert!(
        extracted.status.success(),
        "docs/installation.md extraction fails on the actual release archive: {}",
        String::from_utf8_lossy(&extracted.stderr)
    );
    Ok(())
}
