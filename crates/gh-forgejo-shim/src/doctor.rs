//! Diagnostic checks for shim setup.

use std::path::{Path, PathBuf};

use crate::auth;
use crate::config::{self, Config, EnvMap};
use crate::gui_path;
use crate::repo;
use crate::setup::{self, SetupInspectOptions, TargetMode};
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub repair_commands: Vec<String>,
    pub status: CheckStatus,
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
    let fallback_dirs = options
        .fallback_dirs
        .clone()
        .unwrap_or_else(default_fallback_dirs);
    let launchd_path = options.launchd_path.clone().or_else(|| {
        (options.platform == "darwin")
            .then(gui_path::current_launchd_path)
            .flatten()
    });
    let setup_model = setup::inspect(SetupInspectOptions {
        env: options.env.clone(),
        home: options.home.clone(),
        config: Some(config.clone()),
        bin_dir: options.bin_dir.clone(),
        target: TargetMode::Current,
        fallback_dirs: Some(fallback_dirs.clone()),
        launchd_path,
    })?;
    let configured_fj = config.paths.fj.as_deref().map(Path::new);
    let real_fj = setup::find_tool("fj", configured_fj, &options.env, &fallback_dirs, false);
    let token = auth::discover_fj_token(
        config.hosts.first().map(String::as_str),
        &options.env,
        options.home.as_deref(),
    );

    let mut checks = vec![
        current_path_check(&setup_model),
        real_gh_check(&setup_model, &options.platform),
        managed_gh_check(&setup_model),
    ];
    if let Some(check) = duplicate_managed_gh_check(&setup_model) {
        checks.push(check);
    }

    let should_check_gui_path = options
        .check_gui_path
        .unwrap_or_else(|| options.platform == "darwin");
    if should_check_gui_path {
        checks.push(gui_path_check(&setup_model));
    }

    checks.push(bd_check(&setup_model));
    checks.push(fj_check(real_fj));
    checks.push(Check::new(
        "forgejo hosts",
        !config.hosts.is_empty(),
        if config.hosts.is_empty() {
            "add one with gh-forgejo-shim config add-host HOST".to_string()
        } else {
            config.hosts.join(", ")
        },
        ["gh-forgejo-shim config add-host git.example.com"],
    ));
    checks.push(Check::new(
        "auth token",
        token.is_some(),
        if token.is_some() {
            "found".to_string()
        } else {
            "run gh-forgejo-shim auth login HOST or set FJ_SHIM_TOKEN".to_string()
        },
        ["gh-forgejo-shim auth login HOST"],
    ));

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
                    Vec::<String>::new(),
                ));
            } else if config::is_known_github_host(Some(&repo.host)) {
                checks.push(Check::new(
                    "current repo host",
                    true,
                    format!(
                        "{} will delegate to the real GitHub CLI for {}/{}",
                        repo.host, repo.owner, repo.name
                    ),
                    Vec::<String>::new(),
                ));
            } else {
                checks.push(Check::new(
                    "current repo host",
                    false,
                    format!(
                        "{} is not allowlisted; run gh-forgejo-shim config add-host {}",
                        repo.host, repo.host
                    ),
                    [format!("gh-forgejo-shim config add-host {}", repo.host)],
                ));
            }
        }
    }

    Ok(checks)
}

pub fn format_checks(checks: &[Check]) -> String {
    let mut lines = vec!["gh-forgejo-shim doctor".to_string()];
    let mut repair_commands = Vec::new();
    for check in checks {
        let mark = match check.status {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fix => "fix",
        };
        lines.push(format!("[{mark}] {}: {}", check.name, check.detail));
        repair_commands.extend(check.repair_commands.iter().cloned());
    }
    let repair_commands = dedupe(repair_commands);
    if !repair_commands.is_empty() {
        lines.push(String::new());
        lines.push("repair commands:".to_string());
        for command in repair_commands {
            lines.push(format!("  {command}"));
        }
    }
    lines.join("\n")
}

pub fn has_failures(checks: &[Check]) -> bool {
    checks.iter().any(|check| check.status == CheckStatus::Fix)
}

impl Check {
    fn new<I, S>(
        name: impl Into<String>,
        ok: bool,
        detail: impl Into<String>,
        repair_commands: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            name: name.into(),
            ok,
            detail: detail.into(),
            repair_commands: repair_commands.into_iter().map(Into::into).collect(),
            status: if ok {
                CheckStatus::Ok
            } else {
                CheckStatus::Fix
            },
        }
    }

    fn warn(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok: true,
            detail: detail.into(),
            repair_commands: Vec::new(),
            status: CheckStatus::Warn,
        }
    }
}

