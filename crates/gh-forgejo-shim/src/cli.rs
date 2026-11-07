//! Command-line scaffold shared by the Rust binaries.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::config::{self, EnvMap};
use crate::routing;
use crate::{
    auth, bootstrap, codex_smoke, doctor, external, gh_version, git_recorder, gui_path, setup,
    shim, telemetry, trace_summary, Result, ShimError, VERSION,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryName {
    GhForgejoShim,
    Gfj,
}

impl BinaryName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GhForgejoShim => "gh-forgejo-shim",
            Self::Gfj => "gfj",
        }
    }
}

pub fn run_from_env(binary: BinaryName) -> i32 {
    let mut args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == OsStr::new("gh")) {
        args.remove(0);
        return external::exit_like_child(routing::run_gh_from_env(args));
    }
    // The launcher's browser helper, which a launched gh's GH_BROWSER names (launcher spec §9).
    // Hidden: it isn't in help or completion.
    #[cfg(unix)]
    if args
        .first()
        .is_some_and(|arg| arg == OsStr::new(crate::launcher::browser::SUBCOMMAND))
    {
        return crate::launcher::browser::run(&args[1..]);
    }
    let string_args = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if matches!(
        string_args.get(0..3),
        Some(prefix) if prefix == ["trace", "git-recorder", "run"]
    ) {
        return match run_trace_git_recorder_run(&string_args[3..]) {
            Ok(code) => external::exit_like_child(code),
            Err(error) => {
                eprintln!("gh-forgejo-shim: {error}");
                1
            }
        };
    }

    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut runtime = SystemRuntime::new();
    run_with_runtime(
        binary,
        args,
        &mut stdout.lock(),
        &mut stderr.lock(),
        &mut runtime,
    )
}

pub fn run<I>(binary: BinaryName, args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let mut runtime = SystemRuntime::new();
    run_with_runtime(binary, args, stdout, stderr, &mut runtime)
}

fn run_with_runtime<I>(
    binary: BinaryName,
    args: I,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    runtime: &mut dyn Runtime,
) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    let result = if args.first().is_some_and(|arg| arg == "gh") {
        Ok(routing::run_gh_captured(
            args[1..].to_vec(),
            std::env::vars().collect(),
            std::env::current_dir().ok().as_deref(),
            stdout,
            stderr,
        ))
    } else if args.is_empty() || is_one_of(&args, &["--help", "-h", "help"]) {
        print_help(binary, stdout)
    } else if is_one_of(&args, &["--version", "-V", "version"]) {
        print_version(stdout)
    } else if args.first().is_some_and(|arg| arg == "install-shim") {
        run_install_shim(&args[1..], stdout, runtime).and_then(|code| notice(code, stdout, runtime))
    } else if args.first().is_some_and(|arg| arg == "uninstall-shim") {
        run_uninstall_shim(&args[1..], stdout, runtime)
    } else if args.first().is_some_and(|arg| arg == "install-gui-path") {
        run_install_gui_path(&args[1..], stdout, stderr, runtime)
    } else if args.first().is_some_and(|arg| arg == "uninstall-gui-path") {
        run_uninstall_gui_path(&args[1..], stdout, stderr, runtime)
    } else if args.first().is_some_and(|arg| arg == "doctor") {
        run_doctor(&args[1..], stdout, runtime).and_then(|code| notice(code, stdout, runtime))
    } else if args.first().is_some_and(|arg| arg == "bootstrap") {
        run_bootstrap(&args[1..], stdout, runtime).and_then(|code| notice(code, stdout, runtime))
    } else if args.first().is_some_and(|arg| arg == "telemetry") {
        run_telemetry(&args[1..], stdout, runtime)
    } else if args.first().is_some_and(|arg| arg == "trace") {
        run_trace(&args[1..], stdout, runtime)
    } else if args.first().is_some_and(|arg| arg == "config") {
        run_config(&args[1..], stdout, runtime)
    } else if args.first().is_some_and(|arg| arg == "auth") {
        run_auth(&args[1..], stdout, runtime)
    } else {
        print_pending(binary, stderr)
    };

    match result {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(stderr, "gh-forgejo-shim: {error}");
            1
        }
    }
}

trait Runtime {
    fn env(&self) -> &EnvMap;
    fn home(&self) -> Option<&Path>;
    fn current_exe(&self) -> &Path;
    fn platform(&self) -> &str;
    fn prompt_host(&mut self, prompt: &str) -> Result<String>;
    fn prompt_token(&mut self, prompt: &str) -> Result<String>;
    fn validate_token(&mut self, host: &str, token: &str) -> Result<Option<String>>;
    fn apply_gui_path(&mut self, path_value: &str) -> (bool, Option<String>);
    fn current_launchd_path(&self) -> Option<String>;
}

struct SystemRuntime {
    env: EnvMap,
    home: Option<PathBuf>,
    platform: &'static str,
    current_exe: PathBuf,
}

impl SystemRuntime {
    fn new() -> Self {
        let env = config::current_env();
        let home = env
            .get("HOME")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self {
            env,
            home,
            platform: current_platform(),
            current_exe: std::env::current_exe()
                .unwrap_or_else(|_| PathBuf::from("gh-forgejo-shim")),
        }
    }
}

impl Runtime for SystemRuntime {
    fn env(&self) -> &EnvMap {
        &self.env
    }

    fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    fn current_exe(&self) -> &Path {
        &self.current_exe
    }

    fn platform(&self) -> &str {
        self.platform
    }

    fn prompt_host(&mut self, prompt: &str) -> Result<String> {
        print!("{prompt}");
        io::stdout()
            .flush()
            .map_err(|error| ShimError::new(format!("could not write prompt: {error}")))?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|error| ShimError::new(format!("could not read host: {error}")))?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    fn prompt_token(&mut self, prompt: &str) -> Result<String> {
        rpassword::prompt_password(prompt)
            .map_err(|error| ShimError::new(format!("could not read token: {error}")))
    }

    fn validate_token(&mut self, host: &str, token: &str) -> Result<Option<String>> {
        auth::validate_token(host, token)
    }

    fn apply_gui_path(&mut self, path_value: &str) -> (bool, Option<String>) {
        gui_path::apply_gui_path(path_value)
    }

    fn current_launchd_path(&self) -> Option<String> {
        gui_path::current_launchd_path()
    }
}

fn current_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::consts::OS
    }
}

fn is_one_of(args: &[String], values: &[&str]) -> bool {
    args.len() == 1 && values.contains(&args[0].as_str())
}

