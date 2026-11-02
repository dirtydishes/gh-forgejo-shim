//! The repo-scoped subset of GitHub search emitted by gh. Reject everything else.
use crate::forgejo;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Pulls,
    Issues,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    All,
    Open,
    Closed,
    Merged,
}
impl State {
    pub fn upstream(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Open => "open",
            Self::Closed | Self::Merged => "closed",
        }
    }
    pub fn matches_pull(self, pr: &forgejo::PullRequest) -> bool {
        match self {
            Self::All => true,
            Self::Open => pr.state == "open" && !pr.merged,
            // Search state:closed includes merged PRs; Repository.pullRequests
            // uses the distinct CLOSED enum and filters it separately.
            Self::Closed => pr.state == "closed" || pr.merged,
            Self::Merged => pr.merged,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub enum Sort {
    Relevance,
    #[default]
    CreatedDesc,
    CreatedAsc,
    UpdatedDesc,
    UpdatedAsc,
    CommentsDesc,
    CommentsAsc,
}
impl Sort {
    pub fn upstream(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::CreatedDesc => "latest",
            Self::CreatedAsc => "oldest",
            Self::UpdatedDesc => "recentupdate",
            Self::UpdatedAsc => "leastupdate",
            Self::CommentsDesc => "mostcomment",
            Self::CommentsAsc => "leastcomment",
        }
    }
}
#[derive(Debug)]
pub struct Search {
    pub owner: String,
    pub repo: String,
    pub kind: Kind,
    pub state: State,
    pub filters: Filters,
    pub head: Option<String>,
    pub base: Option<String>,
    pub reviewer: Option<String>,
    pub title_only: bool,
}
#[derive(Debug, Default)]
pub struct Filters {
    pub author: Option<String>,
    pub assignee: Option<String>,
    pub mention: Option<String>,
    pub labels: Vec<String>,
    pub words: Vec<String>,
    pub sort: Sort,
}
impl Filters {
    pub fn upstream(&self, state: State, kind: Kind) -> Vec<(&'static str, String)> {
        let mut query = vec![
            ("state", state.upstream().into()),
            (
                "type",
                match kind {
                    Kind::Pulls => "pulls",
                    Kind::Issues => "issues",
                }
                .into(),
            ),
            ("sort", self.sort.upstream().into()),
        ];
        for (name, value) in [
            ("created_by", &self.author),
            ("assigned_by", &self.assignee),
            ("mentioned_by", &self.mention),
        ] {
            if let Some(value) = value {
                query.push((name, value.clone()));
            }
        }
        if !self.labels.is_empty() {
            query.push(("labels", self.labels.join(",")));
        }
        if !self.words.is_empty() {
            query.push(("q", self.keywords()));
        }
        query
    }

    /// Forgejo's keyword syntax (`modules/indexer/issues/internal/qstring.go:47-110`): `+` makes
    /// a term required, as every GitHub term is. Quotes make it exact, so neither a fuzzy match
    /// nor one of Forgejo's own filters like `is:open`. Inside quotes, `\` escapes `"` and `\`.
    fn keywords(&self) -> String {
        self.words
            .iter()
            .map(|word| format!("+\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect::<Vec<_>>()
            .join(" ")
    }
}
/// Terms, each marked if it opens with a quote: that makes it a keyword, while gh's
/// `label:"needs work"` stays a qualifier.
fn tokens(query: &str) -> Result<Vec<(String, bool)>, String> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut was_quoted = false;
    let mut escaped = false;
    for c in query.chars() {
        if escaped {
            word.push(c);
            escaped = false;
        } else if c == '\\' && quoted {
            escaped = true;
        } else if c == '"' {
            was_quoted |= word.is_empty() && !quoted;
            quoted = !quoted;
        } else if c.is_whitespace() && !quoted {
            if !word.is_empty() {
                result.push((std::mem::take(&mut word), was_quoted));
            }
            was_quoted = false;
        } else {
            word.push(c);
        }
    }
    if quoted || escaped {
        return Err("unterminated search quote".into());
    }
    if !word.is_empty() {
        result.push((word, was_quoted));
    }
    Ok(result)
}
impl Search {
    pub fn parse(query: &str, viewer: &str) -> Result<Self, String> {
        let mut result = Self {
            owner: String::new(),
            repo: String::new(),
            kind: Kind::Pulls,
            state: State::All,
            filters: Filters::default(),
            head: None,
            base: None,
            reviewer: None,
            title_only: false,
        };
        let mut repository = false;
        let mut kind = None;
        let mut state = None;
        let login = |value: &str| {
            if value == "@me" {
                viewer.to_owned()
            } else {
                value.to_owned()
            }
        };
        let mut depth = 0;
        let mut sort_explicit = false;
        for (term, quoted) in tokens(query)? {
            if quoted {
                result.filters.words.push(term);
                continue;
            }
            match term.as_str() {
                "(" => {
                    depth += 1;
                    continue;
                }
                ")" if depth > 0 => {
                    depth -= 1;
                    continue;
                }
                ")" => return Err("unmatched search parenthesis".into()),
                _ => {}
            }
            let Some((key, value)) = term.split_once(':') else {
                if matches!(term.as_str(), "OR" | "AND" | "NOT") || term.starts_with('-') {
                    return Err(format!("unsupported search term: {term}"));
                }
                result.filters.words.push(term);
                continue;
            };
            if value.is_empty() {
                return Err(format!("empty search qualifier: {key}"));
            }
            match (key, value) {
                ("repo", value) if !repository => {
                    let (owner, repo) = value
                        .split_once('/')
                        .filter(|(o, r)| !o.is_empty() && !r.is_empty() && !r.contains('/'))
                        .ok_or("invalid repo qualifier")?;
                    result.owner = owner.into();
                    result.repo = repo.into();
                    repository = true;
                }
                ("is" | "type", "pr" | "issue") => {
                    let next = if value == "pr" {
                        Kind::Pulls
                    } else {
                        Kind::Issues
                    };
                    if kind.is_some_and(|k| k != next) {
                        return Err("conflicting search types".into());
                    }
                    kind = Some(next);
                    result.kind = next;
                }
                ("is" | "state", "open" | "closed" | "merged") => {
                    let next = match value {
                        "open" => State::Open,
                        "closed" => State::Closed,
                        _ => State::Merged,
                    };
                    if state.is_some_and(|s| s != next) {
                        return Err("conflicting search states".into());
                    }
                    state = Some(next);
                    result.state = next;
                }
                ("author", value) if result.filters.author.is_none() => {
                    result.filters.author = Some(login(value))
                }
                ("assignee", value) if result.filters.assignee.is_none() => {
                    result.filters.assignee = Some(login(value))
                }
                ("review-requested", value) if result.reviewer.is_none() => {
                    result.reviewer = Some(login(value))
                }
                ("label", value) => result.filters.labels.push(value.into()),
                ("head", value) if result.head.is_none() => result.head = Some(value.into()),
                ("base", value) if result.base.is_none() => result.base = Some(value.into()),
                ("in", "title") => result.title_only = true,
                ("sort", value) if !sort_explicit => {
                    sort_explicit = true;
                    result.filters.sort = match value {
                        "created-desc" => Sort::CreatedDesc,
                        "created-asc" => Sort::CreatedAsc,
                        "updated-desc" => Sort::UpdatedDesc,
                        "updated-asc" => Sort::UpdatedAsc,
                        "comments-desc" => Sort::CommentsDesc,
                        "comments-asc" => Sort::CommentsAsc,
                        _ => return Err(format!("unsupported sort qualifier: {value}")),
                    }
                }
                _ => return Err(format!("unsupported search qualifier: {key}:{value}")),
            }
        }
        if depth != 0 {
            return Err("unmatched search parenthesis".into());
        }
        if !sort_explicit && !result.filters.words.is_empty() {
            result.filters.sort = Sort::Relevance;
        }
        if !repository || (kind.is_none() && result.reviewer.is_none()) {
            return Err("search requires repo and type qualifiers".into());
        }
        if result.kind == Kind::Issues
            && (result.head.is_some()
                || result.base.is_some()
                || result.reviewer.is_some()
                || result.state == State::Merged)
        {
            return Err("PR qualifiers cannot filter issues".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_gh_qualifiers_and_rejects_unhandled_semantics() {
        let s = Search::parse("author:@me assignee:someone label:\"needs work\" head:s/pass base:main repo:o/r type:pr is:merged sort:updated-asc scenario in:title", "viewer").unwrap();
        assert_eq!(s.filters.author.as_deref(), Some("viewer"));
        assert_eq!(s.filters.labels, ["needs work"]);
        assert!(s.title_only);
        assert_eq!(
            s.filters.upstream(s.state, s.kind),
            vec![
                ("state", "closed".into()),
                ("type", "pulls".into()),
                ("sort", "leastupdate".into()),
                ("created_by", "viewer".into()),
                ("assigned_by", "someone".into()),
                ("labels", "needs work".into()),
                ("q", "+\"scenario\"".into())
            ]
        );
        let s = Search::parse(
            r#"repo:o/r type:issue "broken feature" "is:open" "say \"hi\"" plain"#,
            "viewer",
        )
        .unwrap();
        assert_eq!(s.state, State::All, "a quoted qualifier is a keyword");
        assert_eq!(
            s.filters.keywords(),
            r#"+"broken feature" +"is:open" +"say \"hi\"" +"plain""#
        );
        for term in [
            "unknown:x",
            "is:draft",
            "in:body",
            "sort:invalid",
            "repo:o/other",
            "author:a author:b",
            "is:open is:merged",
            "OR",
            "label:\"unclosed",
        ] {
            assert!(
                Search::parse(&format!("repo:o/r type:pr {term}"), "viewer").is_err(),
                "{term}"
            );
        }
    }
}
