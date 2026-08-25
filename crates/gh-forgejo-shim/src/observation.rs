//! Exact process shapes whose output the shim may count without breaking streaming.

use crate::invocation::{Command, ParsedInvocation};

const PR_CHECKS_FIELDS: &str =
    "bucket,completedAt,description,event,link,name,startedAt,state,workflow";
const PR_LIST_FIELDS: &str = "number,url,state,headRefName";
const PR_VIEW_FIELDS: &str = "additions,author,autoMergeRequest,baseRefName,comments,createdAt,deletions,headRefName,headRefOid,isDraft,latestReviews,mergedAt,mergedBy,mergeStateStatus,mergeable,number,reviews,reviewDecision,reviewRequests,state,title,updatedAt,url,body,commits";

#[derive(Debug, Clone, Copy)]
enum ContractArgument<'a> {
    Literal(&'a str),
    NonOption,
    PullNumber,
}

pub(crate) fn observes_build_6720_output(invocation: &ParsedInvocation) -> bool {
    if invocation.help_requested() || invocation.ambiguity().is_some() {
        return false;
    }

    let argv = invocation.canonical_argv();
    match invocation.command() {
        Command::GlobalDelegate => argv == ["--version"],
        Command::AuthStatus => matches_contract_shape(
            argv,
            &[
                ContractArgument::Literal("auth"),
                ContractArgument::Literal("status"),
                ContractArgument::Literal("--active"),
                ContractArgument::Literal("--hostname"),
                ContractArgument::NonOption,
            ],
        ),
        Command::PrList => matches_contract_shape(
            argv,
            &[
                ContractArgument::Literal("pr"),
                ContractArgument::Literal("list"),
                ContractArgument::Literal("--head"),
                ContractArgument::NonOption,
                ContractArgument::Literal("--author"),
                ContractArgument::Literal("@me"),
                ContractArgument::Literal("--state"),
                ContractArgument::Literal("all"),
                ContractArgument::Literal("--json"),
                ContractArgument::Literal(PR_LIST_FIELDS),
                ContractArgument::Literal("--repo"),
                ContractArgument::NonOption,
            ],
        ),
        Command::PrView => matches_contract_shape(
            argv,
            &[
                ContractArgument::Literal("pr"),
                ContractArgument::Literal("view"),
                ContractArgument::PullNumber,
                ContractArgument::Literal("--json"),
                ContractArgument::Literal(PR_VIEW_FIELDS),
                ContractArgument::Literal("--repo"),
                ContractArgument::NonOption,
            ],
        ),
        Command::PrChecks => matches_contract_shape(
            argv,
            &[
                ContractArgument::Literal("pr"),
                ContractArgument::Literal("checks"),
                ContractArgument::PullNumber,
                ContractArgument::Literal("--json"),
                ContractArgument::Literal(PR_CHECKS_FIELDS),
                ContractArgument::Literal("--repo"),
                ContractArgument::NonOption,
            ],
        ),
        _ => false,
    }
}

fn matches_contract_shape(argv: &[String], shape: &[ContractArgument<'_>]) -> bool {
    argv.len() == shape.len()
        && argv
            .iter()
            .zip(shape)
            .all(|(argument, expected)| match expected {
                ContractArgument::Literal(literal) => argument == literal,
                ContractArgument::NonOption => !argument.is_empty() && !argument.starts_with('-'),
                ContractArgument::PullNumber => {
                    argument.parse::<u64>().is_ok_and(|number| number > 0)
                }
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn observes_only_typed_build_6720_process_shapes() {
        for values in [
            vec!["--version"],
            vec![
                "auth",
                "status",
                "--active",
                "--hostname",
                "git.example.com",
            ],
            vec![
                "pr",
                "list",
                "--head",
                "main",
                "--author",
                "@me",
                "--state",
                "all",
                "--json",
                PR_LIST_FIELDS,
                "--repo",
                "git.example.com/owner/repo",
            ],
            vec![
                "pr",
                "view",
                "13",
                "--json",
                PR_VIEW_FIELDS,
                "--repo",
                "git.example.com/owner/repo",
            ],
            vec![
                "pr",
                "checks",
                "13",
                "--json",
                PR_CHECKS_FIELDS,
                "--repo",
                "git.example.com/owner/repo",
            ],
        ] {
            let invocation = ParsedInvocation::parse(&argv(&values));
            assert!(
                observes_build_6720_output(&invocation),
                "not observed: {values:?}"
            );
        }

        for values in [
            vec![
                "pr",
                "checks",
                "--watch",
                "--json",
                PR_CHECKS_FIELDS,
                "--repo",
                "git.example.com/owner/repo",
            ],
            vec![
                "pr",
                "view",
                "--help",
                "--json",
                PR_VIEW_FIELDS,
                "--repo",
                "git.example.com/owner/repo",
            ],
        ] {
            let invocation = ParsedInvocation::parse(&argv(&values));
            assert!(
                !observes_build_6720_output(&invocation),
                "observed: {values:?}"
            );
        }
    }
}