fn print_help(binary: BinaryName, stdout: &mut dyn Write) -> Result<i32> {
    let name = binary.as_str();
    writeln!(stdout, "{name} {VERSION}")?;
    writeln!(stdout)?;
    writeln!(
        stdout,
        "Rust runtime for gh-forgejo-shim management commands."
    )?;
    writeln!(stdout)?;
    writeln!(stdout, "USAGE:")?;
    writeln!(stdout, "    {name} [--help]")?;
    writeln!(stdout, "    {name} [--version]")?;
    writeln!(stdout, "    {name} version")?;
    writeln!(stdout, "    {name} install-shim [--bin-dir DIR] [--force]")?;
    writeln!(stdout, "    {name} uninstall-shim [--bin-dir DIR]")?;
    writeln!(
        stdout,
        "    {name} install-gui-path [--path PATH] [--no-apply]"
    )?;
    writeln!(stdout, "    {name} uninstall-gui-path")?;
    writeln!(
        stdout,
        "    {name} bootstrap [--target current|user-local] [--bin-dir DIR] [--force] [--no-gui-path] [--dry-run]"
    )?;
    writeln!(stdout, "    {name} doctor")?;
    writeln!(stdout, "    {name} trace <summarize|smoke|git-recorder>")?;
    writeln!(stdout, "    {name} config <add-host|remove-host|list>")?;
    writeln!(stdout, "    {name} auth <login|import|status|logout>")?;
    writeln!(stdout, "    {name} telemetry <on|off|status|show>")?;
    writeln!(stdout)?;
    writeln!(stdout, "STATUS:")?;
    writeln!(
        stdout,
        "    native Rust runtime for managed gh dispatch, bootstrap, doctor, config, auth, shim lifecycle, trace tools, and Forgejo-routed command compatibility."
    )?;
    writeln!(stdout)?;
    writeln!(stdout, "COMMANDS:")?;
    writeln!(stdout, "    auth       manage native Forgejo auth")?;
    writeln!(
        stdout,
        "    bootstrap  install the shim and print setup repairs"
    )?;
    writeln!(stdout, "    config     manage persistent configuration")?;
    writeln!(stdout, "    doctor     check shim configuration")?;
    writeln!(stdout, "    help       print help")?;
    writeln!(
        stdout,
        "    install-shim    install the user-local gh wrapper"
    )?;
    writeln!(
        stdout,
        "    install-gui-path    make macOS GUI apps inherit the shim PATH"
    )?;
    writeln!(
        stdout,
        "    uninstall-shim  remove the user-local gh wrapper"
    )?;
    writeln!(
        stdout,
        "    uninstall-gui-path  remove the macOS GUI PATH LaunchAgent"
    )?;
    writeln!(
        stdout,
        "    telemetry  the anonymous daily usage report: on, off, status, show"
    )?;
    writeln!(stdout, "    version    print version")?;
    writeln!(
        stdout,
        "    trace      capture and summarize Codex gh/git diagnostics"
    )?;
    Ok(0)
}

fn print_version(stdout: &mut dyn Write) -> Result<i32> {
    writeln!(stdout, "{VERSION}")?;
    Ok(0)
}

fn print_pending(binary: BinaryName, stderr: &mut dyn Write) -> Result<i32> {
    writeln!(
        stderr,
        "{}: unsupported management command.",
        binary.as_str()
    )?;
    writeln!(
        stderr,
        "Run `{} --help` for supported commands. Forgejo-routed `gh` compatibility is available through the managed `gh` wrapper.",
        binary.as_str()
    )?;
    Ok(2)
}

fn run_install_shim(args: &[String], stdout: &mut dyn Write, runtime: &dyn Runtime) -> Result<i32> {
    let mut bin_dir_arg = None;
    let mut force = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--force" => {
                force = true;
                index += 1;
            }
            "--bin-dir" => {
                let Some(value) = args.get(index + 1) else {
                    return Err(ShimError::new(
                        "usage: gh-forgejo-shim install-shim [--bin-dir DIR] [--force]",
                    ));
                };
                bin_dir_arg = Some(value.as_str());
                index += 2;
            }
            value if value.starts_with("--bin-dir=") => {
                bin_dir_arg = Some(value.trim_start_matches("--bin-dir="));
                index += 1;
            }
            _ => {
                return Err(ShimError::new(
                    "usage: gh-forgejo-shim install-shim [--bin-dir DIR] [--force]",
                ));
            }
        }
    }

    let bin_dir = resolve_optional_path(bin_dir_arg, runtime.home())?;
    // The shim delegates to real gh, so it isn't installed without one new enough (decision D3):
    // the one the wrapper will run once installed. With --force that isn't a gh at the target,
    // which the shim replaces; without it, install leaves an unmanaged gh there alone.
    let config =
        config::load_config_with_env(Some(&config::config_path(runtime.home())), runtime.env())?;
    let target = shim::shim_path(bin_dir.as_deref(), runtime.home())?;
    let real_gh = setup::find_real_gh_after_install(
        Some(&config),
        runtime.env(),
        &setup::default_fallback_dirs(),
        force.then_some(target.as_path()),
    );
    let gh = gh_version::check(real_gh.as_ref(), runtime.env(), runtime.platform());
    match gh.status {
        gh_version::Status::Supported => {}
        gh_version::Status::AllowedUnknown => writeln!(stdout, "warning: real gh: {}", gh.detail)?,
        gh_version::Status::Refused => {
            let mut message = format!("not installing the gh shim: real gh: {}", gh.detail);
            if !gh.repair_commands.is_empty() {
                message.push_str("\nrepair commands:");
                for command in &gh.repair_commands {
                    message.push_str("\n  ");
                    message.push_str(command);
                }
            }
            return Err(ShimError::new(message));
        }
    }
    let path = shim::install_shim(
        bin_dir.as_deref(),
        runtime.home(),
        runtime.current_exe(),
        force,
    )?;
    writeln!(stdout, "installed gh shim at {}", path.display())?;
    Ok(0)
}

fn run_uninstall_shim(
    args: &[String],
    stdout: &mut dyn Write,
    runtime: &dyn Runtime,
) -> Result<i32> {
    let mut bin_dir_arg = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--bin-dir" => {
                let Some(value) = args.get(index + 1) else {
                    return Err(ShimError::new(
                        "usage: gh-forgejo-shim uninstall-shim [--bin-dir DIR]",
                    ));
                };
                bin_dir_arg = Some(value.as_str());
                index += 2;
            }
            value if value.starts_with("--bin-dir=") => {
                bin_dir_arg = Some(value.trim_start_matches("--bin-dir="));
                index += 1;
            }
            _ => {
                return Err(ShimError::new(
                    "usage: gh-forgejo-shim uninstall-shim [--bin-dir DIR]",
                ));
            }
        }
    }

    let paths = match resolve_optional_path(bin_dir_arg, runtime.home())? {
        Some(bin_dir) => vec![shim::uninstall_shim(Some(&bin_dir), runtime.home())?],
        None => {
            shim::uninstall_default_shims(runtime.home(), &setup::current_path_dirs(runtime.env()))?
        }
    };
    if paths.is_empty() {
        writeln!(
            stdout,
            "no gh shim found in ~/.local/bin or in PATH directories under HOME"
        )?;
    }
    for path in paths {
        writeln!(stdout, "removed gh shim at {}", path.display())?;
    }
    Ok(0)
}