fn current_path_check(model: &setup::SetupModel) -> Check {
    if let Some(first_gh) = model.first_current_gh.as_ref() {
        return Check::new(
            "current PATH",
            true,
            format!("bare gh resolves to {}", first_gh.display()),
            Vec::<String>::new(),
        );
    }
    if model.codex_restricted_path {
        let real_gh = model
            .real_gh
            .as_ref()
            .map(|tool| tool.path.display().to_string())
            .unwrap_or_else(|| "a common install dir".to_string());
        return Check::new(
            "current PATH",
            false,
            format!(
                "Codex-style restricted PATH cannot resolve bare gh, but real gh exists at {real_gh}"
            ),
            path_repair_commands(&model.target_bin_dir),
        );
    }
    Check::new(
        "current PATH",
        false,
        "bare gh is missing from the current process PATH",
        path_repair_commands(&model.target_bin_dir),
    )
}

fn real_gh_check(model: &setup::SetupModel, platform: &str) -> Check {
    if let Some(real_gh) = model.real_gh.as_ref() {
        return Check::new(
            "real gh",
            true,
            format!("found {} via {}", real_gh.path.display(), real_gh.source),
            Vec::<String>::new(),
        );
    }
    Check::new(
        "real gh",
        false,
        "could not find the real GitHub CLI from config, env, current PATH, or common install dirs",
        real_gh_repair_commands(platform),
    )
}

fn managed_gh_check(model: &setup::SetupModel) -> Check {
    if model.first_current_gh_is_managed {
        let first_gh = model.first_current_gh.as_ref().expect("managed gh path");
        if model.real_gh.is_some() {
            return Check::new(
                "managed gh",
                true,
                format!(
                    "{} is the first gh and can delegate to real gh",
                    first_gh.display()
                ),
                Vec::<String>::new(),
            );
        }
        return Check::new(
            "managed gh",
            false,
            format!(
                "{} is first in PATH, but no real gh was found for delegation",
                first_gh.display()
            ),
            real_gh_repair_commands(""),
        );
    }

    if let Some(first_gh) = model.first_current_gh.as_ref() {
        return Check::new(
            "managed gh",
            false,
            format!(
                "gh resolves to unmanaged {} before the managed wrapper",
                first_gh.display()
            ),
            path_repair_commands(&model.target_bin_dir),
        );
    }

    if model.target_shim_path.exists() && crate::shim::is_managed_shim(&model.target_shim_path) {
        return Check::new(
            "managed gh",
            false,
            format!(
                "{} exists, but current PATH does not resolve gh to it",
                model.target_shim_path.display()
            ),
            path_repair_commands(&model.target_bin_dir),
        );
    }

    Check::new(
        "managed gh",
        false,
        format!("{} is not installed", model.target_shim_path.display()),
        [format!(
            "gh-forgejo-shim bootstrap --bin-dir {}",
            setup::shell_quote(&model.target_bin_dir.display().to_string())
        )],
    )
}

fn duplicate_managed_gh_check(model: &setup::SetupModel) -> Option<Check> {
    if model.managed_gh_candidates.len() <= 1
        || !model.first_current_gh_is_managed
        || model.real_gh.is_none()
    {
        return None;
    }
    Some(Check::warn(
        "duplicate managed gh",
        format!(
            "{} managed wrappers are visible; first one works, but remove older duplicates when convenient",
            model.managed_gh_candidates.len()
        ),
    ))
}

fn gui_path_check(model: &setup::SetupModel) -> Check {
    if model.launchd_contains_required_dirs {
        return Check::new(
            "macOS GUI PATH",
            true,
            "launchd PATH exposes the shim and common package-manager dirs",
            Vec::<String>::new(),
        );
    }
    let detail = if model.launchd_path.is_some() {
        format!(
            "launchd PATH is missing {} required dirs for GUI apps",
            model.launchd_missing_required_dirs.len()
        )
    } else {
        "launchd PATH is unset for GUI apps".to_string()
    };
    Check::new(
        "macOS GUI PATH",
        false,
        detail,
        [
            format!(
                "gh-forgejo-shim install-gui-path --path {}",
                setup::shell_quote(&model.desired_launchd_path)
            ),
            "restart existing GUI apps".to_string(),
        ],
    )
}

fn bd_check(model: &setup::SetupModel) -> Check {
    if let Some(bd) = model.bd.as_ref() {
        return Check::new(
            "bd",
            true,
            format!(
                "found {} via {}; repo workflows may use it",
                bd.path.display(),
                bd.source
            ),
            Vec::<String>::new(),
        );
    }
    Check::warn(
        "bd",
        "not found; repo workflows may need Beads, but bd is not part of the GitHub CLI shim",
    )
}

fn fj_check(real_fj: Option<setup::ToolPath>) -> Check {
    if let Some(real_fj) = real_fj {
        return Check::new(
            "fj",
            true,
            format!("found {} via {}", real_fj.path.display(), real_fj.source),
            Vec::<String>::new(),
        );
    }
    Check::warn(
        "fj",
        "not found; auth import can still use env, shim storage, tea, or gitea configs",
    )
}

fn path_repair_commands(target_bin_dir: &Path) -> Vec<String> {
    vec![
        format!("export PATH=\"{}:$PATH\"", target_bin_dir.display()),
        format!(
            "gh-forgejo-shim bootstrap --bin-dir {}",
            setup::shell_quote(&target_bin_dir.display().to_string())
        ),
        "gh-forgejo-shim doctor".to_string(),
    ]
}

