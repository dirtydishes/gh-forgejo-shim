use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::config::normalize_host;

const HOST_FIELDS: &[&str] = &[
    "host",
    "hostname",
    "url",
    "server",
    "server_url",
    "base_url",
];

pub(crate) fn parse_json(text: &str) -> Option<Value> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = UniqueValue::deserialize(&mut deserializer).ok()?.0;
    deserializer.end().ok()?;
    Some(value)
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueValueVisitor)
    }
}

struct UniqueValueVisitor;

impl<'de> Visitor<'de> for UniqueValueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        UniqueValue::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut items = Vec::new();
        while let Some(UniqueValue(value)) = sequence.next_element()? {
            items.push(value);
        }
        Ok(UniqueValue(Value::Array(items)))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "duplicate JSON object key: {key}"
                )));
            }
            let UniqueValue(value) = map.next_value()?;
            object.insert(key, value);
        }
        Ok(UniqueValue(Value::Object(object)))
    }
}

pub(crate) fn find_token(data: &Value, host: Option<&str>) -> Option<String> {
    let host = host.map(normalize_host);
    find_json_token(data, host.as_deref())
}

fn find_json_token(data: &Value, host: Option<&str>) -> Option<String> {
    match data {
        Value::Object(object) => {
            let Some(host) = host else {
                return token_from_object(object).or_else(|| {
                    object
                        .values()
                        .find_map(|value| find_json_token(value, None))
                });
            };

            for (key, value) in object {
                if normalize_host(key) == host {
                    if let Some(token) = token_from_keyed_value(value, host) {
                        return Some(token);
                    }
                }
            }

            let contains_record_field = object
                .keys()
                .any(|key| is_host_field(key) || is_token_field(key));
            if contains_record_field {
                return token_for_record(
                    token_from_object(object),
                    json_host_values(object),
                    host,
                    false,
                );
            }

            object
                .values()
                .find_map(|value| find_json_token(value, Some(host)))
        }
        Value::Array(items) => items.iter().find_map(|item| find_json_token(item, host)),
        _ => None,
    }
}

fn token_from_keyed_value(value: &Value, host: &str) -> Option<String> {
    match value {
        Value::String(token) => nonempty(token),
        Value::Object(object) => token_for_record(
            token_from_object(object),
            json_host_values(object),
            host,
            true,
        ),
        _ => None,
    }
}

fn token_from_object(object: &Map<String, Value>) -> Option<String> {
    ["token", "access_token"]
        .into_iter()
        .find_map(|key| object.get(key).and_then(Value::as_str).and_then(nonempty))
}

fn json_host_values(object: &Map<String, Value>) -> Vec<Option<&str>> {
    HOST_FIELDS
        .iter()
        .filter_map(|key| object.get(*key).map(Value::as_str))
        .collect()
}

fn token_for_record<I, S>(
    token: Option<String>,
    host_values: I,
    expected_host: &str,
    keyed_host: bool,
) -> Option<String>
where
    I: IntoIterator<Item = Option<S>>,
    S: AsRef<str>,
{
    let token = token?;
    let mut saw_host = false;
    for value in host_values {
        saw_host = true;
        let value = value?;
        if normalize_host(value.as_ref()) != expected_host {
            return None;
        }
    }
    (keyed_host || saw_host).then_some(token)
}

pub(crate) fn find_token_in_text(text: &str, host: Option<&str>) -> Option<String> {
    if let Some(host) = host {
        let host = normalize_host(host);
        let lines = text.lines().collect::<Vec<_>>();
        let mut found_list_entry = false;

        for (index, line) in lines.iter().enumerate() {
            let Some(indent) = text_list_item_indent(line) else {
                continue;
            };
            found_list_entry = true;
            let end = lines[index + 1..]
                .iter()
                .position(|candidate| text_scope_ends(candidate, indent))
                .map_or(lines.len(), |offset| index + 1 + offset);
            if let Some(token) = token_from_text_entry(&lines[index..end], &host, Some(indent)) {
                return Some(token);
            }
        }
        if found_list_entry {
            return None;
        }
        return token_from_text_entry(&lines, &host, None);
    }
    token_line(text)
}

