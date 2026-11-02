# Field mapping: Forgejo → GitHub (A3)

Status: v1.1, 2026-11-01 (Opus). v1 folds in the G2a evidence (`docs/G2A-FINDINGS.md`); v1.1 adds §8, web URLs. The gateway (`fga-core::map`) and gh-forgejo-shim share this spec.

**Sources:**
- GitHub's public GraphQL schema (`schema.docs.graphql`, fetched 2026-10-30).
- `cli/cli` at `v2.103.3`.
- GitHub REST docs.
- Forgejo 15 swagger (`/swagger.v1.json`).

Rules marked **decision** are ours, because GitHub doesn't document the behavior. Each can be revisited
when a live comparison against the GitHub twin disagrees.

## What clients actually read

| Field | gh | Claude | ChatGPT |
|---|---|---|---|
| `statusCheckRollup.contexts` (per check) | yes: `pr checks`, `pr view`, `pr status` | REST check data (TBD) | yes |
| `statusCheckRollup.state` (rollup) | **no**, never read | likely | yes |
| `mergeStateStatus` | `pr merge` (blocks on DIRTY/BLOCKED/BEHIND; merges now only on CLEAN/HAS_HOOKS/UNSTABLE), `pr status` (BEHIND) | ? | yes (merge button) |
| `mergeable` | `pr status` (UNKNOWN → "status unknown"), `pr update-branch` | ? | yes |
| `reviewDecision` | `pr status`, `pr view` labels | ? | yes |

**gh always sends introspection** (`__type(name:"PullRequest")`, `StatusCheckRollupContextConnection`,
`WorkflowRun`) and picks fields from the answer. The schema subset therefore has to be *honest*: list
only the fields we serve.

## 1. `PullRequest.state`, `isDraft`, `closed`, `merged`

| GitHub | Forgejo |
|---|---|
| `MERGED` | `merged == true` |
| `CLOSED` | `state == "closed" && !merged` |
| `OPEN` | `state == "open"` |
| `isDraft` | `draft`. Forgejo derives it from a WIP title prefix |
| `title` | **v1 decision:** when `draft`, strip the leading Forgejo WIP prefix (configured, default `WIP:` and `[WIP]`, case-insensitive, plus following whitespace) so titles match GitHub, where draft is a flag. Mutations that set a title on a draft must re-add the prefix (G3) |
| REST `state` | `open`/`closed`. Merged PRs are `closed`, with `merged: true` and `merged_at` set |

## 2. `mergeable`

**v1, from G2a evidence:** Forgejo 15's API `mergeable` is one boolean. `pr.Mergeable()` returns false
for *checking*, *conflict*, *error* and *WIP* alike, so the API can't prove a conflict.

| GitHub | Forgejo |
|---|---|
| `MERGEABLE` | open, and `mergeable == true` |
| `UNKNOWN` | open and `mergeable == false` after one re-read about 300 ms later within the deadline, **or** draft (WIP always reports false), **or** closed or merged |
| `CONFLICTING` | **Never emitted** until there's a source that proves a conflict. Candidate: a later Forgejo API field, or a dry-run merge check |

**Consequence:** `gh pr merge` attempts the merge on UNKNOWN, and Forgejo rejects a conflicting merge.
Return that rejection as a GitHub-shaped error (G3).
## 3. Check contexts (`statusCheckRollup.contexts`)

Forgejo has commit statuses only. Reads return the state in the field **`status`**, while POSTs send
`state`. (`GET repos/{o}/{r}/commits/{sha}/statuses`; states `pending`,
`success`, `error`, `failure`, `warning`). Forgejo Actions jobs report through them, with context
`"<workflow> / <job> (<event>)"` and `target_url` pointing to the Actions run.