fn real_gh_repair_commands(platform: &str) -> Vec<String> {
    let mut commands = Vec::new();
    if platform == "darwin" {
        commands.push("brew install gh".to_string());
    }
    commands.push("FJ_SHIM_REAL_GH=/path/to/gh gh-forgejo-shim doctor".to_string());
    commands
}

fn default_fallback_dirs() -> Vec<PathBuf> {
    crate::external::DEFAULT_FALLBACK_DIRS
        .iter()
        .map(PathBuf::from)
        .collect()
}

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        if !result.contains(&value) {
            result.push(value);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn missing_bd_and_duplicate_wrappers_are_only_warnings_when_managed_gh_works() -> Result<()> {
        let fixture = Fixture::new()?;
        let shim_a = fixture.home.join(".local").join("bin");
        let shim_b = fixture.home.join(".cargo").join("bin");
        let real_dir = fixture.root.join("real");
        fs::create_dir_all(&shim_a)?;
        fs::create_dir_all(&shim_b)?;
        fs::create_dir_all(&real_dir)?;
        write_executable(
            &shim_a.join("gh"),
            &crate::shim::shim_script(&fixture.root.join("gh-forgejo-shim")),
        )?;
        write_executable(
            &shim_b.join("gh"),
            &crate::shim::shim_script(&fixture.root.join("gh-forgejo-shim")),
        )?;
        write_executable(&real_dir.join("gh"), "#!/bin/sh\nexit 0\n")?;

        let checks = run_checks(CheckOptions {
            config: Some(Config {
                hosts: vec!["git.example.com".to_string()],
                paths: config::PathsConfig { gh: None, fj: None },
                path: fixture.home.join("config.toml"),
            }),
            config_path: None,
            env: EnvMap::from([
                ("HOME".to_string(), fixture.home.display().to_string()),
                (
                    "PATH".to_string(),
                    format!("{}:{}", shim_a.display(), shim_b.display()),
                ),
                ("FJ_SHIM_TOKEN".to_string(), "token".to_string()),
            ]),
            bin_dir: None,
            home: Some(fixture.home.clone()),
            cwd: None,
            fallback_dirs: Some(vec![real_dir]),
            launchd_path: None,
            check_gui_path: Some(false),
            check_current_repo: Some(false),
            platform: "linux".to_string(),
        })?;

        let bd = checks
            .iter()
            .find(|check| check.name == "bd")
            .expect("bd check");
        assert_eq!(bd.status, CheckStatus::Warn);
        let duplicate = checks
            .iter()
            .find(|check| check.name == "duplicate managed gh")
            .expect("duplicate check");
        assert_eq!(duplicate.status, CheckStatus::Warn);
        assert!(!has_failures(&checks), "{checks:#?}");
        Ok(())
    }

    #[test]
    fn homebrew_gh_outside_current_path_is_reported_separately() -> Result<()> {
        let fixture = Fixture::new()?;
        let cargo_bin = fixture.home.join(".cargo").join("bin");
        let homebrew = fixture.root.join("homebrew").join("bin");
        fs::create_dir_all(&cargo_bin)?;
        fs::create_dir_all(&homebrew)?;
        write_executable(&homebrew.join("gh"), "#!/bin/sh\nexit 0\n")?;

        let checks = run_checks(CheckOptions {
            config: Some(Config {
                hosts: vec!["git.example.com".to_string()],
                paths: config::PathsConfig { gh: None, fj: None },
                path: fixture.home.join("config.toml"),
            }),
            config_path: None,
            env: EnvMap::from([
                ("HOME".to_string(), fixture.home.display().to_string()),
                ("PATH".to_string(), cargo_bin.display().to_string()),
                ("FJ_SHIM_TOKEN".to_string(), "token".to_string()),
            ]),
            bin_dir: None,
            home: Some(fixture.home.clone()),
            cwd: None,
            fallback_dirs: Some(vec![homebrew]),
            launchd_path: None,
            check_gui_path: Some(false),
            check_current_repo: Some(false),
            platform: "linux".to_string(),
        })?;

        let current_path = checks
            .iter()
            .find(|check| check.name == "current PATH")
            .expect("current PATH check");
        assert_eq!(current_path.status, CheckStatus::Fix);
        assert!(current_path.detail.contains("Codex-style restricted PATH"));
        let real_gh = checks
            .iter()
            .find(|check| check.name == "real gh")
            .expect("real gh check");
        assert_eq!(real_gh.status, CheckStatus::Ok);
        assert!(real_gh.detail.contains("common install dirs"));
        Ok(())
    }

    struct Fixture {
        root: PathBuf,
        home: PathBuf,
    }

    impl Fixture {
        fn new() -> io::Result<Self> {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gh-forgejo-shim-doctor-{}-{id}",
                std::process::id()
            ));
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
        make_executable(path)
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
