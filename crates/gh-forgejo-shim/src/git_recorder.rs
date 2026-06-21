//! Temporary git recorder support.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::{Result, ShimError};

pub const MARKER: &str = "managed by gh-forgejo-shim git recorder";
pub const TRACE_PATH_ENV: &str = "FJ_SHIM_TRACE";
pub const TRACE_ENV: &str = TRACE_PATH_ENV;
pub const TRACE_BODY_ENV: &str = "FJ_SHIM_TRACE_BODY";
pub const REAL_GIT_ENV: &str = "FJ_SHIM_REAL_GIT";

const SCHEMA: &str = "gh-forgejo-shim.git-recorder.v1";
const SECRET_REPLACEMENT: &str = "[REDACTED]";
const EXCERPT_LIMIT: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRecorder {
    pub wrapper_dir: PathBuf,
    pub wrapper_path: PathBuf,
    pub trace_path: PathBuf,
    pub real_git: PathBuf,
    pub path: String,
    pub env: HashMap<String, String>,
}

pub fn create_git_recorder(
    trace_path: &Path,
    real_git: Option<&Path>,
    root_dir: Option<&Path>,
    wrapper_dir: Option<&Path>,
    env: &HashMap<String, String>,
    recorder_program: &Path,
) -> Result<GitRecorder> {
    let git_path = resolve_real_git(real_git, env)?;
    let directory = make_wrapper_dir(root_dir, wrapper_dir)?;
    let wrapper = directory.join("git");
    if wrapper.exists() && !is_recorder_wrapper(&wrapper) {
        return Err(ShimError::new(format!(
            "{} exists and is not managed by gh-forgejo-shim",
            wrapper.display()
        )));
    }

    let trace = absolute_path(trace_path)?;
    let real_git = absolute_path(&git_path)?;
    fs::write(
        &wrapper,
        git_recorder_script(recorder_program, &real_git, &trace),
    )?;
    make_executable(&wrapper)?;
    fs::write(
        directory.join(".gh-forgejo-shim-git-recorder"),
        format!("{MARKER}\n"),
    )?;

    let path = prepend_path(&directory, env.get("PATH").map_or("", String::as_str));
    let mut launch_env = env.clone();
    launch_env.insert("PATH".to_string(), path.clone());
    launch_env.insert(TRACE_PATH_ENV.to_string(), trace.display().to_string());
    launch_env.insert(REAL_GIT_ENV.to_string(), real_git.display().to_string());

    Ok(GitRecorder {
        wrapper_dir: directory,
        wrapper_path: wrapper,
        trace_path: trace,
        real_git,
        path,
        env: launch_env,
    })
}

pub fn remove_git_recorder(path: &Path) -> Result<PathBuf> {
    let mut directory = expand_home(path);
    if directory.is_file() {
        directory = directory
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ShimError::new("git recorder path has no parent directory"))?;
    }
    if !directory.exists() {
        return Ok(directory);
    }

    let wrapper = directory.join("git");
    if !is_recorder_wrapper(&wrapper) {
        return Err(ShimError::new(format!(
            "{} is not a gh-forgejo-shim git recorder directory",
            directory.display()
        )));
    }
    fs::remove_dir_all(&directory)?;
    Ok(directory)
}

pub fn git_recorder_script(recorder_program: &Path, real_git: &Path, trace_path: &Path) -> String {
    format!(
        "#!/bin/sh\n# {MARKER}\nexec {} trace git-recorder run --real-git {} --trace {} -- \"$@\"\n",
        shell_quote(&recorder_program.display().to_string()),
        shell_quote(&real_git.display().to_string()),
        shell_quote(&trace_path.display().to_string())
    )
}