fn run_install_gui_path(
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    runtime: &mut dyn Runtime,
) -> Result<i32> {
    if runtime.platform() != "darwin" {
        writeln!(
            stderr,
            "gh-forgejo-shim: install-gui-path is only supported on macOS"
        )?;
        return Ok(1);
    }

    let mut path_value = None::<String>;
    let mut no_apply = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--path" => {
                let Some(value) = args.get(index + 1) else {
                    return Err(ShimError::new(
                        "usage: gh-forgejo-shim install-gui-path [--path PATH] [--no-apply]",
                    ));
                };
                path_value = Some(value.clone());
                index += 2;
            }
            value if value.starts_with("--path=") => {
                path_value = Some(value.trim_start_matches("--path=").to_string());
                index += 1;
            }
            "--no-apply" => {
                no_apply = true;
                index += 1;
            }
            _ => {
                return Err(ShimError::new(
                    "usage: gh-forgejo-shim install-gui-path [--path PATH] [--no-apply]",
                ));
            }
        }
    }

    let mut result = gui_path::install_gui_path(
        path_value.as_deref(),
        None,
        runtime.home(),
        runtime.env(),
        false,
        None,
    )?;

    if !no_apply {
        let (applied, apply_error) = runtime.apply_gui_path(&result.path_value);
        result.applied = applied;
        result.apply_error = apply_error;
    }

    writeln!(
        stdout,
        "installed macOS GUI PATH LaunchAgent at {}",
        result.plist_path.display()
    )?;
    writeln!(stdout, "PATH={}", result.path_value)?;
    if no_apply {
        writeln!(
            stdout,
            "restart your login session or load the LaunchAgent before reopening GUI apps"
        )?;
    } else if result.applied {
        writeln!(
            stdout,
            "applied PATH to the current launchd user session; restart GUI apps to inherit it"
        )?;
    } else {
        writeln!(
            stderr,
            "warning: could not apply PATH immediately: {}",
            result.apply_error.as_deref().unwrap_or("unknown error")
        )?;
        writeln!(stdout, "the LaunchAgent will apply PATH at the next login")?;
    }
    Ok(0)
}

fn run_uninstall_gui_path(
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    runtime: &dyn Runtime,
) -> Result<i32> {
    if !args.is_empty() {
        return Err(ShimError::new("usage: gh-forgejo-shim uninstall-gui-path"));
    }
    if runtime.platform() != "darwin" {
        writeln!(
            stderr,
            "gh-forgejo-shim: uninstall-gui-path is only supported on macOS"
        )?;
        return Ok(1);
    }

    let path = gui_path::uninstall_gui_path(None, runtime.home())?;
    writeln!(
        stdout,
        "removed macOS GUI PATH LaunchAgent at {}",
        path.display()
    )?;
    writeln!(
        stdout,
        "restart your login session to return GUI apps to the default launchd PATH"
    )?;
    Ok(0)
}

fn resolve_optional_path(value: Option<&str>, home: Option<&Path>) -> Result<Option<PathBuf>> {
    value.map(|value| expand_user_path(value, home)).transpose()
}

fn expand_user_path(value: &str, home: Option<&Path>) -> Result<PathBuf> {
    if value == "~" {
        return home
            .map(Path::to_path_buf)
            .ok_or_else(|| ShimError::new("HOME is required to expand ~"));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        let home = home.ok_or_else(|| ShimError::new("HOME is required to expand ~"))?;
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(value))
}

fn run_config(args: &[String], stdout: &mut dyn Write, runtime: &dyn Runtime) -> Result<i32> {
    let path = config::config_path(runtime.home());
    match args {
        [command, host] if command == "add-host" => {
            forgejo_host_arg(host)?;
            let config = config::add_host(host, Some(&path))?;
            writeln!(stdout, "added Forgejo host: {host}")?;
            print_hosts(&config.hosts, stdout)?;
            Ok(0)
        }
        [command, host] if command == "remove-host" => {
            let config = config::remove_host(host, Some(&path))?;
            writeln!(stdout, "removed Forgejo host: {host}")?;
            print_hosts(&config.hosts, stdout)?;
            Ok(0)
        }
        [command] if command == "list" => {
            let config = config::load_config_with_env(Some(&path), runtime.env())?;
            print_hosts(&config.hosts, stdout)?;
            Ok(0)
        }
        _ => Err(ShimError::new(
            "usage: gh-forgejo-shim config <add-host HOST|remove-host HOST|list>",
        )),
    }
}

fn print_hosts(hosts: &[String], stdout: &mut dyn Write) -> Result<()> {
    if hosts.is_empty() {
        writeln!(stdout, "no Forgejo hosts configured")?;
        return Ok(());
    }
    for host in hosts {
        writeln!(stdout, "{host}")?;
    }
    Ok(())
}

fn run_doctor(args: &[String], stdout: &mut dyn Write, runtime: &dyn Runtime) -> Result<i32> {
    if !args.is_empty() {
        return Err(ShimError::new("usage: gh-forgejo-shim doctor"));
    }
    let checks = doctor::run_checks(doctor::CheckOptions {
        config: None,
        config_path: Some(config::config_path(runtime.home())),
        env: runtime.env().clone(),
        bin_dir: None,
        home: runtime.home().map(Path::to_path_buf),
        cwd: std::env::current_dir().ok(),
        fallback_dirs: None,
        launchd_path: None,
        check_gui_path: None,
        check_current_repo: None,
        check_token_scopes: None,
        platform: runtime.platform().to_string(),
    })?;
    writeln!(stdout, "{}", doctor::format_checks(&checks))?;
    Ok(if doctor::has_failures(&checks) { 1 } else { 0 })
}

fn run_bootstrap(
    args: &[String],
    stdout: &mut dyn Write,
    runtime: &mut dyn Runtime,
) -> Result<i32> {
    let mut bin_dir_arg = None;
    let mut force = false;
    let mut target = setup::TargetMode::Current;
    let mut no_gui_path = false;
    let mut dry_run = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--force" => {
                force = true;
                index += 1;
            }
            "--dry-run" => {
                dry_run = true;
                index += 1;
            }
            "--no-gui-path" => {
                no_gui_path = true;
                index += 1;
            }
            "--target" => {
                let Some(value) = args.get(index + 1) else {
                    return Err(ShimError::new(
                        "usage: gh-forgejo-shim bootstrap [--target current|user-local] [--bin-dir DIR] [--force] [--no-gui-path] [--dry-run]",
                    ));
                };
                target = parse_bootstrap_target(value)?;
                index += 2;
            }
            value if value.starts_with("--target=") => {
                target = parse_bootstrap_target(value.trim_start_matches("--target="))?;
                index += 1;
            }
            "--bin-dir" => {
                let Some(value) = args.get(index + 1) else {
                    return Err(ShimError::new(
                        "usage: gh-forgejo-shim bootstrap [--target current|user-local] [--bin-dir DIR] [--force] [--no-gui-path] [--dry-run]",
                    ));
                };
                bin_dir_arg = Some(value.as_str());
                index += 2;
            }
            value if value.starts_with("--bin-dir=") => {
                bin_dir_arg = Some(value.trim_start_matches("--bin-dir="));
                index += 1;
            }
            _ => {
                return Err(ShimError::new(
                    "usage: gh-forgejo-shim bootstrap [--target current|user-local] [--bin-dir DIR] [--force] [--no-gui-path] [--dry-run]",
                ));
            }
        }
    }

    let bin_dir = resolve_optional_path(bin_dir_arg, runtime.home())?;
    let mut result = bootstrap::run_bootstrap(bootstrap::BootstrapOptions {
        cwd: std::env::current_dir().ok(),
        env: runtime.env().clone(),
        bin_dir,
        config_path: Some(config::config_path(runtime.home())),
        force,
        home: runtime.home().map(Path::to_path_buf),
        executable: shim_command_executable(runtime.current_exe()),
        target,
        dry_run,
        no_gui_path,
        launchd_path: (runtime.platform() == "darwin")
            .then(|| runtime.current_launchd_path())
            .flatten(),
        platform: runtime.platform().to_string(),
        fallback_dirs: None,
    })?;

    if let Some(path_value) = result.gui_path_to_apply().map(ToOwned::to_owned) {
        let (applied, apply_error) = runtime.apply_gui_path(&path_value);
        result.record_gui_path_apply(applied, apply_error);
    }

    writeln!(stdout, "{}", bootstrap::format_bootstrap(&result))?;
    Ok(if result.ok() { 0 } else { 1 })
}

