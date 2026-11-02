//! Adversarial regression tests. All credentials and tools are disposable fakes.
#![cfg(unix)]

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use gh_forgejo_shim::git_recorder;

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
        ] {
            fs::create_dir_all(dir)?;
        }
        fixture.tool("gh", "#!/bin/sh\nexit 0\n")?;
        fixture.tool("git", "#!/bin/sh\nexit 1\n")?;
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
            ("TMPDIR".into(), self.root.join("tmp").display().to_string()),
            ("FJ_SHIM_AUTH_STORE".into(), "file".into()),
            ("FJ_SHIM_HOSTS".into(), "forge.example.test".into()),
            (
                "FJ_SHIM_REAL_GH".into(),
                self.bin.join("gh").display().to_string(),
            ),
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
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
