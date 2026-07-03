//! First-run setup checks for a Forgejo checkout.

use std::path::{Path, PathBuf};

use crate::auth;
use crate::config::{self, EnvMap};
use crate::external;
use crate::gui_path;
use crate::repo::{self, RepoRef};
use crate::setup::{self, SetupInspectOptions, TargetMode};
use crate::shim;
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapCheckStatus {
    Ok,
    Warn,
    Fix,
    Plan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub repair_commands: Vec<String>,
    pub status: BootstrapCheckStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapGuiPathApply {
    pub path_value: String,
    pub plist_path: PathBuf,
    pub applied: Option<bool>,
    pub apply_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapResult {
    pub repo: Option<RepoRef>,
    pub config_path: PathBuf,
    pub shim: Option<PathBuf>,
    pub checks: Vec<BootstrapCheck>,
    pub dry_run: bool,
    pub gui_path_apply: Option<BootstrapGuiPathApply>,
}

#[derive(Debug, Clone)]
pub struct BootstrapOptions {
    pub cwd: Option<PathBuf>,
    pub env: EnvMap,
    pub bin_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub force: bool,
    pub home: Option<PathBuf>,
    pub executable: PathBuf,
    pub target: TargetMode,
    pub dry_run: bool,
    pub no_gui_path: bool,
    pub launchd_path: Option<String>,
    pub platform: String,
    pub fallback_dirs: Option<Vec<PathBuf>>,
}

impl BootstrapResult {
    pub fn ok(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status != BootstrapCheckStatus::Fix)
    }

    pub fn gui_path_to_apply(&self) -> Option<&str> {
        self.gui_path_apply.as_ref().and_then(|action| {
            action
                .applied
                .is_none()
                .then_some(action.path_value.as_str())
        })
    }

    pub fn record_gui_path_apply(&mut self, applied: bool, apply_error: Option<String>) {
        let Some(action) = self.gui_path_apply.as_mut() else {
            return;
        };
        action.applied = Some(applied);
        action.apply_error = apply_error.clone();
        let Some(check) = self
            .checks
            .iter_mut()
            .find(|check| check.name == "macOS GUI PATH")
        else {
            return;
        };
        if applied {
            check.status = BootstrapCheckStatus::Ok;
            check.ok = true;
            check.detail = format!(
                "updated launchd PATH via {}; restart existing GUI apps to inherit it",
                action.plist_path.display()
            );
        } else {
            check.status = BootstrapCheckStatus::Warn;
            check.ok = true;
            check.detail = format!(
                "wrote {} but could not apply launchd PATH immediately: {}; restart existing GUI apps after the next login",
                action.plist_path.display(),
                apply_error.as_deref().unwrap_or("unknown error")
            );
        }
    }
}

impl BootstrapOptions {
    pub fn new(env: EnvMap, executable: PathBuf) -> Self {
        Self {
            cwd: None,
            env,
            bin_dir: None,
            config_path: None,
            force: false,
            home: None,
            executable,
            target: TargetMode::Current,
            dry_run: false,
            no_gui_path: false,
            launchd_path: None,
            platform: std::env::consts::OS.to_string(),
            fallback_dirs: None,
        }
    }
}

pub fn run_bootstrap(options: BootstrapOptions) -> Result<BootstrapResult> {
    let repo = repo::detect_from_git(options.cwd.as_deref());
    let config_path = options
        .config_path
        .clone()
        .unwrap_or_else(|| config::config_path(options.home.as_deref()));
    let mut config = config::load_config_with_env(Some(&config_path), &options.env)?;
    let setup_model = setup::inspect(SetupInspectOptions {
        env: options.env.clone(),
        home: options.home.clone(),
        config: Some(config.clone()),
        bin_dir: options.bin_dir.clone(),
        target: options.target,
        fallback_dirs: options.fallback_dirs.clone(),
        launchd_path: options.launchd_path.clone(),
    })?;
    let target_bin_dir = setup_model.target_bin_dir.clone();
    let target_shim = setup_model.target_shim_path.clone();
    let repo_is_github = repo
        .as_ref()
        .is_some_and(|repo| config::is_known_github_host(Some(&repo.host)));
    let mut checks = Vec::new();

    if let Some(repo) = repo.as_ref() {
        if repo_is_github {
            checks.push(BootstrapCheck::new(
                "repository",
                true,
                format!(
                    "detected GitHub repository {}/{}/{}; leaving {} out of the Forgejo allowlist",
                    repo.host, repo.owner, repo.name, repo.host
                ),
                Vec::<String>::new(),
            ));
        } else if options.dry_run {
            checks.push(BootstrapCheck::plan(
                "repository",
                format!(
                    "would allowlist {} for {}/{}",
                    repo.host, repo.owner, repo.name
                ),
            ));
        } else {
            config = config::add_host(&repo.host, Some(&config_path))?;
            checks.push(BootstrapCheck::new(
                "repository",
                true,
                format!(
                    "detected {}/{}/{} and allowlisted {}",
                    repo.host, repo.owner, repo.name, repo.host
                ),
                Vec::<String>::new(),
            ));
        }
    } else {
        checks.push(BootstrapCheck::new(
            "repository",
            false,
            "could not detect a Forgejo repository from git remotes",
            [
                "git remote add origin https://git.example.com/owner/repo.git",
                "git fetch origin",
                "git remote set-head origin -a",
                "gh-forgejo-shim config add-host git.example.com",
            ],
        ));
    }

    checks.push(real_gh_check(&setup_model, &options.platform));

    let installed = if options.dry_run {
        dry_run_shim_check(&target_bin_dir, &target_shim, options.force, &mut checks);
        None
    } else {
        match shim::install_shim(
            Some(&target_bin_dir),
            options.home.as_deref(),
            &options.executable,
            options.force,
        ) {
            Ok(path) => {
                checks.push(BootstrapCheck::new(
                    "shim",
                    true,
                    format!("installed {}", path.display()),
                    Vec::<String>::new(),
                ));
                Some(path)
            }
            Err(error) => {
                checks.push(BootstrapCheck::new(
                    "shim",
                    false,
                    error.to_string(),
                    [format!(
                        "gh-forgejo-shim install-shim --bin-dir {} --force",
                        setup::shell_quote(&target_bin_dir.display().to_string())
                    )],
                ));
                None
            }
        }
    };

    let post_install_model = if options.dry_run {
        setup_model.clone()
    } else {
        setup::inspect(SetupInspectOptions {
            env: options.env.clone(),
            home: options.home.clone(),
            config: Some(config.clone()),
            bin_dir: Some(target_bin_dir.clone()),
            target: TargetMode::Current,
            fallback_dirs: options.fallback_dirs.clone(),
            launchd_path: options.launchd_path.clone(),
        })?
    };
    checks.push(if options.dry_run {
        planned_path_check(&setup_model)
    } else {
        path_check(&post_install_model)
    });

    let gui_path_apply = gui_path_action(&setup_model, &options, &mut checks)?;

    if repo_is_github {
        checks.push(BootstrapCheck::new(
            "auth",
            true,
            "Forgejo auth is not needed for GitHub delegation",
            Vec::<String>::new(),
        ));
    } else {
        checks.push(auth_check(
            repo.as_ref(),
            &options.env,
            options.home.as_deref(),
        ));
    }
    checks.extend(origin_checks(repo.as_ref(), options.cwd.as_deref()));

    Ok(BootstrapResult {
        repo,
        config_path: config.path,
        shim: installed,
        checks,
        dry_run: options.dry_run,
        gui_path_apply,
    })
}

pub fn format_bootstrap(result: &BootstrapResult) -> String {
    let mut lines = vec!["gh-forgejo-shim bootstrap".to_string()];
    if let Some(repo) = result.repo.as_ref() {
        lines.push(format!("repo: {}/{}/{}", repo.host, repo.owner, repo.name));
    } else {
        lines.push("repo: unknown".to_string());
    }
    lines.push(format!("config: {}", result.config_path.display()));

    let mut repair_commands = Vec::new();
    for check in &result.checks {
        let mark = match check.status {
            BootstrapCheckStatus::Ok => "ok",
            BootstrapCheckStatus::Warn => "warn",
            BootstrapCheckStatus::Fix => "fix",
            BootstrapCheckStatus::Plan => "plan",
        };
        lines.push(format!("[{mark}] {}: {}", check.name, check.detail));
        repair_commands.extend(check.repair_commands.iter().cloned());
    }

    if repair_commands.is_empty() {
        lines.push(String::new());
        if result.dry_run {
            lines.push("dry run: no files were written".to_string());
        } else {
            lines.push("setup looks ready".to_string());
        }
    } else {
        lines.push(String::new());
        if result.dry_run {
            lines.push("dry run: no files were written".to_string());
        }
        lines.push("repair commands:".to_string());
        for command in dedupe(repair_commands) {
            lines.push(format!("  {command}"));
        }
    }
    lines.join("\n")
}

impl BootstrapCheck {
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
                BootstrapCheckStatus::Ok
            } else {
                BootstrapCheckStatus::Fix
            },
        }
    }

    fn warn(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok: true,
            detail: detail.into(),
            repair_commands: Vec::new(),
            status: BootstrapCheckStatus::Warn,
        }
    }

    fn plan(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok: true,
            detail: detail.into(),
            repair_commands: Vec::new(),
            status: BootstrapCheckStatus::Plan,
        }
    }
}