- **Decision: Actions statuses become `CheckRun`; everything else becomes `StatusContext`.** gh, ChatGPT
  and Claude render CheckRuns with workflow and event information. Identify an Actions status by its
  `target_url` pointing to `/{o}/{r}/actions/runs/` and its context matching `^(.+) / (.+) \((.+)\)$`.
  Then:
  - `name` ← job, `checkSuite.workflowRun.workflow.name` ← workflow, `.event` ← event.
  - `detailsUrl` ← `target_url`. Forgejo returns it **relative** (`/o/r/actions/runs/5/jobs/1`), so prefix the forge's public web base. `startedAt` ← `created_at`, `completedAt` ← `updated_at` when terminal.
  - `status`/`conclusion`:

    | Forgejo | `status` | `conclusion` |
    |---|---|---|
    | pending, description "Waiting to run" | `QUEUED` | null |
    | pending, other descriptions ("Has started running") | `IN_PROGRESS` | null |
    | success | `COMPLETED` | `SUCCESS` |
    | failure | `COMPLETED` | `FAILURE` |
    | error | `COMPLETED` | `FAILURE` (gh: fail) |
    | warning | `COMPLETED` | `NEUTRAL` (gh: skipping, i.e. not a failure and not pending) |

    Skipped Actions jobs report as `success` with a description such as "Has been skipped". If the
    description says it was skipped, return `SKIPPED`.
- **Non-Actions statuses** become `StatusContext {context, state, targetUrl, description, createdAt}`,
  with `state` mapped pending→PENDING, success→SUCCESS, error→ERROR, failure→FAILURE and
  warning→**SUCCESS**. The last is a decision: StatusState has no "warning", and gh would bucket an
  unknown value as pending, which means exit 8 forever.
- **Latest per context only.** Forgejo returns status history, so keep the newest `updated_at` per
  `context`. gh dedupes too, but the rollup state must not count stale entries.
- **`isRequired(pullRequestId:)`** is true when the base branch's protection has `enable_status_check`
  and the context matches one of `status_check_contexts` (glob).
- **Rollup `state`:** apply GitHub's documented combined-status rule to the mapped contexts:
  - any FAILURE or ERROR (and CheckRun conclusions FAILURE, TIMED_OUT, CANCELLED, ACTION_REQUIRED) → `FAILURE`;
  - else any pending (CheckRun not COMPLETED, StatusContext PENDING) → `PENDING`;
  - else, if there are contexts → `SUCCESS`;
  - **no contexts → `statusCheckRollup: null`**. gh then prints "no checks reported" (exit 1), matching
    the GitHub oracle for `s/nochecks`.

## 4. `reviewDecision`

**Decision:** null unless the base branch protection has `required_approvals > 0` or
`block_on_rejected_reviews`. This matches the commonly observed GitHub behavior, which is undocumented.
When protection applies:

1. Take the reviews (`pulls/{n}/reviews`), drop `dismissed`, drop `stale` if `dismiss_stale_approvals`
   is set, and drop state COMMENT and PENDING. Keep the **latest per user**.
2. If any is REQUEST_CHANGES and `block_on_rejected_reviews` is set (or `official` is true) → `CHANGES_REQUESTED`.
3. Else, if the count of APPROVED (official only, when `enable_approvals_whitelist` is set) ≥
   `required_approvals` → `APPROVED`.
4. Else → `REVIEW_REQUIRED`.

`latestReviews` is the latest non-pending review per user, in GitHub's `PullRequestReview` shape:
`state` maps APPROVED → APPROVED, REQUEST_CHANGES → CHANGES_REQUESTED, COMMENT → COMMENTED, and dismissed
reviews → DISMISSED.

## 5. `mergeStateStatus`

**Decision:** evaluate in this order and return the first that applies (open PRs only; closed or
merged → `UNKNOWN`):

1. `mergeable == UNKNOWN` → `UNKNOWN`.
2. `mergeable == CONFLICTING` → `DIRTY`.
3. ~~`isDraft` → `DRAFT`~~. **Removed in v1:** the GitHub twin reports `CLEAN` for a clean draft
   (`fixtures/oracle/probe-4.stdout`). Drafts follow the remaining steps, and because their `mergeable`
   is UNKNOWN (§2) they resolve to `UNKNOWN` at step 1.
4. Protection `block_on_outdated_branch` and the head doesn't contain the base → `BEHIND`. Forgejo 15's Compare has no
   `behind_by`, so use the reverse comparison `GET repos/{o}/{r}/compare/{head}...{base}` and check that
   `total_commits > 0` (v1, G2a evidence). Skip the check if the endpoint is missing.
