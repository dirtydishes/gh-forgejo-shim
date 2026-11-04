use super::*;
use async_graphql::{Enum, ID, InputObject};

#[derive(InputObject)]
pub struct CreatePullRequestInput {
    pub repository_id: ID,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub title: String,
    pub body: Option<String>,
    #[graphql(default_with = "Some(false)")]
    pub draft: Option<bool>,
    #[graphql(default_with = "Some(true)")]
    pub maintainer_can_modify: Option<bool>,
    pub client_mutation_id: Option<String>,
}

/// `gh issue create` sends these; labels, assignees, a milestone and projects need gh's metadata
/// lookups first and aren't served yet, so their fields are left out and refused as unknown.
#[derive(InputObject)]
pub struct CreateIssueInput {
    pub repository_id: ID,
    pub title: String,
    pub body: Option<String>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct AddCommentInput {
    pub subject_id: ID,
    pub body: String,
    pub client_mutation_id: Option<String>,
}

#[derive(Clone, Copy, Enum, Eq, PartialEq)]
pub enum PullRequestUpdateState {
    Open,
    Closed,
}

#[derive(InputObject)]
pub struct UpdatePullRequestInput {
    pub pull_request_id: ID,
    pub title: Option<String>,
    pub body: Option<String>,
    pub base_ref_name: Option<String>,
    pub state: Option<PullRequestUpdateState>,
    pub maintainer_can_modify: Option<bool>,
    pub client_mutation_id: Option<String>,
}

macro_rules! pull_id_input {
    ($name:ident) => {
        #[derive(InputObject)]
        pub struct $name {
            pub pull_request_id: ID,
            pub client_mutation_id: Option<String>,
        }
    };
}
pull_id_input!(ClosePullRequestInput);
pull_id_input!(ReopenPullRequestInput);
pull_id_input!(ConvertPullRequestToDraftInput);
pull_id_input!(MarkPullRequestReadyForReviewInput);
pull_id_input!(DisablePullRequestAutoMergeInput);

macro_rules! merge_input {
    ($name:ident) => {
        #[derive(InputObject)]
        pub struct $name {
            pub pull_request_id: ID,
            #[graphql(default_with = "Some(PullRequestMergeMethod::Merge)")]
            pub merge_method: Option<PullRequestMergeMethod>,
            pub expected_head_oid: Option<GitObjectId>,
            pub commit_headline: Option<String>,
            pub commit_body: Option<String>,
            pub client_mutation_id: Option<String>,
        }
    };
}
merge_input!(MergePullRequestInput);
merge_input!(EnablePullRequestAutoMergeInput);