fn real_gh_check(model: &setup::SetupModel, platform: &str) -> BootstrapCheck {
    if let Some(real_gh) = model.real_gh.as_ref() {
        return BootstrapCheck::new(
            "real gh",
            true,
            format!("found {} via {}", real_gh.path.display(), real_gh.source),
            Vec::<String>::new(),
        );
    }
    BootstrapCheck::new(
        "real gh",
        false,
        "could not find the real GitHub CLI from config, env, current PATH, or common install dirs",
        real_gh_repair_commands(platform),
    )
}

fn dry_run_shim_check(
    target_bin_dir: &Path,
    target_shim: &Path,
    force: bool,
    checks: &mut Vec<BootstrapCheck>,
) {
    if target_shim.exists() && !shim::is_managed_shim(target_shim) && !force {
        checks.push(BootstrapCheck::new(
            "shim",
            false,
            format!(
                "{} exists and is not managed by gh-forgejo-shim; pass --force to overwrite",
                target_shim.display()
            ),
            [format!(
                "gh-forgejo-shim bootstrap --bin-dir {} --force",
                setup::shell_quote(&target_bin_dir.display().to_string())
            )],
        ));
        return;
    }
    checks.push(BootstrapCheck::plan(
        "shim",
        format!("would install managed gh shim at {}", target_shim.display()),
    ));
}

