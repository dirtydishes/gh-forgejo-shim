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
/// Shared with the git recorder, so either trace redacts what the other does. Matched anywhere in
/// a key.
const SENSITIVE_KEY_PARTS: &[&str] = &[
    "api-key",
    "api_key",
    "credential",
    "passphrase",
    "passwd",
    "password",
    "private-key",
    "private_key",
    "secret",
    "token",
];
/// Matched only as a whole part of a key, split at `-`, `_` and `.`: `db_pass` and
/// `Authorization` are secrets, `bypass` and `authorizations_url` aren't.
const SENSITIVE_KEY_WORDS: &[&str] = &["authorization", "pass"];
/// Authorization schemes. A first word that isn't one of these is part of the credential.
const AUTH_SCHEMES: &[&str] = &[
    "aws4-hmac-sha256",
    "basic",
    "bearer",
    "digest",
    "negotiate",
    "ntlm",
    "token",
];
/// Token prefixes, each with the shortest suffix that makes a match a token rather than a word.
const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("github_pat_", 20),
    ("ghp_", 8),
    ("gho_", 8),
    ("ghu_", 8),
    ("ghs_", 8),
    ("ghr_", 8),
    ("glpat-", 20),
    ("gitea_", 8),
];

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
    redact_text_with(value, REDACTED)
}

/// Replaces each credential in free text with `replacement`: URL userinfo, the value of a
/// sensitive `key=value`, `key: value` or `"key": "value"` (every Authorization header among
/// them), and known token prefixes. The gh and git traces use different replacements.
pub(crate) fn redact_text_with(value: &str, replacement: &str) -> String {
    let result = redact_url_credentials(value, replacement);
    let result = redact_sensitive_key_values(&result, replacement);
    redact_token_prefixes(&result, replacement)
}

fn redact_url_credentials(value: &str, replacement: &str) -> String {
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
        // A password can hold a literal `?`, `#` or `@`, so the userinfo runs to the last `@`
        // before the path.
        let userinfo_end = value[credential_start..]
            .find(|character: char| character.is_whitespace() || character == '/')
            .map_or(value.len(), |offset| credential_start + offset);

        index = authority_end;
        if let Some(relative_at) = value[credential_start..userinfo_end].rfind('@') {
            let at = credential_start + relative_at;
            if at > credential_start {
                ranges.push((credential_start, at));
            }
            index = index.max(at + 1);
        }
    }

    replace_ranges(value, &ranges, replacement)
}

fn redact_sensitive_key_values(value: &str, replacement: &str) -> String {
    let mut ranges = Vec::new();
    let mut index = 0;

    while let Some((delimiter_index, delimiter)) = find_assignment_delimiter(value, index) {
        if let Some((value_start, value_end)) =
            sensitive_value_range(value, delimiter_index).filter(|(start, end)| start < end)
        {
            ranges.push((value_start, value_end));
            index = value_end;
        } else {
            index = delimiter_index + delimiter.len_utf8();
        }
    }

    replace_ranges(value, &ranges, replacement)
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

    // JSON inside a JSON string: the value runs to the next escaped quote.
    if value[value_start..].starts_with("\\\"") {
        let quoted_start = value_start + 2;
        let quoted_end = value[quoted_start..]
            .find("\\\"")
            .map_or(value.len(), |offset| quoted_start + offset);
        return Some((quoted_start, quoted_end));
    }
    if matches!(first, '"' | '\'') {
        let quoted_start = value_start + first.len_utf8();
        return Some((quoted_start, closing_quote(value, quoted_start, first)));
    }
    if key.to_ascii_lowercase().contains("authorization") {
        return Some(authorization_credential_range(value, value_start));
    }

    // An unquoted value runs to whitespace, or to the next query parameter after `=`. Ending it
    // at `,` or `;` would keep the rest of a password such as `p@ss;w,rd`.
    let query = value[delimiter_index..].starts_with('=');
    let value_end = value[value_start..]
        .find(|character: char| character.is_whitespace() || (query && character == '&'))
        .map_or(value.len(), |offset| value_start + offset);
    Some((value_start, value_end))
}

/// The index of the quote that ends a string starting at `start`, past any backslash-escaped
/// quote, or the end of `value`.
fn closing_quote(value: &str, start: usize, quote: char) -> usize {
    let mut escaped = false;
    for (offset, character) in value[start..].char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            _ if character == quote => return start + offset,
            _ => {}
        }
    }
    value.len()
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

pub(crate) fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    SENSITIVE_KEY_PARTS
        .iter()
        .any(|part| normalized.contains(part))
        || normalized
            .split(['-', '_', '.'])
            .any(|word| SENSITIVE_KEY_WORDS.contains(&word))
}

fn is_key_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
}

/// Skips spaces and tabs, not line breaks: a key with nothing after it on its line has no value,
/// and the next line isn't one.
fn skip_whitespace(value: &str, mut index: usize) -> usize {
    while let Some(character) = value[index..].chars().next() {
        if !matches!(character, ' ' | '\t') {
            break;
        }
        index += character.len_utf8();
    }
    index
}

