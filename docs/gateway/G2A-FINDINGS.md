# G2a PR read findings

The live GitHub capture and Forgejo 15 comparison produce 13 passes, 1 expected failure, and
0 unexpected differences across 14 cases. The G1 assertions also pass. The case definitions live
in `scripts/oracle/cases.json`; each committed oracle case has `.exit`, `.stdout`, and `.stderr` files.
`wide-1` adds the large JSON selection from the recorder to the brief's required cases.

## Mapping gaps with evidence

### A false mergeable value does not prove a conflict

Forgejo's API exposes a boolean `mergeable` and no checking or conflict discriminator. The live clean
draft PR returns `draft: true` and `mergeable: false`. The Swagger `PullRequest` object confirms the
boolean type. The web router has no `pulls/{number}.json` endpoint.

Forgejo 15's [API conversion](https://codeberg.org/forgejo/forgejo/src/tag/v15.0.0/services/convert/pull.go#L89)
calls `pr.Mergeable(ctx)`. Its [implementation](https://codeberg.org/forgejo/forgejo/src/tag/v15.0.0/models/issues/pull.go#L880)
returns false for checking, conflicts, errors, and WIP PRs. The REST response loses that distinction.
MAPPING.md v1 §2 therefore forbids emitting `CONFLICTING` from this source.

The gateway re-reads an open false PR once after 300 ms, within the request deadline. A persistent
false maps to `UNKNOWN`. The pure merge-state function still supports `CONFLICTING` → `DIRTY`, but
the current Forgejo API cannot prove that input. The gateway also returns `UNKNOWN` for closed and
merged PRs, and for drafts even if the upstream boolean is true, as specified. Tests cover a retry
that resolves, a persistent false, and the precedence rules independently of this upstream limit.

### GitHub reports CLEAN for the draft twin

`fixtures/oracle/probe-4.stdout` contains `isDraft: true`, `mergeable: "MERGEABLE"`, and
`mergeStateStatus: "CLEAN"`. MAPPING.md v1 §5 removes the former `DRAFT` step to match that evidence.

The pure mapper and its table tests now apply the remaining rules to drafts. The live Forgejo draft
produces `UNKNOWN` first under §5's precedence because of the boolean limit above. No normalizer
changes that difference. `probe-4` is an expected failure only for the exact `UNKNOWN` merge fields.

### Draft state needs a title prefix in Forgejo 15

The GitHub twin's title is `scenario: draft`. Forgejo derives draft state from `WIP: scenario: draft`.
The API conversion and `IsWorkInProgress` source above confirm that dependency. Keeping both the exact
GitHub title and native Forgejo draft state is not possible through this API. The seed preserves
draft state; the shared REST and GraphQL title mapper strips the leading prefix for drafts, per
MAPPING.md v1 §1. `FGA_WIP_PREFIXES` configures the comma-separated prefixes, default `WIP:,[WIP]`.
Matching ignores case and strips following whitespace. Non-draft titles stay unchanged.

### Compare has no behind_by

Forgejo 15's Swagger `Compare` object contains `commits`, `files`, and `total_commits`, but no
`behind_by`. MAPPING.md §5's requested field is absent even though the endpoint exists.

The gateway uses `behind_by` if an upstream supplies it. On Forgejo 15 it reverses the comparison
and checks `total_commits`. The local scenario gives 1 for `main...s/pass` and 6 for `s/pass...main`
after the three closed merge scenarios land. These are commits since the merge base in each direction.
The reverse count correctly identifies base commits absent from the head. A missing compare endpoint
still skips the check, as the spec requires. The request memo avoids repeating either comparison.

### Status creation and status reads use different field names

The status POST body uses `state`. A returned `CommitStatus` uses `status`. The unedited response in
`fixtures/forgejo15-status.json` and Swagger confirm that name. The DTO maps `status` to its typed
internal state. This detail was not explicit in the mapping tables.

## Oracle differences and limits

`view-4` and `status` now pass with the WIP prefix removed. `probe-4` accepts only
`mergeable: MERGEABLE` → `UNKNOWN` and `mergeStateStatus: CLEAN` → `UNKNOWN`.
That case must still fail against the oracle. An exact match fails the gate so the expected failure
must be removed after review. Any title difference or unrelated difference also fails the gate.

`pr status -R` works in gh 2.103.3. Both hosts use that form. The capture therefore has no current-branch
section; it compares the author and review-request sections. PRs 3 and 9 appear in the live author's
open PR list, so the seed includes them. Closed PRs 8 and 10 keep the full snapshot's numbering and
states. No change to the GitHub twin was needed.

The schema's search support is limited to the author and review-request queries that `pr status`
sends. General PR search and list commands remain G2b work. Projects return an empty connection,
and `autoMergeRequest` returns null, as the brief requires. The twin has no reviews or protection.
Review policy, latest reviews, required contexts, and protected merge-state order have table tests;
mock-upstream tests also exercise required checks and both connection pages through GraphQL.

The read subset reports no review reactions, comment minimization, or created-edit history. These
fields have no matching data in the oracle. Requested users and teams map from the PR response. Forgejo has no distinct team slug, so the
mapping uses the native team name as the identifier. MAPPING.md does not yet define that choice.

## Normalization

The comparison replaces only hostnames, `/pulls/N` versus `/pull/N`, timestamps, relative times,
40-character SHAs, node IDs, check URLs, check durations, and email addresses. JSON key order has no
meaning, so the comparison sorts object keys. Array order remains significant. Login and display name need no
normalization because the local account matches `dirtydishes`. Commit author names also match.
Email addresses carry personal data and are not part of this PR mapping contract. The capture runner
scrubs every email address from stdout and stderr to `email@example.invalid` before writing files;
the seed snapshot uses the same scrubber. The comparison applies that same email normalizer to both
hosts. The committed fixtures have also passed through `scripts/oracle/scrub.py`. Local seed commits
and the `dirtydishes` admin account use `dirtydishes@users.noreply.invalid`.
The separate G1 admin retains `e2e@example.invalid`, because Forgejo rejects duplicate account emails.

The seed does not run actual workflows. It posts Actions-shaped statuses for the same head commits,
including stale failures, queued rows, and final results. No normalization changes names, conclusions,
workflow names, PR titles, draft flags, merge states, review decisions, check counts, or exit codes.

## Reproduction and ownership

`scripts/oracle/capture.sh` refreshes the GitHub fixtures with read-only gh commands and an exact
allowlist. It never prints or writes the GitHub token. It captures to an ignored temporary directory
and replaces the scrubbed fixtures after all expected exit codes pass. `scripts/e2e.sh` uses the
committed snapshot without host credentials. The script checks the allowlist and comparison failure rules
before Docker setup.

Runtime evidence lives under `.local/e2e/fga-g2a-<pid>/`, including `oracle/report.json`, the Forgejo
Swagger, the draft PR, both compare responses, and status history. Containers, the network, and the
TLS volume have `fga-` names and the runner removes only its own resources. Build cache IDs also use
that prefix. No changes to DESIGN.md or MAPPING.md belong to this implementation.
