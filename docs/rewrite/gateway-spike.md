# Spike: can a GitHub-API gateway replace the gh CLI shim? (Option G)

Date: 2026-10-30. Opus. Inputs: [`rewrite-plan.md`](rewrite-plan.md) §7 and [`clients.md`](clients.md).
The recommendation below (a separate gateway repo) was later replaced by option B: real gh plus an
in-process gateway in this repo. See [`plan-review-2026-11-01.md`](plan-review-2026-11-01.md).

## Method

A small recorder poses as a GitHub Enterprise host (`forge.test`, self-signed TLS). It forwards
`/api/v3/*` (prefix stripped) and `/api/graphql` to api.github.com with the maintainer's token, rewrites
URLs back to `forge.test`, and logs every exchange. A driver script installs **real gh 2.103.3** in a
Debian container, logs gh in to `forge.test` as a GHE host with a dummy token, and runs T0–T2 plus a
few agent commands against `dirtydishes/gfj-oracle`. The write flow created, commented on, edited, drafted, readied and
merged PR #10.

The per-command summary of what gh sent is `docs/gateway/gh-2.103.3-contract-summary.txt`, which
arrives with the gateway move. The raw exchange records, with the account email and `/user` body
redacted, stayed with the spike and aren't in this repo.

## Results

1. **gh fully accepts a GHE-shaped host.** `auth login --hostname`, `auth status`, `api`, `repo view`,
   `pr list|view|checks|diff|status|create|comment|edit|ready|merge`, `issue list` and `run list` all
   work. Output is real gh output, because real gh produced it. The `pr checks` exit 1 is correct
   behavior: PR #2 fails and #5 has no checks.
2. **The one round-1 failure was GHES version detection.** `pr list --author @me` calls
   `GET /api/v3/meta` and parses `installed_version`, which github.com doesn't return
   (`malformed version:`). Reporting `installed_version: 3.19.0` fixed it. The gateway must claim a GHES
   version, and that version gates gh's feature detection.
3. **The surface gh needs for T0–T2 + issues + runs is small:**
   - **21 GraphQL operations**, all named and fixed per gh version:
     - `PullRequestByNumber` (used by nearly every pr command), `PullRequestList`, `PullRequestSearch`,
       `PullRequestStatus`, `PullRequestStatusChecks`, `PullRequestForBranch`,
       `PullRequestProjectItems`, `RepositoryInfo`, `UserCurrent`, `IssueList`;
     - introspection: `PullRequest_fields`, `PullRequest_fields2`, `Issue_fields`,
       `SearchType_enumValues`, which are `__type(name:…){fields}` queries;
     - mutations: `PullRequestCreate`, `CommentCreate`, `PullRequestUpdate`,
       `ConvertPullRequestToDraft`, `PullRequestReadyForReview`, `PullRequestMerge`.
   - **7 REST endpoints:**
     - `GET /api/v3/`, `/user` and `/meta`;
     - `GET /repos/{o}/{r}/pulls/{n}` (diff and status);
     - `GET /repos/{o}/{r}/commits/{sha}/pulls`;
     - `GET /repos/{o}/{r}/actions/runs` and `/actions/workflows`.
4. **Introspection means a real GraphQL server, not a document table.** gh asks the schema which fields
   exist before building queries. ChatGPT sends its own 6 documents, and Superset, Nimbalyst and Sculptor
   send others. A GraphQL server implementing a **subset of GitHub's schema**, with resolvers backed by
   Forgejo's `/api/v1`, answers all of them. Unknown fields get a standard GraphQL validation error. This
   also replaces the "match documents by normalized hash" idea for T1g.

## How this compares with Option S (gh CLI shim)

| | S: CLI shim | G: API gateway |
|---|---|---|
| ChatGPT PR card (6 GraphQL docs) | needs GraphQL anyway | yes (same schema subset) |
| Claude desktop and Claude Code (`/api/v3/repos…`, `…/pulls?head=`) | **impossible** | yes (2 more REST endpoints) |
| Agents running arbitrary gh commands | each command emulated by hand | anything inside the schema subset works |
| Output, `--json`, `--jq`, templates, error text | emulated, which caused most past bugs | free: real gh |
| PATH wrapper, LaunchAgent, GUI-PATH bug class | required | gone; plain `gh auth login --hostname <forge>` |
| Deployment | user's machine | next to Forgejo (a reverse-proxy route for `/api/v3` and `/api/graphql`) |
| Hosted Forgejo (Codeberg etc.) | works | not without operator cooperation |

Both options share the hard semantic work: field mappings, check rollups, review state and
mergeability (A3). G moves that work into resolvers and drops everything else that made S fragile.

## Open questions (next spike steps)

1. **Does ChatGPT target the forge host?** Its GraphQL calls pass `--hostname <host from the PR>`. Its
   `pr list --repo` takes the host from the remote. Test: point a ChatGPT project at the Forgejo twin
   with gh logged in to the gateway and watch the access log.
2. **Auth pass-through.** gh sends `Authorization: token <t>`. The gateway forwards it to Forgejo as
   `token <t>`. Forgejo PATs need `read:repository` / `write:repository` / `read:user` scopes, and gh
   checks the `X-OAuth-Scopes` header on `GET /api/v3/` (`auth status` warns about missing `repo`/
   `read:org`). The gateway should synthesize the scopes header from the Forgejo token's scopes.
3. **Git transport.** `pr checkout` and `pr create` need git push/fetch, which go to Forgejo's own git
   HTTP/SSH on the same host and are untouched. Expected fine, not yet tested.
4. **Node IDs.** Mutations take GraphQL node IDs (`PullRequestMerge(pullRequestId:)`). The gateway must
   mint stable opaque IDs that encode `(type, owner, repo, number/id)`.
5. **Language and framework.** Rust with `async-graphql` matches the existing repo. Go would allow
   reusing GitHub's public schema SDL through gqlgen. Decide in the design doc.

## Recommendation

Build G as its own repo, as Kell suggested: a "GitHub API for Forgejo" gateway, aimed first at Claude
users (desktop and CLI) and gh-driven agents, with ChatGPT's PR card as the second target. Keep the CLI
shim only as a fallback for hosted forges, or retire it. Revisit after the next spike step (open
question 1) and a minimal end-to-end slice: `GET /api/v3/repos/{o}/{r}` and `pulls?head=` against the
real Forgejo twin, plus `PullRequestByNumber`, with real gh and Claude Code pointed at it.
