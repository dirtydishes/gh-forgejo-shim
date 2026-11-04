#![forbid(unsafe_code)]

use std::collections::HashMap;
use yaml_rust2::{
    parser::{Event, Parser},
    scanner::TScalarStyle,
};

const MAX_WORKFLOW_BYTES: usize = 256 * 1024;

pub(super) fn from_content(content: &str, path: &str) -> String {
    parse(content).unwrap_or_else(|| path.to_owned())
}

fn parse(content: &str) -> Option<String> {
    if content.len() > MAX_WORKFLOW_BYTES {
        return None;
    }
    let mut parser = Parser::new_from_str(content);
    if parser.next_token().ok()?.0 != Event::StreamStart
        || parser.next_token().ok()?.0 != Event::DocumentStart
        || !matches!(parser.next_token().ok()?.0, Event::MappingStart(..))
    {
        return None;
    }

    // Consume the whole document so errors after `name` also cause a fallback.
    // Store only scalar anchors; collection aliases never expand into YAML trees.
    let mut anchors = HashMap::new();
    let mut depth = 1_usize;
    let mut expecting_key = true;
    let mut key_is_name = false;
    let mut seen_name = false;
    let mut name = None;
    loop {
        let value = match parser.next_token().ok()?.0 {
            Event::StreamEnd => return name,
            Event::DocumentStart => return None,
            Event::MappingStart(..) | Event::SequenceStart(..) => {
                if depth == 1 {
                    if !expecting_key && key_is_name {
                        return None;
                    }
                    expecting_key = !expecting_key;
                    key_is_name = false;
                }
                depth += 1;
                continue;
            }
            Event::MappingEnd | Event::SequenceEnd => {
                depth = depth.checked_sub(1)?;
                continue;
            }
            Event::Scalar(value, style, anchor, tag) => {
                let string_tag = tag
                    .as_ref()
                    .is_some_and(|tag| tag.handle == "tag:yaml.org,2002:" && tag.suffix == "str");
                let null = style == TScalarStyle::Plain
                    && matches!(value.as_str(), "" | "~" | "null" | "Null" | "NULL");
                let value = ((tag.is_none() && !null) || string_tag).then_some(value);
                if anchor != 0 {
                    anchors.insert(anchor, value.clone());
                }
                value
            }
            Event::Alias(anchor) => anchors.get(&anchor).cloned().flatten(),
            _ => continue,
        };
        if depth == 1 {
            if expecting_key {
                key_is_name = value.as_deref() == Some("name");
                if key_is_name {
                    if seen_name {
                        return None;
                    }
                    seen_name = true;
                }
            } else if key_is_name {
                name = Some(value?);
            }
            expecting_key = !expecting_key;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = ".forgejo/workflows/ci.yml";

    #[test]
    fn reads_only_the_top_level_name() {
        let content = "on: [push]\njobs:\n  test:\n    name: job name\nname: native ci\n";
        assert_eq!(from_content(content, PATH), "native ci");
    }

    #[test]
    fn reads_quoted_multiline_and_anchored_names() {
        for (content, expected) in [
            ("name: 'ci: checks #1'\n", "ci: checks #1"),
            ("\"name\": \"ci\\nchecks\"\n", "ci\nchecks"),
            ("name: >-\n  ci\n  checks\n", "ci checks"),
            ("name: |\n  ci\n  checks\n", "ci\nchecks\n"),
            ("name: &title ci checks\n", "ci checks"),
            ("title: &title ci checks\nname: *title\n", "ci checks"),
            (
                "jobs: {test: {name: &title ci checks}}\nname: *title\n",
                "ci checks",
            ),
            ("name: !!str null\n", "null"),
        ] {
            assert_eq!(from_content(content, PATH), expected, "{content}");
        }
    }

    #[test]
    fn missing_or_non_scalar_names_use_the_path() {
        for content in [
            "on: [push]\njobs: {test: {name: job name}}\n",
            "",
            "[name, ci]",
            "name:",
            "name: null",
            "name: [ci, checks]",
            "name: {title: ci}",
            "title: &title [ci]\nname: *title\n",
        ] {
            assert_eq!(from_content(content, PATH), PATH, "{content}");
        }
    }

    #[test]
    fn caps_workflow_bytes_before_parsing() {
        let mut content = "name: ci\n#".to_owned();
        content.push_str(&"x".repeat(MAX_WORKFLOW_BYTES - content.len()));
        assert_eq!(content.len(), MAX_WORKFLOW_BYTES);
        assert_eq!(from_content(&content, PATH), "ci");
        content.push('x');
        assert_eq!(from_content(&content, PATH), PATH);

        let content = format!("name: ci\n#{}", "é".repeat(MAX_WORKFLOW_BYTES / 2));
        assert!(content.chars().count() < MAX_WORKFLOW_BYTES);
        assert_eq!(from_content(&content, PATH), PATH);
    }

    #[test]
    fn malformed_or_ambiguous_yaml_uses_the_path() {
        for content in [
            "name: 'unterminated",
            "name: ci\njobs: [unterminated",
            "name: ci\njobs:\n  test: true\n broken: false",
            "name: *missing",
            "name: ci\nname: other",
            "name: ci\n---\nname: other",
        ] {
            assert_eq!(from_content(content, PATH), PATH, "{content}");
        }
    }

    #[test]
    fn skips_collection_aliases_without_expanding_them() {
        let mut content = "name: ci\na0: &a0 [x, x]\n".to_owned();
        for index in 1..32 {
            let previous = index - 1;
            content.push_str(&format!(
                "a{index}: &a{index} [*a{previous}, *a{previous}]\n"
            ));
        }
        assert_eq!(from_content(&content, PATH), "ci");

        let content = format!("name: ci\njobs: {}0{}", "[".repeat(128), "]".repeat(128));
        assert_eq!(from_content(&content, PATH), "ci");

        // The scanner rejects nesting past its own limit; that also uses the path.
        let content = format!("name: ci\njobs: {}0{}", "[".repeat(1024), "]".repeat(1024));
        assert_eq!(from_content(&content, PATH), PATH);
    }
}
