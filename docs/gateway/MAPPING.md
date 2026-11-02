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
    | pending, description "Blocked by required conditions" | `WAITING` | null |
    | pending, other descriptions ("Has started running") | `IN_PROGRESS` | null |
    | success | `COMPLETED` | `SUCCESS` |
    | success, description "Has been skipped" | `COMPLETED` | `SKIPPED` |
    | failure, error or warning | `COMPLETED` | `FAILURE` (gh: fail) |
    | failure, error or warning, description "Has been cancelled" | `COMPLETED` | `CANCELLED` |

    A `warning` is a failure. Forgejo ranks it below pending and never lets it pass a required-status
    gate (`modules/structs/commit_status.go:23-29,55`), so `gh pr checks` exits 1 on one.
- **Non-Actions statuses** become `StatusContext {context, state, targetUrl, description, createdAt}`,
  with `state` mapped pending→PENDING, success→SUCCESS, error→ERROR, and failure and
  warning→FAILURE. StatusState has no "warning", and FAILURE is how Forgejo's own gate treats it.
- **Checks belong to the head commit.** gh reads them through `commits(last: 1)`. Forgejo lists PR
  commits newest first (`git log base..head`, `modules/git/repo_compare.go`), so reverse them into
  GitHub's oldest-first order; otherwise `last: 1` is the oldest commit and a fix pushed on a failing
  commit still reports the failure.
- **Latest per context only.** Forgejo returns status history, so keep the newest `updated_at` per
  `context`. gh dedupes too, but the rollup state must not count stale entries.
- **`isRequired(pullRequestId:)`** is true when the base branch's protection has `enable_status_check`
  and the context matches one of `status_check_contexts` (glob). With no patterns, every context is
  required, as in Forgejo (`services/pull/commit_status.go:23-71`).
- **Rollup `state`:** apply GitHub's documented combined-status rule to the mapped contexts:
  - any FAILURE or ERROR (and CheckRun conclusions FAILURE, TIMED_OUT, CANCELLED, ACTION_REQUIRED) → `FAILURE`;
  - else any pending (CheckRun not COMPLETED, StatusContext PENDING) → `PENDING`;
  - else, if there are contexts → `SUCCESS`;
  - **no contexts → `statusCheckRollup: null`**. gh then prints "no checks reported" (exit 1), matching
    the GitHub oracle for `s/nochecks`.

## 4. `reviewDecision`

**Decision:** null unless the base branch protection has `required_approvals > 0`,
`block_on_rejected_reviews` or `block_on_official_review_requests`. This matches the commonly observed
GitHub behavior, which is undocumented. When protection applies, follow Forgejo's own review gate
(`models/issues/pull.go:901-952`):

1. Count only **official** reviews: each reviewer's latest approval or rejection, made while they
   could write or were whitelisted (`models/issues/review.go:259-288,410-467`). Drop dismissed ones.
2. If `block_on_rejected_reviews` is set and any official review is REQUEST_CHANGES →
   `CHANGES_REQUESTED`.
3. Count the official APPROVED reviews, leaving out stale ones only when `ignore_stale_approvals` is
   set. If that count is below `required_approvals`, or `block_on_official_review_requests` is set and
   an official review request is open → `REVIEW_REQUIRED`.
4. Else, if the caller could read only the branch's protection summary (non-admins; see below) →
   null. The flags it hides can still fail the gate.
5. Else → `APPROVED`.

Forgejo serves the full protection rules only to repo admins. Everyone else gets the branch's summary
of its effective rule: status-check settings and `required_approvals`, but not the rejection,
stale-approval, review-request or outdated-branch flags. With that partial rule, `reviewDecision` is
null and `mergeStateStatus` is `UNKNOWN` (§5) wherever the visible gates pass.

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
6. Only the partial protection summary was readable (§4), **or** the rule has protected-file patterns
   → `UNKNOWN`. Forgejo refuses a merge that changes a protected file (`services/pull/merge.go:544-547`),
   but the API doesn't say which files those are, and its matcher lowercases names and globs with both
   `.` and `/` as separators (`models/git/protected_branch.go:214`). Until a port of that matcher can
   report `BLOCKED` exactly, any non-blank pattern means `UNKNOWN`.
