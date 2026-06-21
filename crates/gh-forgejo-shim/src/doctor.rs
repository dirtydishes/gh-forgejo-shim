//! Diagnostic checks for shim setup.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::auth;
use crate::config::{self, Config, EnvMap};
use crate::external;
use crate::repo;
use crate::shim;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct CheckOptions {
    pub config: Option<Config>,
    pub config_path: Option<PathBuf>,
    pub env: EnvMap,
    pub bin_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub fallback_dirs: Option<Vec<PathBuf>>,
    pub launchd_path: Option<String>,
    pub check_gui_path: Option<bool>,
    pub check_current_repo: Option<bool>,
    pub platform: String,
}

impl CheckOptions {
    pub fn new(env: EnvMap) -> Self {
        Self {
            config: None,
            config_path: None,
            env,
            bin_dir: None,
            home: None,
            cwd: None,
            fallback_dirs: None,
            launchd_path: None,
            check_gui_path: None,
            check_current_repo: None,
            platform: std::env::consts::OS.to_string(),
        }
    }
}

pub fn run_checks(options: CheckOptions) -> Result<Vec<Check>> {
    let should_check_current_repo = options
        .check_current_repo
        .unwrap_or(options.config.is_none());
    let config = match options.config {
        Some(config) => config,
        None => {
            let path = options
                .config_path
                .clone()
                .unwrap_or_else(|| config::config_path(options.home.as_deref()));
            config::load_config_with_env(Some(&path), &options.env)?
        }
    };
    let target_bin_dir = options.bin_dir.clone().unwrap_or_else(|| {
        shim::default_bin_dir(options.home.as_deref())
            .unwrap_or_else(|_| PathBuf::from(".local/bin"))
    });
    let wrapper = shim::shim_path(Some(&target_bin_dir), options.home.as_deref())?;
    let env_hash = env_hash(&options.env);

    let configured_gh = config.paths.gh.as_deref().map(Path::new);
    let configured_fj = config.paths.fj.as_deref().map(Path::new);
    let real_gh = if let Some(fallback_dirs) = options.fallback_dirs.as_deref() {
        external::find_program_with_fallbacks("gh", configured_gh, &env_hash, fallback_dirs)
    } else {
        external::find_program("gh", configured_gh, &env_hash)
    };
    let real_fj = if let Some(fallback_dirs) = options.fallback_dirs.as_deref() {
        external::find_program_with_fallbacks("fj", configured_fj, &env_hash, fallback_dirs)
    } else {
        external::find_program("fj", configured_fj, &env_hash)
    };
    let token = auth::discover_fj_token(
        config.hosts.first().map(String::as_str),
        &options.env,
        options.home.as_deref(),
    );

    let mut checks = vec![
        Check::new(
            "real gh",
            real_gh.is_some(),
            real_gh
                .as_ref()
                .map(|path| display_path(path))
                .unwrap_or_else(|| "set FJ_SHIM_REAL_GH or install GitHub CLI".to_string()),
        ),
        Check::new(
            "fj",
            real_fj.is_some(),
            real_fj
                .as_ref()
                .map(|path| display_path(path))
                .unwrap_or_else(|| "set FJ_SHIM_REAL_FJ or install fj".to_string()),
        ),
        Check::new(
            "forgejo hosts",
            !config.hosts.is_empty(),
            if config.hosts.is_empty() {
                "add one with gh-forgejo-shim config add-host HOST".to_string()
            } else {
                config.hosts.join(", ")
            },
        ),
        Check::new(
            "auth token",
            token.is_some(),
            if token.is_some() {
                "found".to_string()
            } else {
                "run gh-forgejo-shim auth login HOST or set FJ_SHIM_TOKEN".to_string()
            },
        ),
    ];

    if should_check_current_repo {
        if let Some(repo) = repo::detect_from_git(options.cwd.as_deref()) {
            if config.is_forgejo_host(Some(&repo.host)) {
                checks.push(Check::new(
                    "current repo host",
                    true,
                    format!(
                        "{} is allowlisted for {}/{}",
                        repo.host, repo.owner, repo.name
                    ),
                ));
            } else if config::is_known_github_host(Some(&repo.host)) {
                checks.push(Check::new(
                    "current repo host",
                    true,
                    format!(
                        "{} will delegate to the real GitHub CLI for {}/{}",
                        repo.host, repo.owner, repo.name
                    ),
                ));
            } else {
                checks.push(Check::new(
                    "current repo host",
                    false,
                    format!(
                        "{} is not allowlisted; run gh-forgejo-shim config add-host {}",
                        repo.host, repo.host
                    ),
                ));
            }
        }
    }

    checks.push(shim_path_check(&wrapper, &options.env));

    let should_check_gui_path = options
        .check_gui_path
        .unwrap_or_else(|| options.platform == "darwin");
    if should_check_gui_path {
        let gui_path = options.launchd_path.or_else(current_launchd_path);
        checks.push(gui_path_check(&target_bin_dir, gui_path.as_deref()));
    }

    Ok(checks)
}

