//! One command-aware interpretation of managed `gh` arguments.
//!
//! Routing and native handlers share this parsed value. Raw argument tokens are
//! never scanned a second time, so provider selectors cannot change meaning at
//! the handler boundary.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    GlobalDelegate,
    AuthStatus,
    AuthToken,
    Api,
    RepoView,
    PrChecks,
    PrCheckout,
    PrComment,
    PrCreate,
    PrDiff,
    PrList,
    PrStatus,
    PrView,
    IssueCreate,
    IssueList,
    IssueView,
    Unsupported,
}

impl Command {
    pub const fn is_supported_local(self) -> bool {
        !matches!(self, Self::GlobalDelegate | Self::Unsupported)
    }

    const fn prefix_len(self) -> usize {
        match self {
            Self::GlobalDelegate | Self::Unsupported => 1,
            Self::Api => 1,
            _ => 2,
        }
    }

    const fn positional_kind(self) -> PositionalKind {
        match self {
            Self::RepoView => PositionalKind::Repository,
            Self::PrChecks
            | Self::PrCheckout
            | Self::PrComment
            | Self::PrDiff
            | Self::PrView
            | Self::IssueView => PositionalKind::Url,
            _ => PositionalKind::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedArg {
    Flag { name: &'static str },
    Value { name: &'static str, value: String },
    Positional(String),
}

impl ParsedArg {
    fn append_canonical(&self, output: &mut Vec<String>) {
        match self {
            Self::Flag { name } => output.push((*name).to_string()),
            Self::Value { name, value } => {
                output.push((*name).to_string());
                output.push(value.clone());
            }
            Self::Positional(value) => output.push(value.clone()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderTarget {
    repo: Option<String>,
    positional_repo: Option<String>,
    host: Option<String>,
}

impl ProviderTarget {
    pub fn repo_spec(&self) -> Option<&str> {
        self.repo.as_deref().or(self.positional_repo.as_deref())
    }

    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    pub fn repo_is_flag(&self) -> bool {
        self.repo.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedInvocation {
    command: Command,
    args: Vec<ParsedArg>,
    canonical_argv: Vec<String>,
    provider: ProviderTarget,
    ambiguity: Option<String>,
    help_requested: bool,
}

impl ParsedInvocation {
    pub fn parse(argv: &[String]) -> Self {
        let command = classify(argv);
        if command == Command::GlobalDelegate {
            return Self {
                command,
                args: Vec::new(),
                canonical_argv: argv.to_vec(),
                provider: ProviderTarget::default(),
                ambiguity: None,
                help_requested: false,
            };
        }

        let mut parsed = Self {
            command,
            args: Vec::new(),
            canonical_argv: command_prefix(argv, command),
            provider: ProviderTarget::default(),
            ambiguity: None,
            help_requested: false,
        };
        parsed.parse_tail(argv);
        for arg in &parsed.args {
            arg.append_canonical(&mut parsed.canonical_argv);
        }
        parsed
    }

    pub const fn command(&self) -> Command {
        self.command
    }

    pub fn canonical_argv(&self) -> &[String] {
        &self.canonical_argv
    }

    pub fn provider_target(&self) -> &ProviderTarget {
        &self.provider
    }

    pub fn ambiguity(&self) -> Option<&str> {
        self.ambiguity.as_deref()
    }

    pub const fn help_requested(&self) -> bool {
        self.help_requested
    }

    fn parse_tail(&mut self, argv: &[String]) {
        let mut index = self.command.prefix_len().min(argv.len());
        let mut positional_seen = false;
        while index < argv.len() {
            let token = argv[index].as_str();
            if token == "--" {
                for value in &argv[index + 1..] {
                    self.record_positional(value, &mut positional_seen);
                }
                return;
            }
            if let Some(long) = token.strip_prefix("--") {
                let (name, attached) = long
                    .split_once('=')
                    .map_or((long, None), |(name, value)| (name, Some(value)));
                let rendered = format!("--{name}");
                let Some(spec) = option_by_long(self.command, &rendered) else {
                    self.mark_ambiguous(token);
                    return;
                };
                match spec.arity {
                    Arity::Flag if attached.is_none() => self.push_flag(spec),
                    Arity::Flag => {
                        self.mark_ambiguous(token);
                        return;
                    }
                    Arity::Value => {
                        let value = if let Some(value) = attached {
                            value
                        } else if let Some(value) = argv.get(index + 1) {
                            index += 1;
                            value.as_str()
                        } else {
                            self.mark_ambiguous(token);
                            return;
                        };
                        self.push_value(spec, value);
                    }
                }
                index += 1;
                continue;
            }
            if token.starts_with('-') && token != "-" {
                if !self.parse_short_token(token, argv.get(index + 1)) {
                    return;
                }
                if short_token_consumes_next(self.command, token) {
                    index += 1;
                }
                index += 1;
                continue;
            }
            self.record_positional(token, &mut positional_seen);
            index += 1;
        }
    }

    fn parse_short_token(&mut self, token: &str, next: Option<&String>) -> bool {
        let cluster = &token[1..];
        for (offset, short) in cluster.char_indices() {
            let Some(spec) = option_by_short(self.command, short) else {
                self.mark_ambiguous(token);
                return false;
            };
            match spec.arity {
                Arity::Flag => self.push_flag(spec),
                Arity::Value => {
                    let suffix_start = offset + short.len_utf8();
                    let suffix = cluster[suffix_start..]
                        .strip_prefix('=')
                        .unwrap_or(&cluster[suffix_start..]);
                    let value = if !suffix.is_empty() {
                        suffix
                    } else if let Some(value) = next {
                        value.as_str()
                    } else {
                        self.mark_ambiguous(token);
                        return false;
                    };
                    self.push_value(spec, value);
                    return true;
                }
            }
        }
        true
    }

    fn push_flag(&mut self, spec: OptionSpec) {
        if spec.role == Role::Help {
            self.help_requested = true;
        }
        self.args.push(ParsedArg::Flag { name: spec.long });
    }

    fn push_value(&mut self, spec: OptionSpec, value: &str) {
        match spec.role {
            Role::Repo => self.provider.repo = Some(value.to_string()),
            Role::Host => self.provider.host = Some(value.to_string()),
            Role::Ordinary | Role::Help => {}
        }
        self.args.push(ParsedArg::Value {
            name: spec.long,
            value: value.to_string(),
        });
    }

    fn record_positional(&mut self, value: &str, seen: &mut bool) {
        if !*seen {
            match self.command.positional_kind() {
                PositionalKind::Repository => {
                    self.provider.positional_repo = Some(value.to_string())
                }
                PositionalKind::Url if is_repo_url(value) => {
                    self.provider.positional_repo = Some(value.to_string())
                }
                PositionalKind::None | PositionalKind::Url => {}
            }
            *seen = true;
        }
        self.args.push(ParsedArg::Positional(value.to_string()));
    }

    fn mark_ambiguous(&mut self, token: &str) {
        self.ambiguity = Some(format!(
            "provider is ambiguous because option {token} has no certified grammar"
        ));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arity {
    Flag,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Ordinary,
    Repo,
    Host,
    Help,
}

#[derive(Debug, Clone, Copy)]
struct OptionSpec {
    long: &'static str,
    short: Option<char>,
    arity: Arity,
    role: Role,
}

impl OptionSpec {
    const fn flag(long: &'static str, short: Option<char>) -> Self {
        Self {
            long,
            short,
            arity: Arity::Flag,
            role: Role::Ordinary,
        }
    }

    const fn value(long: &'static str, short: Option<char>) -> Self {
        Self {
            long,
            short,
            arity: Arity::Value,
            role: Role::Ordinary,
        }
    }

    const fn repo() -> Self {
        Self {
            long: "--repo",
            short: Some('R'),
            arity: Arity::Value,
            role: Role::Repo,
        }
    }

    const fn host(short: Option<char>) -> Self {
        Self {
            long: "--hostname",
            short,
            arity: Arity::Value,
            role: Role::Host,
        }
    }

    const fn help() -> Self {
        Self {
            long: "--help",
            short: Some('h'),
            arity: Arity::Flag,
            role: Role::Help,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PositionalKind {
    None,
    Repository,
    Url,
}

const OUTPUT: &[OptionSpec] = &[
    OptionSpec::value("--json", None),
    OptionSpec::value("--jq", Some('q')),
    OptionSpec::value("--template", Some('t')),
];

const PR_CREATE: &[OptionSpec] = &[
    OptionSpec::value("--title", Some('t')),
    OptionSpec::value("--body", Some('b')),
    OptionSpec::value("--body-file", Some('F')),
    OptionSpec::value("--base", Some('B')),
    OptionSpec::value("--head", Some('H')),
    OptionSpec::value("--json", None),
    OptionSpec::value("--reviewer", Some('r')),
    OptionSpec::value("--reviewers", None),
    OptionSpec::value("--assignee", Some('a')),
    OptionSpec::value("--assignees", None),
    OptionSpec::value("--label", Some('l')),
    OptionSpec::value("--labels", None),
    OptionSpec::value("--project", Some('p')),
    OptionSpec::value("--projects", None),
    OptionSpec::value("--milestone", Some('m')),
    OptionSpec::value("--template", Some('T')),
    OptionSpec::value("--recover", None),
    OptionSpec::flag("--fill", Some('f')),
    OptionSpec::flag("--fill-first", None),
    OptionSpec::flag("--fill-verbose", None),
    OptionSpec::flag("--web", Some('w')),
    OptionSpec::flag("--draft", Some('d')),
    OptionSpec::flag("--maintainer-can-modify", None),
    OptionSpec::flag("--no-maintainer-edit", None),
    OptionSpec::flag("--no-maintainer-can-modify", None),
    OptionSpec::flag("--dry-run", None),
];

const PR_LIST: &[OptionSpec] = &[
    OptionSpec::value("--state", Some('s')),
    OptionSpec::value("--limit", Some('L')),
    OptionSpec::value("--head", Some('H')),
    OptionSpec::value("--base", Some('B')),
    OptionSpec::value("--author", Some('A')),
    OptionSpec::value("--app", None),
    OptionSpec::value("--assignee", Some('a')),
    OptionSpec::value("--label", Some('l')),
    OptionSpec::value("--search", Some('S')),
    OptionSpec::flag("--draft", Some('d')),
    OptionSpec::flag("--web", Some('w')),
];

const PR_CHECKS: &[OptionSpec] = &[
    OptionSpec::value("--interval", Some('i')),
    OptionSpec::flag("--web", Some('w')),
    OptionSpec::flag("--watch", None),
    OptionSpec::flag("--fail-fast", None),
    OptionSpec::flag("--required", None),
];

const PR_CHECKOUT: &[OptionSpec] = &[
    OptionSpec::value("--branch", Some('b')),
    OptionSpec::flag("--detach", None),
    OptionSpec::flag("--force", Some('f')),
    OptionSpec::flag("--recurse-submodules", None),
];

const PR_COMMENT: &[OptionSpec] = &[
    OptionSpec::value("--body", Some('b')),
    OptionSpec::value("--body-file", Some('F')),
    OptionSpec::flag("--editor", Some('e')),
    OptionSpec::flag("--edit-last", None),
    OptionSpec::flag("--web", Some('w')),
];

const PR_DIFF: &[OptionSpec] = &[
    OptionSpec::value("--color", None),
    OptionSpec::value("--exclude", Some('e')),
    OptionSpec::flag("--web", Some('w')),
    OptionSpec::flag("--name-only", None),
    OptionSpec::flag("--patch", None),
];

const PR_VIEW: &[OptionSpec] = &[
    OptionSpec::flag("--comments", Some('c')),
    OptionSpec::flag("--web", Some('w')),
];

const PR_STATUS: &[OptionSpec] = &[OptionSpec::flag("--conflict-status", Some('c'))];

const REPO_VIEW: &[OptionSpec] = &[
    OptionSpec::value("--branch", Some('b')),
    OptionSpec::flag("--web", Some('w')),
];

const ISSUE_VIEW: &[OptionSpec] = &[
    OptionSpec::flag("--web", Some('w')),
    OptionSpec::flag("--comments", Some('c')),
];

const ISSUE_LIST: &[OptionSpec] = &[
    OptionSpec::value("--state", Some('s')),
    OptionSpec::value("--limit", Some('L')),
    OptionSpec::value("--label", Some('l')),
    OptionSpec::value("--search", Some('S')),
    OptionSpec::value("--author", Some('A')),
    OptionSpec::value("--assignee", Some('a')),
    OptionSpec::value("--mention", None),
    OptionSpec::value("--milestone", Some('m')),
    OptionSpec::value("--app", None),
    OptionSpec::flag("--web", Some('w')),
];

const ISSUE_CREATE: &[OptionSpec] = &[
    OptionSpec::value("--title", Some('t')),
    OptionSpec::value("--body", Some('b')),
    OptionSpec::value("--body-file", Some('F')),
    OptionSpec::value("--assignee", Some('a')),
    OptionSpec::value("--label", Some('l')),
    OptionSpec::value("--milestone", Some('m')),
    OptionSpec::value("--project", Some('p')),
    OptionSpec::value("--recover", None),
    OptionSpec::value("--template", Some('T')),
    OptionSpec::flag("--web", Some('w')),
    OptionSpec::flag("--editor", Some('e')),
];

const AUTH_STATUS: &[OptionSpec] = &[
    OptionSpec::value("--json", None),
    OptionSpec::value("--jq", Some('q')),
    OptionSpec::value("--template", None),
    OptionSpec::flag("--show-token", Some('t')),
    OptionSpec::flag("--active", Some('a')),
];

const AUTH_TOKEN: &[OptionSpec] = &[OptionSpec::value("--user", Some('u'))];

const API: &[OptionSpec] = &[
    OptionSpec::value("--cache", None),
    OptionSpec::value("--field", Some('F')),
    OptionSpec::value("--header", Some('H')),
    OptionSpec::value("--input", None),
    OptionSpec::value("--method", Some('X')),
    OptionSpec::value("--preview", Some('p')),
    OptionSpec::value("--raw-field", Some('f')),
    OptionSpec::value("--jq", Some('q')),
    OptionSpec::value("--template", Some('t')),
    OptionSpec::flag("--silent", None),
    OptionSpec::flag("--include", Some('i')),
    OptionSpec::flag("--paginate", None),
    OptionSpec::flag("--verbose", None),
];

fn classify(argv: &[String]) -> Command {
    match argv {
        [] => Command::GlobalDelegate,
        [command] if matches!(command.as_str(), "--version" | "version") => Command::GlobalDelegate,
        [command, ..] if matches!(command.as_str(), "--help" | "-h" | "help") => {
            Command::GlobalDelegate
        }
        [command, subcommand, ..] => match (command.as_str(), subcommand.as_str()) {
            ("auth", "status") => Command::AuthStatus,
            ("auth", "token") => Command::AuthToken,
            ("repo", "view") => Command::RepoView,
            ("pr", "checks") => Command::PrChecks,
            ("pr", "checkout" | "co") => Command::PrCheckout,
            ("pr", "comment") => Command::PrComment,
            ("pr", "create" | "new") => Command::PrCreate,
            ("pr", "diff") => Command::PrDiff,
            ("pr", "list") => Command::PrList,
            ("pr", "status") => Command::PrStatus,
            ("pr", "view") => Command::PrView,
            ("issue", "create" | "new") => Command::IssueCreate,
            ("issue", "list" | "ls") => Command::IssueList,
            ("issue", "view") => Command::IssueView,
            _ if command == "api" => Command::Api,
            _ => Command::Unsupported,
        },
        [command] if command == "api" => Command::Api,
        _ => Command::Unsupported,
    }
}

fn command_prefix(argv: &[String], command: Command) -> Vec<String> {
    argv.iter()
        .take(command.prefix_len().min(argv.len()))
        .cloned()
        .collect()
}

fn option_by_long(command: Command, name: &str) -> Option<OptionSpec> {
    all_options(command)
        .find(|spec| spec.long == name)
        .or_else(|| {
            uses_repo(command)
                .then_some(OptionSpec::repo())
                .filter(|spec| spec.long == name)
        })
        .or_else(|| host_option(command).filter(|spec| spec.long == name))
        .or_else(|| Some(OptionSpec::help()).filter(|spec| spec.long == name))
}

fn option_by_short(command: Command, short: char) -> Option<OptionSpec> {
    all_options(command)
        .find(|spec| spec.short == Some(short))
        .or_else(|| {
            uses_repo(command)
                .then_some(OptionSpec::repo())
                .filter(|spec| spec.short == Some(short))
        })
        .or_else(|| host_option(command).filter(|spec| spec.short == Some(short)))
        .or_else(|| Some(OptionSpec::help()).filter(|spec| spec.short == Some(short)))
}

fn all_options(command: Command) -> impl Iterator<Item = OptionSpec> {
    let command_options = match command {
        Command::AuthStatus => AUTH_STATUS,
        Command::AuthToken => AUTH_TOKEN,
        Command::Api => API,
        Command::RepoView => REPO_VIEW,
        Command::PrChecks => PR_CHECKS,
        Command::PrCheckout => PR_CHECKOUT,
        Command::PrComment => PR_COMMENT,
        Command::PrCreate => PR_CREATE,
        Command::PrDiff => PR_DIFF,
        Command::PrList => PR_LIST,
        Command::PrStatus => PR_STATUS,
        Command::PrView => PR_VIEW,
        Command::IssueCreate => ISSUE_CREATE,
        Command::IssueList => ISSUE_LIST,
        Command::IssueView => ISSUE_VIEW,
        Command::GlobalDelegate | Command::Unsupported => &[],
    };
    let output = if uses_output(command) { OUTPUT } else { &[] };
    output.iter().chain(command_options.iter()).copied()
}

const fn uses_output(command: Command) -> bool {
    matches!(
        command,
        Command::RepoView
            | Command::PrChecks
            | Command::PrList
            | Command::PrStatus
            | Command::PrView
            | Command::IssueList
            | Command::IssueView
    )
}

const fn uses_repo(command: Command) -> bool {
    matches!(
        command,
        Command::RepoView
            | Command::PrChecks
            | Command::PrCheckout
            | Command::PrComment
            | Command::PrCreate
            | Command::PrDiff
            | Command::PrList
            | Command::PrStatus
            | Command::PrView
            | Command::IssueCreate
            | Command::IssueList
            | Command::IssueView
            | Command::Unsupported
    )
}

const fn host_option(command: Command) -> Option<OptionSpec> {
    match command {
        Command::AuthStatus | Command::AuthToken => Some(OptionSpec::host(Some('h'))),
        Command::Api => Some(OptionSpec::host(None)),
        _ => None,
    }
}

fn short_token_consumes_next(command: Command, token: &str) -> bool {
    let cluster = token.strip_prefix('-').unwrap_or(token);
    for (offset, short) in cluster.char_indices() {
        let Some(spec) = option_by_short(command, short) else {
            return false;
        };
        if spec.arity == Arity::Value {
            return cluster[offset + short.len_utf8()..]
                .trim_start_matches('=')
                .is_empty();
        }
    }
    false
}

fn is_repo_url(value: &str) -> bool {
    value.split_once("://").is_some_and(|(scheme, _)| {
        matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "ssh" | "git"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parses_boolean_clusters_and_attached_value_options_once() {
        let parsed = ParsedInvocation::parse(&argv(&[
            "issue",
            "view",
            "-cRgit.example.com/owner/repo",
            "-q.number",
            "13",
        ]));

        assert_eq!(parsed.ambiguity(), None);
        assert_eq!(
            parsed.provider_target().repo_spec(),
            Some("git.example.com/owner/repo")
        );
        assert_eq!(
            parsed.canonical_argv(),
            argv(&[
                "issue",
                "view",
                "--comments",
                "--repo",
                "git.example.com/owner/repo",
                "--jq",
                ".number",
                "13",
            ])
        );
    }

    #[test]
    fn unknown_option_arity_is_ambiguous_instead_of_guessed() {
        let parsed = ParsedInvocation::parse(&argv(&[
            "release",
            "create",
            "v1",
            "--notes",
            "--repo=github.com/owner/repo",
        ]));

        assert!(parsed.ambiguity().is_some());
        assert_eq!(parsed.provider_target().repo_spec(), None);
    }

    #[test]
    fn command_help_is_an_inherited_global_flag() {
        for values in [
            &["pr", "view", "--help"][..],
            &["issue", "list", "-h"][..],
            &["auth", "status", "--help"][..],
        ] {
            let parsed = ParsedInvocation::parse(&argv(values));
            assert_eq!(parsed.ambiguity(), None, "argv: {values:?}");
            assert!(parsed.help_requested(), "argv: {values:?}");
        }

        let auth_host = ParsedInvocation::parse(&argv(&["auth", "status", "-hgit.example.com"]));
        assert!(!auth_host.help_requested());
        assert_eq!(auth_host.provider_target().host(), Some("git.example.com"));
    }
}