fn planned_path_check(model: &setup::SetupModel) -> BootstrapCheck {
    if model.target_shim_first_in_current_path {
        return BootstrapCheck::new(
            "current PATH",
            true,
            format!(
                "{} is already the first gh in current PATH",
                model.target_shim_path.display()
            ),
            Vec::<String>::new(),
        );
    }
    if !model.target_bin_dir_in_current_path {
        return BootstrapCheck::new(
            "current PATH",
            false,
            format!("{} is not in current PATH", model.target_bin_dir.display()),
            path_repair_commands(&model.target_bin_dir),
        );
    }
    if let Some(first_gh) = model.first_current_gh.as_ref() {
        if !target_dir_precedes_path(model, first_gh) {
            return BootstrapCheck::new(
                "current PATH",
                false,
                format!(
                    "gh currently resolves to {} before planned shim {}",
                    first_gh.display(),
                    model.target_shim_path.display()
                ),
                path_repair_commands(&model.target_bin_dir),
            );
        }
    }
    BootstrapCheck::plan(
        "current PATH",
        format!(
            "would make {} the first gh in current PATH",
            model.target_shim_path.display()
        ),
    )
}

fn path_check(model: &setup::SetupModel) -> BootstrapCheck {
    if model.target_shim_first_in_current_path {
        return BootstrapCheck::new(
            "current PATH",
            true,
            format!(
                "{} is the first gh in current PATH",
                model.target_shim_path.display()
            ),
            Vec::<String>::new(),
        );
    }

    if model.first_current_gh_is_managed {
        let first_gh = model.first_current_gh.as_ref().expect("managed gh path");
        return BootstrapCheck::warn(
            "current PATH",
            format!(
                "managed gh at {} is first before target {}; this works, but duplicate wrappers should be cleaned up later",
                first_gh.display(),
                model.target_shim_path.display()
            ),
        );
    }

    let detail = if model.codex_restricted_path {
        let real_gh = model
            .real_gh
            .as_ref()
            .map(|tool| tool.path.display().to_string())
            .unwrap_or_else(|| "a common install dir".to_string());
        format!(
            "Codex-style restricted PATH cannot resolve bare gh, but real gh exists at {real_gh}"
        )
    } else if let Some(first_gh) = model.first_current_gh.as_ref() {
        format!(
            "gh currently resolves to {} before {}",
            first_gh.display(),
            model.target_shim_path.display()
        )
    } else if !model.target_bin_dir_in_current_path {
        format!("{} is not in current PATH", model.target_bin_dir.display())
    } else {
        format!(
            "{} exists but gh was not found in PATH",
            model.target_shim_path.display()
        )
    };
    BootstrapCheck::new(
        "current PATH",
        false,
        detail,
        path_repair_commands(&model.target_bin_dir),
    )
}

