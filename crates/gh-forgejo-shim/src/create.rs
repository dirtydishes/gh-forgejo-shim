//! Forgejo create workflow parsing and git-derived defaults.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::external::git_output;
use crate::{Result, ShimError};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateOptions {
    pub title: Option<String>,
    pub body: Option<String>,
    pub body_file: Option<String>,
    pub base: Option<String>,
    pub head: Option<String>,
    pub repo: Option<String>,
    pub fill: bool,
    pub fill_first: bool,
    pub fill_verbose: bool,
    pub web: bool,
    pub draft: bool,
    pub json_fields: Vec<String>,
}

const FLAGS_WITH_VALUES: &[(&str, CreateValue)] = &[
    ("--title", CreateValue::Title),
    ("-t", CreateValue::Title),
    ("--body", CreateValue::Body),
    ("-b", CreateValue::Body),
    ("--body-file", CreateValue::BodyFile),
    ("-F", CreateValue::BodyFile),
    ("--base", CreateValue::Base),
    ("-B", CreateValue::Base),
    ("--head", CreateValue::Head),
    ("-H", CreateValue::Head),
    ("--repo", CreateValue::Repo),
    ("-R", CreateValue::Repo),
    ("--json", CreateValue::JsonFields),
];

const UNSUPPORTED_WITH_VALUES: &[&str] = &[
    "--reviewer",
    "--reviewers",
    "-r",
    "--assignee",
    "--assignees",
    "-a",
    "--label",
    "--labels",
    "-l",
    "--project",
    "--projects",
    "-p",
    "--milestone",
    "-m",
    "--template",
    "-T",
    "--recover",
];

const UNSUPPORTED_BOOL_FLAGS: &[&str] = &[
    "--maintainer-can-modify",
    "--no-maintainer-edit",
    "--no-maintainer-can-modify",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateValue {
    Title,
    Body,
    BodyFile,
    Base,
    Head,
    Repo,
    JsonFields,
}

pub fn parse_create_args(argv: &[String]) -> Result<CreateOptions> {
    parse_create_args_with_reader(argv, &mut io::stdin())
}

pub fn parse_create_args_with_reader(
    argv: &[String],
    stdin: &mut dyn Read,
) -> Result<CreateOptions> {
    let mut options = CreateOptions::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();

        if UNSUPPORTED_BOOL_FLAGS.contains(&arg) || UNSUPPORTED_WITH_VALUES.contains(&arg) {
            return Err(ShimError::new(format!(
                "unsupported Forgejo PR create flag: {arg}"
            )));
        }

        if let Some((flag, value)) = arg.split_once('=') {
            if UNSUPPORTED_WITH_VALUES.contains(&flag) {
                return Err(ShimError::new(format!(
                    "unsupported Forgejo PR create flag: {flag}"
                )));
            }
            if let Some(kind) = value_flag_kind(flag) {
                set_create_value(&mut options, kind, value, stdin)?;
                index += 1;
                continue;
            }
        }

        if let Some(kind) = value_flag_kind(arg) {
            let value = argv
                .get(index + 1)
                .map(String::as_str)
                .ok_or_else(|| ShimError::new(format!("missing value for {arg}")))?;
            set_create_value(&mut options, kind, value, stdin)?;
            index += 2;
            continue;
        }

        match arg {
            "--fill" => options.fill = true,
            "--fill-first" => options.fill_first = true,
            "--fill-verbose" => options.fill_verbose = true,
            "--web" | "-w" => options.web = true,
            "--draft" | "-d" => options.draft = true,
            _ if arg.starts_with('-') => {
                return Err(ShimError::new(format!(
                    "unsupported Forgejo PR create flag: {arg}"
                )));
            }
            _ => {
                return Err(ShimError::new(format!(
                    "unexpected positional argument for Forgejo PR create: {arg}"
                )));
            }
        }
        index += 1;
    }
    Ok(options)
}

pub fn body_from_file(path_value: &str) -> Result<String> {
    body_from_reader(path_value, &mut io::stdin())
}

