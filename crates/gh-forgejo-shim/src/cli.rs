//! Command-line scaffold shared by the Rust binaries.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::config::{self, EnvMap};
use crate::routing;
use crate::{
    auth, bootstrap, codex_smoke, doctor, git_recorder, gui_path, shim, trace_summary, Result,
    ShimError, VERSION,
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
        return routing::run_gh_from_env(args);
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
            Ok(code) => code,
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
        run_install_shim(&args[1..], stdout, runtime)
    } else if args.first().is_some_and(|arg| arg == "uninstall-shim") {
        run_uninstall_shim(&args[1..], stdout, runtime)
    } else if args.first().is_some_and(|arg| arg == "install-gui-path") {
        run_install_gui_path(&args[1..], stdout, stderr, runtime)
    } else if args.first().is_some_and(|arg| arg == "uninstall-gui-path") {
        run_uninstall_gui_path(&args[1..], stdout, stderr, runtime)
    } else if args.first().is_some_and(|arg| arg == "doctor") {
        run_doctor(&args[1..], stdout, runtime)
    } else if args.first().is_some_and(|arg| arg == "bootstrap") {
        run_bootstrap(&args[1..], stdout, runtime)
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
    writeln!(stdout, "    {name} bootstrap [--bin-dir DIR] [--force]")?;
    writeln!(stdout, "    {name} doctor")?;
    writeln!(stdout, "    {name} trace <summarize|smoke|git-recorder>")?;
    writeln!(stdout, "    {name} config <add-host|remove-host|list>")?;
    writeln!(stdout, "    {name} auth <login|import|status|logout>")?;
    writeln!(stdout)?;
    writeln!(stdout, "STATUS:")?;
    writeln!(
        stdout,
        "    managed gh dispatch, bootstrap, doctor, config, auth, shim lifecycle, and macOS GUI PATH commands are implemented in Rust; other command behavior remains pending."
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
        "{}: Rust setup/config/auth phase only; this command is not implemented yet.",
        binary.as_str()
    )?;
    writeln!(
        stderr,
        "Only the managed gh dispatcher and selected setup commands are native in this phase."
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

    let bin_dir = resolve_optional_path(bin_dir_arg, runtime.home())?;
    let path = shim::uninstall_shim(bin_dir.as_deref(), runtime.home())?;
    writeln!(stdout, "removed gh shim at {}", path.display())?;
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
        platform: runtime.platform().to_string(),
    })?;
    writeln!(stdout, "{}", doctor::format_checks(&checks))?;
    Ok(if checks.iter().all(|check| check.ok) {
        0
    } else {
        1
    })
}

fn run_bootstrap(args: &[String], stdout: &mut dyn Write, runtime: &dyn Runtime) -> Result<i32> {
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
                        "usage: gh-forgejo-shim bootstrap [--bin-dir DIR] [--force]",
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
                    "usage: gh-forgejo-shim bootstrap [--bin-dir DIR] [--force]",
                ));
            }
        }
    }

    let bin_dir = resolve_optional_path(bin_dir_arg, runtime.home())?;
    let result = bootstrap::run_bootstrap(bootstrap::BootstrapOptions {
        cwd: std::env::current_dir().ok(),
        env: runtime.env().clone(),
        bin_dir,
        config_path: Some(config::config_path(runtime.home())),
        force,
        home: runtime.home().map(Path::to_path_buf),
        executable: shim_command_executable(runtime.current_exe()),
    })?;
    writeln!(stdout, "{}", bootstrap::format_bootstrap(&result))?;
    Ok(if result.ok() { 0 } else { 1 })
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
        [command] if command == "import" => auth_import(None, stdout, runtime),
        [command, host] if command == "import" => auth_import(Some(host), stdout, runtime),
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
    let host = prompt_host(host_arg, runtime)?;
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
    let storage = auth::write_stored_token(&host, &token, home.as_deref(), Some(&platform))?;
    let config_path = config::config_path(home.as_deref());
    config::add_host(&host, Some(&config_path))?;

    writeln!(
        stdout,
        "{}",
        validated_message("logged in", &host, login.as_deref())
    )?;
    writeln!(stdout, "saved auth in {storage}")?;
    Ok(0)
}

fn auth_import(
    host_arg: Option<&String>,
    stdout: &mut dyn Write,
    runtime: &mut dyn Runtime,
) -> Result<i32> {
    let host = prompt_host(host_arg, runtime)?;
    let home = runtime.home().map(Path::to_path_buf);
    let discovery = auth::discover_import_token(&host, runtime.env(), home.as_deref()).ok_or_else(|| {
        ShimError::new(
            "no Forgejo token found in FJ_SHIM_TOKEN, FORGEJO_TOKEN, GITEA_TOKEN, FJ_TOKEN, or common fj/tea/gitea config files",
        )
    })?;

    let login = runtime.validate_token(&host, &discovery.token)?;
    let platform = runtime.platform().to_string();
    let storage =
        auth::write_stored_token(&host, &discovery.token, home.as_deref(), Some(&platform))?;
    let config_path = config::config_path(home.as_deref());
    config::add_host(&host, Some(&config_path))?;

    writeln!(
        stdout,
        "{}",
        validated_message("imported auth", &host, login.as_deref())
    )?;
    writeln!(stdout, "source: {}", discovery.source)?;
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
    let mut all_authenticated = true;

    for host in hosts {
        let status = auth::stored_auth_status(&host, home.as_deref(), Some(&platform));
        if status.exists {
            let storage = status.storage.unwrap_or_default();
            writeln!(stdout, "{host}: logged in ({storage})")?;
            continue;
        }

        if let Some(discovery) = auth::discover_token(
            Some(&host),
            runtime.env(),
            home.as_deref(),
            Some(&platform),
            false,
        ) {
            writeln!(
                stdout,
                "{host}: token available from {}; run gh-forgejo-shim auth import {host}",
                discovery.source
            )?;
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
            auth::discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home))
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
    fn import_uses_env_token_without_printing_secret() -> Result<()> {
        let home = temp_root()?;
        let mut runtime = FakeRuntime::new(home.clone());
        runtime
            .env
            .insert("FJ_SHIM_TOKEN".to_string(), "import-token".to_string());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_with_runtime(
            BinaryName::GhForgejoShim,
            os_args(["auth", "import", "git.example.com"]),
            &mut stdout,
            &mut stderr,
            &mut runtime,
        );

        let output =
            String::from_utf8(stdout).map_err(|error| ShimError::new(error.to_string()))?;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert_eq!(
            auth::discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home))
                .as_deref(),
            Some("import-token")
        );
        assert!(output.contains("source: FJ_SHIM_TOKEN"), "{output}");
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
            auth::discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home)).is_none()
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
            auth::discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home)).is_none()
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
            auth::discover_fj_token(Some("git.example.com"), &EnvMap::new(), Some(&home)).is_none()
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