fn gui_path_action(
    model: &setup::SetupModel,
    options: &BootstrapOptions,
    checks: &mut Vec<BootstrapCheck>,
) -> Result<Option<BootstrapGuiPathApply>> {
    if options.platform != "darwin" {
        return Ok(None);
    }
    if options.no_gui_path {
        checks.push(BootstrapCheck::warn(
            "macOS GUI PATH",
            "skipped by --no-gui-path",
        ));
        return Ok(None);
    }
    if model.launchd_contains_required_dirs {
        checks.push(BootstrapCheck::new(
            "macOS GUI PATH",
            true,
            "launchd PATH already exposes the shim and common package-manager dirs",
            Vec::<String>::new(),
        ));
        return Ok(None);
    }
    if options.dry_run {
        checks.push(BootstrapCheck::plan(
            "macOS GUI PATH",
            format!(
                "would set launchd PATH to {}; restart existing GUI apps after applying it",
                model.desired_launchd_path
            ),
        ));
        return Ok(None);
    }

    let result = gui_path::install_gui_path(
        Some(&model.desired_launchd_path),
        None,
        options.home.as_deref(),
        &options.env,
        false,
        None,
    )?;
    checks.push(BootstrapCheck::new(
        "macOS GUI PATH",
        true,
        format!(
            "wrote {}; applying launchd PATH next; restart existing GUI apps to inherit it",
            result.plist_path.display()
        ),
        Vec::<String>::new(),
    ));
    Ok(Some(BootstrapGuiPathApply {
        path_value: result.path_value,
        plist_path: result.plist_path,
        applied: None,
        apply_error: None,
    }))
}

