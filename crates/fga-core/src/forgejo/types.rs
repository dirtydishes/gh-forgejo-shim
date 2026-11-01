use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct User {
    pub id: i64,
    pub login: String,
    pub full_name: Option<String>,
    pub html_url: Option<String>,
    pub avatar_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Repository {
    pub id: i64,
    pub name: String,
    pub full_name: String,
    pub owner: User,
    pub private: bool,
    pub html_url: String,
    pub default_branch: String,
    pub fork: bool,
    pub archived: bool,
    pub permissions: Option<Permissions>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub ssh_url: String,
    #[serde(default)]
    pub has_issues: bool,
    #[serde(default)]
    pub has_wiki: bool,
    pub parent: Option<Box<Repository>>,
    #[serde(default)]
    pub default_merge_style: String,
    #[serde(default)]
    pub allow_merge_commits: bool,
    #[serde(default)]
    pub allow_rebase: bool,
    #[serde(default)]
    pub allow_squash_merge: bool,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct Permissions {
    pub admin: bool,
    pub push: bool,
    pub pull: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BranchRepository {
    pub name: Option<String>,
    pub owner: User,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Branch {
    pub label: String,
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
    pub repo: Option<BranchRepository>,
}

impl Branch {
    pub fn label(&self) -> String {
        match &self.repo {
            Some(repo) => format!("{}:{}", repo.owner.login, self.ref_name),
            None => self.label.clone(),
        }
    }

    pub(super) fn matches(&self, filter: &str) -> bool {
        match filter.split_once(':') {
            Some((owner, branch)) => {
                self.label()
                    .split_once(':')
                    .is_some_and(|(login, _)| login.eq_ignore_ascii_case(owner))
                    && self.ref_name == branch
            }
            None => self.ref_name == filter,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PullRequest {
    pub number: i32,
    pub title: String,
    pub state: String,
    pub merged: bool,
    pub draft: bool,
    pub html_url: String,
    pub head: Branch,
    pub base: Branch,
    pub user: User,
    pub merged_at: Option<String>,
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub mergeable: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    pub closed_at: Option<String>,
    #[serde(rename = "merged_by", alias = "merge_user")]
    pub merged_by: Option<User>,
    #[serde(default)]
    pub additions: i32,
    #[serde(default)]
    pub deletions: i32,
    #[serde(default)]
    pub changed_files: i32,
    #[serde(default)]
    pub allow_maintainer_edit: bool,
    #[serde(default)]
    pub labels: Vec<Label>,
    pub milestone: Option<Milestone>,
    #[serde(default)]
    pub assignees: Option<Vec<User>>,
    #[serde(default)]
    pub requested_reviewers: Option<Vec<User>>,
    pub requested_reviewers_teams: Option<Vec<RequestedTeam>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CommitStatus {
    pub id: i64,
    pub context: String,
    #[serde(rename = "status")]
    pub state: CommitStatusState,
    pub target_url: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitStatusState {
    Pending,
    Success,
    Error,
    Failure,
    Warning,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Protection {
    pub rule_name: String,
    pub enable_status_check: bool,
    pub status_check_contexts: Vec<String>,
    pub required_approvals: i32,
    pub block_on_rejected_reviews: bool,
    pub dismiss_stale_approvals: bool,
    pub enable_approvals_whitelist: bool,
    pub block_on_outdated_branch: bool,
}

/// gobwas/glob's meta characters (`syntax/lexer.go` `specials`), which Forgejo uses to tell a
/// plain rule name from a glob.
const GLOB_SYNTAX: &[char] = &['*', '?', '\\', '[', ']', '{', '}'];

impl Protection {
    fn is_plain_name(&self) -> bool {
        !self.rule_name.contains(GLOB_SYNTAX)
    }

    /// Forgejo v15 `ProtectedBranch.Match` (`models/git/protected_branch.go:81-107`): a plain name
    /// matches case-insensitively; a glob uses `/` as separator; an invalid glob matches literally.
    pub fn matches_branch(&self, branch: &str) -> bool {
        if self.is_plain_name() {
            return self.rule_name.eq_ignore_ascii_case(branch);
        }
        globset::GlobBuilder::new(&self.rule_name)
            .literal_separator(true)
            .build()
            .map_or(self.rule_name == branch, |glob| {
                glob.compile_matcher().is_match(branch)
            })
    }

    /// The rule Forgejo applies to `branch`: plain names before globs, then creation order, which
    /// is the API's order (`models/git/protected_branch_list.go:18-44`).
    pub fn for_branch(mut rules: Vec<Protection>, branch: &str) -> Option<Protection> {
        rules.sort_by_key(|rule| !rule.is_plain_name());
        rules.into_iter().find(|rule| rule.matches_branch(branch))
    }

    /// Required status contexts are separator-free globs; an invalid one never matches
    /// (`services/pull/commit_status.go:28-41`).
    pub fn required_contexts(&self) -> Vec<globset::GlobMatcher> {
        if !self.enable_status_check {
            return Vec::new();
        }
        self.status_check_contexts
            .iter()
            .filter_map(|pattern| globset::Glob::new(pattern).ok())
            .map(|glob| glob.compile_matcher())
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Review {
    pub id: i64,
    pub user: User,
    pub body: String,
    pub state: String,
    pub submitted_at: Option<String>,
    pub commit_id: String,
    #[serde(default)]
    pub dismissed: bool,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub official: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Label {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub color: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Milestone {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub due_on: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub user: User,
    pub body: String,
    pub created_at: String,
    pub html_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TimelineEvent {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub created_at: String,
    pub user: User,
}
#[derive(Clone, Debug, Deserialize)]
pub struct PullCommit {
    pub sha: String,
    pub commit: CommitMetadata,
    pub author: Option<User>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct CommitMetadata {
    pub message: String,
    pub author: CommitAuthor,
    pub committer: CommitAuthor,
}
#[derive(Clone, Debug, Deserialize)]
pub struct CommitAuthor {
    pub name: String,
    pub email: String,
    pub date: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RequestedTeam {
    pub name: String,
    pub organization: TeamOrganization,
}
#[derive(Clone, Debug, Deserialize)]
pub struct TeamOrganization {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Issue {
    pub id: i64,
    pub number: i32,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub html_url: String,
    pub user: User,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    #[serde(default)]
    pub labels: Vec<Label>,
    pub assignees: Option<Vec<User>>,
    pub milestone: Option<Milestone>,
    #[serde(default)]
    pub is_locked: bool,
    pub pull_request: Option<serde_json::Value>,
}