7. Any context non-passing (rollup ≠ SUCCESS, and not null) → `UNSTABLE`.
8. Otherwise → `CLEAN`.

Never emit `HAS_HOOKS`. `isMergeQueueEnabled` is always false. `viewerCanMergeAsAdmin` is not
implemented: the schema has no such field, and `Protection` doesn't read `apply_to_admins`.

## 6. Merge and auto-merge (G3)

| gh / GitHub | Forgejo |
|---|---|
| `mergePullRequest(pullRequestId, mergeMethod, commitHeadline, commitBody, expectedHeadOid)` | `POST pulls/{n}/merge {Do: merge\|squash\|rebase, MergeTitleField, MergeMessageField, head_commit_id}` |
| head mismatch | Forgejo returns 409. The gateway returns the GraphQL error `{"type":"UNPROCESSABLE","path":["mergePullRequest"],"message":"Head branch was modified. Review and try the merge again."}`. **Confirmed 2026-10-30** against the GitHub twin: gh prints `GraphQL: Head branch was modified. Review and try the merge again. (mergePullRequest)` and exits 1 |
| `enablePullRequestAutoMerge` | `POST pulls/{n}/merge {..., merge_when_checks_succeed: true}` |
| `disablePullRequestAutoMerge` | `DELETE pulls/{n}/merge` (cancel scheduled merge) |
| `autoMergeRequest` | The latest `pull_scheduled_merge` or `pull_cancel_scheduled_merge` event on the issue timeline. Non-null only for an open PR whose latest such event is a schedule ([G3 findings](G3-FINDINGS.md)). Forgejo's API has no read for the schedule itself |
| REST `PUT pulls/{n}/merge` | The same mapping (not needed by any captured client yet) |

**Limit: auto-merge can stay enabled after Forgejo drops the schedule (F16).** Forgejo's worker deletes
the schedule before it tries the merge, and writes no event when that merge fails
(`services/automerge/automerge.go:227-236`). It fails, for example, when the scheduled merge style was
disallowed in the meantime, or when the base moves under it. The timeline's latest event is then still
the schedule, so `autoMergeRequest` stays non-null for as long as the PR is open. Forgejo's own UI has the
same blind spot (the FIXME at `automerge.go:231`).

The gateway can't prove a schedule still exists. Forgejo 15 has no read for it, and the worker deletes it
only after passing statuses and `CheckPullMergeable`, which the gateway can't reproduce exactly. A guess
would trade a rare stale "enabled" for a wrong "disabled" on schedules that are still waiting.

`gh pr merge --disable-auto` doesn't clear it either. With no schedule, Forgejo answers the cancel with
404 and writes no cancel event (`RemoveScheduledAutoMerge`, `automerge.go:71-78`). The gateway reports
that 404 as success, so gh says auto-merge is disabled while `autoMergeRequest` still shows it.

To clear it, run `gh pr merge --auto` again. The gateway tries the merge at once (`merge` in
`crates/fga-core/src/graphql/mutations.rs`), so one of three things happens:

- the PR merges;
- gh shows the error the worker hit, such as a disallowed merge style, which the user can then fix;
- Forgejo refuses for a reason a later event can lift (branch protection, conflicts), and the gateway
  schedules afresh. A later `--disable-auto` then writes the cancel event.

## 7. IDs

Node IDs are opaque base64 strings of `fga:<Type>:<owner>/<repo>:<number|id>`, per DESIGN.md.
`databaseId` holds the Forgejo numeric ID. `fullDatabaseId` is the same value as a string.
`PullRequestReviewThread` and `PullRequestReviewComment` IDs identify their node within a response,
but `node(id:)` doesn't look them up; it returns null for them.
A review thread's `viewerCanResolve` and `viewerCanUnresolve` follow Forgejo's `CanMarkConversation`:
the PR's author or a repository writer. Forgejo also lets an official reviewer without write access
resolve a conversation; the gateway reports `false` for them, since it can't tell cheaply.