pub fn format_checks(checks: &[Check]) -> String {
    let mut lines = vec!["gh-forgejo-shim doctor".to_string()];
    for check in checks {
        let mark = if check.ok { "ok" } else { "warn" };
        lines.push(format!("[{mark}] {}: {}", check.name, check.detail));
    }
    lines.join("\n")
}

impl Check {
    fn new(name: impl Into<String>, ok: bool, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok,
            detail: detail.into(),
        }
    }
}

fn shim_path_check(wrapper: &Path, env: &EnvMap) -> Check {
    if wrapper.exists() && shim::is_managed_shim(wrapper) {
        let first_gh = first_program("gh", env.get("PATH").map_or("", String::as_str));
        if first_gh
            .as_deref()
            .is_some_and(|path| same_file(path, wrapper))
        {
            return Check::new(
                "shim path",
                true,
                format!("{} is the first gh in PATH", wrapper.display()),
            );
        }
        if let Some(first_gh) = first_gh {
            return Check::new(
                "shim path",
                false,
                format!(
                    "gh resolves to {} before {}",
                    first_gh.display(),
                    wrapper.display()
                ),
            );
        }
        return Check::new(
            "shim path",
            false,
            format!(
                "{} exists but no gh executable was found in PATH",
                wrapper.display()
            ),
        );
    }
    if wrapper.exists() {
        return Check::new(
            "shim path",
            false,
            format!(
                "{} exists but is not managed by gh-forgejo-shim",
                wrapper.display()
            ),
        );
    }
    Check::new(
        "shim path",
        false,
        format!("{} is not installed", wrapper.display()),
    )
}

fn gui_path_check(bin_dir: &Path, gui_path: Option<&str>) -> Check {
    if gui_path.is_some_and(|path| path_contains_dir(path, bin_dir)) {
        return Check::new(
            "macOS gui PATH",
            true,
            format!("{} is visible to new GUI apps", bin_dir.display()),
        );
    }
    if gui_path.is_some() {
        return Check::new(
            "macOS gui PATH",
            false,
            format!(
                "{} is not in launchd PATH; run gh-forgejo-shim install-gui-path",
                bin_dir.display()
            ),
        );
    }
    Check::new(
        "macOS gui PATH",
        false,
        "launchd PATH is unset; run gh-forgejo-shim install-gui-path if GUI apps cannot find gh",
    )
}

fn current_launchd_path() -> Option<String> {
    let output = Command::new("launchctl")
        .args(["getenv", "PATH"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn first_program(name: &str, path_value: &str) -> Option<PathBuf> {
    for directory in std::env::split_paths(path_value) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let candidate = directory.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn path_contains_dir(path_value: &str, bin_dir: &Path) -> bool {
    std::env::split_paths(path_value).any(|path| path == bin_dir)
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && is_executable(&metadata.permissions())
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

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

fn env_hash(env: &EnvMap) -> HashMap<String, String> {
    env.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
