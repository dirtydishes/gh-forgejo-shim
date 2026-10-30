# R3 implementation deliverable

## 1. What changed

- `contract/README.md`: Documents selective captures, shim-local cases, and grammar checks.
- `contract/cases/error-invalid-bool.json`: Captures the client error from official gh through the R1 capture script.
- `contract/cases/error-jq-without-json.json`: Captures the client error from official gh through the R1 capture script.
- `contract/cases/error-missing-json-value.json`: Captures the client error from official gh through the R1 capture script.
- `contract/cases/error-missing-value.json`: Captures the client error from official gh through the R1 capture script.
- `contract/cases/error-unknown-flag.json`: Captures the client error from official gh through the R1 capture script.
- `contract/cases/error-unknown-json-field.json`: Captures the client error from official gh through the R1 capture script.
- `contract/cases/error-unknown-pr-command.json`: Captures the client error from official gh through the R1 capture script.
- `contract/cases/error-unknown-short-flag.json`: Captures the client error from official gh through the R1 capture script.
- `contract/cases/local-unsupported-api-repos.json`: Checks local rejection and the real-gh decoy without a GitHub oracle.
- `contract/cases/local-unsupported-edit.json`: Checks local rejection and the real-gh decoy without a GitHub oracle.
- `contract/cases/local-unsupported-merge.json`: Checks local rejection and the real-gh decoy without a GitHub oracle.
- `contract/cases/pr-view-attached-repo.json`: Adds a passing flag-form parity case with an official-gh capture.
- `contract/cases/pr-view-equal-repo.json`: Adds a passing flag-form parity case with an official-gh capture.
- `contract/cases/pr-view-repeated-json.json`: Adds a passing flag-form parity case with an official-gh capture.
- `contract/cases/pr-view-short-repo.json`: Adds a passing flag-form parity case with an official-gh capture.
- `contract/tiers.toml`: Copies the supplied grammar unchanged except for 15 native support labels.
- `contract/usage-errors.json`: Stores pinned gh help for exact error usage and command names.
- `crates/gh-forgejo-shim/src/external.rs`: Keeps delegated arguments as OsString to preserve their bytes.
- `crates/gh-forgejo-shim/src/invocation.rs`: Adds the shared grammar parser, typed errors, and parser tests.
- `crates/gh-forgejo-shim/src/lib.rs`: Exports invocation.
- `crates/gh-forgejo-shim/src/repo.rs`: Uses parsed repository hints and removes argv extraction helpers.
- `crates/gh-forgejo-shim/src/routing.rs`: Resolves every host before validation and blocks unsupported Forgejo delegation.
- `crates/gh-forgejo-shim/tests/cli_scaffold.rs`: Registers invocation process tests.
- `crates/gh-forgejo-shim/tests/support/invocation.rs`: Checks local rejection, help, hostname routing, and byte-preserving delegation.
- `docs/implementation/r3-grammar-strict.md`: Records this deliverable and exact captured errors.
- `scripts/contract/capture-usage.py`: Rebuilds usage captures with pinned gh help and no login.
- `scripts/contract/capture.py`: Skips local cases, adds --case selection, and fixes the missing Path import.
- `scripts/contract/check-parser.py`: Compares 107 argument errors with pinned gh using dummy auth and a closed proxy.
- `scripts/contract/common.py`: Adds exact local/error allowlists and validates attached repository flags.
- `scripts/contract/runner.py`: Reads expected output for shim-local cases without changing comparison rules.
- `scripts/contract/test_contract.py`: Checks local-case safety, capture exclusion, and stream/exit comparisons.

The dispatcher constructs one `Invocation`. Command discovery and flag parsing are phases of that parser,
as in Cobra. Routing reads its fields and validation result. The existing handlers still parse their own
arguments, as required by the brief. The adapter expands attached short values and boolean clusters,
combines repeated JSON fields, and applies the last boolean value before calling those handlers.

## 2. Gate output

All required gates passed. Logs from this run are under ignored `target/r3`.

```text
cargo fmt --all -- --check
(no output; exit 0)

cargo clippy --workspace --all-targets --locked -- -D warnings
    Checking gh-forgejo-shim v0.1.2 (/Users/kell/code/.worktrees/gh-forgejo-shim/r3-grammar-strict/crates/gh-forgejo-shim)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.12s

cargo test --workspace --locked
test result: ok. 137 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.60s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.81s
Ran 19 tests in 0.167s
OK
test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.26s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

scripts/contract/run.sh, before
pass 6, xfail 4, unexpected 0

scripts/contract/run.sh, after
pass 21, xfail 4, unexpected 0

/Users/kell/code/foreman/tools/linux-gate.sh "$PWD" sh -c 'cargo test --workspace --locked'
test result: ok. 137 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.78s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
Ran 19 tests in 0.013s
OK
test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.08s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

python3 scripts/contract/check-parser.py
parser oracle: 107 matched, 0 mismatched

! rg -n '^fn (is_supported_command|is_supported_auth_command|is_supported_api_command|hostname_arg|api_endpoint|has_flag_value_prefix)\b' crates/gh-forgejo-shim/src/routing.rs
(no output; exit 0)
```

