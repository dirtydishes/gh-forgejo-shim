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
const SENSITIVE_KEY_PARTS: &[&str] = &["credential", "password", "secret", "token"];

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
    result = redact_sensitive_key_values(&result);
    result = redact_token_prefixes(&result);
    redact_authorization_header(&result)
}

fn redact_url_credentials(value: &str) -> String {
    let mut ranges = Vec::new();
    let mut index = 0;

    while let Some(relative_scheme) = value[index..].find("://") {
        let scheme_index = index + relative_scheme;
        let credential_start = scheme_index + "://".len();
        let authority_end = value[credential_start..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '/' | '?' | '#')
            })
            .map_or(value.len(), |offset| credential_start + offset);

        if let Some(relative_at) = value[credential_start..authority_end].find('@') {
            let at = credential_start + relative_at;
            if at > credential_start {
                ranges.push((credential_start, at));
            }
        }

        index = authority_end;
    }

    replace_ranges(value, &ranges)
}

fn redact_sensitive_key_values(value: &str) -> String {
    let mut ranges = Vec::new();
    let mut index = 0;

    while let Some((delimiter_index, delimiter)) = find_assignment_delimiter(value, index) {
        if let Some((value_start, value_end)) = sensitive_value_range(value, delimiter_index) {
            ranges.push((value_start, value_end));
            index = value_end;
        } else {
            index = delimiter_index + delimiter.len_utf8();
        }
    }

    replace_ranges(value, &ranges)
}

fn find_assignment_delimiter(value: &str, start: usize) -> Option<(usize, char)> {
    value[start..]
        .char_indices()
        .find(|(_, character)| matches!(character, '=' | ':'))
        .map(|(offset, character)| (start + offset, character))
}

fn sensitive_value_range(value: &str, delimiter_index: usize) -> Option<(usize, usize)> {
    let key = key_before_delimiter(value, delimiter_index)?;
    if !is_sensitive_key(key) {
        return None;
    }

    let mut value_start = delimiter_index + 1;
    value_start = skip_whitespace(value, value_start);
    let (first_offset, first) = value[value_start..].char_indices().next()?;
    value_start += first_offset;

    if matches!(first, '"' | '\'') {
        let quoted_start = value_start + first.len_utf8();
        let quoted_end = value[quoted_start..]
            .find(first)
            .map_or(value.len(), |offset| quoted_start + offset);
        return Some((quoted_start, quoted_end));
    }

    let value_end = value[value_start..]
        .find(|character: char| {
            character.is_whitespace() || matches!(character, ',' | '}' | ']' | ';' | '&')
        })
        .map_or(value.len(), |offset| value_start + offset);
    Some((value_start, value_end))
}

fn key_before_delimiter(value: &str, delimiter_index: usize) -> Option<&str> {
    let key_end = value[..delimiter_index].trim_end().len();
    if key_end == 0 {
        return None;
    }

    let last = value[..key_end].chars().next_back()?;
    if matches!(last, '"' | '\'') {
        let quote_end = key_end - last.len_utf8();
        let quote_start = value[..quote_end].rfind(last)?;
        return Some(&value[quote_start + last.len_utf8()..quote_end]);
    }

    let mut key_start = key_end;
    for (index, character) in value[..key_end].char_indices().rev() {
        if is_key_character(character) {
            key_start = index;
        } else {
            break;
        }
    }
    (key_start < key_end).then_some(&value[key_start..key_end])
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    SENSITIVE_KEY_PARTS
        .iter()
        .any(|part| normalized.contains(part))
}

fn is_key_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
}

fn skip_whitespace(value: &str, mut index: usize) -> usize {
    while let Some(character) = value[index..].chars().next() {
        if !character.is_whitespace() {
            break;
        }
        index += character.len_utf8();
    }
    index
}

fn redact_token_prefixes(value: &str) -> String {
    let mut ranges = Vec::new();
    let mut index = 0;

    while index < value.len() {
        if let Some(length) = token_prefix_match_len(value, index) {
            ranges.push((index, index + length));
            index += length;
        } else if let Some(character) = value[index..].chars().next() {
            index += character.len_utf8();
        } else {
            break;
        }
    }

    replace_ranges(value, &ranges)
}

fn token_prefix_match_len(value: &str, index: usize) -> Option<usize> {
    let bytes = value.as_bytes();
    if index > 0 && is_token_character(bytes[index - 1]) {
        return None;
    }

    if bytes[index..].starts_with(b"github_pat_") {
        return token_len_after_prefix(bytes, index, b"github_pat_".len(), 20);
    }

    if bytes[index..].len() >= 4
        && bytes[index] == b'g'
        && bytes[index + 1] == b'h'
        && matches!(bytes[index + 2], b'p' | b'o' | b'u' | b's' | b'r')
        && bytes[index + 3] == b'_'
    {
        return token_len_after_prefix(bytes, index, 4, 8);
    }

    None
}

fn token_len_after_prefix(
    bytes: &[u8],
    index: usize,
    prefix_len: usize,
    min_suffix_len: usize,
) -> Option<usize> {
    let suffix_start = index + prefix_len;
    let mut end = suffix_start;
    while end < bytes.len() && is_token_character(bytes[end]) {
        end += 1;
    }
    (end - suffix_start >= min_suffix_len).then_some(end - index)
}

fn is_token_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn replace_ranges(value: &str, ranges: &[(usize, usize)]) -> String {
    if ranges.is_empty() {
        return value.to_string();
    }

    let mut redacted = String::with_capacity(value.len());
    let mut copied_until = 0;
    for &(start, end) in ranges {
        if start < copied_until {
            continue;
        }
        redacted.push_str(&value[copied_until..start]);
        redacted.push_str(REDACTED);
        copied_until = end;
    }
    redacted.push_str(&value[copied_until..]);
    redacted
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
    create_parent_dirs(path)?;

    let mut options = OpenOptions::new();
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

fn create_parent_dirs(path: &Path) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
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

    #[test]
    fn redacts_sensitive_key_values_and_token_prefixes() {
        let token = "ghp_abcdefghijklmnopqrstuvwxyz123456";
        let argv = vec![
            "api".to_string(),
            "user".to_string(),
            format!("--field=access_token={token}"),
            r#"{"password":"hunter2","credential":"plain-secret"}"#.to_string(),
            "https://alice:hunter2@git.example.com/owner/repo".to_string(),
        ];

        let redacted = redact_argv(&argv).join(" ");

        assert!(!redacted.contains(token), "{redacted}");
        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(!redacted.contains("plain-secret"), "{redacted}");
        assert!(redacted.contains(REDACTED), "{redacted}");
    }

    #[test]
    fn local_trace_path_does_not_try_to_create_empty_parent() {
        create_parent_dirs(Path::new("trace.jsonl")).expect("local trace path should be valid");
    }
}
