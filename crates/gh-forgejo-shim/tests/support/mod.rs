use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub struct CliFixture {
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    bin: PathBuf,
}

impl CliFixture {
    pub fn new() -> io::Result<Self> {
        let id = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("gh-forgejo-shim-rust-{}-{id}", std::process::id()));
        let home = root.join("home");
        let repo = root.join("repo");
        let bin = root.join("bin");

        fs::create_dir_all(&home)?;
        fs::create_dir_all(&repo)?;
        fs::create_dir_all(&bin)?;

        Ok(Self {
            root,
            home,
            repo,
            bin,
        })
    }

    pub fn init_git_repo(&self) -> io::Result<()> {
        self.git(["init", "-b", "feature"])?;
        self.git(["config", "user.email", "test@example.com"])?;
        self.git(["config", "user.name", "Test User"])?;
        fs::write(self.repo.join("README.md"), "test\n")?;
        self.git(["add", "README.md"])?;
        self.git(["commit", "-m", "initial"])?;
        self.git([
            "remote",
            "add",
            "origin",
            "https://git.example.com/owner/repo.git",
        ])?;
        Ok(())
    }

    pub fn command(&self, binary: &str) -> io::Result<Command> {
        let mut command = Command::new(cargo_bin_path(binary)?);
        command
            .current_dir(&self.repo)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.bin)
            .env("FJ_SHIM_HOSTS", "git.example.com")
            .env("FJ_SHIM_REAL_GH", "/missing/gh");
        Ok(command)
    }

    fn git<I, S>(&self, args: I) -> io::Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = args
            .into_iter()
            .map(|arg| arg.as_ref().to_owned())
            .collect::<Vec<OsString>>();
        let status = Command::new("git")
            .args(&args)
            .current_dir(&self.repo)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "git {args:?} failed with {status}"
            )))
        }
    }
}

impl Drop for CliFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Debug, Deserialize)]
pub struct CompatibilityContract {
    pub schema: String,
    pub bead: String,
    pub command_matrix: Vec<CommandMatrixEntry>,
    pub golden_probes: Vec<GoldenProbe>,
}

#[derive(Debug, Deserialize)]
pub struct CommandMatrixEntry {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct GoldenProbe {
    pub id: String,
}

pub fn load_contract() -> Result<CompatibilityContract, Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/implementation/rust-rewrite/contracts/compatibility-contract.v1.json");
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn cargo_bin_path(binary: &str) -> io::Result<PathBuf> {
    let env_name = format!("CARGO_BIN_EXE_{binary}");
    std::env::var_os(&env_name)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{env_name} is not set")))
}