fn text_list_item_indent(line: &str) -> Option<usize> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    trimmed
        .starts_with("- ")
        .then_some(line.len() - trimmed.len())
}

fn text_scope_ends(line: &str, entry_indent: usize) -> bool {
    let trimmed = line.trim_start_matches([' ', '\t']);
    !trimmed.is_empty() && !trimmed.starts_with('#') && line.len() - trimmed.len() <= entry_indent
}

fn token_from_text_entry(lines: &[&str], host: &str, list_indent: Option<usize>) -> Option<String> {
    let direct_indent = list_indent.map_or(0, |indent| indent + 2);
    let mut host_values = Vec::new();
    let mut token = None;

    for (index, line) in lines.iter().enumerate() {
        let field = if index == 0 {
            list_indent
                .and_then(|_| line.trim_start_matches([' ', '\t']).strip_prefix("- "))
                .unwrap_or_else(|| line.trim_start_matches([' ', '\t']))
        } else {
            let trimmed = line.trim_start_matches([' ', '\t']);
            if line.len() - trimmed.len() != direct_indent {
                continue;
            }
            trimmed
        };
        let Some((key, value)) = text_mapping_field(field) else {
            continue;
        };
        if is_host_field(&key) {
            host_values.push(value);
        } else if is_token_field(&key) && token.is_none() {
            token = value;
        }
    }

    token_for_record(token, host_values, host, false)
}

fn text_mapping_field(field: &str) -> Option<(String, Option<String>)> {
    let field = field.trim();
    let (key, tail) = if let Some(quote) = field.chars().next().filter(|c| matches!(c, '"' | '\''))
    {
        let closing = field[quote.len_utf8()..].find(quote)? + quote.len_utf8();
        (
            &field[quote.len_utf8()..closing],
            &field[closing + quote.len_utf8()..],
        )
    } else {
        match field.find([':', '=']) {
            Some(delimiter) => (field[..delimiter].trim(), &field[delimiter..]),
            None => (field.split_whitespace().next()?, ""),
        }
    };
    let tail = tail.trim_start();
    if key.is_empty() {
        return None;
    }
    let value = (tail.starts_with(':') || tail.starts_with('='))
        .then(|| parse_token_tail(tail))
        .flatten();
    Some((key.to_ascii_lowercase(), value))
}

fn is_host_field(key: &str) -> bool {
    HOST_FIELDS.contains(&key)
}

fn is_token_field(key: &str) -> bool {
    matches!(key, "token" | "access_token")
}

fn token_line(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        for key in ["token", "access_token"] {
            if lower.starts_with(key) {
                if let Some(token) = parse_token_tail(&trimmed[key.len()..]) {
                    return Some(token);
                }
            }
        }
    }
    None
}

