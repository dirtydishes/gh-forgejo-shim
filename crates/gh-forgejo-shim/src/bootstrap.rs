//! First-run setup checks for a Forgejo checkout.

use std::path::{Path, PathBuf};

use crate::auth;
use crate::config::{self, Config, EnvMap};
use crate::doctor;
use crate::external;
use crate::gh_version;
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
    let config_path = options
        .config_path
        .clone()
        .unwrap_or_else(|| config::config_path(options.home.as_deref()));
    let mut config = config::load_config_with_env(Some(&config_path), &options.env)?;
    let plan = plan_repo(options.cwd.as_deref(), &options.env, &config);
    let left_to_gh = matches!(
        plan,
        Some(RepoPlan::LeaveToGh { .. } | RepoPlan::NotResolved { .. })
    );
    let github_allowlisted = matches!(plan, Some(RepoPlan::GitHubAllowlisted { .. }));
    // Auth advice names a repository's host only when bootstrap allowlists it; any other host
    // here may be GitHub's, and `auth login` would send it the Forgejo token to validate.
    let allowlisting = matches!(plan, Some(RepoPlan::Allowlist { .. }));
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
    let mut checks = Vec::new();

    // Nothing is written or installed for a gh the shim can't use (decision D3). The gh checked
    // is the one the wrapper will run once the shim is in place: with --force, not a gh at the
    // target, which the shim replaces. A dry run goes on, to show the rest of the plan.
    let fallback_dirs = options
        .fallback_dirs
        .clone()
        .unwrap_or_else(setup::default_fallback_dirs);
    let real_gh = setup::find_real_gh_after_install(
        Some(&config),
        &options.env,
        &fallback_dirs,
        options.force.then_some(target_shim.as_path()),
    );
    let gh_check = real_gh_check(real_gh.as_ref(), &options.env, &options.platform);
    if gh_check.status == BootstrapCheckStatus::Fix && !options.dry_run {
        return Ok(BootstrapResult {
            repo: None,
            config_path: config.path,
            shim: None,
            checks: vec![gh_check],
            dry_run: false,
            gui_path_apply: None,
        });
    }

    let repo = match plan {
        Some(RepoPlan::Allowlist { repo, entry }) => {
            if options.dry_run {
                checks.push(BootstrapCheck::plan(
                    "repository",
                    format!("would allowlist {entry} for {}/{}", repo.owner, repo.name),
                ));
            } else {
                config = config::add_host(&entry, Some(&config_path))?;
                let mut message = format!(
                    "detected {}/{}/{} and allowlisted {entry}",
                    repo.host, repo.owner, repo.name
                );
                // FJ_SHIM_HOSTS replaces the file's hosts, so the entry written there doesn't
                // apply in a shell that sets it.
                if options.env.contains_key("FJ_SHIM_HOSTS") {
                    message.push_str(
                        "; FJ_SHIM_HOSTS is set, and replaces the config file's hosts until you \
                         unset it or add this host to it",
                    );
                }
                checks.push(BootstrapCheck::new(
                    "repository",
                    true,
                    message,
                    Vec::<String>::new(),
                ));
            }
            Some(repo)
        }
        Some(RepoPlan::LeaveToGh { repo, notes }) => {
            let mut message = if doctor::is_github_host(&options.env, &repo.host) {
                format!(
                    "detected GitHub repository {}/{}/{}",
                    repo.host, repo.owner, repo.name
                )
            } else {
                format!(
                    "detected {}/{}/{}, which gh keeps",
                    repo.host, repo.owner, repo.name
                )
            };
            message.push_str("; ");
            message.push_str(&allowlist_clause(&config, &repo.host));
            for note in notes {
                message.push_str("; ");
                message.push_str(&note);
            }
            checks.push(BootstrapCheck::new(
                "repository",
                true,
                message,
                Vec::<String>::new(),
            ));
            Some(repo)
        }
        Some(RepoPlan::NotResolved { first, notes }) => {
            let pinned = ["GH_HOST", "GH_REPO"]
                .iter()
                .find_map(|name| {
                    options
                        .env
                        .get(*name)
                        .filter(|value| !value.is_empty())
                        .map(|value| format!(" ({name} is {value})"))
                })
                .unwrap_or_default();
            let mut message = format!(
                "gh resolves no repository here{pinned}, so the wrapper hands this checkout to \
                 real gh; {}",
                allowlist_clause(&config, &first.host)
            );
            for note in notes {
                message.push_str("; ");
                message.push_str(&note);
            }
            checks.push(BootstrapCheck::new(
                "repository",
                true,
                message,
                Vec::<String>::new(),
            ));
            Some(first)
        }
        Some(RepoPlan::Conflict {
            repo,
            hostname,
            entries,
        }) => {
            let message = if entries.len() > 1 {
                format!(
                    "the allowlist has more than one entry for {hostname}, so the wrapper routes \
                     none of them; keep one with gh-forgejo-shim config remove-host"
                )
            } else {
                format!(
                    "the allowlist's entry {} doesn't serve {}: a host with a port needs an entry \
                     with that port, and a second entry for {hostname} would leave both \
                     unrouted; replace it with gh-forgejo-shim config remove-host and config \
                     add-host",
                    entries.join(", "),
                    repo.host
                )
            };
            checks.push(BootstrapCheck::new(
                "repository",
                false,
                message,
                Vec::<String>::new(),
            ));
            Some(repo)
        }
        Some(RepoPlan::GitHubAllowlisted { repo, entry }) => {
            // The repair has to remove the entry that routes it, as written: `remove-host` with
            // gh's spelling would leave `host:port` in place. FJ_SHIM_HOSTS replaces the file's
            // hosts, so with it set the entry comes from there.
            let (repair, command) = if options.env.contains_key("FJ_SHIM_HOSTS") {
                (
                    format!(
                        "remove {entry} from FJ_SHIM_HOSTS, which replaces the config file's hosts"
                    ),
                    None,
                )
            } else {
                // Quoted: `[::1]:8443` is a glob pattern to the shell, and a matching file name
                // would make the command succeed while removing nothing.
                let command = format!(
                    "gh-forgejo-shim config remove-host {}",
                    setup::shell_quote(&entry)
                );
                (format!("run {command}"), Some(command))
            };
            checks.push(BootstrapCheck::new(
                "repository",
                false,
                format!(
                    "{} is in your gh hosts.yml and in the Forgejo allowlist as {entry}, so the \
                     wrapper sends {}/{} to Forgejo; bootstrap adds nothing and gives no auth \
                     advice for it; if it's GitHub Enterprise, {repair}",
                    repo.host, repo.owner, repo.name
                ),
                command.into_iter().collect::<Vec<_>>(),
            ));
            Some(repo)
        }
        Some(RepoPlan::Unusable { repo }) => {
            checks.push(BootstrapCheck::new(
                "repository",
                false,
                format!(
                    "{} isn't a host name the shim can use; leaving it out of the Forgejo allowlist",
                    repo.host
                ),
                Vec::<String>::new(),
            ));
            Some(repo)
        }
        None => {
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
            None
        }
    };

    checks.push(gh_check);

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

    if left_to_gh {
        checks.push(BootstrapCheck::new(
            "auth",
            true,
            "Forgejo auth is not needed for GitHub delegation",
            Vec::<String>::new(),
        ));
    } else if github_allowlisted {
        checks.push(BootstrapCheck::new(
            "auth",
            true,
            "no auth advice for a host your gh hosts.yml lists",
            Vec::<String>::new(),
        ));
    } else {
        checks.push(auth_check(
            repo.as_ref().filter(|_| allowlisting),
            &config,
            &options.env,
            options.home.as_deref(),
            &options.platform,
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

/// What bootstrap does with the checkout's repository.
enum RepoPlan {
    /// Allowlist `entry`: the wrapper routes `repo` to it, or will once it's allowlisted.
    Allowlist { repo: RepoRef, entry: String },
    /// gh resolves `repo` and real gh keeps it. `notes` say what was passed over and why, each a
    /// clause of the message.
    LeaveToGh { repo: RepoRef, notes: Vec<String> },
    /// gh resolves no repository here, so the wrapper hands every command to real gh: `GH_HOST`
    /// or `GH_REPO` points elsewhere, or gh can't read `first`, the first remote.
    NotResolved { first: RepoRef, notes: Vec<String> },
    /// `entries` share `hostname` and don't serve this checkout: two or more route none of
    /// them, and one has another port. Adding an entry wouldn't help.
    Conflict {
        repo: RepoRef,
        hostname: String,
        entries: Vec<String>,
    },
    /// `repo`'s host is GitHub's (in the user's gh hosts.yml, say) yet the allowlist routes it:
    /// bootstrap adds nothing and gives no auth advice, which would send it the Forgejo token.
    /// `entry` is the allowlist entry that routes it, as written there (`host:port`, say).
    GitHubAllowlisted { repo: RepoRef, entry: String },
    /// `repo`'s host isn't a host name the shim can use.
    Unusable { repo: RepoRef },
}

/// Asks the resolver itself which host, if any, to allowlist. The candidate is the repository gh
/// resolves, or else the first remote, whichever isn't GitHub's (`doctor::is_github_host`, which
/// counts GitHub Enterprise hosts from the user's hosts.yml). Bootstrap allowlists it only if
/// the wrapper would then route this checkout to it: whether it does depends on what else gh
/// knows, such as a hosts.yml that names github.com.
fn plan_repo(cwd: Option<&Path>, env: &EnvMap, config: &Config) -> Option<RepoPlan> {
    let found = doctor::current_repo(cwd, env, config);
    let first_remote = repo::detect_from_git(cwd);
    // What gh acts on here, if it resolves anything itself; doctor's first-remote fallback is
    // not that, and with GH_HOST or GH_REPO pointing elsewhere there's nothing at all.
    let resolved = found
        .as_ref()
        .filter(|found| found.resolved_by_gh)
        .map(|found| found.repo.clone());
    let first = found.map(|found| found.repo).or(first_remote.clone())?;
    let leave = |notes: Vec<String>| match resolved.clone() {
        Some(repo) => RepoPlan::LeaveToGh { repo, notes },
        None => RepoPlan::NotResolved {
            first: first.clone(),
            notes,
        },
    };
    // Already routed: write the entry that routes it. gh drops a remote's port, and its spelling
    // beside an entry with one would leave two entries for one hostname, which routes neither.
    if let Some(repo) = resolved.as_ref() {
        if let Some(entry) = doctor::allowlist_entry(config, &repo.host) {
            // A GitHub host stays out of bootstrap's hands even when the user allowlisted it.
            if doctor::is_github_host(env, &repo.host) {
                return Some(RepoPlan::GitHubAllowlisted {
                    repo: repo.clone(),
                    entry,
                });
            }
            return Some(RepoPlan::Allowlist {
                repo: repo.clone(),
                entry,
            });
        }
    }
    let candidates = [resolved.clone(), first_remote]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let mut notes = Vec::new();
    // Nothing tells a GitHub Enterprise server in hosts.yml from a Forgejo one listed there, so
    // such a host is passed over, and bootstrap says so. No ready command: for GHES, it would
    // send the Forgejo token there.
    if let Some(listed) = candidates.iter().find(|repo| {
        !config::is_known_github_host(Some(&repo.host)) && doctor::in_gh_hosts_yml(env, &repo.host)
    }) {
        notes.push(format!(
            "{} is in your gh hosts.yml, so bootstrap takes it for GitHub Enterprise and leaves \
             it out; if it's a Forgejo server, allowlist it with gh-forgejo-shim config add-host \
             HOST",
            listed.host
        ));
    }
    let Some(candidate) = candidates
        .into_iter()
        .find(|repo| !doctor::is_github_host(env, &repo.host))
    else {
        return Some(leave(notes));
    };
    let Some(entry) = config::normalized_forgejo_host(&candidate.host) else {
        return Some(if candidate.host == first.host && resolved.is_none() {
            RepoPlan::Unusable { repo: candidate }
        } else {
            leave(notes)
        });
    };
    // gh drops a remote's port; the entry keeps the one the remote's web URL names (jko.41).
    let entry = doctor::entry_with_web_port(cwd, &candidate).unwrap_or(entry);
    let hostname = repo::gh_hostname(&candidate.host);
    let allowlisted = doctor::allowlist_entry(config, &candidate.host);
    let sharing = config
        .hosts
        .iter()
        .filter(|listed| repo::gh_hostname(listed) == hostname)
        .cloned()
        .collect::<Vec<_>>();
    if allowlisted.is_none() && !sharing.is_empty() {
        return Some(RepoPlan::Conflict {
            repo: candidate,
            hostname,
            entries: sharing,
        });
    }
    let mut trial = config.clone();
    if allowlisted.is_none() {
        trial.hosts.push(entry.clone());
    }
    // Only a repository gh itself resolves counts: the wrapper never routes doctor's fallback to
    // the first remote.
    let routed = doctor::current_repo(cwd, env, &trial)
        .filter(|trial| trial.resolved_by_gh)
        .map(|trial| trial.repo)
        .filter(|repo| {
            doctor::routes_to_forgejo(&trial, &repo.host)
                && repo::gh_hostname(&repo.host) == hostname
        });
    if let Some(repo) = routed {
        return Some(RepoPlan::Allowlist {
            repo,
            entry: allowlisted.unwrap_or(entry),
        });
    }
    // An allowlisted host needs no hint: there's nothing to add.
    if allowlisted.is_none() {
        if gh_reads_a_remote_on(cwd, env, config, &hostname) {
            if let Some(repo) = resolved.as_ref().filter(|repo| repo.host != candidate.host) {
                notes.push(format!(
                    "{} was left out too, because gh, and so the wrapper, acts on {}/{}/{} here",
                    candidate.host, repo.host, repo.owner, repo.name
                ));
            }
            notes.push(format!(
                "to allowlist it for other checkouts, run gh-forgejo-shim config add-host {}",
                setup::shell_quote(&entry)
            ));
        } else {
            // The first remote as git spells it, which gh may not: an SSH alias, `www.`, or a
            // path gh can't read. A command naming it could name a host gh never uses.
            notes.push(format!(
                "{} isn't a host gh reads from this checkout's remotes (an SSH alias, `www.` or a \
                 path gh can't parse, say), so bootstrap leaves it out; to allowlist a Forgejo \
                 server, run gh-forgejo-shim config add-host HOST with the host gh uses",
                candidate.host
            ));
        }
    }
    Some(leave(notes))
}

/// Whether gh reads a remote on `hostname` in this checkout, as it spells remotes: `GH_HOST`
/// makes gh keep only the remotes on that host, so a hit means gh sees a remote spelled so.
fn gh_reads_a_remote_on(cwd: Option<&Path>, env: &EnvMap, config: &Config, hostname: &str) -> bool {
    // GH_REPO would win over the remotes, so it goes.
    let mut env = env.clone();
    env.remove("GH_REPO");
    env.insert("GH_HOST".to_string(), hostname.to_string());
    doctor::current_repo(cwd, &env, config)
        .is_some_and(|found| repo::gh_hostname(&found.repo.host) == hostname)
}

/// What bootstrap says about `host` and the allowlist when it adds nothing: an entry for that
/// hostname may already be there, from the config file or `FJ_SHIM_HOSTS`.
fn allowlist_clause(config: &Config, host: &str) -> String {
    let hostname = repo::gh_hostname(host);
    if config
        .hosts
        .iter()
        .any(|listed| repo::gh_hostname(listed) == hostname)
    {
        format!("{host} is already in the Forgejo allowlist, and bootstrap leaves it as it is")
    } else {
        format!("leaving {host} out of the Forgejo allowlist")
    }
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

/// Whether real gh is there and new enough ([`gh_version::MINIMUM`]).
fn real_gh_check(
    real_gh: Option<&setup::ToolPath>,
    env: &EnvMap,
    platform: &str,
) -> BootstrapCheck {
    let check = gh_version::check(real_gh, env, platform);
    match check.status {
        gh_version::Status::Supported => {
            BootstrapCheck::new("real gh", true, check.detail, Vec::<String>::new())
        }
        gh_version::Status::AllowedUnknown => BootstrapCheck::warn("real gh", check.detail),
        gh_version::Status::Refused => {
            BootstrapCheck::new("real gh", false, check.detail, check.repair_commands)
        }
    }
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

fn auth_check(
    repo: Option<&RepoRef>,
    config: &Config,
    env: &EnvMap,
    home: Option<&Path>,
    platform: &str,
) -> BootstrapCheck {
    let host = repo
        .map(|repo| repo.host.as_str())
        .or(match config.hosts.as_slice() {
            [host] => Some(host.as_str()),
            _ => None,
        });
    if host
        .and_then(|host| auth::resolve_token(host, config, env, home, Some(platform)))
        .is_some()
    {
        return BootstrapCheck::new("auth", true, "found Forgejo token", Vec::<String>::new());
    }
    let login_command = repo.map_or_else(
        || "gh-forgejo-shim auth login HOST".to_string(),
        |repo| {
            format!(
                "gh-forgejo-shim auth login {}",
                setup::shell_quote(&repo.host)
            )
        },
    );
    let import_command = repo.map_or_else(
        || "gh-forgejo-shim auth import --from fj --hostname HOST".to_string(),
        |repo| {
            format!(
                "gh-forgejo-shim auth import --from fj --hostname {}",
                setup::shell_quote(&repo.host)
            )
        },
    );
    BootstrapCheck::new(
        "auth",
        false,
        "no Forgejo token found in shim storage or configured environment variables; use auth import --from fj or --from tea",
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