5. Protection requirements unmet → `BLOCKED`. That means either `reviewDecision` ∈
   {REVIEW_REQUIRED, CHANGES_REQUESTED}, or required status contexts that are missing, pending or
   failing.
6. Any context non-passing (rollup ≠ SUCCESS, and not null) → `UNSTABLE`.
7. Otherwise → `CLEAN`.

Never emit `HAS_HOOKS`. `isMergeQueueEnabled` is always false. `viewerCanMergeAsAdmin` ←
`apply_to_admins == false` and the viewer is a repo admin (G3).

## 6. Merge and auto-merge (G3)

| gh / GitHub | Forgejo |
|---|---|
| `mergePullRequest(pullRequestId, mergeMethod, commitHeadline, commitBody, expectedHeadOid)` | `POST pulls/{n}/merge {Do: merge\|squash\|rebase, MergeTitleField, MergeMessageField, head_commit_id}` |
| head mismatch | Forgejo returns 409. The gateway returns the GraphQL error `{"type":"UNPROCESSABLE","path":["mergePullRequest"],"message":"Head branch was modified. Review and try the merge again."}`. **Confirmed 2026-10-30** against the GitHub twin: gh prints `GraphQL: Head branch was modified. Review and try the merge again. (mergePullRequest)` and exits 1 |
| `enablePullRequestAutoMerge` | `POST pulls/{n}/merge {..., merge_when_checks_succeed: true}` |
| `disablePullRequestAutoMerge` | `DELETE pulls/{n}/merge` (cancel scheduled merge) |
| `autoMergeRequest` | **Open:** Forgejo's API has no "get scheduled merge". Candidates are the PR timeline (scheduled-merge comment type) or the issue comments API. Until verified, return `null`. G3 must solve this, because ChatGPT shows auto-merge state |
| REST `PUT pulls/{n}/merge` | The same mapping (not needed by any captured client yet) |

## 7. IDs

Node IDs are opaque base64 strings of `fga:<Type>:<owner>/<repo>:<number|id>`, per DESIGN.md.
`databaseId` holds the Forgejo numeric ID. `fullDatabaseId` is the same value as a string.

## 8. Web URLs (v1.1)

**Decision:** every pull request web URL the gateway or the shim emits uses GitHub's layout,
`{web base}/{o}/{r}/pull/{n}`, not Forgejo's `/pulls/{n}`. That covers GraphQL `PullRequest.url`, REST
`html_url`, `pr list --json url`, and URLs derived from a PR (`#issuecomment-…`, `#discussion_r…`, and
`/files`). Other web URLs (issues, commits, runs, repos) keep Forgejo's paths; they already match GitHub's.

The gateway answers `GET /{o}/{r}/pull/{n}{rest}` with `301 Location: /{o}/{r}/pulls/{n}{rest}`, keeping
the query and fragment. The deployment's proxy routes that path pattern to the gateway (G5), so every
emitted link opens.

**Evidence** (live client probe, 2026-11-01):
- ChatGPT 22943 on a Forgejo remote received `pr list` output with `…/pulls/1`. It found the PR, then
  ignored it: no GraphQL followed, and no PR showed.
- With `…/pull/1`, the same app immediately sent all six of its GraphQL documents to
  `--hostname <forge>`. The bundle parses PR URLs with `\/pull\/(\d+)`.
- Forgejo has no `/pull/{n}` route: Codeberg (Forgejo 16) returns 404 for it and 200 for `/pulls/{n}`.

This replaces DESIGN.md's earlier "emit what Forgejo actually serves" rule for PR URLs.

## Open items to verify against the live twin (G2/G3)

1. ~~Forgejo "checking" vs "conflict"~~: **resolved v1**. Not distinguishable; see §2.
2. ~~Actions status context format~~: **verified 2026-10-30** on the twin, for example `ci / lint (push)`.
   The history keeps several rows per context, and `target_url` is relative
   (`/dirtydishes/gfj-oracle/actions/runs/5/jobs/1`).
3. **Skipped-job reporting** in Forgejo Actions.
4. **Scheduled auto-merge discoverability** (`autoMergeRequest`).
5. ~~GitHub's exact head-mismatch error text~~: **confirmed**, see §6.
6. **`reviewDecision` null rule**, compared against the GitHub twin with and without branch protection.
