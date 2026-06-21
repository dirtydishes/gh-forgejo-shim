//! First-run setup checks for a Forgejo checkout.

use std::fs;
use std::path::{Path, PathBuf};

use crate::auth;
use crate::config::{self, EnvMap};
use crate::external;
use crate::repo::{self, RepoRef};
use crate::shim;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub repair_commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapResult {
    pub repo: Option<RepoRef>,
    pub config_path: PathBuf,
    pub shim: Option<PathBuf>,
    pub checks: Vec<BootstrapCheck>,
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
}

impl BootstrapResult {
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|check| check.ok)
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
        }
    }
}

pub fn run_bootstrap(options: BootstrapOptions) -> Result<BootstrapResult> {
    let target_bin_dir = match options.bin_dir.clone() {
        Some(bin_dir) => bin_dir,
        None => shim::default_bin_dir(options.home.as_deref())?,
    };
    let target_shim = shim::shim_path(Some(&target_bin_dir), options.home.as_deref())?;
    let repo = repo::detect_from_git(options.cwd.as_deref());
    let config_path = options
        .config_path
        .clone()
        .unwrap_or_else(|| config::config_path(options.home.as_deref()));
    let mut config = config::load_config_with_env(Some(&config_path), &options.env)?;
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

    let installed = match shim::install_shim(
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
                    shell_quote(&target_bin_dir.display().to_string())
                )],
            ));
            None
        }
    };

    checks.push(path_check(&target_bin_dir, &target_shim, &options.env));
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
        let mark = if check.ok { "ok" } else { "fix" };
        lines.push(format!("[{mark}] {}: {}", check.name, check.detail));
        repair_commands.extend(check.repair_commands.iter().cloned());
    }

    if repair_commands.is_empty() {
        lines.push(String::new());
        lines.push("setup looks ready".to_string());
    } else {
        lines.push(String::new());
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
        }
    }
}

fn path_check(bin_dir: &Path, target_shim: &Path, env: &EnvMap) -> BootstrapCheck {
    let first_gh = first_program("gh", env.get("PATH").map_or("", String::as_str));
    if first_gh.as_deref().is_some_and(|path| {
        target_shim.exists() && same_file(path, target_shim) && shim::is_managed_shim(target_shim)
    }) {
        return BootstrapCheck::new(
            "PATH",
            true,
            format!("{} is the first gh in PATH", target_shim.display()),
            Vec::<String>::new(),
        );
    }

    let export = format!("export PATH=\"{}:$PATH\"", bin_dir.display());
    let detail = if let Some(first_gh) = first_gh {
        format!(
            "gh currently resolves to {} before {}",
            first_gh.display(),
            target_shim.display()
        )
    } else if !path_contains_dir(env.get("PATH").map_or("", String::as_str), bin_dir) {
        format!("{} is not in PATH", bin_dir.display())
    } else {
        format!(
            "{} exists but gh was not found in PATH",
            target_shim.display()
        )
    };
    BootstrapCheck::new(
        "PATH",
        false,
        detail,
        [export, "gh-forgejo-shim doctor".to_string()],
    )
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
            format!("git remote add origin {}", shell_quote(&url)),
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

fn shell_quote(value: &str) -> String {
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

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        if !result.contains(&value) {
            result.push(value);
        }
    }
    result
}
