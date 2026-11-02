//! Adversarial routing regressions. These assertions describe required behavior.
#![cfg(unix)]

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
const HOST: &str = "forgejo.example.test";

struct Fixture {
    root: PathBuf,
    home: PathBuf,
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
        let bin = root.join("bin with spaces");
        for dir in [&home, &bin, &root.join("real")] {
            fs::create_dir_all(dir)?;
        }
        let real_gh = root.join("real/gh");
        executable(&real_gh, "#!/bin/sh\nprintf 'real-gh\\n'\n")?;
        Ok(Self {
            root,
            home,
            bin,
            real_gh,
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gh-forgejo-shim"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("GH_CONFIG_DIR", self.home.join(".config/gh"))
            .env("PATH", &self.bin)
            .env("FJ_SHIM_HOSTS", HOST)
            .env("FJ_SHIM_REAL_GH", &self.real_gh)
            .env("FJ_SHIM_AUTH_STORE", "file")
            .env("FJ_SHIM_TRACE", "0")
            .env("LC_ALL", "C")
            .current_dir(&self.root)
            .stdin(Stdio::null());
        command
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

fn executable(path: &Path, contents: &str) -> io::Result<()> {
    fs::write(path, contents)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
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
