//! Trace JSONL summary support.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::{Result, ShimError};

pub const SLOW_CALL_MS: f64 = 1000.0;

#[derive(Debug, Clone, PartialEq)]
pub struct TraceSummary {
    pub total: usize,
    pub failures: Vec<Value>,
    pub slow_calls: Vec<Value>,
    pub unsupported: Vec<Value>,
    pub probe_counts: BTreeMap<String, usize>,
    pub route_counts: BTreeMap<String, usize>,
    pub parse_errors: Vec<String>,
}

pub fn load_trace(path: &Path) -> Result<(Vec<Value>, Vec<String>)> {
    let text = fs::read_to_string(path)?;
    let mut records = Vec::new();
    let mut errors = Vec::new();

    for (index, line) in text.lines().enumerate() {
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(stripped) {
            Ok(Value::Object(_)) => {
                records.push(serde_json::from_str(stripped).map_err(|error| {
                    ShimError::new(format!("could not load trace record: {error}"))
                })?)
            }
            Ok(_) => errors.push(format!("line {}: expected object", index + 1)),
            Err(error) => errors.push(format!("line {}: {}", index + 1, json_error_message(error))),
        }
    }

    Ok((records, errors))
}

pub fn summarize_trace(records: &[Value], slow_ms: f64) -> TraceSummary {
    let mut failures = Vec::new();
    let mut slow_calls = Vec::new();
    let mut unsupported = Vec::new();
    let mut probe_counts = BTreeMap::new();
    let mut route_counts = BTreeMap::new();

    for record in records {
        if !matches!(exit_code(record), None | Some(0)) {
            failures.push(record.clone());
        }
        if duration_ms(record) >= slow_ms {
            slow_calls.push(record.clone());
        }
        if is_unsupported(record) {
            unsupported.push(record.clone());
        }
        if let Some(probe) = classify_probe(record) {
            *probe_counts.entry(probe).or_insert(0) += 1;
        }
        if let Some(route) = route_kind(record) {
            *route_counts.entry(route).or_insert(0) += 1;
        }
    }

    TraceSummary {
        total: records.len(),
        failures,
        slow_calls,
        unsupported,
        probe_counts,
        route_counts,
        parse_errors: Vec::new(),
    }
}

pub fn summarize_trace_file(path: &Path, slow_ms: f64) -> Result<TraceSummary> {
    let (records, parse_errors) = load_trace(path)?;
    let mut summary = summarize_trace(&records, slow_ms);
    summary.parse_errors = parse_errors;
    Ok(summary)
}

pub fn format_trace_summary(summary: &TraceSummary) -> String {
    let mut lines = vec![format!("Trace records: {}", summary.total)];
    if !summary.parse_errors.is_empty() {
        lines.push(format!("Parse errors: {}", summary.parse_errors.len()));
        for error in summary.parse_errors.iter().take(5) {
            lines.push(format!("  - {error}"));
        }
    }
    lines.push(format_counter("Routes", &summary.route_counts));
    lines.push(format_counter("Known Codex probes", &summary.probe_counts));
    lines.push(format_records("Failures", &summary.failures));
    lines.push(format_records("Slow calls", &summary.slow_calls));
    lines.push(format_records(
        "Unsupported/delegated commands",
        &summary.unsupported,
    ));
    lines.join("\n")
}

pub fn classify_probe(record: &Value) -> Option<String> {
    let kind = record.get("kind").and_then(Value::as_str)?;
    let argv = argv(record);
    if kind == "git" {
        return classify_git_probe(&argv);
    }
    if kind != "gh" {
        return None;
    }
    if argv == ["--version"] || argv == ["version"] {
        return Some("gh --version".to_string());
    }
    if argv.len() >= 2 && argv[..2] == ["auth", "status"] {
        return Some("gh auth status".to_string());
    }
    if argv.len() >= 2
        && argv[0] == "api"
        && matches!(api_endpoint(&argv[1..]).as_deref(), Some("user" | "/user"))
    {
        return Some("gh api user".to_string());
    }
    if argv.len() >= 2 && argv[..2] == ["pr", "status"] {
        return Some("gh pr status".to_string());
    }
    if argv.len() >= 2 && argv[..2] == ["pr", "list"] {
        return Some("gh pr list".to_string());
    }
    if argv.len() >= 2 && argv[..2] == ["repo", "view"] {
        return Some("gh repo view".to_string());
    }
    None
}

fn classify_git_probe(argv: &[String]) -> Option<String> {
    if argv == ["rev-parse", "--show-toplevel"] {
        return Some("git repo root".to_string());
    }
    if argv.starts_with(&["remote".to_string(), "-v".to_string()])
        || argv.starts_with(&["remote".to_string()])
    {
        return Some("git remotes".to_string());
    }
    if argv.len() >= 3
        && argv[..3] == ["symbolic-ref", "--quiet", "--short"]
        && argv.iter().any(|arg| arg == "refs/remotes/origin/HEAD")
    {
        return Some("git default branch".to_string());
    }
    if argv == ["branch", "--show-current"] {
        return Some("git current branch".to_string());
    }
    if argv.len() >= 3 && argv[..3] == ["rev-parse", "--abbrev-ref", "--symbolic-full-name"] {
        return Some("git upstream".to_string());
    }
    if argv.first().is_some_and(|arg| arg == "for-each-ref") {
        return Some("git refs".to_string());
    }
    None
}

