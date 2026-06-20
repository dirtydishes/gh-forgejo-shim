//! Minimal JSONL tracing for native dispatcher decisions.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::repo::RepoRef;
use crate::routing::RouteDecision;

pub const TRACE_ENV: &str = "FJ_SHIM_TRACE";

const FALSE_VALUES: &[&str] = &["", "0", "false", "no", "off"];
const REDACTED: &str = "<redacted>";
const SENSITIVE_FLAGS: &[&str] = &["--authorization", "--password", "--secret", "--token"];

pub fn tracing_enabled(env: &HashMap<String, String>) -> bool {
    env.get(TRACE_ENV)
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !FALSE_VALUES.contains(&normalized.as_str())
        })
        .unwrap_or(false)
}

pub fn append_gh_record(
    env: &HashMap<String, String>,
    argv: &[String],
    cwd: Option<&Path>,
    decision: Option<&RouteDecision>,
    duration_ms: f64,
    exit_code: i32,
) -> bool {
    let Some(path) = trace_path(env) else {
        return false;
    };
    let record = build_gh_record(argv, cwd, decision, duration_ms, exit_code);
    append_jsonl(&path, &record).is_ok()
}

fn build_gh_record(
    argv: &[String],
    cwd: Option<&Path>,
    decision: Option<&RouteDecision>,
    duration_ms: f64,
    exit_code: i32,
) -> Value {
    let route = decision
        .map(|decision| {
            json!({
                "kind": decision.kind.as_str(),
                "reason": decision.reason,
            })
        })
        .unwrap_or_else(|| json!({"kind": "unknown"}));

    let host = decision.and_then(RouteDecision::trace_host);
    let repo = decision.and_then(|decision| decision.repo.as_ref());

    json!({
        "kind": "gh",
        "timestamp_unix_ms": timestamp_unix_ms(),
        "cwd": cwd
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| std::env::current_dir().map_or_else(|_| String::new(), |path| path.display().to_string())),
        "argv": redact_argv(argv),
        "route": route,
        "host": host,
        "repo": repo.map(repo_record),
        "duration_ms": duration_ms,
        "exit_code": exit_code,
        "stdout": {"bytes": null},
        "stderr": {"bytes": null},
    })
}

fn repo_record(repo: &RepoRef) -> Value {
    json!({
        "host": repo.host,
        "owner": repo.owner,
        "name": repo.name,
        "full_name": format!("{}/{}", repo.owner, repo.name),
    })
}

pub fn redact_argv(argv: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(argv.len());
    let mut redact_next = false;
    let mut redact_header_next = false;

    for arg in argv {
        if redact_next {
            redacted.push(REDACTED.to_string());
            redact_next = false;
            continue;
        }
        if redact_header_next {
            redacted.push(redact_text(arg));
            redact_header_next = false;
            continue;
        }

        if SENSITIVE_FLAGS.contains(&arg.as_str()) {
            redacted.push(arg.clone());
            redact_next = true;
            continue;
        }

        if let Some((flag, value)) = arg.split_once('=') {
            if SENSITIVE_FLAGS.contains(&flag) {
                redacted.push(format!("{flag}={REDACTED}"));
                continue;
            }
            if matches!(flag, "-H" | "--header") {
                redacted.push(format!("{flag}={}", redact_text(value)));
                continue;
            }
        }

        if matches!(arg.as_str(), "-H" | "--header") {
            redacted.push(arg.clone());
            redact_header_next = true;
            continue;
        }

        redacted.push(redact_text(arg));
    }

    redacted
}

fn redact_text(value: &str) -> String {
    let mut result = redact_url_credentials(value);
    for name in [
        "FJ_SHIM_TOKEN",
        "FORGEJO_TOKEN",
        "GITEA_TOKEN",
        "FJ_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
    ] {
        result = redact_assignment(&result, name);
    }
    redact_authorization_header(&result)
}

fn redact_url_credentials(value: &str) -> String {
    if let Some(scheme_index) = value.find("://") {
        let credential_start = scheme_index + 3;
        if let Some(relative_at) = value[credential_start..].find('@') {
            let at = credential_start + relative_at;
            if value[credential_start..at].contains(':') {
                let mut redacted = value.to_string();
                redacted.replace_range(credential_start..at, REDACTED);
                return redacted;
            }
        }
    }
    value.to_string()
}

fn redact_assignment(value: &str, name: &str) -> String {
    let prefix = format!("{name}=");
    if let Some(index) = value.find(&prefix) {
        let value_start = index + prefix.len();
        let value_end = value[value_start..]
            .find(char::is_whitespace)
            .map_or(value.len(), |offset| value_start + offset);
        let mut redacted = value.to_string();
        redacted.replace_range(value_start..value_end, REDACTED);
        return redacted;
    }
    value.to_string()
}

fn redact_authorization_header(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let Some(index) = lower.find("authorization:") else {
        return value.to_string();
    };
    let after_name = index + "authorization:".len();
    let scheme_start = value[after_name..]
        .find(|character: char| !character.is_whitespace())
        .map_or(value.len(), |offset| after_name + offset);
    let scheme_end = value[scheme_start..]
        .find(char::is_whitespace)
        .map_or(value.len(), |offset| scheme_start + offset);
    let credential_start = value[scheme_end..]
        .find(|character: char| !character.is_whitespace())
        .map_or(scheme_start, |offset| scheme_end + offset);
    let credential_end = value[credential_start..]
        .find(|character: char| character.is_whitespace() || matches!(character, ',' | ';'))
        .map_or(value.len(), |offset| credential_start + offset);
    let mut redacted = value.to_string();
    redacted.replace_range(credential_start..credential_end, REDACTED);
    redacted
}

fn trace_path(env: &HashMap<String, String>) -> Option<PathBuf> {
    let value = env.get(TRACE_ENV)?.trim();
    let normalized = value.to_ascii_lowercase();
    if FALSE_VALUES.contains(&normalized.as_str()) {
        return None;
    }
    Some(expand_home(value, env))
}

fn expand_home(value: &str, env: &HashMap<String, String>) -> PathBuf {
    if value == "~" {
        return env
            .get("HOME")
            .map_or_else(|| PathBuf::from(value), PathBuf::from);
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = env.get("HOME") {
            return Path::new(home).join(rest);
        }
    }
    PathBuf::from(value)
}

fn append_jsonl(path: &Path, record: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, record)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn timestamp_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_flag_values() {
        let argv = vec![
            "api".to_string(),
            "user".to_string(),
            "--token".to_string(),
            "secret".to_string(),
            "--header=Authorization: token ghp_secret".to_string(),
        ];

        assert_eq!(
            redact_argv(&argv),
            vec![
                "api",
                "user",
                "--token",
                REDACTED,
                "--header=Authorization: token <redacted>"
            ]
        );
    }
}