The last check uses the actual removed helper names instead of the brief's example names.
`repo.rs` also loses `extract_repo_arg` and `extract_repo_url_arg`.
All ten original contract cases keep their pass or xfail status. The four existing xfails remain unchanged.
The supplied grammar matches byte for byte after restoring the native labels to planned.
The final reply supplies `git diff --stat 3c2a495` and the enclosing commit SHA.

The Linux gate ran through a checkout-local Docker adapter. It added a `gfjs-` container name and
changed the script's shared volume names to unique `gfjs-` names. The original gate script stayed unchanged.
Cleanup removed only those exact volumes. No owned Linux containers or volumes remain.

## 3. Native commands and evidence

All evidence below comes from the unchanged `crates/gh-forgejo-shim/src/read_only.rs` dispatch functions.
The previous routing allowlists also contained these canonical commands. These are the only native labels.

- `auth status`: `run_auth` matches `status` and calls `parse_auth_status_args`.
- `auth token`: `run_auth` matches `token` and calls `parse_auth_token_args`.
- `api`: `run_api` handles only `user` and `/user`. The new support check keeps this endpoint restriction.
- `repo view`: `run_repo_view_command` matches `view` and calls `run_repo_view`.
- `pr checks`: `run_pr_command` matches `checks` and calls `run_checks`.
- `pr checkout`: `run_pr_command` matches `checkout` and `co`, then calls `run_checkout`.
- `pr comment`: `run_pr_command` matches `comment` and calls `run_comment`.
- `pr create`: `run_pr_command` matches `create` and `new`, then calls `run_create`.
- `pr diff`: `run_pr_command` matches `diff` and calls `run_diff`.
- `pr list`: `run_pr_command` matches `list` and calls `run_list`.
- `pr status`: `run_pr_command` matches `status` and calls `run_status`.
- `pr view`: `run_pr_command` matches `view` and calls `run_view`.
- `issue create`: `run_issue_command` matches `create` and `new`, then calls `run_issue_create`.
- `issue list`: `run_issue_command` matches `list` and `ls`, then calls `run_issue_list`.
- `issue view`: `run_issue_command` matches `view` and calls `run_issue_view`.

## 4. Captured gh error texts

Every case below has exit code 1 and empty stdout. Each block contains the full raw stderr.
The trailing two spaces in some blocks come from gh. The committed case JSON also preserves every byte.
All eight captures came from `scripts/contract/capture-oracle.sh --case ...` and checksum-pinned gh 2.103.3.

### error-invalid-bool

`gh pr view 1 --comments=no`

```text
invalid argument "no" for "-c, --comments" flag: strconv.ParseBool: parsing "no": invalid syntax

Usage:  gh pr view [<number> | <url> | <branch>] [flags]

Flags:
  -c, --comments          View pull request comments
  -q, --jq expression     Filter JSON output using a jq expression
      --json fields       Output JSON with the specified fields
  -t, --template string   Format JSON output using a Go template; see "gh help formatting"
  -w, --web               Open a pull request in the browser
  
```

### error-jq-without-json

`gh pr view 1 --jq .number`

```text
cannot use `--jq` without specifying `--json`
```

### error-missing-json-value

`gh pr view 1 --json`

```text
Specify one or more comma-separated fields for `--json`:
  additions
  assignees
  author
  autoMergeRequest
  baseRefName
  baseRefOid
  body
  changedFiles
  closed
  closedAt
  closingIssuesReferences
  comments
  commits
  createdAt
  deletions
  files
  fullDatabaseId
  headRefName
  headRefOid
  headRepository
  headRepositoryOwner
  id
  isCrossRepository
  isDraft
  labels
  latestReviews
  maintainerCanModify
  mergeCommit
  mergeStateStatus
  mergeable
  mergedAt
  mergedBy
  milestone
  number
  potentialMergeCommit
  projectCards
  projectItems
  reactionGroups
  reviewDecision
  reviewRequests
  reviews
  state
  statusCheckRollup
  title
  updatedAt
  url
```

### error-missing-value

`gh pr view 1 --template`

```text
flag needs an argument: --template

Usage:  gh pr view [<number> | <url> | <branch>] [flags]

Flags:
  -c, --comments          View pull request comments
  -q, --jq expression     Filter JSON output using a jq expression
      --json fields       Output JSON with the specified fields
  -t, --template string   Format JSON output using a Go template; see "gh help formatting"
  -w, --web               Open a pull request in the browser
  
```

### error-unknown-flag

`gh pr view 1 --gfjs-unknown`