/// The usage-report notice, the first time a setup command runs with reports on. Never from the gh
/// wrapper, whose output agents read.
fn notice(code: i32, stdout: &mut dyn Write, runtime: &dyn Runtime) -> Result<i32> {
    telemetry::notice(runtime.env(), stdout)?;
    Ok(code)
}

fn run_telemetry(args: &[String], stdout: &mut dyn Write, runtime: &dyn Runtime) -> Result<i32> {
    let env = runtime.env();
    let get = |name: &str| env.get(name).cloned();
    let status = |stdout: &mut dyn Write| -> Result<()> {
        match telemetry::status(&get) {
            telemetry::Status::On(url) => writeln!(
                stdout,
                "on: one anonymous usage report a day to {url}; `gfj telemetry show` prints it"
            )?,
            telemetry::Status::Off(reason) => writeln!(stdout, "off: {reason}")?,
        }
        Ok(())
    };
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    match words.as_slice() {
        ["on"] | ["off"] => {
            config::set_telemetry(&config::config_path(runtime.home()), words[0] == "on")?;
            status(stdout)?;
        }
        ["status"] => status(stdout)?,
        ["show"] => writeln!(stdout, "{}", telemetry::show(env))?,
        // The wrapper's background sender; not in help.
        ["send", "--caller", caller] => telemetry::send(env, caller.parse().ok()),
        _ => {
            return Err(ShimError::new(
                "usage: gh-forgejo-shim telemetry <on|off|status|show>",
            ))
        }
    }
    Ok(0)
}

fn parse_bootstrap_target(value: &str) -> Result<setup::TargetMode> {
    match value {
        "current" => Ok(setup::TargetMode::Current),
        "user-local" => Ok(setup::TargetMode::UserLocal),
        _ => Err(ShimError::new(
            "usage: gh-forgejo-shim bootstrap [--target current|user-local] [--bin-dir DIR] [--force] [--no-gui-path] [--dry-run]",
        )),
    }
}

fn shim_command_executable(current_exe: &Path) -> PathBuf {
    if current_exe.file_name().and_then(|name| name.to_str()) == Some(BinaryName::Gfj.as_str()) {
        if let Some(parent) = current_exe.parent() {
            return parent.join(BinaryName::GhForgejoShim.as_str());
        }
    }
    current_exe.to_path_buf()
}

fn run_trace(args: &[String], stdout: &mut dyn Write, runtime: &dyn Runtime) -> Result<i32> {
    match args {
        [command, trace_path] if command == "summarize" => {
            let path = expand_user_path(trace_path, runtime.home())?;
            let summary = trace_summary::summarize_trace_file(&path, trace_summary::SLOW_CALL_MS)?;
            writeln!(stdout, "{}", trace_summary::format_trace_summary(&summary))?;
            Ok(0)
        }
        [command] if command == "smoke" => run_trace_smoke(&[], stdout, runtime),
        [command, rest @ ..] if command == "smoke" => run_trace_smoke(rest, stdout, runtime),
        [command, rest @ ..] if command == "git-recorder" => {
            run_trace_git_recorder(rest, stdout, runtime)
        }
        _ => Err(ShimError::new(
            "usage: gh-forgejo-shim trace <summarize TRACE_PATH|smoke [--cwd CWD]|git-recorder <create|remove|run>>",
        )),
    }
}

fn run_trace_smoke(args: &[String], stdout: &mut dyn Write, runtime: &dyn Runtime) -> Result<i32> {
    let mut cwd = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--cwd" => {
                let Some(value) = args.get(index + 1) else {
                    return Err(ShimError::new(
                        "usage: gh-forgejo-shim trace smoke [--cwd CWD]",
                    ));
                };
                cwd = Some(expand_user_path(value, runtime.home())?);
                index += 2;
            }
            value if value.starts_with("--cwd=") => {
                cwd = Some(expand_user_path(
                    value.trim_start_matches("--cwd="),
                    runtime.home(),
                )?);
                index += 1;
            }
            _ => {
                return Err(ShimError::new(
                    "usage: gh-forgejo-shim trace smoke [--cwd CWD]",
                ))
            }
        }
    }

    let results = codex_smoke::run_codex_smoke(cwd.as_deref(), &env_hash(runtime.env()));
    writeln!(stdout, "{}", codex_smoke::format_codex_smoke(&results))?;
    Ok(if codex_smoke::required_failures(&results) == 0 {
        0
    } else {
        1
    })
}

fn run_trace_git_recorder(
    args: &[String],
    stdout: &mut dyn Write,
    runtime: &dyn Runtime,
) -> Result<i32> {
    match args {
        [command, rest @ ..] if command == "create" => {
            run_trace_git_recorder_create(rest, stdout, runtime)
        }
        [command, path] if command == "remove" => {
            let path = expand_user_path(path, runtime.home())?;
            let removed = git_recorder::remove_git_recorder(&path)?;
            writeln!(stdout, "removed temporary git recorder at {}", removed.display())?;
            Ok(0)
        }
        [command, rest @ ..] if command == "run" => run_trace_git_recorder_run(rest),
        _ => Err(ShimError::new(
            "usage: gh-forgejo-shim trace git-recorder <create TRACE_PATH [--real-git PATH]|remove PATH>",
        )),
    }
}

