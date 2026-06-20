//! External command discovery and delegation helpers.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::shim::is_managed_shim;

pub const DEFAULT_FALLBACK_DIRS: &[&str] = &[
    "~/.local/bin",
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/opt/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub fn find_program(
    name: &str,
    configured: Option<&Path>,
    env: &HashMap<String, String>,
) -> Option<PathBuf> {
    let fallback_dirs = DEFAULT_FALLBACK_DIRS
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    find_program_with_fallbacks(name, configured, env, &fallback_dirs)
}

pub fn find_program_with_fallbacks(
    name: &str,
    configured: Option<&Path>,
    env: &HashMap<String, String>,
    fallback_dirs: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(configured) = configured.filter(|path| !path.as_os_str().is_empty()) {
        let path = expand_home(configured, env);
        if is_executable_file(&path) {
            if name == "gh" && should_skip_gh_candidate(&path) {
                // A configured managed shim is ignored so a real gh in PATH can still win.
            } else {
                return Some(path);
            }
        } else {
            return None;
        }
    }

    for directory in candidate_dirs(env, fallback_dirs) {
        let candidate = directory.join(name);
        if !is_executable_file(&candidate) {
            continue;
        }
        if name == "gh" && should_skip_gh_candidate(&candidate) {
            continue;
        }
        return Some(candidate);
    }
    None
}

pub fn run_program_capture(
    path: &Path,
    argv: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> io::Result<ProgramOutput> {
    let output = command_with_env(path, argv, env, cwd).output()?;
    Ok(ProgramOutput {
        code: output.status.code().unwrap_or(1),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

pub fn run_program_inherit(
    path: &Path,
    argv: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> io::Result<i32> {
    let status = command_with_env(path, argv, env, cwd).status()?;
    Ok(status.code().unwrap_or(1))
}

pub fn git_output(args: &[&str], cwd: Option<&Path>) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn command_with_env(
    path: &Path,
    argv: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> Command {
    let mut command = Command::new(path);
    command.args(argv).env_clear().envs(env);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
}

fn candidate_dirs(env: &HashMap<String, String>, fallback_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = Vec::<PathBuf>::new();
    let mut result = Vec::<PathBuf>::new();

    let path_dirs = env
        .get("PATH")
        .map(|value| std::env::split_paths(value).collect::<Vec<_>>())
        .unwrap_or_default();

    for directory in path_dirs.into_iter().chain(fallback_dirs.iter().cloned()) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let normalized = expand_home(&directory, env);
        if seen.iter().any(|existing| existing == &normalized) {
            continue;
        }
        seen.push(normalized.clone());
        result.push(normalized);
    }

    result
}

fn expand_home(path: &Path, env: &HashMap<String, String>) -> PathBuf {
    let value = path.as_os_str().to_string_lossy();
    if value == "~" {
        return env
            .get("HOME")
            .map_or_else(|| path.to_path_buf(), PathBuf::from);
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = env.get("HOME") {
            return Path::new(home).join(rest);
        }
    }
    path.to_path_buf()
}

fn should_skip_gh_candidate(path: &Path) -> bool {
    is_managed_shim(path) || is_current_executable(path)
}

fn is_current_executable(path: &Path) -> bool {
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    same_file(path, &current)
}

fn same_file(left: &Path, right: &Path) -> bool {
    let (Ok(left), Ok(right)) = (left.canonicalize(), right.canonicalize()) else {
        return false;
    };
    left == right
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    metadata.is_file() && is_executable(&metadata.permissions())
}

#[cfg(unix)]
fn is_executable(permissions: &std::fs::Permissions) -> bool {
    use std::os::unix::fs::PermissionsExt;

    permissions.mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_permissions: &std::fs::Permissions) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn finds_real_gh_in_fallback_dirs_when_path_is_minimal() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let fallback = fixture.root.join("homebrew").join("bin");
        let real_gh = fixture.fake_executable(&fallback, "gh", "#!/bin/sh\nexit 0\n")?;
        let env = HashMap::from([
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("HOME".to_string(), fixture.root.display().to_string()),
        ]);

        let found = find_program_with_fallbacks("gh", None, &env, &[fallback]);

        assert_eq!(found, Some(real_gh));
        Ok(())
    }

    #[test]
    fn skips_managed_gh_shim_in_fallback_dirs() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let shim_dir = fixture.root.join("local").join("bin");
        let real_dir = fixture.root.join("homebrew").join("bin");
        fixture.fake_executable(
            &shim_dir,
            "gh",
            "#!/bin/sh\n# managed by gh-forgejo-shim\nexec python -m gh_forgejo_shim gh \"$@\"\n",
        )?;
        let real_gh = fixture.fake_executable(&real_dir, "gh", "#!/bin/sh\nexit 0\n")?;
        let env = HashMap::from([
            ("PATH".to_string(), String::new()),
            ("HOME".to_string(), fixture.root.display().to_string()),
        ]);

        let found = find_program_with_fallbacks("gh", None, &env, &[shim_dir, real_dir]);

        assert_eq!(found, Some(real_gh));
        Ok(())
    }

    #[test]
    fn configured_path_wins_over_fallback_dirs() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let configured = fixture.fake_executable(
            &fixture.root.join("configured"),
            "gh",
            "#!/bin/sh\nexit 0\n",
        )?;
        let fallback =
            fixture.fake_executable(&fixture.root.join("fallback"), "gh", "#!/bin/sh\nexit 0\n")?;
        let env = HashMap::from([("PATH".to_string(), String::new())]);
        let fallback_dir = fallback
            .parent()
            .ok_or_else(|| io::Error::other("fallback executable has no parent"))?
            .to_path_buf();

        let found = find_program_with_fallbacks("gh", Some(&configured), &env, &[fallback_dir]);

        assert_eq!(found, Some(configured));
        Ok(())
    }

    #[test]
    fn configured_managed_gh_shim_does_not_win() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let configured = fixture.fake_executable(
            &fixture.root.join("configured"),
            "gh",
            "#!/bin/sh\n# managed by gh-forgejo-shim\nexec python -m gh_forgejo_shim gh \"$@\"\n",
        )?;
        let fallback =
            fixture.fake_executable(&fixture.root.join("fallback"), "gh", "#!/bin/sh\nexit 0\n")?;
        let env = HashMap::from([("PATH".to_string(), String::new())]);
        let fallback_dir = fallback
            .parent()
            .ok_or_else(|| io::Error::other("fallback executable has no parent"))?
            .to_path_buf();

        let found = find_program_with_fallbacks("gh", Some(&configured), &env, &[fallback_dir]);

        assert_eq!(found, Some(fallback));
        Ok(())
    }

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> io::Result<Self> {
            let root = std::env::temp_dir().join(format!(
                "gh-forgejo-shim-external-{}-{}",
                std::process::id(),
                NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root)?;
            Ok(Self { root })
        }

        fn fake_executable(&self, root: &Path, name: &str, contents: &str) -> io::Result<PathBuf> {
            fs::create_dir_all(root)?;
            let path = root.join(name);
            fs::write(&path, contents)?;
            make_executable(&path)?;
            Ok(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
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
}
