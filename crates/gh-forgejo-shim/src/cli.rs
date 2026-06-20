//! Command-line scaffold shared by the Rust binaries.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::config::{self, EnvMap};
use crate::routing;
use crate::{auth, Result, ShimError, VERSION};

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
    if binary == BinaryName::GhForgejoShim
        && args.first().is_some_and(|arg| arg == OsStr::new("gh"))
    {
        args.remove(0);
        return routing::run_gh_from_env(args);
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

    let result =
        if binary == BinaryName::GhForgejoShim && args.first().is_some_and(|arg| arg == "gh") {
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
    fn platform(&self) -> &str;
    fn prompt_host(&mut self, prompt: &str) -> Result<String>;
    fn prompt_token(&mut self, prompt: &str) -> Result<String>;
    fn validate_token(&mut self, host: &str, token: &str) -> Result<Option<String>>;
}

struct SystemRuntime {
    env: EnvMap,
    home: Option<PathBuf>,
    platform: &'static str,
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
    writeln!(stdout, "    {name} config <add-host|remove-host|list>")?;
    writeln!(stdout, "    {name} auth <login|import|status|logout>")?;
    writeln!(stdout)?;
    writeln!(stdout, "STATUS:")?;
    writeln!(
        stdout,
        "    managed gh dispatch, config, and auth management commands are implemented in Rust; other command behavior remains pending."
    )?;
    writeln!(stdout)?;
    writeln!(stdout, "COMMANDS:")?;
    writeln!(stdout, "    auth       manage native Forgejo auth")?;
    writeln!(stdout, "    config     manage persistent configuration")?;
    writeln!(stdout, "    help       print help")?;
    writeln!(stdout, "    version    print version")?;
    Ok(0)
}

fn print_version(stdout: &mut dyn Write) -> Result<i32> {
    writeln!(stdout, "{VERSION}")?;
    Ok(0)
}

fn print_pending(binary: BinaryName, stderr: &mut dyn Write) -> Result<i32> {
    writeln!(
        stderr,
        "{}: Rust config/auth phase only; this command is not implemented yet.",
        binary.as_str()
    )?;
    writeln!(
        stderr,
        "Only the managed gh dispatcher is native in this phase."
    )?;
    Ok(2)
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
    if auth::delete_stored_token(&host, home.as_deref(), Some(&platform)) {
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
        host_prompts: Vec<String>,
        token_prompts: Vec<String>,
        login: Option<String>,
        validated: Vec<(String, String)>,
    }

    impl FakeRuntime {
        fn new(home: PathBuf) -> Self {
            Self {
                env: EnvMap::new(),
                home,
                platform: "linux".to_string(),
                host_prompts: Vec::new(),
                token_prompts: Vec::new(),
                login: Some("alice".to_string()),
                validated: Vec::new(),
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
            Ok(self.login.clone())
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