fn parse_token_tail(value: &str) -> Option<String> {
    let value = value.trim_start();
    let value = value
        .strip_prefix(':')
        .or_else(|| value.strip_prefix('='))?
        .trim_start();
    let value = value
        .strip_prefix('"')
        .or_else(|| value.strip_prefix('\''))
        .unwrap_or(value);
    let token = value
        .chars()
        .take_while(|character| !matches!(character, '"' | '\'' | '#' | ' ' | '\t' | '\r' | '\n'))
        .collect::<String>();
    (!token.is_empty()).then_some(token)
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn text_token_scan_handles_utf8_near_scan_boundary() {
        let padding = "a".repeat(2000 - "git.example.com".len() - 1);
        let text = format!("git.example.com{padding}é\ntoken: secret");

        assert_eq!(find_token_in_text(&text, Some("git.example.com")), None);
    }

    #[test]
    fn text_token_discovery_does_not_cross_yaml_host_entries() {
        let text = "servers:\n  - host: https://auth.target.test\n    name: target\n  - host: https://auth.other.test\n    token: other-secret\n";

        assert_eq!(find_token_in_text(text, Some("auth.target.test")), None);
    }

    #[test]
    fn text_token_discovery_does_not_inherit_a_match_across_key_order() {
        let text = "servers:\n  - host: https://auth.target.test\n    name: target\n  - name: other\n    token: other-secret\n    host: https://auth.other.test\n";

        assert_eq!(find_token_in_text(text, Some("auth.target.test")), None);
    }

    #[test]
    fn text_token_discovery_accepts_a_matching_entry_in_any_key_order() {
        let text = "servers:\n  - name: target\n    token: target-secret\n    host: https://auth.target.test\n";

        assert_eq!(
            find_token_in_text(text, Some("auth.target.test")).as_deref(),
            Some("target-secret")
        );
    }

    #[test]
    fn text_token_discovery_rejects_tokens_outside_the_direct_list_entry() {
        for text in [
            "servers:\n  - host: https://auth.target.test\n    name: target\n    credentials:\n      - name: nested\n        token: nested-secret\n",
            "servers:\n  - host: https://auth.target.test\n    name: target\nmetadata:\n  token: later-secret\n",
        ] {
            assert_eq!(find_token_in_text(text, Some("auth.target.test")), None);
        }
    }

    #[test]
    fn text_token_discovery_rejects_every_invalid_host_occurrence() {
        for text in [
            "servers:\n  - host: https://auth.target.test\n    \"host\": https://auth.other.test\n    token: other-secret\n",
            "servers:\n  - host:\n    url: https://auth.target.test\n    token: target-secret\n",
        ] {
            assert_eq!(find_token_in_text(text, Some("auth.target.test")), None);
        }
    }

    #[test]
    fn text_token_discovery_matches_the_whole_normalized_host() {
        let text =
            "servers:\n  - host: https://auth.target.test.evil\n    token: substring-secret\n";

        assert_eq!(find_token_in_text(text, Some("auth.target.test")), None);
    }

    #[test]
    fn json_token_discovery_accepts_keyed_and_explicit_hosts() {
        for data in [
            json!({"hosts": {"auth.target.test": {"token": "keyed-secret"}}}),
            json!({"servers": [{"host": "auth.target.test", "token": "direct-secret"}]}),
        ] {
            assert!(find_token(&data, Some("auth.target.test")).is_some());
        }
    }

    #[test]
    fn json_token_discovery_rejects_every_invalid_host_field() {
        for data in [
            json!({
                "servers": [{
                    "host": "auth.target.test",
                    "server": "auth.other.test",
                    "token": "other-secret"
                }]
            }),
            json!({
                "servers": [{
                    "host": "auth.target.test",
                    "server": "",
                    "token": "empty-secret"
                }]
            }),
            json!({
                "servers": [{
                    "host": "auth.target.test",
                    "server": 42,
                    "token": "invalid-secret"
                }]
            }),
        ] {
            assert_eq!(find_token(&data, Some("auth.target.test")), None);
        }
    }

    #[test]
    fn json_token_discovery_rejects_a_conflict_inside_a_matching_key() {
        let data = json!({
            "hosts": {
                "auth.target.test": {
                    "host": "auth.other.test",
                    "token": "other-secret"
                }
            }
        });

        assert_eq!(find_token(&data, Some("auth.target.test")), None);
    }

    #[test]
    fn json_token_discovery_validates_record_fields_before_keyed_children() {
        for data in [
            json!({
                "host": "auth.other.test",
                "auth.target.test": {"token": "conflict-secret"}
            }),
            json!({
                "host": "",
                "auth.target.test": {"token": "empty-secret"}
            }),
            json!({
                "host": 42,
                "auth.target.test": {"token": "invalid-secret"}
            }),
            json!({
                "token": "container-secret",
                "auth.target.test": {"token": "nested-secret"}
            }),
        ] {
            assert_eq!(find_token(&data, Some("auth.target.test")), None);
        }
    }

    #[test]
    fn json_token_discovery_rejects_ambiguous_matching_keyed_entries() {
        for data in [
            json!({
                "hosts": {
                    "auth.target.test": {"token": "first-secret"},
                    "https://auth.target.test/path": {"token": "second-secret"}
                }
            }),
            json!({
                "hosts": {
                    "auth.target.test": {
                        "host": "auth.other.test",
                        "token": "invalid-secret"
                    },
                    "https://auth.target.test/path": {"token": "valid-secret"}
                }
            }),
        ] {
            assert_eq!(find_token(&data, Some("auth.target.test")), None);
        }
    }
}