fn redact_token_prefixes(value: &str, replacement: &str) -> String {
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

    replace_ranges(value, &ranges, replacement)
}

fn token_prefix_match_len(value: &str, index: usize) -> Option<usize> {
    let bytes = value.as_bytes();
    // A token can follow `-` (`x-ghp_…`), though `-` may also appear inside one.
    if index > 0 && (bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_') {
        return None;
    }
    TOKEN_PREFIXES.iter().find_map(|(prefix, min_suffix_len)| {
        bytes[index..]
            .starts_with(prefix.as_bytes())
            .then(|| token_len_after_prefix(bytes, index, prefix.len(), *min_suffix_len))?
    })
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
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn replace_ranges(value: &str, ranges: &[(usize, usize)], replacement: &str) -> String {
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
        redacted.push_str(replacement);
        copied_until = end;
    }
    redacted.push_str(&value[copied_until..]);
    redacted
}

/// An Authorization value runs to the end of its line. A leading scheme word (`token`,
/// `Bearer`) stays readable when a credential follows it on that line; any other first word is
/// part of the credential.
fn authorization_credential_range(value: &str, start: usize) -> (usize, usize) {
    let line = value[start..]
        .split(['\n', '\r'])
        .next()
        .unwrap_or_default()
        .trim_end();
    let credential = line
        .find(char::is_whitespace)
        .filter(|&space| AUTH_SCHEMES.contains(&line[..space].to_ascii_lowercase().as_str()))
        .map_or(0, |space| line.len() - line[space..].trim_start().len());
    (start + credential, start + line.len())
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

/// Appends `record` as one line. Concurrent writers (parallel `gh` calls, the git recorder) stay
/// whole records because the line goes out in one append-mode write, which local filesystems
/// keep whole. NFS doesn't promise that. No lock: one taken by another process could hang every
/// traced call after its command has already run.
pub(crate) fn append_jsonl(path: &Path, record: &Value) -> io::Result<()> {
    create_parent_dirs(path)?;

    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    let mut file = options.open(path)?;
    // The mode above applies only to a new file. Records can hold repo paths and command lines,
    // so an existing file others can read is narrowed before anything is added to it. Only a
    // regular file: a trace sent to /dev/null or a terminal leaves that device's mode alone.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = file.metadata()?;
        if metadata.is_file() && metadata.permissions().mode() & 0o077 != 0 {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
    }
    file.write_all(&line)
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

    #[cfg(unix)]
    #[test]
    fn a_trace_sent_to_a_device_leaves_its_mode_alone() -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let null = Path::new("/dev/null");
        let before = fs::metadata(null)?.permissions().mode();

        append_jsonl(null, &json!({"kind": "gh"}))?;

        assert_eq!(fs::metadata(null)?.permissions().mode(), before);
        Ok(())
    }

    #[test]
    fn git_replacement_keeps_lines_and_covers_its_token_prefixes() {
        let text = "push glpat-abcdefghij_klmnopqrst\napi_key: k1\n";

        assert_eq!(
            redact_text_with(text, "[REDACTED]"),
            "push [REDACTED]\napi_key: [REDACTED]\n"
        );
    }

    #[test]
    fn redacts_every_authorization_header_within_its_line() {
        let text = "> Authorization: plain-one\nnext line\n> authorization=Bearer two three\n";

        assert_eq!(
            redact_text(text),
            "> Authorization: <redacted>\nnext line\n> authorization=Bearer <redacted>\n"
        );
    }

    #[test]
    fn redacts_schemeless_authorization_url_passwords_and_nested_json() {
        for (text, secret) in [
            ("-c http.extraHeader=Authorization: abc123 foo", "abc123"),
            ("Authorization: s3cr3tvalue (expired)", "s3cr3tvalue"),
            ("https://alice:pa#ss@host/x", "pa#ss"),
            ("https://alice:pa?ss@host/x", "pa?ss"),
            (r#"{"body": "{\"token\": \"sec ret,tail\"}"}"#, "ret,tail"),
        ] {
            let redacted = redact_text_with(text, "[REDACTED]");
            assert!(!redacted.contains(secret), "{text} -> {redacted}");
        }
        assert_eq!(
            redact_text("Authorization: Bearer abc"),
            "Authorization: Bearer <redacted>"
        );
    }

    #[test]
    fn ordinary_words_stay_readable_and_values_stay_on_their_line() {
        for text in [
            "bypass_pull_request_allowances=1 passed: 3 compass=north",
            r#"{"authorizations_url": "https://api.example.test/authorizations"}"#,
            "Authorization:\nnext line here",
        ] {
            assert_eq!(redact_text(text), text);
        }
        assert_eq!(redact_text("db_pass=hunter2"), "db_pass=<redacted>");
        assert_eq!(redact_text("x-ghp_abcdefghijklmnop"), "x-<redacted>");
    }

    #[test]
    fn local_trace_path_does_not_try_to_create_empty_parent() {
        create_parent_dirs(Path::new("trace.jsonl")).expect("local trace path should be valid");
    }
}
