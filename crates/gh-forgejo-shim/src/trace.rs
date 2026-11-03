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
/// The exceptions to "a key with `pass` anywhere in it is a secret": ordinary words, matched as
/// a whole key part (`bypass_pull_request_allowances`). Redaction errs toward hiding: any other
/// key with `pass` in it is a secret, `byPassEnabled` too (coordinator, 2026-11-03).
const ORDINARY_PASS_WORDS: &[&str] = &[
    "allpass",
    "bandpass",
    "bypass",
    "compass",
    "encompass",
    "highpass",
    "lowpass",
    "overpass",
    "passed",
    "surpass",
    "trespass",
    "underpass",
];
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
/// Token prefixes. A match runs to the next whitespace: an excerpt can cut a token short, and
/// whatever follows it in the same word may be the rest of it.
const TOKEN_PREFIXES: &[&str] = &[
    "github_pat_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "glpat-",
    "gitea_",
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

    let authorization = key.to_ascii_lowercase().contains("authorization");
    // A quoted value runs to its own closing quote, across lines if it spans them; JSON inside a
    // JSON string to the escaped quote that closes it.
    let quoted = if value[value_start..].starts_with("\\\"") {
        let quoted_start = value_start + 2;
        Some((quoted_start, closing_escaped_quote(value, quoted_start), 2))
    } else if matches!(first, '"' | '\'') {
        let quoted_start = value_start + first.len_utf8();
        Some((
            quoted_start,
            closing_quote(value, quoted_start, first),
            first.len_utf8(),
        ))
    } else {
        None
    };
    if let Some((quoted_start, quoted_end, quote_len)) = quoted {
        let after = (quoted_end + quote_len).min(value.len());
        // A quoted scheme, or nothing, with the credential after it (`"token" secret`): that
        // credential is hidden too, scheme and all.
        if authorization {
            let quoted_text = value[quoted_start..quoted_end].trim().to_ascii_lowercase();
            if quoted_text.is_empty() || AUTH_SCHEMES.contains(&quoted_text.as_str()) {
                if let Some(end) = credential_after_scheme(value, after) {
                    return Some((quoted_start, end));
                }
            }
        }
        // A secret doesn't end at a quote with more of it glued on, as in `"first"tail` or the
        // shell's `'it'\''s'`.
        if quoted_end < value.len() {
            // JSON when the key itself was quoted (`"token":`), a shell word otherwise.
            let json = value[..delimiter_index].trim_end().ends_with(['"', '\'']);
            let end = glued_end(value, after, json);
            if end > after {
                return Some((quoted_start, end));
            }
        }
        return Some((quoted_start, quoted_end));
    }
    if authorization {
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

/// Where a word glued on at `start` ends (`json`: the key was quoted, as in `"token":"a",`): at
/// whitespace or a delimiter, but a quoted part inside
/// it runs to its closing quote, spaces and line breaks too, as the shell reads
/// `password="a b"`, so a secret further along the word is hidden with it.
fn glued_end(value: &str, start: usize, json: bool) -> usize {
    let mut index = start;
    while let Some(character) = value[index..].chars().next() {
        // In JSON a comma or bracket ends the value and the next field stays; in a shell word
        // they're literal (`password="a",tail` is one word), and only the shell's own
        // separators end it.
        let boundary = if json {
            matches!(character, ',' | ';' | '}' | ']' | ')' | '&')
        } else {
            matches!(character, ';' | '&' | '|' | '<' | '>' | ')')
        };
        if character.is_whitespace() || boundary {
            break;
        }
        if character == '\\' {
            index += 1;
            if let Some(escaped) = value[index..].chars().next() {
                index += escaped.len_utf8();
            }
            continue;
        }
        if matches!(character, '"' | '\'') {
            let close = closing_quote(value, index + 1, character);
            index = (close + 1).min(value.len());
            continue;
        }
        index += character.len_utf8();
    }
    index
}

/// The end of a credential after a quoted Authorization scheme or empty quote (`"token" x`), or
/// `None` when none follows: the line ends or a delimiter comes first. A quoted credential runs to
/// its closing quote, across lines; anything else to the end of its line.
fn credential_after_scheme(value: &str, after: usize) -> Option<usize> {
    let gap = value[after..].chars().next()?;
    if !gap.is_whitespace() || matches!(gap, '\n' | '\r') {
        return None;
    }
    let start = skip_whitespace(value, after);
    let next = value[start..].chars().next()?;
    if matches!(next, ',' | ';' | '}' | ']' | ')' | '&' | '\n' | '\r') {
        return None;
    }
    let line_end = authorization_credential_range(value, start).1;
    // A quoted credential can run past the line's end; anything after it on the line is hidden
    // too (`"Bearer" "first" tail`).
    if matches!(next, '"' | '\'') {
        return Some(glued_end(value, start, false).max(line_end));
    }
    // The rest of the line, whatever it looks like: a credential such as base64 `…=` can look
    // like a `key=value` field, and when in doubt the trace hides it.
    Some(line_end)
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

/// The index of the `\"` that closes a JSON string held inside another JSON string, from
/// `start`. The outer string spells the inner one's `"` as `\"` and its `\` as `\\`, so a quote
/// escaped inside the inner value is `\\\"`, and only an unescaped one closes it.
fn closing_escaped_quote(value: &str, start: usize) -> usize {
    let bytes = value.as_bytes();
    let mut index = start;
    let mut inner_escaped = false;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 1 < bytes.len() {
            match bytes[index + 1] {
                b'"' if !inner_escaped => return index,
                b'\\' => inner_escaped = !inner_escaped,
                _ => inner_escaped = false,
            }
            index += 2;
        } else {
            inner_escaped = false;
            index += 1;
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
        let key = &value[quote_start + last.len_utf8()..quote_end];
        // An escaped key inside a JSON string, `\"pass\"`, ends in the backslash of its `\"`.
        return Some(key.strip_suffix('\\').unwrap_or(key));
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
    if SENSITIVE_KEY_PARTS
        .iter()
        .any(|part| normalized.contains(part))
    {
        return true;
    }
    key.split(['-', '_', '.']).any(|part| {
        let whole = key_word(part);
        // The exceptions match exactly: `bypass1` isn't `bypass`.
        let exact = part.to_ascii_lowercase();
        (exact.contains("pass") && !ORDINARY_PASS_WORDS.contains(&exact.as_str()))
            || whole == "authorization"
            // `HTTPAuthorization`, not `authorizations_url`.
            || camel_words(part)
                .into_iter()
                .any(|word| key_word(word) == "authorization")
    })
}

/// A key word as it's compared: lowercase, trailing digits dropped (`authorization2`).
fn key_word(word: &str) -> String {
    word.trim_end_matches(|character: char| character.is_ascii_digit())
        .to_ascii_lowercase()
}

/// A key part split at camelCase boundaries, acronyms included: `smtpPass` is `smtp` `Pass`,
/// `HTTPAuthorization` is `HTTP` `Authorization`, `dbPASSHash` is `db` `PASS` `Hash`.
fn camel_words(part: &str) -> Vec<&str> {
    let characters = part.char_indices().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut start = 0;
    for position in 1..characters.len() {
        let (index, character) = characters[position];
        let previous = characters[position - 1].1;
        let next_lower = characters
            .get(position + 1)
            .is_some_and(|(_, next)| next.is_ascii_lowercase());
        if character.is_ascii_uppercase()
            && (previous.is_ascii_lowercase()
                || previous.is_ascii_digit()
                || (previous.is_ascii_uppercase() && next_lower))
        {
            words.push(&part[start..index]);
            start = index;
        }
    }
    words.push(&part[start..]);
    words
}

fn is_key_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
}

/// Skips whitespace other than line breaks (a no-break space too): a key with nothing after it
/// on its line has no value, and the next line isn't one.
fn skip_whitespace(value: &str, mut index: usize) -> usize {
    while let Some(character) = value[index..].chars().next() {
        if !character.is_whitespace() || matches!(character, '\n' | '\r') {
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
    let prefix = TOKEN_PREFIXES
        .iter()
        .find(|prefix| bytes[index..].starts_with(prefix.as_bytes()))?;
    let suffix_start = index + prefix.len();
    let end = value[suffix_start..]
        .find(char::is_whitespace)
        .map_or(value.len(), |offset| suffix_start + offset);
    (end > suffix_start).then_some(end - index)
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
    write_once(&mut file, &line)
}

/// Appends `line` in one `write(2)`, so concurrent O_APPEND writers can't land inside it. A short
/// write isn't retried, as `write_all` would: the rest would land after whatever another writer
/// appended meanwhile, splitting this record around theirs. The cut-off part stays (jko.37) and
/// the call fails. An interrupted call wrote nothing, so it's tried again.
fn write_once(out: &mut impl Write, line: &[u8]) -> io::Result<()> {
    loop {
        match out.write(line) {
            Ok(written) if written == line.len() => return Ok(()),
            Ok(written) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    format!(
                        "trace record cut off after {written} of {} bytes",
                        line.len()
                    ),
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn create_parent_dirs(path: &Path) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        crate::shim::create_dirs(parent, 0o700)?;
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

    /// A writer whose `write` calls return `results` in turn, recording the bytes of each call.
    struct ScriptedWriter {
        results: Vec<io::Result<usize>>,
        calls: Vec<Vec<u8>>,
    }

    impl Write for ScriptedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.calls.push(buf.to_vec());
            self.results.remove(0)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_short_trace_write_fails_without_appending_the_rest() {
        let mut out = ScriptedWriter {
            results: vec![Ok(4), Ok(100)],
            calls: Vec::new(),
        };
        let error = write_once(&mut out, b"{\"a\":1}\n").expect_err("short write");
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
        assert_eq!(out.calls.len(), 1, "the rest of the record was written");
    }

    #[test]
    fn an_interrupted_trace_write_is_tried_again_whole() {
        let mut out = ScriptedWriter {
            results: vec![Err(io::ErrorKind::Interrupted.into()), Ok(8)],
            calls: Vec::new(),
        };
        write_once(&mut out, b"{\"a\":1}\n").expect("second try writes it");
        assert_eq!(
            out.calls,
            [b"{\"a\":1}\n".to_vec(), b"{\"a\":1}\n".to_vec()]
        );
    }

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
    fn each_ordinary_pass_word_stays_readable() {
        for word in ORDINARY_PASS_WORDS {
            let text = format!("{word}=1");
            assert_eq!(redact_text(&text), text, "{word}");
        }
        assert_eq!(
            redact_text("byPass_pull_request_allowances=1"),
            "byPass_pull_request_allowances=1"
        );
        assert_eq!(
            redact_text(r#"{"lowpass":"8kHz","highpass":"120Hz"}"#),
            r#"{"lowpass":"8kHz","highpass":"120Hz"}"#
        );
        // A shell word that ends at its real closing quote keeps the field after it (Sol r4#4).
        assert_eq!(
            redact_text(r#"password='it'\''s' status=200"#),
            "password='<redacted> status=200"
        );
    }

    #[test]
    fn ambiguous_keys_are_redacted_by_decision() {
        // When in doubt, redact (coordinator, 2026-11-03): a key with `pass` in it that isn't an
        // exact ordinary word, and the rest of a line after a quoted Authorization scheme or
        // empty quote, are hidden even when they're harmless.
        for (text, redacted) in [
            ("byPassEnabled=true", "byPassEnabled=<redacted>"),
            ("byPASSOption=true", "byPASSOption=<redacted>"),
            (
                r#"authorization="" status=200"#,
                r#"authorization="<redacted>"#,
            ),
        ] {
            assert_eq!(redact_text(text), redacted, "{text}");
        }
    }

    #[test]
    fn a_glued_tail_stops_at_a_query_separator() {
        assert_eq!(
            redact_text(r#"authorization="x"&state=1 next"#),
            r#"authorization="<redacted>"&state=1 next"#
        );
    }

    #[test]
    fn a_quoted_authorization_value_ends_at_its_quote_and_keeps_the_next_field() {
        assert_eq!(
            redact_text(r#"{"authorization":"","status":200}"#),
            r#"{"authorization":"","status":200}"#
        );
        assert_eq!(
            redact_text(r#"{"authorization":"Bearer x","status":200}"#),
            r#"{"authorization":"<redacted>","status":200}"#
        );
    }

    #[test]
    fn neither_trace_leaks_what_the_old_git_redactor_hid() {
        for (text, secret) in [
            // An Authorization value runs to the end of its line, quoted or not.
            (r#"Authorization: "first"audit-tail"#, "audit-tail"),
            // JSON inside a JSON string: an escaped quote inside the value doesn't end it.
            (r#""{\"token\":\"first\\\"audit-tail\"}""#, "audit-tail"),
            // An escaped key is read without the backslash of its `\"`.
            (r#""{\"pass\":\"audit-tail\"}""#, "audit-tail"),
            (r#""{\"authorization\":\"audit-tail\"}""#, "audit-tail"),
            // A no-break space before the value.
            ("--field=password=\u{a0}audit-tail", "audit-tail"),
            // A short or cut-off token, and whatever follows it in the same word.
            ("ghp_abcdefg", "abcdefg"),
            ("glpat-audit-token", "audit-token"),
            ("ghp_abcdefgh/secret-tail", "secret-tail"),
            // A quoted Authorization value runs to its closing quote, across a line break.
            ("authorization: \"first\n  audit-tail\"", "audit-tail"),
            (
                "--field=authorization=\"first\n  audit-tail\"",
                "audit-tail",
            ),
            (
                r#""{\"authorization\":\"first\"audit-tail\"}""#,
                "audit-tail",
            ),
            // A quoted scheme, or nothing, with the credential after it (Opus o1#1 on #94).
            (r#"Authorization: "token" audit-cred"#, "audit-cred"),
            (r#"Authorization: "" audit-cred2"#, "audit-cred2"),
            (
                r#"http.extraHeader=Authorization: "Bearer" audit-cred"#,
                "audit-cred",
            ),
            // `pass` inside a key's last word, camelCase, or with a digit after it.
            ("sendemail.smtpPass=audit-hunter2", "audit-hunter2"),
            ("ftppass: audit-ftp", "audit-ftp"),
            ("passcode=audit-1234", "audit-1234"),
            ("authorization2: audit-abc", "audit-abc"),
            // Text glued to any quoted secret's closing quote.
            (r#"--config=password='it'\''s-audit-tail'"#, "audit-tail"),
            (r#""{\"token\":\"first\"audit-tail\"}""#, "audit-tail"),
            (r#"token="a"audit-b"#, "audit-b"),
            // A quoted part later in a glued word, spaces and all (Sol r3#1).
            (r#"token="a"password="first audit-tail""#, "audit-tail"),
            // A quoted credential after a quoted scheme, across a line break (Sol r3#2).
            (
                "Authorization: \"Bearer\" \"first\naudit-tail\"",
                "audit-tail",
            ),
            // Acronym boundaries in a key (Sol r3#3).
            (r#"{"HTTPAuthorization":"audit-http"}"#, "audit-http"),
            (r#"{"dbPASSHash":"audit-hash"}"#, "audit-hash"),
            // A shell escape inside a glued word (Sol r4#4).
            (r#"password="a"\;audit-tail"#, "audit-tail"),
            // Base64 that ends in `=` after a quoted scheme; `pass` anywhere in a key (Sol r4).
            (
                r#"Authorization: "Basic" YWxpY2U6cGFzc3dvcmQ="#,
                "YWxpY2U6cGFzc3dvcmQ",
            ),
            ("newpass=audit-secret", "audit-secret"),
            ("postgrespass=audit-secret", "audit-secret"),
            ("dbpassHash=audit-secret", "audit-secret"),
            // An exception is an exact word; after a quoted scheme the whole line; a comma in a
            // shell word (Sol r5).
            ("bypass1=audit-secret", "audit-secret"),
            (
                r#"Authorization: "Bearer" "first" audit-tail"#,
                "audit-tail",
            ),
            (r#"password="a",audit-tail"#, "audit-tail"),
        ] {
            for replacement in [REDACTED, "[REDACTED]"] {
                let redacted = redact_text_with(text, replacement);
                assert!(!redacted.contains(secret), "{text} -> {redacted}");
            }
        }
    }

    #[test]
    fn local_trace_path_does_not_try_to_create_empty_parent() {
        create_parent_dirs(Path::new("trace.jsonl")).expect("local trace path should be valid");
    }
}
