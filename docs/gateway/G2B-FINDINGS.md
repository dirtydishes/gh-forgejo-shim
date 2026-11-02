# G2b read findings

## Search semantics

The capture stores gh 2.103.3's request documents and variables in
`fixtures/oracle/*.requests.json`. The capture keeps no debug headers or response bodies.
Search filters use `SearchType: ISSUE_ADVANCED` after introspection. The gateway supports ISSUE
and ISSUE_ADVANCED for this subset and rejects other search modes.

`pr list --author @me --head s/pass --state all` sends
`author:@me head:s/pass repo:dirtydishes/gfj-oracle type:pr`.
The gateway resolves `@me` against the authenticated viewer. It sends `created_by`, `assigned_by`,
`labels`, `q`, `state`, and `type=pulls|issues` to the repository issues endpoint. PR author searches
also send `poster` to the pulls endpoint. Head, base, merged state, and requested reviewers need
local checks. All upstream pages precede GraphQL filtering and pagination.

Forgejo 15's Swagger says that issue `labels` matches **any** listed label and discards unknown names.
GitHub requires each label qualifier. The gateway checks every returned label name, including unknown
names, before pagination. A separate disposable repository tests positive author, assignee, and
quoted-label matches, missing labels, multiple labels, and `in:title` against a body-only match.
Unknown qualifiers return a GraphQL UNPROCESSABLE error, with GitHub's `message`, `type`, and `path`.

GitHub distinguishes list states from search states. The captured direct `--state closed` list returns
only PR 7. The author-filtered list sends
`author:dirtydishes repo:dirtydishes/gfj-oracle state:closed type:pr` and returns 10, 8, 7, 6,
including merged PRs. The gateway preserves this difference.

## Relevance order has no shared definition

GitHub's `( scenario in:title ) repo:dirtydishes/gfj-oracle type:pr` returns
4, 5, 3, 2, 1, 9, 7, 6, 8, 10. Forgejo's native `sort=relevance` returns
7, 6, 5, 4, 3, 2, 1, 8, 9, 10. The issue search returns 11, 12 on GitHub and 12, 11 on Forgejo.
Both searches return the same complete records. MAPPING.md does not define a cross-forge relevance
score. The gateway uses Forgejo's native score rather than inventing an approximation to GitHub's.

`list-search` and `issues-search` are strict expected failures for order alone. Their comparison
requires equal multisets of complete JSON records. Missing, duplicate, changed, or extra records,
changed exit codes, and stderr still fail. An exact order match also fails until the expected failure
is removed. No normalizer reorders arrays. The additional `sort:created-desc` PR search passes exactly.
These two expected failures extend the brief's explicit allowance for run differences.

## Repository and issue reads

Default `repo view` reads the GraphQL description and then REST `/readme` with a raw Accept header.
The seed uses the captured README and repository visibility and description. PR 1 uses the captured
diff as its seed commit, so both `pr diff 1` and `--name-only` compare actual file contents and paths.
Both forms read the same REST pull resource with a diff Accept header.

Issues share PR numbering. The seed creates issues 11 and 12 after PRs 1–10 and closes issue 12.
The read subset maps their author, body, dates, labels, assignees, milestone, and comments.
Forgejo does not expose GitHub's issue state reason in its Issue response, so `stateReason` is null.
Issue types, parents, sub-issues, and dependencies currently return null or empty results. The twin has
none. Projects retain G2a's empty result. These fields do not claim general Forgejo dependency support.

The pinned octokit SDL lacks `Issue.issueType`, `blockedBy`, `blocking`, and the current search enum
values even though real gh requests them. A fixed read-only `__type` capture supplies their independent
names, argument defaults, and nullability. `pin-schema.py` merges that supplement into the reference
before selecting implemented fields. The pinned fixture records both source hashes. No schema types
or nullability come from guessing the client output.

## Actions reads and limits

Forgejo 15 has `/actions/runs` and `/actions/runs/{id}`, but no workflow-list REST endpoint.
Its ActionRun uses a filename for `workflow_id`, a native status, and `created`, `updated`, `started`
timestamps. The gateway reads YAML workflow names from `.forgejo/workflows`, with `.github/workflows`
as the fallback. A workflow's stable numeric ID is the first eight SHA-256 bytes of its filename with
the sign bit cleared. This choice needs a rule in MAPPING.md. It is not a GitHub database ID.

Waiting and blocked runs map to queued; running maps to in_progress; success, failure, cancelled,
and skipped map to completed with the same conclusion. Unknown states produce an upstream error.
The gateway preserves run IDs and metadata and paginates Forgejo pages into the requested GitHub page.
Status sets use repeated query parameters, such as `status=waiting&status=blocked`. Forgejo returns
HTTP 400 for a comma-separated set. Workflow lookup supports the extra read that `run view --json` sends.

The oracle's five completed runs have no counterparts in the status-only twin seed. `runs` is an
expected failure only when stdout is empty, the oracle has five completed successful ci rows, and
exit code and stderr still match. No new run normalizers hide names, results, IDs, or durations.
A separate repository creates real queued Forgejo Actions runs and checks them through real gh,
including branch/event/status filters and `run view` with mapped JSON fields. No runner executes jobs.
Default `run view` needs jobs; that route returns a GitHub-shaped 404. Jobs, logs, annotations, rerun
attempts, and workflows removed from the current default branch remain outside this subset.

## Commit lookup and seed timing

Forgejo's route is singular, `commits/{sha}/pull`. It returns 404 for the open PR 1 head in this seed.
The GitHub route is plural. The gateway combines the singular result with PRs whose head SHA matches,
then validates commit existence when neither source returns a PR. This handles the Codex status probe
and multiple PRs sharing a head. It does not discover every open PR containing an older interior commit.

The seed logs merge attempts and retries only HTTP 405 with bounded backoff for at most 30 seconds,
after its mergeability wait. Other errors still fail immediately. The successful live runs needed
one attempt for each merge. Fake-clock tests also prove two transient 405 retries, the 30-second
ceiling for persistent 405 responses, and immediate failure for other HTTP errors.

DESIGN.md and MAPPING.md remain unchanged.