```text
unknown flag: --gfjs-unknown

Usage:  gh pr view [<number> | <url> | <branch>] [flags]

Flags:
  -c, --comments          View pull request comments
  -q, --jq expression     Filter JSON output using a jq expression
      --json fields       Output JSON with the specified fields
  -t, --template string   Format JSON output using a Go template; see "gh help formatting"
  -w, --web               Open a pull request in the browser
  
```

### error-unknown-json-field

`gh pr view 1 --json x`

```text
Unknown JSON field: "x"
Available fields:
  additions
  assignees
  author
  autoMergeRequest
  baseRefName
  baseRefOid
  body
  changedFiles
  closed
  closedAt
  closingIssuesReferences
  comments
  commits
  createdAt
  deletions
  files
  fullDatabaseId
  headRefName
  headRefOid
  headRepository
  headRepositoryOwner
  id
  isCrossRepository
  isDraft
  labels
  latestReviews
  maintainerCanModify
  mergeCommit
  mergeStateStatus
  mergeable
  mergedAt
  mergedBy
  milestone
  number
  potentialMergeCommit
  projectCards
  projectItems
  reactionGroups
  reviewDecision
  reviewRequests
  reviews
  state
  statusCheckRollup
  title
  updatedAt
  url
```

### error-unknown-pr-command

`gh pr gfjs-unknown`

```text
unknown command "gfjs-unknown" for "gh pr"

Usage:  gh pr <command> [flags]

Available commands:
  checkout
  checks
  close
  comment
  create
  diff
  edit
  list
  lock
  merge
  ready
  reopen
  revert
  review
  status
  unlock
  update-branch
  view
```

### error-unknown-short-flag

`gh pr view 1 -Z`

```text
unknown shorthand flag: 'Z' in -Z

Usage:  gh pr view [<number> | <url> | <branch>] [flags]

Flags:
  -c, --comments          View pull request comments
  -q, --jq expression     Filter JSON output using a jq expression
      --json fields       Output JSON with the specified fields
  -t, --template string   Format JSON output using a Go template; see "gh help formatting"
  -w, --web               Open a pull request in the browser
  
```

## 5. Matched pflag behaviors and limits

The unit tests, process tests, captured contracts, and 107-probe comparison cover:

- Separate and equal long values, including `--repo=x` and flag values beginning with `-`.
- Separate, attached, and equal short values, including `-R x`, `-Rx`, and `-R=x`.
- Boolean clusters, explicit boolean values, Go boolean spellings, and the remaining-cluster text in errors.
- Repeated flags, the last scalar/boolean value, and accumulated JSON field selections.
- CSV JSON fields, quoted commas, malformed quote errors, and preserved whitespace in field names.
- `--` termination. Later tokens stay positional and cannot supply a repository override.
- Go integer syntax checks, including binary/hex/octal prefixes, separators, signs, and range errors.
- Command-specific short meanings, including `-b` for create/checkout and `-a` for run list/view.
- Cobra command discovery and argument-error precedence, including the `run -a list` ambiguity.
- Missing-value, JSON-field, unknown-command, and shorthand errors, plus spelling suggestions.
- Local help from pinned gh captures and hostname flags that use `-h` instead of help.

The parser stores repeated raw values in `Invocation.flags`. It expands supported input forms for the
existing handlers but does not port their value interpretation or output code. Non-JSON CSV coercion,
duration values, command-specific option relationships, and handler output remain outside this change.
Commands absent from the table fail locally as unsupported. Their full flag grammar is not inferred.

## 6. Deviations and open questions

- Added `usage-errors.json` and its help capture script. The supplied table lacks flag descriptions,
  spacing, and the full subcommand list needed to render gh's exact errors. Help capture reads no login
  and calls `gh help pr merge` and `gh help pr edit`, never the write commands themselves.
- Fixed an existing missing `Path` import in the R1 capture script so the required oracle capture can run.
- Added selective `--case` capture to preserve the existing captures and scenarios. Safety validation
  still checks every case before any login read. Local cases never reach oracle capture.
- Added attached-repository and repeated-JSON passing cases beyond the required separate/equal forms.
- Replaced the old repository argv helpers as well as routing helpers, so routing cannot parse flags twice.
- Preserved delegated arguments as OS strings to meet byte-for-byte GitHub forwarding, including non-UTF-8 bytes.
- Used the Docker adapter described above to satisfy the `gfjs-` naming constraint while running the exact Linux gate.
- No handler, workflow, Beads, original contract case, or comparison rule changed. The comparison has only
  the new `shim-local` expected-output source. No `bd`, push, GitHub write, or cleanup of another agent's resources ran.
- No blocking question remains. Existing handler limits still apply to valid options within a native command.

## 7. Commit

Branch: `foreman/r3-grammar-strict`. The final implementation reply records the commit SHA.
The report belongs to that commit. No push occurred.