pub fn run_git_recorder(real_git: &Path, trace_path: &Path, args: &[String]) -> i32 {
    let started = Instant::now();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from(""));
    let mut command = Command::new(real_git);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut record = base_record(args, &cwd);
    let include_excerpt = env::var(TRACE_BODY_ENV).ok().as_deref() == Some("1");

    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message =
                format!("gh-forgejo-shim git recorder: unable to run real git: {error}\n");
            let _ = io::stderr().write_all(message.as_bytes());
            finish_record(
                &mut record,
                FinishedGit {
                    started,
                    exit_code: 127,
                    stdout_bytes: 0,
                    stderr_bytes: message.len(),
                    stdout_excerpt: &[],
                    stderr_excerpt: message.as_bytes(),
                    include_excerpt,
                },
            );
            let _ = append_jsonl(trace_path, &record);
            return 127;
        }
    };

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => {
            let message =
                format!("gh-forgejo-shim git recorder: unable to wait for real git: {error}\n");
            let _ = io::stderr().write_all(message.as_bytes());
            finish_record(
                &mut record,
                FinishedGit {
                    started,
                    exit_code: 127,
                    stdout_bytes: 0,
                    stderr_bytes: message.len(),
                    stdout_excerpt: &[],
                    stderr_excerpt: message.as_bytes(),
                    include_excerpt,
                },
            );
            let _ = append_jsonl(trace_path, &record);
            return 127;
        }
    };
    let exit_code = output.status.code().unwrap_or(1);
    let _ = io::stdout().write_all(&output.stdout);
    let _ = io::stdout().flush();
    let _ = io::stderr().write_all(&output.stderr);
    let _ = io::stderr().flush();
    let stdout_excerpt = if include_excerpt {
        &output.stdout[..output.stdout.len().min(EXCERPT_LIMIT)]
    } else {
        &[]
    };
    let stderr_excerpt = if include_excerpt {
        &output.stderr[..output.stderr.len().min(EXCERPT_LIMIT)]
    } else {
        &[]
    };

    finish_record(
        &mut record,
        FinishedGit {
            started,
            exit_code,
            stdout_bytes: output.stdout.len(),
            stderr_bytes: output.stderr.len(),
            stdout_excerpt,
            stderr_excerpt,
            include_excerpt,
        },
    );
    let _ = append_jsonl(trace_path, &record);
    exit_code
}

fn base_record(args: &[String], cwd: &Path) -> Value {
    let argv = redact_argv(args);
    let command_argv = std::iter::once("git".to_string())
        .chain(argv.iter().cloned())
        .collect::<Vec<_>>();
    json!({
        "schema": SCHEMA,
        "kind": "git",
        "timestamp": utc_timestamp(),
        "cwd": redact_text(&cwd.display().to_string()),
        "argv": argv,
        "command_argv": command_argv,
    })
}

struct FinishedGit<'a> {
    started: Instant,
    exit_code: i32,
    stdout_bytes: usize,
    stderr_bytes: usize,
    stdout_excerpt: &'a [u8],
    stderr_excerpt: &'a [u8],
    include_excerpt: bool,
}

fn finish_record(record: &mut Value, finished: FinishedGit<'_>) {
    let duration = finished.started.elapsed().as_secs_f64();
    record["duration"] = json!(round_float(duration, 6));
    record["duration_ms"] = json!(round_float(duration * 1000.0, 3));
    record["exit_code"] = json!(finished.exit_code);
    record["stdout_bytes"] = json!(finished.stdout_bytes);
    record["stderr_bytes"] = json!(finished.stderr_bytes);
    record["stdout"] = json!({"bytes": finished.stdout_bytes});
    record["stderr"] = json!({"bytes": finished.stderr_bytes});
    if finished.include_excerpt {
        let stdout_text = redact_text(&String::from_utf8_lossy(finished.stdout_excerpt));
        let stderr_text = redact_text(&String::from_utf8_lossy(finished.stderr_excerpt));
        record["stdout_excerpt"] = json!(stdout_text);
        record["stderr_excerpt"] = json!(stderr_text);
        record["stdout"]["excerpt"] = record["stdout_excerpt"].clone();
        record["stderr"]["excerpt"] = record["stderr_excerpt"].clone();
    }
}

