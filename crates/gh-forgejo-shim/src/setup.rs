//! Shared setup discovery for bootstrap and doctor.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{Config, EnvMap};
use crate::{external, shim, Result};

const COMMON_PACKAGE_MANAGER_DIRS: &[&str] = &[
    "~/.local/bin",
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/opt/local/bin",
    "/home/linuxbrew/.linuxbrew/bin",
];

const SYSTEM_GUI_DIRS: &[&str] = &["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetMode {
    Current,
    UserLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSource {
    Explicit,
    CurrentPath,
    UserLocal,
    FallbackUserLocal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPath {
    pub path: PathBuf,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupModel {
    pub target_bin_dir: PathBuf,
    pub target_shim_path: PathBuf,
    pub target_source: TargetSource,
    pub target_bin_dir_in_current_path: bool,
    pub current_path_dirs: Vec<PathBuf>,
    pub first_current_gh: Option<PathBuf>,
    pub current_gh_candidates: Vec<PathBuf>,
    pub managed_gh_candidates: Vec<PathBuf>,
    pub target_shim_first_in_current_path: bool,
    pub first_current_gh_is_managed: bool,
    pub real_gh: Option<ToolPath>,
    pub bd: Option<ToolPath>,
    pub current_path_missing_common_dirs: Vec<PathBuf>,
    pub codex_restricted_path: bool,
    pub launchd_path: Option<String>,
    pub desired_launchd_path: String,
    pub launchd_contains_required_dirs: bool,
    pub launchd_missing_required_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SetupInspectOptions {
    pub env: EnvMap,
    pub home: Option<PathBuf>,
    pub config: Option<Config>,
    pub bin_dir: Option<PathBuf>,
    pub target: TargetMode,
    pub fallback_dirs: Option<Vec<PathBuf>>,
    pub launchd_path: Option<String>,
}

pub fn inspect(options: SetupInspectOptions) -> Result<SetupModel> {
    let fallback_dirs = options.fallback_dirs.unwrap_or_else(default_fallback_dirs);
    let selection = select_target_bin_dir(
        options.bin_dir.as_deref(),
        options.target,
        &options.env,
        options.home.as_deref(),
    )?;
    let target_shim_path = shim::shim_path(Some(&selection.bin_dir), options.home.as_deref())?;
    let current_path_dirs = current_path_dirs(&options.env);
    let current_gh_candidates = program_candidates_in_path("gh", &options.env);
    let managed_gh_candidates = current_gh_candidates
        .iter()
        .filter(|path| shim::is_managed_shim(path))
        .cloned()
        .collect::<Vec<_>>();
    let first_current_gh = current_gh_candidates.first().cloned();
    let target_bin_dir_in_current_path = current_path_dirs
        .iter()
        .any(|dir| same_file(dir, &selection.bin_dir) || dir == &selection.bin_dir);
    let target_shim_first_in_current_path = first_current_gh.as_ref().is_some_and(|path| {
        same_file(path, &target_shim_path) && shim::is_managed_shim(&target_shim_path)
    });
    let first_current_gh_is_managed = first_current_gh
        .as_ref()
        .is_some_and(|path| shim::is_managed_shim(path));
    let real_gh = find_real_gh(options.config.as_ref(), &options.env, &fallback_dirs);
    let bd = find_tool("bd", None, &options.env, &fallback_dirs, false);
    let current_path_missing_common_dirs =
        expanded_common_package_dirs(options.home.as_deref(), &options.env)
            .into_iter()
            .filter(|dir| {
                !current_path_dirs
                    .iter()
                    .any(|path_dir| same_file(path_dir, dir) || path_dir == dir)
            })
            .collect::<Vec<_>>();
    let codex_restricted_path = first_current_gh.is_none()
        && real_gh
            .as_ref()
            .and_then(|tool| tool.path.parent())
            .is_some_and(|dir| {
                !current_path_dirs
                    .iter()
                    .any(|path_dir| same_file(path_dir, dir))
            })
        && current_path_dirs.iter().any(|dir| {
            let value = dir.to_string_lossy();
            value.ends_with("/.cargo/bin") || value.contains("/.codex/")
        });

    let required_gui_dirs =
        required_gui_dirs(&selection.bin_dir, options.home.as_deref(), &options.env);
    let desired_launchd_path =
        desired_gui_path(&selection.bin_dir, options.home.as_deref(), &options.env);
    let launchd_missing_required_dirs = if let Some(path) = options.launchd_path.as_deref() {
        required_gui_dirs
            .iter()
            .filter(|dir| !path_contains_dir(path, dir))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        required_gui_dirs.clone()
    };

    Ok(SetupModel {
        target_bin_dir: selection.bin_dir,
        target_shim_path,
        target_source: selection.source,
        target_bin_dir_in_current_path,
        current_path_dirs,
        first_current_gh,
        current_gh_candidates,
        managed_gh_candidates,
        target_shim_first_in_current_path,
        first_current_gh_is_managed,
        real_gh,
        bd,
        current_path_missing_common_dirs,
        codex_restricted_path,
        launchd_path: options.launchd_path,
        desired_launchd_path,
        launchd_contains_required_dirs: launchd_missing_required_dirs.is_empty(),
        launchd_missing_required_dirs,
    })
}

pub fn find_tool(
    name: &str,
    configured: Option<&Path>,
    env: &EnvMap,
    fallback_dirs: &[PathBuf],
    skip_managed_gh: bool,
) -> Option<ToolPath> {
    if let Some(configured) = configured.filter(|path| !path.as_os_str().is_empty()) {
        let path = expand_home(configured, env);
        if is_executable_file(&path) {
            if skip_managed_gh && name == "gh" && should_skip_gh_candidate(&path) {
                // Keep looking for the real GitHub CLI.
            } else {
                let source = if name == "gh" && env.contains_key("FJ_SHIM_REAL_GH") {
                    "FJ_SHIM_REAL_GH"
                } else {
                    "configured path"
                };
                return Some(ToolPath {
                    path,
                    source: source.to_string(),
                });
            }
        } else {
            return None;
        }
    }

    for directory in current_path_dirs(env) {
        let candidate = directory.join(name);
        if !is_executable_file(&candidate) {
            continue;
        }
        if skip_managed_gh && name == "gh" && should_skip_gh_candidate(&candidate) {
            continue;
        }
        return Some(ToolPath {
            path: candidate,
            source: "current PATH".to_string(),
        });
    }

    for directory in fallback_dirs {
        let directory = expand_home(directory, env);
        if current_path_dirs(env)
            .iter()
            .any(|path_dir| same_file(path_dir, &directory))
        {
            continue;
        }
        let candidate = directory.join(name);
        if !is_executable_file(&candidate) {
            continue;
        }
        if skip_managed_gh && name == "gh" && should_skip_gh_candidate(&candidate) {
            continue;
        }
        return Some(ToolPath {
            path: candidate,
            source: "common install dirs".to_string(),
        });
    }

    None
}

pub fn env_hash(env: &EnvMap) -> HashMap<String, String> {
    env.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub fn path_contains_dir(path_value: &str, directory: &Path) -> bool {
    std::env::split_paths(path_value)
        .filter(|path| !path.as_os_str().is_empty())
        .any(|path| same_file(&path, directory) || path == directory)
}

pub fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "@%_+=:,./-".contains(character))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn find_real_gh(
    config: Option<&Config>,
    env: &EnvMap,
    fallback_dirs: &[PathBuf],
) -> Option<ToolPath> {
    let configured = config.and_then(|config| config.paths.gh.as_deref().map(Path::new));
    find_tool("gh", configured, env, fallback_dirs, true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetSelection {
    bin_dir: PathBuf,
    source: TargetSource,
}

fn select_target_bin_dir(
    explicit_bin_dir: Option<&Path>,
    target: TargetMode,
    env: &EnvMap,
    home: Option<&Path>,
) -> Result<TargetSelection> {
    if let Some(bin_dir) = explicit_bin_dir {
        return Ok(TargetSelection {
            bin_dir: bin_dir.to_path_buf(),
            source: TargetSource::Explicit,
        });
    }

    if target == TargetMode::UserLocal {
        return Ok(TargetSelection {
            bin_dir: shim::default_bin_dir(home)?,
            source: TargetSource::UserLocal,
        });
    }

    if let Some(bin_dir) = first_safe_current_path_dir(env, home) {
        return Ok(TargetSelection {
            bin_dir,
            source: TargetSource::CurrentPath,
        });
    }

    Ok(TargetSelection {
        bin_dir: shim::default_bin_dir(home)?,
        source: TargetSource::FallbackUserLocal,
    })
}

fn first_safe_current_path_dir(env: &EnvMap, home: Option<&Path>) -> Option<PathBuf> {
    current_path_dirs(env)
        .into_iter()
        .find(|directory| safe_visible_target_dir(directory, home))
}

fn safe_visible_target_dir(directory: &Path, home: Option<&Path>) -> bool {
    if !directory.is_absolute() || !directory.is_dir() || !is_writable_dir(directory) {
        return false;
    }
    let Some(home) = home else {
        return false;
    };
    if !(directory == home || directory.starts_with(home)) {
        return false;
    }
    let candidate = directory.join("gh");
    !candidate.exists() || shim::is_managed_shim(&candidate)
}

fn desired_gui_path(target_bin_dir: &Path, home: Option<&Path>, env: &EnvMap) -> String {
    let mut dirs = required_gui_dirs(target_bin_dir, home, env);
    dirs.extend(SYSTEM_GUI_DIRS.iter().map(PathBuf::from));
    dedupe_paths(dirs)
        .into_iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(":")
}

fn required_gui_dirs(target_bin_dir: &Path, home: Option<&Path>, env: &EnvMap) -> Vec<PathBuf> {
    let mut dirs = vec![target_bin_dir.to_path_buf()];
    dirs.extend(expanded_common_package_dirs(home, env));
    dedupe_paths(dirs)
}

fn expanded_common_package_dirs(home: Option<&Path>, env: &EnvMap) -> Vec<PathBuf> {
    COMMON_PACKAGE_MANAGER_DIRS
        .iter()
        .map(|path| expand_home(Path::new(path), env))
        .map(|path| {
            if path == Path::new("~/.local/bin") {
                home.map_or(path, |home| home.join(".local").join("bin"))
            } else {
                path
            }
        })
        .collect()
}

fn default_fallback_dirs() -> Vec<PathBuf> {
    external::DEFAULT_FALLBACK_DIRS
        .iter()
        .map(PathBuf::from)
        .collect()
}

fn current_path_dirs(env: &EnvMap) -> Vec<PathBuf> {
    env.get("PATH")
        .map(|path| {
            std::env::split_paths(path)
                .filter(|path| !path.as_os_str().is_empty())
                .map(|path| expand_home(&path, env))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn program_candidates_in_path(name: &str, env: &EnvMap) -> Vec<PathBuf> {
    current_path_dirs(env)
        .into_iter()
        .map(|directory| directory.join(name))
        .filter(|candidate| is_executable_file(candidate))
        .collect()
}

fn expand_home(path: &Path, env: &EnvMap) -> PathBuf {
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

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut result = Vec::<PathBuf>::new();
    for path in paths {
        if !result
            .iter()
            .any(|existing| same_file(existing, &path) || existing == &path)
        {
            result.push(path);
        }
    }
    result
}

fn should_skip_gh_candidate(path: &Path) -> bool {
    shim::is_managed_shim(path) || is_current_executable(path)
}

fn is_current_executable(path: &Path) -> bool {
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    same_file(path, &current)
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    metadata.is_file() && is_executable(&metadata.permissions())
}

fn is_writable_dir(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    metadata.is_dir()
        && !metadata.permissions().readonly()
        && writable_permission(&metadata.permissions())
}

#[cfg(unix)]
fn is_executable(permissions: &fs::Permissions) -> bool {
    use std::os::unix::fs::PermissionsExt;

    permissions.mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_permissions: &fs::Permissions) -> bool {
    true
}

#[cfg(unix)]
fn writable_permission(permissions: &fs::Permissions) -> bool {
    use std::os::unix::fs::PermissionsExt;

    permissions.mode() & 0o222 != 0
}

#[cfg(not(unix))]
fn writable_permission(_permissions: &fs::Permissions) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn current_target_prefers_writable_user_path_dir() -> Result<()> {
        let fixture = Fixture::new()?;
        let cargo_bin = fixture.home.join(".cargo").join("bin");
        fs::create_dir_all(&cargo_bin)?;
        let env = EnvMap::from([
            ("HOME".to_string(), fixture.home.display().to_string()),
            ("PATH".to_string(), cargo_bin.display().to_string()),
        ]);

        let model = inspect(SetupInspectOptions {
            env,
            home: Some(fixture.home.clone()),
            config: None,
            bin_dir: None,
            target: TargetMode::Current,
            fallback_dirs: Some(Vec::new()),
            launchd_path: None,
        })?;

        assert_eq!(model.target_bin_dir, cargo_bin);
        assert_eq!(model.target_source, TargetSource::CurrentPath);
        Ok(())
    }

    #[test]
    fn current_target_falls_back_when_visible_dir_is_not_safe() -> Result<()> {
        let fixture = Fixture::new()?;
        let systemish = fixture.root.join("outside").join("bin");
        fs::create_dir_all(&systemish)?;
        let env = EnvMap::from([
            ("HOME".to_string(), fixture.home.display().to_string()),
            ("PATH".to_string(), systemish.display().to_string()),
        ]);

        let model = inspect(SetupInspectOptions {
            env,
            home: Some(fixture.home.clone()),
            config: None,
            bin_dir: None,
            target: TargetMode::Current,
            fallback_dirs: Some(Vec::new()),
            launchd_path: None,
        })?;

        assert_eq!(
            model.target_bin_dir,
            fixture.home.join(".local").join("bin")
        );
        assert_eq!(model.target_source, TargetSource::FallbackUserLocal);
        Ok(())
    }

    #[test]
    fn detects_codex_style_restricted_path_gap() -> Result<()> {
        let fixture = Fixture::new()?;
        let cargo_bin = fixture.home.join(".cargo").join("bin");
        let homebrew = fixture.root.join("homebrew").join("bin");
        fs::create_dir_all(&cargo_bin)?;
        fs::create_dir_all(&homebrew)?;
        write_executable(&homebrew.join("gh"), "#!/bin/sh\nexit 0\n")?;
        let env = EnvMap::from([
            ("HOME".to_string(), fixture.home.display().to_string()),
            ("PATH".to_string(), cargo_bin.display().to_string()),
        ]);

        let model = inspect(SetupInspectOptions {
            env,
            home: Some(fixture.home.clone()),
            config: None,
            bin_dir: None,
            target: TargetMode::Current,
            fallback_dirs: Some(vec![homebrew]),
            launchd_path: None,
        })?;

        assert!(model.codex_restricted_path);
        assert!(model.real_gh.is_some());
        Ok(())
    }

    struct Fixture {
        root: PathBuf,
        home: PathBuf,
    }

    impl Fixture {
        fn new() -> io::Result<Self> {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("gh-forgejo-shim-setup-{}-{id}", std::process::id()));
            fs::create_dir(&root)?;
            let home = root.join("home");
            fs::create_dir(&home)?;
            Ok(Self { root, home })
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn write_executable(path: &Path, contents: &str) -> io::Result<()> {
        fs::write(path, contents)?;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
    }
}
