//! One grammar-driven parse for routing and local gh argument validation.
//!
//! Keep routing hints even when validation fails: a late `-R` must not let an
//! invalid Forgejo invocation escape to the real gh.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Grammar {
    command: BTreeMap<String, Command>,
}

#[derive(Debug, Deserialize)]
struct Command {
    usage: String,
    support: String,
    #[serde(default)]
    json_fields: Vec<String>,
    #[serde(default, rename = "flag")]
    flags: Vec<Flag>,
}

#[derive(Debug, Deserialize)]
struct Flag {
    long: String,
    short: Option<String>,
    takes_value: bool,
    value: Option<String>,
    #[serde(default)]
    inherited: bool,
}

#[derive(Debug, Deserialize)]
struct Usage {
    suffix: String,
    commands: Vec<String>,
    #[serde(default)]
    help: String,
}

fn grammar() -> &'static Grammar {
    static GRAMMAR: OnceLock<Grammar> = OnceLock::new();
    GRAMMAR.get_or_init(|| {
        toml::from_str(include_str!("../../../contract/tiers.toml"))
            .expect("embedded gh grammar must be valid")
    })
}

fn usages() -> &'static BTreeMap<String, Usage> {
    static USAGE: OnceLock<BTreeMap<String, Usage>> = OnceLock::new();
    USAGE.get_or_init(|| {
        serde_json::from_str(include_str!("../../../contract/usage-errors.json"))
            .expect("embedded gh usage captures must be valid")
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationError {
    Flag {
        command: String,
        message: String,
    },
    UnknownCommand {
        parent: String,
        name: String,
    },
    JsonFields {
        command: String,
        unknown: Option<String>,
    },
    Argument(String),
}

impl InvocationError {
    pub const fn exit_code(&self) -> i32 {
        1
    }

    pub fn render(&self) -> String {
        match self {
            Self::Flag { command, message } => format!(
                "{message}\n{}{}",
                usage_suffix(command),
                if usages()
                    .get(command)
                    .is_some_and(|usage| !usage.commands.is_empty())
                {
                    "\n"
                } else {
                    ""
                }
            ),
            Self::UnknownCommand { parent, name } => {
                let suggestions = usages()
                    .get(parent)
                    .map(|usage| {
                        usage
                            .commands
                            .iter()
                            .filter(|cmd| {
                                cmd.starts_with(name.as_str()) || edit_distance(name, cmd) <= 2
                            })
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let mut text = format!("unknown command {name:?} for \"gh {parent}\"\n");
                if !suggestions.is_empty() {
                    text.push_str("\nDid you mean this?\n");
                    for suggestion in suggestions {
                        text.push_str(&format!("\t{suggestion}\n"));
                    }
                }
                text.push_str(&usage_suffix(parent));
                text
            }
            Self::JsonFields { command, unknown } => {
                let mut text = match unknown {
                    Some(field) => format!("Unknown JSON field: {field:?}\nAvailable fields:\n"),
                    None => {
                        "Specify one or more comma-separated fields for `--json`:\n".to_string()
                    }
                };
                if let Some(spec) = grammar().command.get(command) {
                    for field in &spec.json_fields {
                        text.push_str(&format!("  {field}\n"));
                    }
                }
                text
            }
            Self::Argument(message) => format!("{message}\n"),
        }
    }
}

fn usage_suffix(command: &str) -> String {
    usages()
        .get(command)
        .map(|usage| usage.suffix.clone())
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HandlerArg {
    Positional(String),
    Flag {
        long: String,
        value: String,
        takes_value: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Invocation {
    pub command: String,
    pub positionals: Vec<String>,
    pub flags: BTreeMap<String, Vec<String>>,
    pub json_fields: Option<Vec<String>>,
    pub repo_override: Option<String>,
    error: Option<InvocationError>,
    // Preserve flag/positional order for the existing handler parsers, expanding
    // attached short values and clusters into the forms they already understand.
    handler_args: Vec<HandlerArg>,
}

impl Invocation {
    pub fn parse(argv: &[String]) -> Self {
        let (command, command_words, unknown_command) = discover_command(argv);
        let mut parsed = Self {
            command,
            ..Self::default()
        };
        // Cobra finds command words first, then pflag parses the remaining argv.
        // This matters when the same shorthand means different things in sibling commands.
        let argv = argv
            .iter()
            .enumerate()
            .filter(|(index, _)| !command_words.contains(index))
            .map(|(_, arg)| arg.clone())
            .collect::<Vec<_>>();
        let mut index = 0;
        let mut flags_ended = false;
        while index < argv.len() {
            let arg = &argv[index];
            if !flags_ended && arg == "--" {
                flags_ended = true;
            } else if !flags_ended && arg.starts_with("--") {
                let (name, attached) = arg[2..]
                    .split_once('=')
                    .map_or((&arg[2..], None), |(name, value)| (name, Some(value)));
                if let Some(flag) = parsed.find_flag(name, false) {
                    let value =
                        parsed.flag_value(flag, attached, &argv, &mut index, &format!("--{name}"));
                    if let Some(value) = value {
                        parsed.add_flag(flag, value);
                    }
                } else {
                    parsed.fail_flag(format!("unknown flag: --{name}"));
                }
            } else if !flags_ended && arg.starts_with('-') && arg != "-" {
                for (offset, short) in arg[1..].char_indices() {
                    let rest = &arg[1 + offset + short.len_utf8()..];
                    let remaining = format!("-{}", &arg[1 + offset..]);
                    if let Some(flag) = parsed.find_flag(&short.to_string(), true) {
                        let attached = if flag.takes_value && !rest.is_empty() {
                            Some(rest.strip_prefix('=').unwrap_or(rest))
                        } else {
                            rest.strip_prefix('=')
                        };
                        let value = parsed.flag_value(
                            flag,
                            attached,
                            &argv,
                            &mut index,
                            &format!("'{short}' in {remaining}"),
                        );
                        if let Some(value) = value {
                            parsed.add_flag(flag, value);
                        }
                        if flag.takes_value || attached.is_some() {
                            break;
                        }
                    } else {
                        parsed
                            .fail_flag(format!("unknown shorthand flag: '{short}' in {remaining}"));
                        break;
                    }
                }
            } else {
                parsed.positionals.push(arg.clone());
                parsed
                    .handler_args
                    .push(HandlerArg::Positional(arg.clone()));
            }
            index += 1;
        }
        if let Some((parent, name)) = unknown_command {
            if let Some(InvocationError::Flag { command, .. }) = &mut parsed.error {
                *command = parent;
            } else if parsed.error.is_none() {
                parsed.fail(InvocationError::UnknownCommand { parent, name });
            }
        } else if let Some(spec) = grammar().command.get(&parsed.command) {
            parsed.validate_command(spec);
        } else if !usages()
            .get(&parsed.command)
            .is_some_and(|usage| !usage.commands.is_empty())
            && matches!(parsed.error, Some(InvocationError::Flag { .. }))
        {
            // The table covers only part of gh. Fail other commands as unsupported.
            parsed.error = None;
        }
        parsed
    }

    pub fn validate(&self) -> Result<(), &InvocationError> {
        self.error.as_ref().map_or(Ok(()), Err)
    }

    pub fn flag(&self, name: &str) -> Option<&str> {
        self.flags
            .get(name)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    pub fn is_native(&self) -> bool {
        grammar()
            .command
            .get(&self.command)
            .is_some_and(|spec| spec.support == "native")
            && (self.command != "api"
                || matches!(
                    self.positionals.first().map(String::as_str),
                    Some("user" | "/user")
                ))
    }

    pub fn has_grammar(&self) -> bool {
        grammar().command.contains_key(&self.command)
    }

    pub fn help(&self) -> Option<&str> {
        (self.flag("help") == Some("true"))
            .then(|| usages().get(&self.command))
            .flatten()
            .map(|usage| usage.help.as_str())
    }

    pub fn handler_argv(&self) -> Vec<String> {
        let mut argv = self
            .command
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut last = BTreeMap::new();
        for (index, arg) in self.handler_args.iter().enumerate() {
            if let HandlerArg::Flag { long, .. } = arg {
                last.insert(long, index);
            }
        }
        for (index, arg) in self.handler_args.iter().enumerate() {
            match arg {
                HandlerArg::Positional(value) => argv.push(value.clone()),
                HandlerArg::Flag {
                    long,
                    value,
                    takes_value,
                } => {
                    if long == "json" {
                        if last.get(long) == Some(&index) {
                            argv.extend([
                                "--json".into(),
                                self.json_fields.as_deref().unwrap_or_default().join(","),
                            ]);
                        }
                    } else if *takes_value {
                        argv.extend([format!("--{long}"), value.clone()]);
                    } else if value == "true" && last.get(long) == Some(&index) {
                        argv.push(format!("--{long}"));
                    }
                }
            }
        }
        argv
    }

    fn fail(&mut self, error: InvocationError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    fn fail_flag(&mut self, message: String) {
        self.fail(InvocationError::Flag {
            command: self.command.clone(),
            message,
        });
    }

    fn find_flag(&self, name: &str, short: bool) -> Option<&'static Flag> {
        let matches = |flag: &&Flag| {
            if short {
                flag.short.as_deref() == Some(name)
            } else {
                flag.long == name
            }
        };
        if let Some(spec) = grammar().command.get(&self.command) {
            if let Some(flag) = spec.flags.iter().find(matches) {
                return Some(flag);
            }
            return help_flag(name, short);
        }
        grammar()
            .command
            .iter()
            .filter(|(command, _)| {
                self.command.is_empty() || command.starts_with(&format!("{} ", self.command))
            })
            .flat_map(|(_, spec)| &spec.flags)
            .filter(|flag| flag.inherited)
            .find(matches)
            .or_else(|| help_flag(name, short))
            // Repository hints still matter for commands missing from the grammar.
            .or_else(|| {
                grammar().command["pr view"]
                    .flags
                    .iter()
                    .find(|flag| flag.long == "repo" && matches(flag))
            })
            .or_else(|| {
                grammar().command["auth status"].flags.iter().find(|flag| {
                    self.command.starts_with("auth ") && flag.long == "hostname" && matches(flag)
                })
            })
    }

    fn flag_value(
        &mut self,
        flag: &Flag,
        attached: Option<&str>,
        argv: &[String],
        index: &mut usize,
        spelling: &str,
    ) -> Option<String> {
        if !flag.takes_value {
            let value = attached.unwrap_or("true");
            let boolean = match value {
                "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
                "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
                _ => None,
            };
            return match boolean {
                Some(value) => Some(value.to_string()),
                None => {
                    let label = flag.short.as_ref().map_or_else(
                        || format!("--{}", flag.long),
                        |short| format!("-{short}, --{}", flag.long),
                    );
                    self.fail_flag(format!("invalid argument {value:?} for {label:?} flag: strconv.ParseBool: parsing {value:?}: invalid syntax"));
                    None
                }
            };
        }
        let value = if let Some(value) = attached {
            value.to_string()
        } else if let Some(value) = argv.get(*index + 1) {
            *index += 1;
            value.clone()
        } else {
            if flag.long == "json" {
                self.fail(InvocationError::JsonFields {
                    command: self.command.clone(),
                    unknown: None,
                });
            } else {
                self.fail_flag(format!("flag needs an argument: {spelling}"));
            }
            return None;
        };
        if matches!(flag.value.as_deref(), Some("int" | "uint")) {
            let unsigned = flag.value.as_deref() == Some("uint");
            if let Err(reason) = validate_integer(&value, unsigned) {
                let label = flag.short.as_ref().map_or_else(
                    || format!("--{}", flag.long),
                    |short| format!("-{short}, --{}", flag.long),
                );
                let conversion = if unsigned { "ParseUint" } else { "ParseInt" };
                self.fail_flag(format!("invalid argument {value:?} for {label:?} flag: strconv.{conversion}: parsing {value:?}: {reason}"));
            }
        }
        Some(value)
    }

    fn add_flag(&mut self, flag: &Flag, value: String) {
        if flag.long == "repo" {
            self.repo_override = Some(value.clone());
        }
        if flag.long == "json" {
            match csv_fields(&value) {
                Ok(fields) => self.json_fields.get_or_insert_with(Vec::new).extend(fields),
                Err(error) => self.fail_flag(format!(
                    "invalid argument {value:?} for \"--json\" flag: {error}"
                )),
            }
        }
        self.flags
            .entry(flag.long.clone())
            .or_default()
            .push(value.clone());
        self.handler_args.push(HandlerArg::Flag {
            long: flag.long.clone(),
            value,
            takes_value: flag.takes_value,
        });
    }

    fn validate_command(&mut self, spec: &Command) {
        if self.flag("help") == Some("true") {
            return;
        }
        let positional_usage = spec
            .usage
            .strip_prefix(&format!("gh {}", self.command))
            .unwrap_or("")
            .trim();
        let count = self.positionals.len();
        if positional_usage == "[flags]" && count > 0 {
            let word = if count == 1 { "argument" } else { "arguments" };
            let arguments = self
                .positionals
                .iter()
                .map(|value| format!("{value:?}"))
                .collect::<Vec<_>>()
                .join(" ");
            self.fail_flag(format!(
                "unknown {word} {}",
                if count == 1 {
                    arguments
                } else {
                    format!("[{arguments}]")
                }
            ));
        } else if (positional_usage.starts_with('{') || positional_usage.starts_with('<'))
            && count != 1
        {
            self.fail(InvocationError::Argument(format!(
                "accepts 1 arg(s), received {count}"
            )));
        } else if positional_usage != "[flags]" && count > 1 {
            self.fail(InvocationError::Argument(format!(
                "accepts at most 1 arg(s), received {count}"
            )));
        }
        if let Some(fields) = &self.json_fields {
            if let Some(field) = fields
                .iter()
                .find(|field| !spec.json_fields.contains(field))
            {
                self.fail(InvocationError::JsonFields {
                    command: self.command.clone(),
                    unknown: Some(field.clone()),
                });
            } else if fields.is_empty() {
                self.fail(InvocationError::Argument(
                    "Find error: no fields specified".into(),
                ));
            }
        }
        if self.json_fields.is_none() && !spec.json_fields.is_empty() {
            for flag in ["jq", "template"] {
                if self.flag(flag).is_some_and(|value| !value.is_empty()) {
                    self.fail(InvocationError::Argument(format!(
                        "cannot use `--{flag}` without specifying `--json`"
                    )));
                }
            }
        }
    }
}

// Only command discovery consults sibling flag sets. The actual flag parser uses
// the selected command's grammar, so `-b` means body on create and branch on checkout.
fn discover_command(argv: &[String]) -> (String, Vec<usize>, Option<(String, String)>) {
    let mut command = String::new();
    let mut words = Vec::new();
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "--" {
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, attached) = long
                .split_once('=')
                .map_or((long, false), |(name, _)| (name, true));
            if !attached && discovery_takes_value(&command, name, false) {
                index += 1;
            }
        } else if arg.starts_with('-') && arg != "-" {
            for (offset, short) in arg[1..].char_indices() {
                if discovery_takes_value(&command, &short.to_string(), true) {
                    if 1 + offset + short.len_utf8() == arg.len() {
                        index += 1;
                    }
                    break;
                }
            }
        } else if command.is_empty() {
            command = arg.clone();
            words.push(index);
            if !usages()
                .get(&command)
                .is_some_and(|usage| !usage.commands.is_empty())
            {
                break;
            }
        } else {
            let verb = match (command.as_str(), arg.as_str()) {
                ("pr", "co") => "checkout",
                ("pr" | "issue", "new") => "create",
                ("pr" | "issue", "ls") => "list",
                _ => arg,
            };
            words.push(index);
            let unknown = !usages()[&command].commands.iter().any(|name| name == verb);
            let full = format!("{command} {verb}");
            return (full, words, unknown.then(|| (command, arg.clone())));
        }
        index += 1;
    }
    (command, words, None)
}

fn discovery_takes_value(command: &str, name: &str, short: bool) -> bool {
    let flags = grammar()
        .command
        .iter()
        .filter(|(key, _)| command.is_empty() || key.starts_with(&format!("{command} ")))
        .flat_map(|(_, spec)| &spec.flags)
        .filter(|flag| {
            if short {
                flag.short.as_deref() == Some(name)
            } else {
                flag.long == name
            }
        })
        .collect::<Vec<_>>();
    if flags.iter().any(|flag| flag.takes_value) {
        return true;
    }
    // Missing-table commands may still select a host explicitly.
    (name == "repo" || (short && name == "R"))
        || (command.starts_with("auth") && (name == "hostname" || (short && name == "h")))
}

// pflag uses strconv with base 0, accepting Go's prefixes and digit separators.
fn validate_integer(value: &str, unsigned: bool) -> Result<(), &'static str> {
    let (negative, digits) = match value.as_bytes().first() {
        Some(b'-') if !unsigned => (true, &value[1..]),
        Some(b'+') if !unsigned => (false, &value[1..]),
        _ => (false, value),
    };
    let (radix, digits, prefix) = if digits.starts_with("0x") || digits.starts_with("0X") {
        (16, &digits[2..], true)
    } else if digits.starts_with("0b") || digits.starts_with("0B") {
        (2, &digits[2..], true)
    } else if digits.starts_with("0o") || digits.starts_with("0O") {
        (8, &digits[2..], true)
    } else if digits.starts_with('0') && digits.len() > 1 {
        (8, &digits[1..], true)
    } else {
        (10, digits, false)
    };
    let mut previous_digit = prefix;
    for ch in digits.chars() {
        if ch == '_' && previous_digit {
            previous_digit = false;
        } else if ch.is_digit(radix) {
            previous_digit = true;
        } else {
            return Err("invalid syntax");
        }
    }
    if digits.is_empty() || !previous_digit {
        return Err("invalid syntax");
    }
    let magnitude =
        u64::from_str_radix(&digits.replace('_', ""), radix).map_err(|_| "value out of range")?;
    if !unsigned && magnitude > (i64::MAX as u64) + u64::from(negative) {
        return Err("value out of range");
    }
    Ok(())
}

// JSON fields use pflag's StringSlice CSV rules, including quoted commas.
fn csv_fields(value: &str) -> Result<Vec<String>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let bytes = value.as_bytes();
    let mut fields = Vec::new();
    let mut offset = 0;
    loop {
        let mut field = Vec::new();
        if bytes.get(offset) == Some(&b'"') {
            offset += 1;
            loop {
                match bytes.get(offset) {
                    Some(b'"') if bytes.get(offset + 1) == Some(&b'"') => {
                        field.push(b'"');
                        offset += 2;
                    }
                    Some(b'"') => {
                        offset += 1;
                        if !matches!(bytes.get(offset), None | Some(b',' | b'\n' | b'\r')) {
                            return Err(csv_error(
                                value,
                                offset,
                                "extraneous or missing \" in quoted-field",
                            ));
                        }
                        break;
                    }
                    Some(byte) => {
                        field.push(*byte);
                        offset += 1;
                    }
                    None => {
                        return Err(csv_error(
                            value,
                            offset,
                            "extraneous or missing \" in quoted-field",
                        ))
                    }
                }
            }
        } else {
            while let Some(byte) = bytes.get(offset) {
                if matches!(byte, b',' | b'\n')
                    || (*byte == b'\r' && bytes.get(offset + 1) == Some(&b'\n'))
                {
                    break;
                }
                if *byte == b'"' {
                    return Err(csv_error(value, offset, "bare \" in non-quoted-field"));
                }
                field.push(*byte);
                offset += 1;
            }
        }
        fields.push(String::from_utf8(field).expect("CSV slices preserve UTF-8"));
        if bytes.get(offset) != Some(&b',') {
            return Ok(fields);
        }
        offset += 1;
    }
}

fn csv_error(value: &str, offset: usize, message: &str) -> String {
    let line = value[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let column = value[..offset]
        .rfind('\n')
        .map_or(offset + 1, |newline| offset - newline);
    format!("parse error on line {line}, column {column}: {message}")
}

fn help_flag(name: &str, short: bool) -> Option<&'static Flag> {
    static HELP: OnceLock<Flag> = OnceLock::new();
    ((!short && name == "help") || (short && name == "h")).then(|| {
        HELP.get_or_init(|| Flag {
            long: "help".into(),
            short: Some("h".into()),
            takes_value: false,
            value: None,
            inherited: false,
        })
    })
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut row = (0..=right.chars().count()).collect::<Vec<_>>();
    for (i, a) in left.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in right.chars().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(above + 1)
                .min(diagonal + usize::from(a != b));
            diagonal = above;
        }
    }
    row.last().copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Invocation {
        Invocation::parse(
            &args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn captured_client_errors_match_real_gh_verbatim() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/cases");
        let mut count = 0;
        for entry in std::fs::read_dir(root).expect("contract cases") {
            let path = entry.expect("case entry").path();
            if !path
                .file_stem()
                .expect("filename")
                .to_string_lossy()
                .starts_with("error-")
            {
                continue;
            }
            let case: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).expect("case"))
                    .expect("case JSON");
            let argv = serde_json::from_value::<Vec<String>>(case["argv"].clone()).expect("argv");
            let parsed = Invocation::parse(&argv);
            let error = parsed.validate().expect_err("client error");
            assert_eq!(
                error.render(),
                case["oracle"]["stderr"].as_str().expect("stderr"),
                "{argv:?}"
            );
            assert_eq!(
                error.exit_code(),
                case["oracle"]["exit"].as_i64().expect("exit") as i32
            );
            count += 1;
        }
        assert!(count >= 8);
    }

    #[test]
    fn repo_forms_repetitions_and_flags_before_commands() {
        for args in [
            vec!["pr", "view", "1", "-R", "forge.test/a/b"],
            vec!["pr", "view", "1", "-Rforge.test/a/b"],
            vec!["pr", "view", "1", "-R=forge.test/a/b"],
            vec!["--repo=forge.test/a/b", "pr", "view", "1"],
            vec!["pr", "-R", "forge.test/a/b", "view", "1"],
            vec![
                "pr",
                "view",
                "1",
                "-Rgithub.com/a/b",
                "--repo=forge.test/a/b",
            ],
        ] {
            let parsed = parse(&args);
            assert_eq!(parsed.validate(), Ok(()), "{args:?}");
            assert_eq!(parsed.repo_override.as_deref(), Some("forge.test/a/b"));
            assert_eq!(parsed.positionals, ["1"]);
        }
    }

    #[test]
    fn boolean_clusters_and_attached_values_expand_for_handlers() {
        let parsed = parse(&["pr", "checkout", "-fbtopic", "1"]);
        assert_eq!(parsed.validate(), Ok(()));
        assert_eq!(parsed.flag("force"), Some("true"));
        assert_eq!(parsed.flag("branch"), Some("topic"));
        assert_eq!(
            parsed.handler_argv(),
            ["pr", "checkout", "--force", "--branch", "topic", "1"]
        );
        let parsed = parse(&["pr", "view", "-cw=false", "1"]);
        assert_eq!(parsed.validate(), Ok(()));
        assert_eq!(parsed.flag("comments"), Some("true"));
        assert_eq!(parsed.flag("web"), Some("false"));
    }

    #[test]
    fn double_dash_and_flag_values_do_not_supply_routing_hints() {
        let parsed = parse(&["pr", "view", "--", "--repo=github.com/a/b"]);
        assert_eq!(parsed.repo_override, None);
        assert_eq!(parsed.positionals, ["--repo=github.com/a/b"]);
        let parsed = parse(&["pr", "edit", "1", "--title", "--repo=github.com/a/b"]);
        assert_eq!(parsed.repo_override, None);
        assert_eq!(parsed.flag("title"), Some("--repo=github.com/a/b"));
    }

    #[test]
    fn repeated_json_accumulates_and_api_jq_needs_no_json_flag() {
        let parsed = parse(&["pr", "view", "1", "--json=number,title", "--json", "state"]);
        assert_eq!(parsed.validate(), Ok(()));
        assert_eq!(
            parsed.json_fields,
            Some(vec!["number".into(), "title".into(), "state".into()])
        );
        assert_eq!(parse(&["api", "user", "--jq", ".login"]).validate(), Ok(()));
        assert_eq!(
            parsed.handler_argv(),
            ["pr", "view", "1", "--json", "number,title,state"]
        );
    }

    #[test]
    fn sibling_shorthands_use_the_selected_command_grammar() {
        let parsed = parse(&["pr", "-btext", "create"]);
        assert_eq!(parsed.validate(), Ok(()));
        assert_eq!(parsed.flag("body"), Some("text"));
        for args in [vec!["run", "-a2", "view"], vec!["run", "-a", "2", "view"]] {
            let parsed = parse(&args);
            assert_eq!(parsed.validate(), Ok(()));
            assert_eq!(parsed.command, "run view");
            assert_eq!(parsed.flag("attempt"), Some("2"));
        }
        let parsed = parse(&["run", "-a", "list"]);
        assert_eq!(parsed.command, "run");
        assert!(parsed
            .validate()
            .expect_err("Cobra treats list as the value during discovery")
            .render()
            .starts_with("unknown shorthand flag: 'a' in -a"));
    }

    #[test]
    fn integer_and_json_csv_rules_match_pflag() {
        for value in ["0", "0x20", "010", "0b10", "0o7", "1_0", "+1", "-1"] {
            assert_eq!(validate_integer(value, false), Ok(()), "{value}");
        }
        for value in ["08", "nope", "1__0", "_1", "1_", "", "-"] {
            assert_eq!(
                validate_integer(value, false),
                Err("invalid syntax"),
                "{value}"
            );
        }
        assert_eq!(
            validate_integer("18446744073709551616", true),
            Err("value out of range")
        );
        assert_eq!(
            csv_fields("\"number,title\",state"),
            Ok(vec!["number,title".into(), "state".into()])
        );
        assert_eq!(csv_fields("number,"), Ok(vec!["number".into(), "".into()]));
        assert_eq!(
            csv_fields("\"number"),
            Err("parse error on line 1, column 8: extraneous or missing \" in quoted-field".into())
        );
    }

    #[test]
    fn positional_and_syntax_errors_precede_json_validation() {
        let parsed = parse(&["issue", "view", "--json", "x"]);
        assert_eq!(
            parsed.validate().expect_err("missing selector").render(),
            "accepts 1 arg(s), received 0\n"
        );
        let parsed = parse(&["pr", "view", "1", "--json", "x", "--gfjs-unknown"]);
        assert!(parsed
            .validate()
            .expect_err("unknown flag")
            .render()
            .starts_with("unknown flag:"));
        let parsed = parse(&["pr", "view", "1", "--web", "--web=false"]);
        assert_eq!(parsed.handler_argv(), ["pr", "view", "1"]);
    }

    #[test]
    fn late_repository_override_survives_a_parse_error() {
        let parsed = parse(&["pr", "view", "--gfjs-unknown", "-Rforge.test/a/b"]);
        assert!(parsed.validate().is_err());
        assert_eq!(parsed.repo_override.as_deref(), Some("forge.test/a/b"));
    }

    #[test]
    fn valid_commands_missing_from_the_table_stay_local_and_unsupported() {
        let parsed = parse(&["pr", "lock", "1"]);
        assert_eq!(parsed.validate(), Ok(()));
        assert!(!parsed.is_native());
        assert!(!parse(&["api", "repos/a/b/pulls"]).is_native());
        assert!(parse(&["api", "user"]).is_native());
    }
}