fn run_trace_git_recorder_create(
    args: &[String],
    stdout: &mut dyn Write,
    runtime: &dyn Runtime,
) -> Result<i32> {
    let mut trace_path = None;
    let mut real_git = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--real-git" => {
                let Some(value) = args.get(index + 1) else {
                    return Err(ShimError::new(
                        "usage: gh-forgejo-shim trace git-recorder create TRACE_PATH [--real-git PATH]",
                    ));
                };
                real_git = Some(expand_user_path(value, runtime.home())?);
                index += 2;
            }
            value if value.starts_with("--real-git=") => {
                real_git = Some(expand_user_path(
                    value.trim_start_matches("--real-git="),
                    runtime.home(),
                )?);
                index += 1;
            }
            value if trace_path.is_none() => {
                trace_path = Some(expand_user_path(value, runtime.home())?);
                index += 1;
            }
            _ => {
                return Err(ShimError::new(
                    "usage: gh-forgejo-shim trace git-recorder create TRACE_PATH [--real-git PATH]",
                ));
            }
        }
    }
    let trace_path = trace_path.ok_or_else(|| {
        ShimError::new(
            "usage: gh-forgejo-shim trace git-recorder create TRACE_PATH [--real-git PATH]",
        )
    })?;
    let recorder = git_recorder::create_git_recorder(
        &trace_path,
        real_git.as_deref(),
        None,
        None,
        &env_hash(runtime.env()),
        runtime.current_exe(),
    )?;

    writeln!(
        stdout,
        "created temporary git recorder at {}",
        recorder.wrapper_path.display()
    )?;
    writeln!(stdout, "trace file: {}", recorder.trace_path.display())?;
    writeln!(stdout, "launch environment:")?;
    writeln!(
        stdout,
        "  export FJ_SHIM_TRACE={}",
        shell_quote_for_display(&recorder.trace_path.display().to_string())
    )?;
    writeln!(
        stdout,
        "  export PATH={}",
        shell_quote_for_display(&recorder.path)
    )?;
    writeln!(stdout, "remove with:")?;
    writeln!(
        stdout,
        "  gfj trace git-recorder remove {}",
        shell_quote_for_display(&recorder.wrapper_dir.display().to_string())
    )?;
    Ok(0)
}

fn run_trace_git_recorder_run(args: &[String]) -> Result<i32> {
    let mut real_git = None;
    let mut trace_path = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--real-git" => {
                let Some(value) = args.get(index + 1) else {
                    return Err(ShimError::new(
                        "usage: gh-forgejo-shim trace git-recorder run --real-git PATH --trace PATH -- ARGS...",
                    ));
                };
                real_git = Some(PathBuf::from(value));
                index += 2;
            }
            "--trace" => {
                let Some(value) = args.get(index + 1) else {
                    return Err(ShimError::new(
                        "usage: gh-forgejo-shim trace git-recorder run --real-git PATH --trace PATH -- ARGS...",
                    ));
                };
                trace_path = Some(PathBuf::from(value));
                index += 2;
            }
            "--" => {
                index += 1;
                break;
            }
            _ => {
                return Err(ShimError::new(
                    "usage: gh-forgejo-shim trace git-recorder run --real-git PATH --trace PATH -- ARGS...",
                ));
            }
        }
    }
    let real_git = real_git.ok_or_else(|| {
        ShimError::new(
            "usage: gh-forgejo-shim trace git-recorder run --real-git PATH --trace PATH -- ARGS...",
        )
    })?;
    let trace_path = trace_path.ok_or_else(|| {
        ShimError::new(
            "usage: gh-forgejo-shim trace git-recorder run --real-git PATH --trace PATH -- ARGS...",
        )
    })?;
    Ok(git_recorder::run_git_recorder(
        &real_git,
        &trace_path,
        &args[index..],
    ))
}

fn env_hash(env: &EnvMap) -> HashMap<String, String> {
    env.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn shell_quote_for_display(value: &str) -> String {
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

fn run_auth(args: &[String], stdout: &mut dyn Write, runtime: &mut dyn Runtime) -> Result<i32> {
    match args {
        [command] if command == "login" => auth_login(None, stdout, runtime),
        [command, host] if command == "login" => auth_login(Some(host), stdout, runtime),
        [command, args @ ..] if command == "import" => auth_import(args, stdout, runtime),
        [command] if command == "status" => auth_status(None, stdout, runtime),
        [command, host] if command == "status" => auth_status(Some(host), stdout, runtime),
        [command] if command == "logout" => auth_logout(None, stdout, runtime),
        [command, host] if command == "logout" => auth_logout(Some(host), stdout, runtime),
        _ => Err(ShimError::new(
            "usage: gh-forgejo-shim auth <login|import|status|logout> [host]",
        )),
    }
}

fn auth_login(
    host_arg: Option<&String>,
    stdout: &mut dyn Write,
    runtime: &mut dyn Runtime,
) -> Result<i32> {
    // Logout shares prompt_host, so it can still remove a token for any host an older version took.
    let host = forgejo_host_arg(&prompt_host(host_arg, runtime)?)?;
    let token = runtime
        .prompt_token(&format!("Forgejo access token for {host}: "))?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(ShimError::new("Forgejo token is required"));
    }

    let login = runtime.validate_token(&host, &token)?;
    let home = runtime.home().map(Path::to_path_buf);
    let platform = runtime.platform().to_string();
    // The config first: when it is corrupt, the command fails before replacing a stored token.
    let config_path = config::config_path(home.as_deref());
    config::add_host(&host, Some(&config_path))?;
    let storage = auth::write_stored_token(&host, &token, home.as_deref(), Some(&platform))?;

    writeln!(
        stdout,
        "{}",
        validated_message("logged in", &host, login.as_deref())
    )?;
    writeln!(stdout, "saved auth in {storage}")?;
    Ok(0)
}

fn auth_import(args: &[String], stdout: &mut dyn Write, runtime: &mut dyn Runtime) -> Result<i32> {
    let usage = "usage: gfj auth import --from fj|tea [--hostname HOST]";
    let mut source = None;
    let mut hostname = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--from" if source.is_none() => {
                source = Some(crate::auth_import::ImportSource::parse(
                    args.next().ok_or_else(|| ShimError::new(usage))?,
                )?);
            }
            "--hostname" if hostname.is_none() => {
                hostname = Some(args.next().ok_or_else(|| ShimError::new(usage))?.as_str());
            }
            _ => return Err(ShimError::new(usage)),
        }
    }
    let source = source.ok_or_else(|| ShimError::new(usage))?;
    let home = runtime
        .home()
        .ok_or_else(|| ShimError::new("HOME is required for auth import"))?
        .to_path_buf();
    let imported = crate::auth_import::read_import(
        source,
        hostname,
        &home,
        runtime.env(),
        runtime.platform(),
    )?;
    // The config first: when it is corrupt, the command fails before replacing a stored token.
    config::add_host(&imported.host, Some(&config::config_path(Some(&home))))?;
    let storage = auth::write_stored_token(
        &imported.host,
        &imported.token,
        Some(&home),
        Some(runtime.platform()),
    )?;
    writeln!(
        stdout,
        "imported auth for {} from {}",
        imported.host,
        imported.path.display()
    )?;
    writeln!(stdout, "saved auth in {storage}")?;
    Ok(0)
}

fn auth_status(
    host_arg: Option<&String>,
    stdout: &mut dyn Write,
    runtime: &dyn Runtime,
) -> Result<i32> {
    let hosts = status_hosts(host_arg, runtime)?;
    let home = runtime.home().map(Path::to_path_buf);
    let platform = runtime.platform().to_string();
    let config =
        config::load_config_with_env(Some(&config::config_path(home.as_deref())), runtime.env())?;
    let mut all_authenticated = true;

    for host in hosts {
        let status = auth::stored_auth_status(&host, home.as_deref(), Some(&platform));
        if status.exists {
            let storage = status.storage.unwrap_or_default();
            writeln!(stdout, "{host}: logged in ({storage})")?;
            continue;
        }

        if let Some(discovery) = auth::resolve_token(
            &host,
            &config,
            runtime.env(),
            home.as_deref(),
            Some(&platform),
        ) {
            writeln!(stdout, "{host}: token available from {}", discovery.source)?;
        } else {
            all_authenticated = false;
            writeln!(stdout, "{host}: not logged in")?;
        }
    }
    Ok(if all_authenticated { 0 } else { 1 })
}

