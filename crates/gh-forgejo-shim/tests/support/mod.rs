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
        let root = create_unique_root()?;
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

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }

    pub fn bin(&self) -> &Path {
        &self.bin
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

    pub fn write_executable(&self, name: &str, contents: &str) -> io::Result<PathBuf> {
        let path = self.bin.join(name);
        fs::write(&path, contents)?;
        make_executable(&path)?;
        Ok(path)
    }

    pub fn command(&self, binary: &str) -> io::Result<Command> {
        let mut command = Command::new(cargo_bin_path(binary)?);
        command
            .current_dir(&self.repo)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", self.fixture_path()?)
            .env("FJ_SHIM_HOSTS", "git.example.com")
            .env("FJ_SHIM_REAL_GH", "/missing/gh");
        Ok(command)
    }

    fn fixture_path(&self) -> io::Result<OsString> {
        let mut paths = vec![self.bin.clone()];
        if let Some(system_path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&system_path));
        }
        std::env::join_paths(paths).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unable to build fixture PATH: {error}"),
            )
        })
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

fn create_unique_root() -> io::Result<PathBuf> {
    for _ in 0..1000 {
        let id = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("gh-forgejo-shim-rust-{}-{id}", std::process::id()));
        match fs::create_dir(&root) {
            Ok(()) => return Ok(root),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "unable to allocate a unique gh-forgejo-shim Rust fixture directory",
    ))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}