fn resolve_real_git(real_git: Option<&Path>, env: &HashMap<String, String>) -> Result<PathBuf> {
    if let Some(path) = real_git {
        return Ok(expand_home(path));
    }
    if let Some(value) = env
        .get(REAL_GIT_ENV)
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(expand_home(Path::new(value)));
    }
    find_on_path("git", env.get("PATH").map_or("", String::as_str))
        .ok_or_else(|| ShimError::new(format!("could not find git; set {REAL_GIT_ENV}")))
}

fn find_on_path(name: &str, path_value: &str) -> Option<PathBuf> {
    env::split_paths(path_value).find_map(|directory| {
        let candidate = directory.join(name);
        is_executable_file(&candidate).then_some(candidate)
    })
}

fn make_wrapper_dir(root_dir: Option<&Path>, wrapper_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = wrapper_dir {
        let directory = absolute_path(path)?;
        fs::create_dir_all(&directory)?;
        return Ok(directory);
    }

    let root = root_dir.map_or_else(env::temp_dir, expand_home);
    fs::create_dir_all(&root)?;
    for attempt in 0..1000 {
        let directory = root.join(format!(
            "gh-forgejo-shim-git-{}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&directory) {
            Ok(()) => return absolute_path(&directory),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ShimError::new(
        "unable to allocate temporary git recorder directory",
    ))
}

fn prepend_path(directory: &Path, path_value: &str) -> String {
    if path_value.is_empty() {
        return directory.display().to_string();
    }
    match env::join_paths([directory.as_os_str(), std::ffi::OsStr::new(path_value)]) {
        Ok(value) => value.to_string_lossy().into_owned(),
        Err(_) => format!("{}:{path_value}", directory.display()),
    }
}

fn is_recorder_wrapper(wrapper: &Path) -> bool {
    fs::read_to_string(wrapper)
        .map(|text| text.contains(MARKER))
        .unwrap_or(false)
}

fn append_jsonl(path: &Path, record: &Value) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, record)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    let expanded = expand_home(path);
    if expanded.is_absolute() {
        return Ok(expanded);
    }
    Ok(env::current_dir()?.join(expanded))
}

fn expand_home(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if value == "~" {
        return env::var_os("HOME").map_or_else(|| path.to_path_buf(), PathBuf::from);
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_path_buf()
}

pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '_' | '-' | ':')
    }) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn redact_argv(args: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            redacted.push(SECRET_REPLACEMENT.to_string());
            redact_next = false;
            continue;
        }

        let (key, separator, value) = split_assignment(arg);
        if key.starts_with('-') && is_secret_key(key) {
            if separator.is_some() {
                redacted.push(format!("{key}={SECRET_REPLACEMENT}"));
            } else {
                redacted.push(key.to_string());
                redact_next = true;
            }
            continue;
        }
        if separator.is_some() && is_secret_key(key) {
            redacted.push(format!("{key}={SECRET_REPLACEMENT}"));
            continue;
        }
        if separator.is_some() {
            redacted.push(format!("{key}={}", redact_text(value)));
        } else {
            redacted.push(redact_text(arg));
        }
    }
    redacted
}

fn split_assignment(value: &str) -> (&str, Option<char>, &str) {
    value
        .find('=')
        .map(|index| (&value[..index], Some('='), &value[index + 1..]))
        .unwrap_or((value, None, ""))
}

fn redact_text(value: &str) -> String {
    let mut result = redact_url_userinfo(value);
    result = redact_authorization(&result);
    result = redact_secret_assignments(&result);
    redact_known_tokens(&result)
}