fn auth_logout(
    host_arg: Option<&String>,
    stdout: &mut dyn Write,
    runtime: &mut dyn Runtime,
) -> Result<i32> {
    let host = prompt_host(host_arg, runtime)?;
    let home = runtime.home().map(Path::to_path_buf);
    let platform = runtime.platform().to_string();
    if auth::delete_stored_token(&host, home.as_deref(), Some(&platform))? {
        writeln!(stdout, "logged out of {host}")?;
    } else {
        writeln!(stdout, "no stored shim auth for {host}")?;
    }
    Ok(0)
}

/// A host `auth login` may send a token to and `config add-host` may allowlist, by the
/// allowlist's own rule: never a GitHub host, and spelled the way the URL parser reads it back,
/// so the token goes to the host named.
fn forgejo_host_arg(host: &str) -> Result<String> {
    if config::is_known_github_host(Some(host)) {
        return Err(ShimError::new(format!(
            "{host} is a GitHub host; the shim leaves GitHub to the real gh"
        )));
    }
    config::normalized_forgejo_host(host).ok_or_else(|| {
        ShimError::new(format!(
            "{host} isn't a host name the shim can use; write it as HOST or HOST:PORT"
        ))
    })
}

fn prompt_host(host_arg: Option<&String>, runtime: &mut dyn Runtime) -> Result<String> {
    if let Some(host) = host_arg {
        let host = config::normalize_host(host);
        if host.is_empty() {
            return Err(ShimError::new("Forgejo host is required"));
        }
        return Ok(host);
    }

    let config_path = config::config_path(runtime.home());
    let config = config::load_config_with_env(Some(&config_path), runtime.env())?;
    let default = if config.hosts.len() == 1 {
        config.hosts[0].clone()
    } else {
        String::new()
    };
    let prompt = if default.is_empty() {
        "Forgejo host: ".to_string()
    } else {
        format!("Forgejo host [{default}]: ")
    };
    let entered = runtime.prompt_host(&prompt)?;
    let selected = if entered.trim().is_empty() {
        default
    } else {
        entered.trim().to_string()
    };
    let host = config::normalize_host(&selected);
    if host.is_empty() {
        return Err(ShimError::new("Forgejo host is required"));
    }
    Ok(host)
}

fn status_hosts(host_arg: Option<&String>, runtime: &dyn Runtime) -> Result<Vec<String>> {
    if let Some(host) = host_arg {
        let host = config::normalize_host(host);
        if host.is_empty() {
            return Err(ShimError::new("Forgejo host is required"));
        }
        return Ok(vec![host]);
    }

    let config_path = config::config_path(runtime.home());
    let config = config::load_config_with_env(Some(&config_path), runtime.env())?;
    if config.hosts.is_empty() {
        return Err(ShimError::new(
            "pass a host or add one with gh-forgejo-shim config add-host HOST",
        ));
    }
    Ok(config.hosts)
}