pub fn current_branch(cwd: Option<&Path>) -> Option<String> {
    git_output(&["branch", "--show-current"], cwd)
}

pub fn default_base_branch(cwd: Option<&Path>) -> String {
    git_output(
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
        cwd,
    )
    .and_then(|value| value.split_once('/').map(|(_, branch)| branch.to_string()))
    .unwrap_or_else(|| "main".to_string())
}

pub fn fill_title(cwd: Option<&Path>) -> Option<String> {
    git_output(&["log", "-1", "--pretty=%s"], cwd).or_else(|| current_branch(cwd))
}

pub fn fill_body(verbose: bool, cwd: Option<&Path>) -> String {
    if !verbose {
        return String::new();
    }
    git_output(&["log", "-1", "--pretty=%b"], cwd).unwrap_or_default()
}

fn value_flag_kind(flag: &str) -> Option<CreateValue> {
    FLAGS_WITH_VALUES
        .iter()
        .find_map(|(candidate, kind)| (*candidate == flag).then_some(*kind))
}

fn set_create_value(
    options: &mut CreateOptions,
    kind: CreateValue,
    value: &str,
    stdin: &mut dyn Read,
) -> Result<()> {
    match kind {
        CreateValue::Title => options.title = Some(value.to_string()),
        CreateValue::Body => options.body = Some(value.to_string()),
        CreateValue::BodyFile => {
            options.body_file = Some(value.to_string());
            options.body = Some(body_from_reader(value, stdin)?);
        }
        CreateValue::Base => options.base = Some(value.to_string()),
        CreateValue::Head => options.head = Some(value.to_string()),
        CreateValue::Repo => options.repo = Some(value.to_string()),
        CreateValue::JsonFields => options.json_fields = split_fields(value),
    }
    Ok(())
}

fn body_from_reader(path_value: &str, stdin: &mut dyn Read) -> Result<String> {
    if path_value == "-" {
        let mut body = String::new();
        stdin
            .read_to_string(&mut body)
            .map_err(|error| ShimError::new(format!("could not read body file -: {error}")))?;
        return Ok(body);
    }
    fs::read_to_string(path_value)
        .map_err(|error| ShimError::new(format!("could not read body file {path_value}: {error}")))
}

fn split_fields(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    #[test]
    fn parses_supported_create_flags_and_body_file() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("gh-forgejo-shim-create-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        let body_path = root.join("body.md");
        fs::write(&body_path, "hello\n")?;

        let args = vec![
            "--title".to_string(),
            "T".to_string(),
            "-F".to_string(),
            body_path.display().to_string(),
            "-B".to_string(),
            "main".to_string(),
            "-H".to_string(),
            "feature".to_string(),
            "-R".to_string(),
            "git.example.com/owner/repo".to_string(),
            "--fill".to_string(),
            "--fill-first".to_string(),
            "--fill-verbose".to_string(),
            "-w".to_string(),
            "-d".to_string(),
            "--json".to_string(),
            "number,title,url".to_string(),
        ];

        let options = parse_create_args(&args)?;
        fs::remove_dir_all(&root)?;

        assert_eq!(options.title.as_deref(), Some("T"));
        assert_eq!(options.body.as_deref(), Some("hello\n"));
        assert_eq!(options.base.as_deref(), Some("main"));
        assert_eq!(options.head.as_deref(), Some("feature"));
        assert_eq!(options.repo.as_deref(), Some("git.example.com/owner/repo"));
        assert!(options.fill);
        assert!(options.fill_first);
        assert!(options.fill_verbose);
        assert!(options.web);
        assert!(options.draft);
        assert_eq!(options.json_fields, ["number", "title", "url"]);
        Ok(())
    }

    #[test]
    fn unsupported_metadata_flags_fail_clearly() {
        let error = parse_create_args(&["--reviewer".to_string(), "alice".to_string()])
            .expect_err("unsupported flag should fail");
        assert_eq!(
            error.message(),
            "unsupported Forgejo PR create flag: --reviewer"
        );
    }
}