fn redact_url_userinfo(value: &str) -> String {
    let mut output = String::new();
    let mut rest = value;
    while let Some(scheme_index) = rest.find("://") {
        let credential_start = scheme_index + 3;
        let Some(at_relative) = rest[credential_start..].find('@') else {
            output.push_str(&rest[..credential_start]);
            rest = &rest[credential_start..];
            continue;
        };
        let at_index = credential_start + at_relative;
        let authority = &rest[credential_start..at_index];
        if authority.contains('/') || authority.contains(char::is_whitespace) {
            output.push_str(&rest[..=at_index]);
        } else {
            output.push_str(&rest[..credential_start]);
            output.push_str(SECRET_REPLACEMENT);
            output.push('@');
        }
        rest = &rest[at_index + 1..];
    }
    output.push_str(rest);
    output
}

fn redact_authorization(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    for prefix in ["authorization:", "authorization="] {
        if let Some(index) = lower.find(prefix) {
            let secret_start = index + prefix.len();
            let after = &value[secret_start..];
            let token_end = after.find(['\n', '\r', ';', '&']).unwrap_or(after.len());
            return format!(
                "{}{}{}",
                &value[..secret_start],
                SECRET_REPLACEMENT,
                &after[token_end..]
            );
        }
    }
    value.to_string()
}

fn redact_secret_assignments(value: &str) -> String {
    let mut words = Vec::new();
    for word in value.split_whitespace() {
        let (key, separator, _rest) = split_key_value(word);
        if separator.is_some() && is_secret_key(key) {
            let delimiter = separator.unwrap_or('=');
            words.push(format!("{key}{delimiter}{SECRET_REPLACEMENT}"));
        } else {
            words.push(word.to_string());
        }
    }
    words.join(" ")
}

fn split_key_value(value: &str) -> (&str, Option<char>, &str) {
    if let Some(index) = value.find('=') {
        return (&value[..index], Some('='), &value[index + 1..]);
    }
    if let Some(index) = value.find(':') {
        return (&value[..index], Some(':'), &value[index + 1..]);
    }
    (value, None, "")
}

fn redact_known_tokens(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            if is_known_token(word) {
                SECRET_REPLACEMENT.to_string()
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_known_token(value: &str) -> bool {
    let normalized = value.trim_matches(|character: char| {
        !character.is_ascii_alphanumeric() && !matches!(character, '_' | '-')
    });
    normalized.starts_with("github_pat_")
        || normalized.starts_with("ghp_")
        || normalized.starts_with("gho_")
        || normalized.starts_with("ghu_")
        || normalized.starts_with("ghs_")
        || normalized.starts_with("ghr_")
        || normalized.starts_with("glpat-")
        || normalized.starts_with("gitea_")
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "pass",
        "password",
        "passwd",
        "api-key",
        "api_key",
        "authorization",
        "auth-token",
        "auth_token",
        "credential",
        "client-secret",
        "client_secret",
        "access-token",
        "access_token",
        "refresh-token",
        "refresh_token",
        "private-key",
        "private_key",
    ]
    .iter()
    .any(|part| normalized.contains(part))
}

fn utc_timestamp() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("{}.{:03}Z", duration.as_secs(), duration.subsec_millis()),
        Err(_) => "0.000Z".to_string(),
    }
}

fn round_float(value: f64, digits: i32) -> f64 {
    let factor = 10_f64.powi(digits);
    (value * factor).round() / factor
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_argv_values() {
        let argv = redact_argv(&[
            "fetch".to_string(),
            "--password=argv-secret".to_string(),
            "--token".to_string(),
            "next-argv-secret".to_string(),
            "https://user:password@example.com/repo.git".to_string(),
        ]);

        assert_eq!(
            argv,
            vec![
                "fetch",
                "--password=[REDACTED]",
                "--token",
                "[REDACTED]",
                "https://[REDACTED]@example.com/repo.git",
            ]
        );
    }

    #[test]
    fn generated_script_contains_managed_marker() {
        let script = git_recorder_script(
            Path::new("/tmp/gfj"),
            Path::new("/usr/bin/git"),
            Path::new("/tmp/trace.jsonl"),
        );

        assert!(script.contains(MARKER));
        assert!(script.contains("trace git-recorder run"));
    }
}