fn format_counter(title: &str, counter: &BTreeMap<String, usize>) -> String {
    if counter.is_empty() {
        return format!("{title}: none");
    }
    let mut items = counter.iter().collect::<Vec<_>>();
    items.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
    let formatted = items
        .into_iter()
        .map(|(name, count)| format!("{name}={count}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{title}: {formatted}")
}

fn format_records(title: &str, records: &[Value]) -> String {
    if records.is_empty() {
        return format!("{title}: none");
    }
    let mut lines = vec![format!("{title}: {}", records.len())];
    for record in records.iter().take(8) {
        lines.push(format!(
            "  - {} {} exit={} duration={:.0}ms route={}",
            record.get("kind").and_then(Value::as_str).unwrap_or("?"),
            command_text(record),
            exit_code(record)
                .map(|code| code.to_string())
                .unwrap_or_else(|| "None".to_string()),
            duration_ms(record),
            route_kind(record).unwrap_or_else(|| "-".to_string())
        ));
    }
    if records.len() > 8 {
        lines.push(format!("  - ... {} more", records.len() - 8));
    }
    lines.join("\n")
}

fn argv(record: &Value) -> Vec<String> {
    record
        .get("argv")
        .or_else(|| record.get("command"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| match item {
                    Value::String(value) => value.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn command_text(record: &Value) -> String {
    argv(record).join(" ")
}

fn exit_code(record: &Value) -> Option<i64> {
    record.get("exit_code").and_then(Value::as_i64)
}

fn duration_ms(record: &Value) -> f64 {
    record
        .get("duration_ms")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

fn route_kind(record: &Value) -> Option<String> {
    record
        .get("route")
        .and_then(Value::as_object)
        .and_then(|route| route.get("kind"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            record
                .get("route_kind")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

fn route_reason(record: &Value) -> String {
    record
        .get("route")
        .and_then(Value::as_object)
        .and_then(|route| route.get("reason"))
        .and_then(Value::as_str)
        .or_else(|| record.get("route_reason").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

fn is_unsupported(record: &Value) -> bool {
    let reason = route_reason(record).to_ascii_lowercase();
    reason.contains("unsupported")
        || route_kind(record).as_deref() == Some("delegate") && reason.contains("unsupported")
}

fn api_endpoint(argv: &[String]) -> Option<String> {
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "--" {
            return argv.get(index + 1).cloned();
        }
        if api_flag_takes_value(arg) {
            index += 2;
        } else if arg.starts_with('-') {
            index += 1;
        } else {
            return Some(arg.clone());
        }
    }
    None
}

fn api_flag_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "--cache"
            | "-F"
            | "--field"
            | "-H"
            | "--header"
            | "--hostname"
            | "--input"
            | "--method"
            | "-X"
            | "--preview"
            | "-p"
            | "-f"
            | "--raw-field"
            | "--jq"
            | "-q"
            | "--template"
            | "-t"
    )
}

fn json_error_message(error: serde_json::Error) -> String {
    let message = error.to_string();
    message
        .split(" at line ")
        .next()
        .unwrap_or(&message)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_known_codex_gh_and_git_probes() {
        assert_eq!(
            classify_probe(&json!({"kind": "gh", "argv": ["--version"]})).as_deref(),
            Some("gh --version")
        );
        assert_eq!(
            classify_probe(&json!({"kind": "gh", "argv": ["auth", "status"]})).as_deref(),
            Some("gh auth status")
        );
        assert_eq!(
            classify_probe(
                &json!({"kind": "gh", "argv": ["api", "--hostname", "git.example.com", "user"]})
            )
            .as_deref(),
            Some("gh api user")
        );
        assert_eq!(
            classify_probe(&json!({"kind": "git", "argv": ["branch", "--show-current"]}))
                .as_deref(),
            Some("git current branch")
        );
    }

    #[test]
    fn summarizes_failures_slow_calls_unsupported_and_routes() {
        let summary = summarize_trace(
            &[
                json!({"kind":"gh","argv":["pr","status"],"exit_code":1,"duration_ms":25,"route":{"kind":"forgejo","reason":"git remote"}}),
                json!({"kind":"gh","argv":["workflow","list"],"exit_code":0,"duration_ms":1200,"route":{"kind":"delegate","reason":"unsupported command"}}),
                json!({"kind":"git","argv":["branch","--show-current"],"exit_code":0,"duration_ms":5}),
            ],
            SLOW_CALL_MS,
        );

        assert_eq!(summary.total, 3);
        assert_eq!(summary.failures.len(), 1);
        assert_eq!(summary.slow_calls.len(), 1);
        assert_eq!(summary.unsupported.len(), 1);
        assert_eq!(summary.probe_counts.get("gh pr status"), Some(&1));
        assert_eq!(summary.probe_counts.get("git current branch"), Some(&1));
        assert_eq!(summary.route_counts.get("forgejo"), Some(&1));
        assert_eq!(summary.route_counts.get("delegate"), Some(&1));
    }
}