fn validated_message(action: &str, host: &str, login: Option<&str>) -> String {
    if let Some(login) = login.map(str::trim).filter(|login| !login.is_empty()) {
        format!("{action} to {host} as {login}")
    } else {
        format!("{action} to {host}")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

    #[derive(Clone)]
    struct FakeRuntime {
        env: EnvMap,
        home: PathBuf,
        platform: String,
        current_exe: PathBuf,
        host_prompts: Vec<String>,
        token_prompts: Vec<String>,
        login: Option<String>,
        validation_error: Option<String>,
        validated: Vec<(String, String)>,
        gui_path_apply_result: (bool, Option<String>),
        gui_path_applied_values: Vec<String>,
    }

    impl FakeRuntime {
        fn new(home: PathBuf) -> Self {
            Self {
                env: EnvMap::new(),
                current_exe: home.join("bin").join("gh-forgejo-shim"),
                home,
                platform: "linux".to_string(),
                host_prompts: Vec::new(),
                token_prompts: Vec::new(),
                login: Some("alice".to_string()),
                validation_error: None,
                validated: Vec::new(),
                gui_path_apply_result: (true, None),
                gui_path_applied_values: Vec::new(),
            }
        }
    }

    impl Runtime for FakeRuntime {
        fn env(&self) -> &EnvMap {
            &self.env
        }

        fn home(&self) -> Option<&Path> {
            Some(&self.home)
        }

        fn current_exe(&self) -> &Path {
            &self.current_exe
        }

        fn platform(&self) -> &str {
            &self.platform
        }

        fn prompt_host(&mut self, _prompt: &str) -> Result<String> {
            if self.host_prompts.is_empty() {
                return Ok(String::new());
            }
            Ok(self.host_prompts.remove(0))
        }

        fn prompt_token(&mut self, _prompt: &str) -> Result<String> {
            if self.token_prompts.is_empty() {
                return Ok(String::new());
            }
            Ok(self.token_prompts.remove(0))
        }

        fn validate_token(&mut self, host: &str, token: &str) -> Result<Option<String>> {
            self.validated.push((host.to_string(), token.to_string()));
            if let Some(message) = &self.validation_error {
                return Err(ShimError::new(message.clone()));
            }
            Ok(self.login.clone())
        }

        fn apply_gui_path(&mut self, path_value: &str) -> (bool, Option<String>) {
            self.gui_path_applied_values.push(path_value.to_string());
            self.gui_path_apply_result.clone()
        }

        fn current_launchd_path(&self) -> Option<String> {
            None
        }
    }

    #[test]
    fn login_prompts_validates_stores_and_allowlists_host_without_printing_secret() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime
            .host_prompts
            .push("https://Git.Example.com/path".to_string());
        runtime.token_prompts.push("login-token".to_string());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["auth", "login"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert_eq!(
            auth::read_stored_token("git.example.com", Some(&home), Some("linux"))
                .map(|stored| stored.token)
                .as_deref(),
            Some("login-token")
        );
        assert_eq!(
            config::load_config_with_env(Some(&config::config_path(Some(&home))), &EnvMap::new())?
                .hosts,
            vec!["git.example.com"]
        );
        assert_eq!(
            runtime.validated,
            vec![("git.example.com".to_string(), "login-token".to_string())]
        );
        assert!(
            output.contains("logged in to git.example.com as alice"),
            "{output}"
        );
        assert!(!output.contains("login-token"), "{output}");
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn install_shim_writes_user_local_wrapper_with_managed_marker() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime.current_exe = home.join("runtime bin").join("gfj");
        use_supported_gh(&mut runtime, &home)?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["install-shim"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let target = home.join(".local").join("bin").join("gh");
        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert!(output.contains(&format!("installed gh shim at {}", target.display())));
        assert!(shim::is_managed_shim(&target));
        let script = fs::read_to_string(&target)?;
        assert!(script.contains("rollback-compatible marker: gh-forgejo-shim gh"));
        assert!(script.contains("exec '"));
        assert!(script.contains("runtime bin"));
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn install_shim_refuses_unmanaged_target_without_force() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        use_supported_gh(&mut runtime, &home)?;
        let bin = home.join("custom-bin");
        fs::create_dir(&bin)?;
        fs::write(bin.join("gh"), "unrelated")?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::GhForgejoShim,
            os_args(["install-shim", "--bin-dir", bin.to_string_lossy().as_ref()]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let error = String::from_utf8(stderr).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 1);
        assert!(
            error.contains("exists and is not managed by gh-forgejo-shim"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(bin.join("gh"))?, "unrelated");
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn uninstall_shim_removes_managed_file_and_refuses_unmanaged_file() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        let bin = home.join("bin");
        fs::create_dir(&bin)?;
        shim::install_shim(Some(&bin), None, runtime.current_exe(), false)?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::GhForgejoShim,
            os_args([
                "uninstall-shim",
                "--bin-dir",
                bin.to_string_lossy().as_ref(),
            ]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert!(output.contains(&format!("removed gh shim at {}", bin.join("gh").display())));
        assert!(!bin.join("gh").exists());

        fs::write(bin.join("gh"), "unrelated")?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_with_runtime(
            BinaryName::GhForgejoShim,
            os_args([
                "uninstall-shim",
                "--bin-dir",
                bin.to_string_lossy().as_ref(),
            ]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let error = String::from_utf8(stderr).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 1);
        assert!(error.contains("refusing to remove"), "{error}");
        assert_eq!(fs::read_to_string(bin.join("gh"))?, "unrelated");
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn install_gui_path_rejects_non_macos_without_writing_launchagent() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["install-gui-path", "--path", "/tmp/bin:/usr/bin"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let error = String::from_utf8(stderr).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 1);
        assert!(error.contains("install-gui-path is only supported on macOS"));
        assert!(!gui_path::default_plist_path(Some(&home)).exists());
        assert!(runtime.gui_path_applied_values.is_empty());
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn install_gui_path_writes_temp_home_launchagent_and_applies_with_runtime() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime.platform = "darwin".to_string();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["install-gui-path", "--path", "/tmp/bin:/usr/bin"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        let plist_path = gui_path::default_plist_path(Some(&home));
        let plist = fs::read_to_string(&plist_path)?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert!(output.contains(&format!(
            "installed macOS GUI PATH LaunchAgent at {}",
            plist_path.display()
        )));
        assert!(output.contains("PATH=/tmp/bin:/usr/bin"));
        assert!(output.contains("applied PATH to the current launchd user session"));
        assert!(plist.contains("<string>/bin/launchctl</string>"), "{plist}");
        assert!(
            plist.contains("<string>/tmp/bin:/usr/bin</string>"),
            "{plist}"
        );
        assert_eq!(runtime.gui_path_applied_values, vec!["/tmp/bin:/usr/bin"]);
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn install_gui_path_no_apply_skips_runtime_launchctl() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime.platform = "darwin".to_string();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["install-gui-path", "--no-apply"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert!(output.contains("restart your login session or load the LaunchAgent"));
        assert!(output.contains(&format!("PATH={}/.local/bin", home.display())));
        assert!(runtime.gui_path_applied_values.is_empty());
        assert!(gui_path::default_plist_path(Some(&home)).exists());
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn install_gui_path_warns_when_runtime_apply_fails() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime.platform = "darwin".to_string();
        runtime.gui_path_apply_result = (false, Some("launchctl denied".to_string()));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["install-gui-path", "--path=/tmp/bin:/usr/bin"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        let error = String::from_utf8(stderr).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0);
        assert!(error.contains("warning: could not apply PATH immediately: launchctl denied"));
        assert!(output.contains("the LaunchAgent will apply PATH at the next login"));
        assert_eq!(runtime.gui_path_applied_values, vec!["/tmp/bin:/usr/bin"]);
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn uninstall_shim_reads_tilde_path_entries_as_bootstrap_does() -> Result<()> {
        let home = temp_root()?;
        let cargo = home.join(".cargo").join("bin");
        shim::install_shim(Some(&cargo), Some(&home), Path::new("/opt/gfj"), false)?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime.env = EnvMap::from([
            ("HOME".to_string(), home.display().to_string()),
            ("PATH".to_string(), "~/.cargo/bin:/usr/bin:/bin".to_string()),
        ]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["uninstall-shim"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        assert_eq!(code, 0);
        assert!(
            !cargo.join("gh").exists(),
            "the ~/.cargo/bin wrapper stayed"
        );
        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert!(output.contains(".cargo/bin/gh"), "{output}");
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn uninstall_gui_path_removes_only_temp_home_plist() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime.platform = "darwin".to_string();
        let plist_path = gui_path::default_plist_path(Some(&home));
        let Some(parent) = plist_path.parent() else {
            return Err(ShimError::new("plist path has no parent"));
        };
        fs::create_dir_all(parent)?;
        fs::write(&plist_path, "managed test plist")?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["uninstall-gui-path"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert!(output.contains(&format!(
            "removed macOS GUI PATH LaunchAgent at {}",
            plist_path.display()
        )));
        assert!(!plist_path.exists());
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn import_uses_fj_file_without_printing_secret() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        let source_dir = home.join(".local/share/forgejo-cli");
        fs::create_dir_all(&source_dir)?;
        fs::write(
            source_dir.join("keys.json"),
            r#"{"hosts":{"git.example.com":{"type":"Application","token":"import-token"}}}"#,
        )?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::GhForgejoShim,
            os_args([
                "auth",
                "import",
                "--from",
                "fj",
                "--hostname",
                "git.example.com",
            ]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert_eq!(
            auth::read_stored_token("git.example.com", Some(&home), Some("linux"))
                .map(|stored| stored.token)
                .as_deref(),
            Some("import-token")
        );
        assert!(output.contains("forgejo-cli/keys.json"), "{output}");
        assert!(!output.contains("import-token"), "{output}");
        assert_eq!(
            config::load_config_with_env(Some(&config::config_path(Some(&home))), &EnvMap::new())?
                .hosts,
            vec!["git.example.com"]
        );
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn login_validation_failure_does_not_store_or_allowlist_token() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime.token_prompts.push("bad-token".to_string());
        runtime.validation_error = Some("Forgejo API returned HTTP 401: unauthorized".to_string());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["auth", "login", "git.example.com"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        assert_eq!(code, 1);
        let error = String::from_utf8(stderr).map_err(|error| ShimError::new(error.to_string()))?;
        assert!(
            error.contains("Forgejo API returned HTTP 401: unauthorized"),
            "{error}"
        );
        assert!(
            auth::read_stored_token("git.example.com", Some(&home), Some("linux"))
                .map(|stored| stored.token)
                .is_none()
        );
        assert!(config::load_config_with_env(
            Some(&config::config_path(Some(&home))),
            &EnvMap::new()
        )?
        .hosts
        .is_empty());
        assert_eq!(
            runtime.validated,
            vec![("git.example.com".to_string(), "bad-token".to_string())]
        );
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn login_with_corrupt_config_keeps_the_previous_token() -> Result<()> {
        let home = temp_root()?;
        auth::write_stored_token("git.example.com", "old-token", Some(&home), Some("linux"))?;
        let config_path = config::config_path(Some(&home));
        fs::write(&config_path, "hosts = [\"git.example.com\"\n")?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime.token_prompts.push("new-token".to_string());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["auth", "login", "git.example.com"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        assert_eq!(code, 1);
        assert_eq!(
            auth::read_stored_token("git.example.com", Some(&home), Some("linux"))
                .map(|stored| stored.token)
                .as_deref(),
            Some("old-token")
        );
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn login_refuses_a_github_host_before_asking_for_a_token() -> Result<()> {
        for host in ["github.com", "api.github.com", "acme.ghe.com"] {
            let home = temp_root()?;
            let mut runtime = FakeRuntime::new(home.clone());
            runtime.token_prompts.push("forgejo-token".to_string());
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();

            let code = run_with_runtime(
                BinaryName::Gfj,
                os_args(["auth", "login", host]),
                &mut stdout,
                &mut stderr,
                &mut runtime,
            );

            assert_eq!(code, 1, "{host}");
            assert!(
                runtime.validated.is_empty(),
                "{host}: the token was sent for validation"
            );
            assert_eq!(
                runtime.token_prompts.len(),
                1,
                "{host}: the token was asked for"
            );
            fs::remove_dir_all(home).ok();
        }
        Ok(())
    }

    #[test]
    fn login_refuses_a_misspelled_host_and_logout_still_clears_github() -> Result<()> {
        let home = temp_root()?;
        // `local%68ost` reaches localhost, though the allowlist would drop the spelling.
        let mut runtime = FakeRuntime::new(home.clone());
        runtime.token_prompts.push("forgejo-token".to_string());
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["auth", "login", "local%68ost:3000"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );
        assert_eq!(code, 1);
        assert!(
            runtime.validated.is_empty(),
            "the token was sent for validation"
        );

        // An older version may have stored a token for a GitHub host; logout still removes it.
        auth::write_stored_token("api.github.com", "old-token", Some(&home), Some("linux"))?;
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["auth", "logout", "api.github.com"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );
        assert_eq!(code, 0);
        assert!(auth::read_stored_token("api.github.com", Some(&home), Some("linux")).is_none());
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn config_add_host_refuses_a_host_it_would_not_keep() -> Result<()> {
        for host in ["acme.ghe.com", "local%68ost:3000"] {
            let home = temp_root()?;
            let mut runtime = FakeRuntime::new(home.clone());
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            let code = run_with_runtime(
                BinaryName::Gfj,
                os_args(["config", "add-host", host]),
                &mut stdout,
                &mut stderr,
                &mut runtime,
            );
            assert_eq!(code, 1, "{host}");
            assert!(
                !String::from_utf8_lossy(&stdout).contains("added"),
                "{host} reported as added"
            );
            assert!(!config::config_path(Some(&home)).exists(), "{host}");
            fs::remove_dir_all(home).ok();
        }
        Ok(())
    }

    #[test]
    fn login_empty_token_fails_before_validation_or_writes() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime.token_prompts.push("  ".to_string());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["auth", "login", "git.example.com"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        assert_eq!(code, 1);
        let error = String::from_utf8(stderr).map_err(|error| ShimError::new(error.to_string()))?;
        assert!(error.contains("Forgejo token is required"), "{error}");
        assert!(runtime.validated.is_empty());
        assert!(
            auth::read_stored_token("git.example.com", Some(&home), Some("linux"))
                .map(|stored| stored.token)
                .is_none()
        );
        assert!(config::load_config_with_env(
            Some(&config::config_path(Some(&home))),
            &EnvMap::new()
        )?
        .hosts
        .is_empty());
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn status_reports_stored_auth_without_secret() -> Result<()> {
        let home = temp_root()?;
        let runtime = FakeRuntime::new(home.clone());
        config::add_host("git.example.com", Some(&config::config_path(Some(&home))))?;
        auth::write_stored_token(
            "git.example.com",
            "stored-secret",
            Some(&home),
            Some("linux"),
        )?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let mut runtime = runtime.clone();
        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["auth", "status"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert!(output.contains("git.example.com: logged in"), "{output}");
        assert!(!output.contains("stored-secret"), "{output}");
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn logout_removes_only_shim_auth_and_keeps_allowlist() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        config::add_host("git.example.com", Some(&config::config_path(Some(&home))))?;
        auth::write_stored_token(
            "git.example.com",
            "stored-secret",
            Some(&home),
            Some("linux"),
        )?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["auth", "logout", "git.example.com"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert!(output.contains("logged out of git.example.com"), "{output}");
        assert!(!output.contains("stored-secret"), "{output}");
        assert!(
            auth::read_stored_token("git.example.com", Some(&home), Some("linux"))
                .map(|stored| stored.token)
                .is_none()
        );
        assert_eq!(
            config::load_config_with_env(Some(&config::config_path(Some(&home))), &EnvMap::new())?
                .hosts,
            vec!["git.example.com"]
        );
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    #[test]
    fn logout_without_stored_auth_is_a_clear_noop() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::Gfj,
            os_args(["auth", "logout", "git.example.com"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert!(
            output.contains("no stored shim auth for git.example.com"),
            "{output}"
        );
        fs::remove_dir_all(home).ok();
        Ok(())
    }

    fn os_args<const N: usize>(values: [&str; N]) -> Vec<OsString> {
        values.into_iter().map(OsString::from).collect()
    }

    /// Points `runtime` at a fake real gh that reports the oldest supported version, so install
    /// never looks for one in this machine's install dirs, and gives it a HOME of its own. Written from a child `sh`, so no
    /// sibling test's fork holds it open for writing when it runs (ETXTBSY).
    fn use_supported_gh(runtime: &mut FakeRuntime, home: &Path) -> Result<()> {
        use std::process::{Command, Stdio};

        let dir = home.join("real gh");
        fs::create_dir_all(&dir)?;
        let path = dir.join("gh");
        let mut child = Command::new("/bin/sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(&path)
            .stdin(Stdio::piped())
            .spawn()?;
        let written = child
            .stdin
            .take()
            .ok_or_else(|| ShimError::new("no stdin"))?
            .write_all(b"#!/bin/sh\necho 'gh version 2.103.3 (2026-05-01)'\n");
        let status = child.wait()?;
        written?;
        if !status.success() {
            return Err(ShimError::new(format!("writing the fake gh: {status}")));
        }
        runtime
            .env
            .insert("FJ_SHIM_REAL_GH".to_string(), path.display().to_string());
        // Whatever install runs keeps its state in this test's own directory: with no HOME, a
        // real gh writes `.local/state` into the current directory, the repo.
        for (key, sub) in [
            ("HOME", ""),
            ("XDG_CONFIG_HOME", ".config"),
            ("XDG_STATE_HOME", ".local/state"),
            ("XDG_DATA_HOME", ".local/share"),
            ("XDG_CACHE_HOME", ".cache"),
        ] {
            runtime
                .env
                .insert(key.to_string(), home.join(sub).display().to_string());
        }
        Ok(())
    }

    fn temp_root() -> Result<PathBuf> {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "gh-forgejo-shim-cli-test-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(|error| ShimError::new(error.to_string()))?;
        Ok(path)
    }
}
