//! Adversarial routing regressions. These assertions describe required behavior.
#![cfg(unix)]

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
const HOST: &str = "forgejo.example.test";
const REAL_GH: &str = "real-gh\n";

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    bin: PathBuf,
    real_gh: PathBuf,
}

impl Fixture {
    fn new() -> io::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "gfj-routing-audit-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root)?;
        let home = root.join("home with spaces");
        let repo = root.join("repo with spaces");
        let bin = root.join("bin with spaces");
        for dir in [&home, &repo, &bin, &root.join("real")] {
            fs::create_dir_all(dir)?;
        }
        let real_gh = root.join("real/gh");
        executable(&real_gh, "#!/bin/sh\nprintf 'real-gh\\n'\n")?;
        executable(&bin.join("fj"), "#!/bin/sh\nexit 99\n")?;

        // Reuse the installed Git binary, but never its user or system config, and never a
        // wrapper script on PATH (a git recorder, say) that could write outside the fixture.
        let git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .filter_map(|dir| dir.join("git").canonicalize().ok())
            .find(|path| path.is_file() && !is_script(path))
            .ok_or_else(|| {
                io::Error::other("a git binary is required for routing audit fixtures")
            })?;
        symlink(git, bin.join("git"))?;
        let fixture = Self {
            root,
            home,
            repo,
            bin,
            real_gh,
        };
        fixture.git(["init", "-q"])?;
        Ok(fixture)
    }

    fn isolate(&self, command: &mut Command) {
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("GH_CONFIG_DIR", self.home.join(".config/gh"))
            .env("PATH", &self.bin)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("FJ_SHIM_HOSTS", HOST)
            .env("FJ_SHIM_REAL_GH", &self.real_gh)
            .env("FJ_SHIM_REAL_FJ", self.bin.join("fj"))
            .env("FJ_SHIM_AUTH_STORE", "file")
            .env("FJ_SHIM_TRACE", "0")
            .env("LC_ALL", "C")
            .current_dir(&self.repo)
            .stdin(Stdio::null());
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gh-forgejo-shim"));
        self.isolate(&mut command);
        command
    }

    fn gh(&self, args: &[&str]) -> Command {
        let mut command = self.command();
        command.arg("gh").args(args);
        command
    }

    fn git<I, S>(&self, args: I) -> io::Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(self.bin.join("git"));
        self.isolate(&mut command);
        command.args(args);
        let output = self.run(&mut command)?;
        assert!(output.status.success(), "git fixture failed: {output:?}");
        Ok(())
    }

    fn origin(&self, url: &str) -> io::Result<()> {
        self.git(["remote", "add", "origin", url])
    }

    fn trace(
        &self,
        args: &[&str],
        extra_env: &[(&str, &str)],
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let trace = self.root.join("trace.jsonl");
        let mut command = self.gh(args);
        command
            .env("FJ_SHIM_TRACE", &trace)
            .envs(extra_env.iter().copied());
        let output = self.run(&mut command)?;
        assert!(output.status.success(), "routing probe failed: {output:?}");
        let text = fs::read_to_string(trace)?;
        Ok(serde_json::from_str(
            text.lines().last().ok_or("missing route trace")?,
        )?)
    }

    fn run(&self, command: &mut Command) -> io::Result<Output> {
        // Files avoid a full stdout/stderr pipe blocking a recursive child.
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let stdout = self.root.join(format!("stdout-{id}"));
        let stderr = self.root.join(format!("stderr-{id}"));
        command
            .stdout(File::create(&stdout)?)
            .stderr(File::create(&stderr)?)
            .process_group(0);
        let mut child = command.spawn()?;
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if started.elapsed() >= Duration::from_secs(5) {
                // Kill the group as well as the parent: delegation spawns children.
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .env_clear()
                    .status();
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "audit child exceeded 5s and was killed; stdout={:?}; stderr={:?}",
                    fs::read_to_string(&stdout),
                    fs::read_to_string(&stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        Ok(Output {
            status,
            stdout: fs::read(stdout)?,
            stderr: fs::read(stderr)?,
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn is_script(path: &Path) -> bool {
    let mut start = [0_u8; 2];
    File::open(path)
        .and_then(|mut file| io::Read::read_exact(&mut file, &mut start))
        .map_or(true, |()| &start == b"#!")
}

fn executable(path: &Path, contents: &str) -> io::Result<()> {
    fs::write(path, contents)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[test]
fn audit_route_copied_dispatcher_on_path() -> TestResult {
    let fixture = Fixture::new()?;
    fs::copy(
        env!("CARGO_BIN_EXE_gh-forgejo-shim"),
        fixture.bin.join("gh"),
    )?;
    let path = std::env::join_paths([&fixture.bin, fixture.real_gh.parent().ok_or("no parent")?])?;
    let mut command = fixture.gh(&["--version"]);
    command.env_remove("FJ_SHIM_REAL_GH").env("PATH", path);
    let output = fixture.run(&mut command)?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        REAL_GH,
        "a copied dispatcher must not be selected as real gh"
    );
    Ok(())
}

#[test]
fn audit_route_copied_sibling_entry_point_on_path() -> TestResult {
    // gfj and gh-forgejo-shim are both dispatchers but differ in bytes.
    let fixture = Fixture::new()?;
    fs::copy(env!("CARGO_BIN_EXE_gfj"), fixture.bin.join("gh"))?;
    let path = std::env::join_paths([&fixture.bin, fixture.real_gh.parent().ok_or("no parent")?])?;
    let mut command = fixture.gh(&["--version"]);
    command.env_remove("FJ_SHIM_REAL_GH").env("PATH", path);
    let output = fixture.run(&mut command)?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        REAL_GH,
        "a copied gfj must not be selected as real gh"
    );
    Ok(())
}

#[test]
fn audit_route_unreadable_dispatcher_copy_on_path() -> TestResult {
    // A copy that can be executed but not read can't be shown to differ, so it's skipped.
    let fixture = Fixture::new()?;
    let copy = fixture.bin.join("gh");
    fs::copy(env!("CARGO_BIN_EXE_gh-forgejo-shim"), &copy)?;
    fs::set_permissions(&copy, fs::Permissions::from_mode(0o111))?;
    let path = std::env::join_paths([&fixture.bin, fixture.real_gh.parent().ok_or("no parent")?])?;
    let mut command = fixture.gh(&["--version"]);
    command.env_remove("FJ_SHIM_REAL_GH").env("PATH", path);
    let output = fixture.run(&mut command)?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        REAL_GH,
        "an unreadable dispatcher copy must not be selected as real gh"
    );
    Ok(())
}

#[test]
fn audit_route_hardlinked_real_gh_override() -> TestResult {
    let fixture = Fixture::new()?;
    // Keep both hardlink names on the temp filesystem, including in Docker.
    let dispatcher = fixture.root.join("dispatcher");
    fs::copy(env!("CARGO_BIN_EXE_gh-forgejo-shim"), &dispatcher)?;
    let alias = fixture.bin.join("hardlinked-gh");
    fs::hard_link(&dispatcher, &alias)?;
    symlink(&fixture.real_gh, fixture.bin.join("gh"))?;
    let mut command = Command::new(&dispatcher);
    fixture.isolate(&mut command);
    command
        .args(["gh", "--version"])
        .env("FJ_SHIM_REAL_GH", &alias);
    let output = fixture.run(&mut command)?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        REAL_GH,
        "FJ_SHIM_REAL_GH must reject a hardlink to the running dispatcher"
    );
    Ok(())
}

#[test]
#[ignore = "audit R4: fails until gh-forgejo-shim-jko.5 is fixed"]
fn audit_route_version_in_forgejo_checkout() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.origin(&format!("https://{HOST}/local/project.git"))?;
    let output = fixture.run(&mut fixture.gh(&["--version"]))?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        REAL_GH,
        "gh --version must delegate even in a Forgejo checkout; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
#[ignore = "audit R2: fails until gh-forgejo-shim-jko.2 is fixed"]
fn audit_route_short_repo_override() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.origin(&format!("https://{HOST}/local/project.git"))?;
    let trace = fixture.trace(&["pr", "list", "-R", "other/target", "--help"], &[])?;
    assert_eq!(
        trace["repo"]["full_name"], "other/target",
        "-R other/target must not select the checkout's local/project"
    );
    Ok(())
}

#[test]
#[ignore = "audit R2: fails until gh-forgejo-shim-jko.2 is fixed"]
fn audit_route_short_gh_repo_override() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.origin(&format!("https://{HOST}/local/project.git"))?;
    let trace = fixture.trace(&["pr", "list", "--help"], &[("GH_REPO", "other/target")])?;
    assert_eq!(
        trace["repo"]["full_name"], "other/target",
        "GH_REPO=other/target must not select the checkout's local/project"
    );
    Ok(())
}

