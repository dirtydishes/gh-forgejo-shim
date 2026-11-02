use super::glob;
use serde::{Deserialize, Deserializer};

/// Go encodes a nil slice as `null`, which `#[serde(default)]` alone rejects.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

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
    /// The branch name. Forgejo's `ref` becomes `refs/pull/N/head` once the head branch is
    /// deleted, while `label` keeps the name (`services/convert/pull.go:96-106,210-226`); `label`
    /// is empty for AGit PRs.
    pub fn name(&self) -> &str {
        if self.ref_name.starts_with("refs/pull/") && !self.label.is_empty() {
            &self.label
        } else {
            &self.ref_name
        }
    }

    pub fn label(&self) -> String {
        match &self.repo {
            Some(repo) => format!("{}:{}", repo.owner.login, self.name()),
            None => self.name().to_owned(),
        }
    }

    pub(super) fn matches(&self, filter: &str) -> bool {
        match filter.split_once(':') {
            Some((owner, branch)) => {
                self.label()
                    .split_once(':')
                    .is_some_and(|(login, _)| login.eq_ignore_ascii_case(owner))
                    && self.name() == branch
            }
            None => self.name() == filter,
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
    #[serde(deserialize_with = "null_as_default")]
    pub status_check_contexts: Vec<String>,
    pub required_approvals: i32,
    pub block_on_rejected_reviews: bool,
    pub ignore_stale_approvals: bool,
    pub block_on_official_review_requests: bool,
    pub block_on_outdated_branch: bool,
    /// `;`-separated globs; only the admin rule carries them, not the branch summary.
    #[serde(default)]
    pub protected_file_patterns: String,
    /// Set when only the branch summary was readable: the review and outdated-branch flags above
    /// are unknown, not off.
    #[serde(skip)]
    pub partial: bool,
}

impl Protection {
    /// Whether a merge changing a matching file is refused (`models/git/protected_branch.go:214-227`
    /// splits on `;` and skips blank entries). The gateway can't tell which files a PR changes
    /// that match, so callers treat such a rule as unknown.
    pub fn protects_files(&self) -> bool {
        self.protected_file_patterns
            .split(';')
            .any(|pattern| !pattern.trim().is_empty())
    }

    fn is_plain_name(&self) -> bool {
        !self.rule_name.contains(glob::SYNTAX)
    }

    /// Forgejo v15 `ProtectedBranch.Match` (`models/git/protected_branch.go:81-107`): a plain name
    /// matches under `strings.EqualFold`, i.e. Unicode simple case folding, which is what the
    /// regex `(?i)` flag implements; a glob uses `/` as separator; an invalid glob matches
    /// literally.
    pub fn matches_branch(&self, branch: &str) -> bool {
        if self.is_plain_name() {
            let name = format!("(?i)\\A{}\\z", regex::escape(&self.rule_name));
            return regex::Regex::new(&name).is_ok_and(|name| name.is_match(branch));
        }
        glob::compile(&self.rule_name, Some('/'))
            .map_or(self.rule_name == branch, |glob| glob.is_match(branch))
    }

    /// The rule Forgejo applies to `branch`: plain names before globs, then creation order, which
    /// is the API's order (`models/git/protected_branch_list.go:18-44`).
    pub fn for_branch(mut rules: Vec<Protection>, branch: &str) -> Option<Protection> {
        rules.sort_by_key(|rule| !rule.is_plain_name());
        rules.into_iter().find(|rule| rule.matches_branch(branch))
    }

    /// Whether Forgejo's merge gate reads reviews under this rule (`models/issues/pull.go:901-952`).
    pub fn gates_reviews(&self) -> bool {
        self.required_approvals > 0
            || self.block_on_rejected_reviews
            || self.block_on_official_review_requests
    }

    /// Required status contexts are separator-free globs; an invalid one never matches
    /// (`services/pull/commit_status.go:28-41`).
    pub fn required_contexts(&self) -> Vec<regex::Regex> {
        if !self.enable_status_check {
            return Vec::new();
        }
        self.status_check_contexts
            .iter()
            .filter_map(|pattern| glob::compile(pattern, None))
            .collect()
    }
}

/// The effective rule as `GET /branches/{name}` summarizes it for anyone who can read code
/// (`services/convert/convert.go:58-112`). It omits the review and outdated-branch flags, and the
/// rule's name unless the caller is a repository admin.
#[derive(Debug, Deserialize)]
pub struct BranchSummary {
    #[serde(default)]
    pub protected: bool,
    #[serde(default)]
    pub required_approvals: i32,
    #[serde(default)]
    pub enable_status_check: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    pub status_check_contexts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Review {
    pub id: i64,
    /// Null for a team review request (`models/issues/review.go:175-189`).
    pub user: Option<User>,
    pub body: String,
    pub state: String,
    pub submitted_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub html_url: String,
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
