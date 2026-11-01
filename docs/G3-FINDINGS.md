# G3 write mapping findings

## Scheduled auto-merge comes from the timeline

Forgejo 15 exposes scheduled and cancelled merges through
`GET /repos/{owner}/{repo}/issues/{number}/timeline`. The event types are
`pull_scheduled_merge` and `pull_cancel_scheduled_merge`. Both have `created_at` and `user`.
A schedule event has an empty `body`. Cancelling can occur in the same second as scheduling,
so the gateway orders relevant events by timestamp and then numeric event ID. It reads every
page before choosing the latest event.

The binding G3 decision makes `autoMergeRequest` non-null only for an open, unmerged PR whose
latest relevant event schedules a merge. `enabledAt` uses that event's `created_at`, and
`enabledBy` uses its `user`. A closed or merged PR returns null even if its last event schedules
a merge. Repeated disable succeeds when the PR exists but no schedule remains. A missing PR
still returns NOT_FOUND.

The API does not expose the scheduled merge's style. The PR payload has no scheduled-merge
field. Swagger defines POST and DELETE on `pulls/{number}/merge` for scheduling and cancelling.
Its GET only checks whether the PR has already merged, returning 204 or 404. Forgejo stores
`MergeStyle` in its internal scheduled-merge model, but its timeline conversion emits no style.
The gateway therefore uses the repository's `default_merge_style` as the approved approximation:
`squash` maps to SQUASH, `rebase` and `rebase-merge` map to REBASE, and other styles map to MERGE.
This can differ from a schedule's explicitly chosen style, or change when the repository default
changes. `commitHeadline`, `commitBody`, and `authorEmail` are null. The captured GitHub MERGE
schedule also returns null for those fields.

The compact live evidence is `fixtures/forgejo15-auto-merge.json`. The rerunnable seed also saves
raw field names and selected timeline fields under the ignored e2e artifacts directory. No user
email, token, or full account response belongs in the evidence fixture.

Forgejo source at v15.0.0:

- [Schedule and cancel services](https://codeberg.org/forgejo/forgejo/src/tag/v15.0.0/services/automerge/automerge.go)
  create the timeline events and use the internal scheduled merge's style when executing it.
- [Timeline conversion](https://codeberg.org/forgejo/forgejo/src/tag/v15.0.0/services/convert/issue_comment.go)
  projects the event type, user, time, and body without the scheduled merge's style.
- [Merge API](https://codeberg.org/forgejo/forgejo/src/tag/v15.0.0/routers/api/v1/repo/pull.go)
  schedules with POST and cancels with DELETE. Its GET reports merged state, not the stored schedule.

## Auto-merge needs an unmet protection requirement

GitHub schedules auto-merge only when a base requirement remains unmet. The capture creates
`g3/auto-base` from main and protects that branch with required context `check`, `strict: false`,
and no review requirements. Its head adds `ci/slow` to keep the workflow pending. The local
Forgejo seed protects the same base with `ci / check (pull_request)`, its corresponding Actions
context, and no strict or review requirement. Main remains unprotected on both sides.

The GitHub twin already allows auto-merge and all three merge methods. The capture does not
change those repository settings. Existing g3 refs are never updated or deleted. New captures
use a fresh numbered suffix, and a per-run PR refusal floor includes issues as well as PRs.

## Conflict rejection differs before the mutation

GitHub's gh sees DIRTY and rejects a conflicted PR before sending a merge mutation. Forgejo's
boolean cannot distinguish a conflict from a mergeability check, so MAPPING §2 requires UNKNOWN.
Gh then attempts the mutation. Forgejo rejects it with HTTP 405.

A direct mutation on the GitHub conflict PR returns:

```json
{"type":"UNPROCESSABLE","path":["mergePullRequest"],"message":"Pull Request has merge conflicts"}
```

`fixtures/oracle/writes/conflict-error.json` contains that live response. The gateway uses this
message for Forgejo's merge 405, and the e2e seed compares the direct mutation's type, path, and
message against GitHub. Other 405 causes, including a WIP or an unfinished mergeability check,
share Forgejo's status code. The mapping cannot prove a conflict from 405 alone.

The gh conflict case has a strict expected stderr difference. Its stdout, exit code, and complete
read-back state must still match. The allowed stderr pair is the exact GitHub two-line preflight
rejection and `GraphQL: Pull Request has merge conflicts (mergePullRequest)`. An unrelated error,
changed PR state, or disappearance of the difference fails the comparison.

Development runs returned transient 405 responses for clean rebase and squash cases after
mergeability had reported true. The seed now posts its status before waiting for two settled
mergeability reads. The next full run succeeded with the same repository merge settings. The
gateway does not retry a merge command or claim that this response proves a conflict.

## Gh uses separate close and reopen mutations

Gh 2.103.3 sends `closePullRequest` and `reopenPullRequest`, selecting the PR ID and closed flag.
The captured request documents in `fixtures/oracle/writes` record both operations. The gateway
also supports `updatePullRequest(state:)` as required by the brief. All three use Forgejo's PR PATCH.

Before creation and merging, RepositoryInfo selects `mergeCommitAllowed`, `rebaseMergeAllowed`,
and `squashMergeAllowed`. These map to Forgejo's `allow_merge_commits`, `allow_rebase`, and
`allow_squash_merge`. The independent GitHub SDL fixture now pins those fields and the mutation
inputs and payloads. No new schema names or defaults come from guessing gh output.

## Writes need identities normalized per case

The write comparison reuses the read oracle's host, URL path, timestamp, SHA, node ID, and email
normalizers. Fresh write cases also need these identity substitutions:

- The case's recorded PR number becomes PR in its JSON number, PR URL, and gh's `#number` text.
  A PR created on GitHub and one created in a fresh container cannot share a database sequence.
  Other numeric fields remain significant.
- The recorded head branch becomes the case's canonical `g3/name`. Numbered capture suffixes
  on the two setup base branches also disappear. The actual head and base names still must match
  the case after that substitution.
- The numeric `#issuecomment-ID` fragment becomes a placeholder. The URL path, fragment kind,
  comment count, order, body, and author remain significant.

No normalizer changes titles, bodies, draft flags, PR states, merge methods, comment records,
or exit codes. The conflict expected failure accepts one exact stderr pair. Tests reject extra
stderr, changed comment bodies, and an expected failure that starts matching.