fn auth_check(repo: Option<&RepoRef>, env: &EnvMap, home: Option<&Path>) -> BootstrapCheck {
    if auth::discover_fj_token(repo.map(|repo| repo.host.as_str()), env, home).is_some() {
        return BootstrapCheck::new("auth", true, "found Forgejo token", Vec::<String>::new());
    }
    let login_command = repo.map_or_else(
        || "gh-forgejo-shim auth login HOST".to_string(),
        |repo| format!("gh-forgejo-shim auth login {}", repo.host),
    );
    let import_command = repo.map_or_else(
        || "gh-forgejo-shim auth import HOST".to_string(),
        |repo| format!("gh-forgejo-shim auth import {}", repo.host),
    );
    BootstrapCheck::new(
        "auth",
        false,
        "no Forgejo token found in shim storage, env, or common fj/tea/gitea config files",
        [
            login_command,
            import_command,
            "gh-forgejo-shim doctor".to_string(),
        ],
    )
}

fn origin_checks(repo: Option<&RepoRef>, cwd: Option<&Path>) -> Vec<BootstrapCheck> {
    let mut checks = Vec::new();
    if let Some(origin_url) = external::git_output(&["remote", "get-url", "origin"], cwd) {
        if repo::parse_repo_spec(&origin_url, None).is_some() {
            checks.push(BootstrapCheck::new(
                "origin",
                true,
                format!("origin points at {origin_url}"),
                Vec::<String>::new(),
            ));
        } else {
            checks.push(BootstrapCheck::new(
                "origin",
                false,
                format!(
                    "origin exists but does not look like a Forgejo repository URL: {origin_url}"
                ),
                origin_repair_commands(repo),
            ));
        }
    } else {
        checks.push(BootstrapCheck::new(
            "origin",
            false,
            "origin remote is missing",
            origin_repair_commands(repo),
        ));
    }

    if let Some(origin_head) = external::git_output(
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
        cwd,
    ) {
        checks.push(BootstrapCheck::new(
            "origin/HEAD",
            true,
            origin_head,
            Vec::<String>::new(),
        ));
    } else {
        checks.push(BootstrapCheck::new(
            "origin/HEAD",
            false,
            "origin/HEAD is not set",
            [
                "git fetch origin",
                "git remote set-head origin -a",
                "git remote set-head origin main",
            ],
        ));
    }

    if let Some(upstream) = external::git_output(
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        cwd,
    ) {
        checks.push(BootstrapCheck::new(
            "branch upstream",
            true,
            upstream,
            Vec::<String>::new(),
        ));
    } else if let Some(branch) = external::git_output(&["branch", "--show-current"], cwd) {
        checks.push(BootstrapCheck::new(
            "branch upstream",
            false,
            format!("{branch} does not track an origin branch"),
            [format!(
                "git branch --set-upstream-to=origin/{branch} {branch}"
            )],
        ));
    } else {
        checks.push(BootstrapCheck::new(
            "branch upstream",
            false,
            "not on a named branch",
            Vec::<String>::new(),
        ));
    }

    checks
}

fn origin_repair_commands(repo: Option<&RepoRef>) -> [String; 3] {
    if let Some(repo) = repo {
        let url = format!("https://{}/{}/{}.git", repo.host, repo.owner, repo.name);
        return [
            format!("git remote add origin {}", setup::shell_quote(&url)),
            "git fetch origin".to_string(),
            "git remote set-head origin -a".to_string(),
        ];
    }
    [
        "git remote add origin https://git.example.com/owner/repo.git".to_string(),
        "git fetch origin".to_string(),
        "git remote set-head origin -a".to_string(),
    ]
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

fn target_dir_precedes_path(model: &setup::SetupModel, path: &Path) -> bool {
    let Some(candidate_dir) = path.parent() else {
        return false;
    };
    let target_index = path_dir_index(model, &model.target_bin_dir);
    let candidate_index = path_dir_index(model, candidate_dir);
    matches!((target_index, candidate_index), (Some(target), Some(candidate)) if target <= candidate)
}

fn path_dir_index(model: &setup::SetupModel, directory: &Path) -> Option<usize> {
    model
        .current_path_dirs
        .iter()
        .enumerate()
        .find_map(|(index, path_dir)| {
            (setup::same_file(path_dir, directory) || path_dir == directory).then_some(index)
        })
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