The `viewerCan*` flags are hints for a client's buttons; Forgejo enforces every action. Editing,
closing and reopening a PR or a comment, and resolving a conversation, need write access to Forgejo's
pull request or issue unit, but its API reports only code access (`permissions.push`). The gateway
uses code access for both (Kell, 2026-11-02, rather than rebuilding Forgejo's team and collaborator
rules from extra reads). That is exact for owners, admins, collaborators and personal repositories;
in an organization repository where a team grants different access to code and pull requests, a flag
can be wrong either way.

Auto-merge flags follow Forgejo's merge check. They are false in an archived repository, need code
write access as the request's token allows it, and ask the base branch's `user_can_merge`, which
reflects a merge allowlist; only code writers can join one. If the base branch is deleted, write
access decides. Two cases the gateway can't see, so a flag can be wrong (Kell, 2026-11-02):

- A user on the allowlist who later lost write access may still merge, schedule a merge or cancel
  one, but reads as a token limited to other repositories, which may not. Both report no code write and `user_can_merge: true`; the
  gateway answers `false` for both.
- After the base branch is deleted, a protection rule can still match its name. The gateway reads
  no rules here, for any viewer, so it answers by write access alone.

A review thread's `originalLine` is the line the reviewer commented on, read from the end of the
comment's diff hunk. For a new-side comment, Forgejo's own `position` is the line in the commit that
last changed it, which differs once later commits move the line; an old-side position is the line
the reviewer chose. `line` is the same value while the PR's head is still the
commit the thread's review was made on, and null after a push: Forgejo's API doesn't say whether the
push moved or deleted the line. GitHub would still give a current line for a thread whose line
survived the push. Without a hunk (Forgejo's `CODE_COMMENT_LINES = 0`), both are null.

Forgejo sets a pending review's commit to the head at submit time and doesn't update its comments'
hunks (`models/issues/review.go`). A review started before a push and submitted after it therefore
reads as current, and its threads keep their old line; only Forgejo's unexposed `Invalidated` flag
would tell (Kell, 2026-11-02).

## 8. Web URLs (v1.1)

**Decision:** every pull request web URL the gateway or the shim emits uses GitHub's layout,
`{web base}/{o}/{r}/pull/{n}`, not Forgejo's `/pulls/{n}`. That covers GraphQL `PullRequest.url`, REST
`html_url`, `pr list --json url`, and URLs derived from a PR (`#issuecomment-…`, `#discussion_r…`, and
`/files`). Other web URLs (issues, commits, runs, repos) keep Forgejo's paths; they already match GitHub's.
One exception: the wrapper's own `--web` commands give Forgejo's `/pulls/{n}`, a page that opens without a
redirect in a client-only install.

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

## 9. Bodies (`bodyHTML`)

**Decision (2026-11-02):** `bodyHTML` on pull requests, issue comments and review comments is the body
as escaped paragraphs: the source text, HTML-escaped, with blank lines as paragraph breaks and single
newlines as `<br>`. Markdown isn't rendered.

**Limitation:** ChatGPT's PR pane shows Markdown as written. A heading reads `## Heading`, and links
and emphasis stay as source text.

**Why:** Forgejo's API returns no rendered HTML. Rendering through Forgejo's markdown endpoint costs one
request per body, and the timeline document can carry about 200 bodies. Rendering inside the gateway
would pass raw HTML from a PR body to clients that trust `bodyHTML` as sanitized, as GitHub's is.
Escaping costs nothing and can't inject markup. Optional sanitized rendering is
`gh-forgejo-shim-xml`, if the pane turns out to need it.

## Open items to verify against the live twin (G2/G3)

1. ~~Forgejo "checking" vs "conflict"~~: **resolved v1**. Not distinguishable; see §2.
2. ~~Actions status context format~~: **verified 2026-10-30** on the twin, for example `ci / lint (push)`.
   The history keeps several rows per context, and `target_url` is relative
   (`/dirtydishes/gfj-oracle/actions/runs/5/jobs/1`).
3. **Skipped-job reporting** in Forgejo Actions.
4. **Scheduled auto-merge discoverability** (`autoMergeRequest`).
5. ~~GitHub's exact head-mismatch error text~~: **confirmed**, see §6.
6. **`reviewDecision` null rule**, compared against the GitHub twin with and without branch protection.