#[test]
#[ignore = "audit R5: fails until gh-forgejo-shim-jko.6 is fixed"]
fn audit_route_https_port_allowlist() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.origin(&format!("https://{HOST}:3443/owner/repo.git"))?;
    let host = format!("{HOST}:3443");
    let trace = fixture.trace(&["pr", "view", "--help"], &[("FJ_SHIM_HOSTS", &host)])?;
    assert_eq!(
        trace["route"]["kind"], "forgejo",
        "an HTTPS remote must retain its port when matching the configured host; detected host={}",
        trace["host"]
    );
    Ok(())
}

#[test]
fn audit_route_signal_passthrough() -> TestResult {
    let fixture = Fixture::new()?;
    executable(&fixture.real_gh, "#!/bin/sh\nkill -TERM $$\n")?;
    let output = fixture.run(&mut fixture.gh(&["--version"]))?;
    assert_eq!(
        output.status.signal(),
        Some(15),
        "the wrapper must preserve real gh's SIGTERM; observed status={}",
        output.status
    );
    Ok(())
}

#[test]
fn audit_route_signal_passthrough_restores_default_actions() -> TestResult {
    // Rust ignores SIGPIPE and handles SIGSEGV itself; real-time signals are outside
    // signal-hook's table. SIGSEGV and SIGRTMIN run on Linux only: macOS shows a crash
    // dialog for every process that dies of SIGSEGV.
    let signals: &[i32] = if cfg!(target_os = "linux") {
        &[13, 11, 34]
    } else {
        &[13]
    };
    for &signal in signals {
        let fixture = Fixture::new()?;
        executable(&fixture.real_gh, &format!("#!/bin/sh\nkill -{signal} $$\n"))?;
        let output = fixture.run(&mut fixture.gh(&["--version"]))?;
        assert_eq!(
            output.status.signal(),
            Some(signal),
            "the wrapper must die of real gh's signal {signal}; observed status={}",
            output.status
        );
    }
    Ok(())
}

#[test]
fn audit_route_force_install_symlink() -> TestResult {
    let fixture = Fixture::new()?;
    let original = fs::read(&fixture.real_gh)?;
    symlink(&fixture.real_gh, fixture.bin.join("gh"))?;
    let mut command = fixture.command();
    command
        .args(["install-shim", "--force", "--bin-dir"])
        .arg(&fixture.bin);
    let output = fixture.run(&mut command)?;
    assert!(output.status.success(), "install failed: {output:?}");
    assert_eq!(
        fs::read_to_string(&fixture.real_gh)?,
        String::from_utf8(original)?,
        "install --force must replace the gh symlink without overwriting the real gh it points to"
    );
    Ok(())
}

#[test]
#[ignore = "audit R7: fails until gh-forgejo-shim-jko.7 is fixed"]
fn audit_route_non_utf8_environment_passthrough() -> TestResult {
    let fixture = Fixture::new()?;
    let mut command = fixture.gh(&["--version"]);
    command.env("AUDIT_OPAQUE_VALUE", OsString::from_vec(vec![0xff]));
    let output = fixture.run(&mut command)?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        REAL_GH,
        "an unrelated non-UTF-8 environment value must not crash passthrough; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
