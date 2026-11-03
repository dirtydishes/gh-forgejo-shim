# forgejo-github-api: design

Status: draft v0, 2026-10-30. Owner: Kell. Written by Opus from a gateway spike (gh 2.103 against
a recording proxy) and live captures of the gh calls ChatGPT and Claude make, taken 2026-10-30.

## What it is

A small HTTP service that answers the **GitHub Enterprise Server API** (`/api/v3/*` REST and
`/api/graphql`) for a Forgejo instance, translating each request into Forgejo's `/api/v1`. It runs next
to Forgejo and is mounted on the **same hostname** by the reverse proxy. Forgejo doesn't use those two
path prefixes.

Clients then treat the forge as a GHE host:

- **Real `gh`:** `gh auth login --hostname git.example.com` with a Forgejo token. Every gh command
  inside the supported surface works, with gh's own output, flags, `--json`, `--jq` and errors.
- **Claude Code (CLI) and Claude desktop:** these call `gh auth token --hostname <remote host>`, then
  GitHub REST on that host directly (`GET /api/v3/repos/{o}/{r}`, `GET /api/v3/repos/{o}/{r}/pulls?head=…`).
  Both were captured on 2026-10-30. With gh logged in to the forge, they light up with no extra setup.
- **ChatGPT desktop (Codex):** it drives `gh` with fixed GraphQL documents, which the GraphQL surface
  answers.
- **Agents running `gh` in a shell:** they work for anything inside the supported surface, and get a
  GitHub-shaped error for anything outside it.

## Goals and non-goals

**Goals, in priority order:**

1. Claude Code and Claude desktop show PR status and checks for Forgejo repos. Their agents can run
   `gh pr view|checks|create|comment|merge`.
2. Real `gh` works for the T0–T2 surface (below).
3. ChatGPT's PR card works (its 6 read documents and `pr merge`).
4. Run as one container that an operator adds with a few reverse-proxy lines.

**Non-goals for now:**

- The GitHub web UI, Apps, webhooks, Actions APIs beyond read, Projects, Discussions, and org/team
  administration.
- The ChatGPT review pane mutations (review threads, pending reviews). These are deferred until there's
  capture evidence.
- Hosted forges where the operator can't add the route, such as Codeberg.

## Deployment shape

```
client ──https──▶ reverse proxy (git.example.com)
                    ├─ /api/v3/*      ─▶ forgejo-github-api ─▶ Forgejo /api/v1 (internal URL)
                    ├─ /api/graphql   ─▶ forgejo-github-api
                    └─ everything else ─▶ Forgejo (web, git over HTTP, /api/v1)
```

- **Stateless.** The client's `Authorization: token <forgejo PAT>` is forwarded upstream unchanged, and
  the gateway stores no credentials.
- **Configuration:** the upstream Forgejo base URL, the public base URL, the claimed GHES version, and
  timeouts.
- **Shipped as** a container image plus nginx, Caddy and NPM snippets.

## Architecture

- **Rust**, matching gh-forgejo-shim:
  - `axum` for HTTP;
  - `async-graphql` for the GraphQL server;
  - `reqwest` (rustls) for one shared Forgejo client with a per-request deadline and pagination
    helpers.
- **Workspace:** two crates.
  - `crates/fga-core`: a library holding the Forgejo client, `map/`, `ids`, and the GraphQL schema with
    its resolvers.
  - `crates/fga-server`: the axum binary.

  gh-forgejo-shim (the CLI fallback) is meant to depend on `fga-core`. That way the Forgejo → GitHub
  mapping is written once, and the shim can run the same GraphQL schema in-process for
  `gh api graphql` (ChatGPT's documents).
- **Modules:**
  - `rest/`: one handler per GitHub REST route.
  - `graphql/`: the schema subset (types, queries, mutations) plus resolvers.
  - `forgejo/`: the typed Forgejo client.
  - `map/`: field projection, Forgejo → GitHub, shared by REST and GraphQL.
  - `ids`: global node IDs.
  - `meta`: `/`, `/meta`, version and scopes headers.
- **GraphQL is a real server, not a document table.** gh sends schema introspection (`__type(name:…)`)
  to feature-detect fields, and every client ships its own documents. The schema implements a **subset
  of GitHub's public schema**, with the same type, field, enum and argument names and nullability. A
  query outside the subset gets GitHub-style validation errors. A test compares the implemented SDL with
  a pinned copy of GitHub's public schema (`octokit/graphql-schema`), so the subset can never drift from
  the real names.

## Surface v1 (from the gh 2.103.3 capture; see `docs/gh-2.103.3-contract-summary.txt`)

**REST:**

| Route | Used by | Notes |
|---|---|---|
| `GET /api/v3/` | gh auth login/status | Must return `X-OAuth-Scopes` (see Auth) |
| `GET /api/v3/meta` | gh feature detection | Must include `installed_version` (configured, default `3.19.0`); gh fails with `malformed version` without it |
| `GET /api/v3/rate_limit` | gh api rate_limit | A fixed full budget; Forgejo has no rate-limit API (gateway README) |
| `GET /api/v3/user` | gh, ChatGPT probes | |
| `GET /api/v3/repos/{o}/{r}` | Claude | |
| `GET /api/v3/repos/{o}/{r}/pulls?head={o}:{branch}&state=&per_page=` | Claude | Branch → PR lookup |
| `GET /api/v3/repos/{o}/{r}/pulls/{n}` | gh pr diff, pr status | With `Accept: application/vnd.github.v3.diff`, return the diff |
| `GET /api/v3/repos/{o}/{r}/commits/{sha}/pulls` | Codex CLI | |
| `GET /api/v3/repos/{o}/{r}/actions/runs/{id}/jobs`, `/artifacts` | gh run view | Forgejo 15's API has neither per run, so a run's `jobs_url` and `artifacts_url` point here and answer empty lists; 404 for an unknown run |
| `GET /api/v3/repos/{o}/{r}/commits/{ref}/status`, `/commits/{ref}/check-runs` | Claude desktop | A PR head's CI state; the check mapping in MAPPING §3 |
| `GET /api/v3/repos/{o}/{r}/actions/runs`, `/actions/workflows` | gh run list, Claude desktop (`head_sha`) | A repeated parameter takes its last value. A run Forgejo never started (its Unix-0 start time) reports `created_at` as `run_started_at`. Forgejo's `unknown` (a run with no jobs, which it counts as pending) is `queued`, like `waiting` and `blocked`, and `status=queued` asks Forgejo for all three |
| `DELETE /api/v3/repos/{o}/{r}/git/refs/heads/{branch}` | gh pr merge and pr close --delete-branch | gh escapes `heads/{branch}` as one segment (`heads%2F…`); the route takes the whole ref and serves only `heads/`. Reads the branch, then Forgejo's branch delete, with the caller's token (or the host's upstream token, where one is set). gh reads 404 and 422 as "already deleted", so only a branch known to be absent (Forgejo's branch read names it as missing, or its name has an empty part) answers GitHub's 422 `Reference does not exist`. A default or protected branch, no write access, or any other 404 (a repository the token can't see, a disabled Code unit) is `403`; a name the path guard won't send is `400`; anything but Forgejo's `204` is `502` |

**GraphQL operations gh 2.103.3 sends** (named and fixed per gh version):

- **Queries:** `UserCurrent`, `RepositoryInfo`, `PullRequestByNumber`, `PullRequestList`,
  `PullRequestSearch`, `PullRequestStatus`, `PullRequestStatusChecks`, `PullRequestForBranch`,
  `PullRequestProjectItems` (empty result), and `IssueList`.
- **Introspection:** `PullRequest_fields`, `PullRequest_fields2`, `Issue_fields` and
  `SearchType_enumValues`, which come for free from the server.
- **Mutations:** `PullRequestCreate`, `CommentCreate`, `PullRequestUpdate`,
  `ConvertPullRequestToDraft`, `PullRequestReadyForReview` and `PullRequestMerge`.
- **ChatGPT 22943's 6 documents** use fields on `PullRequest`, `Repository`, `Commit.statusCheckRollup`,
  reviews, review threads and `viewer*`.

## Auth

- **Forwarding:** the gateway passes `Authorization` through. A missing or invalid token produces
  GitHub's `401 {"message":"Bad credentials"}` shape.
- **Scopes:** gh checks `X-OAuth-Scopes` on `GET /api/v3/` and warns if `repo`/`read:org` are missing.
  Forgejo doesn't expose a token's scopes, so the gateway **claims `repo, read:org, workflow`** and lets
  Forgejo enforce the real permissions. The README tells users which Forgejo token scopes to grant:
  `read:user`, `write:repository`, `write:issue`, `read:organization`.

## Identity and IDs

- **Node IDs** are opaque `base64("fga:" + type + ":" + owner/repo + ":" + number-or-id)`. They're
  stable across restarts, and `node(id:)` decodes them without any storage.
- **Database IDs** are Forgejo's numeric IDs.
- **URLs:** API URLs use the public base (`https://host/api/v3/...`), and `html_url`/`url` fields use
  Forgejo's web URLs. Pull request URLs are the exception: they use GitHub's `/{o}/{r}/pull/{n}`, and the
  gateway redirects that path to Forgejo's `/pulls/{n}` (MAPPING.md §8).

## Semantic mapping (to be completed)

| GitHub | Forgejo source | Rule (draft) |
|---|---|---|
| `PullRequest.state` OPEN/CLOSED/MERGED | `state`, `merged` | merged ⇒ MERGED |
| `isDraft` | `draft` | Forgejo 15 has a `draft` field (WIP prefix) |
| `mergeable` MERGEABLE/CONFLICTING/UNKNOWN | `mergeable` bool | false ⇒ CONFLICTING; closed ⇒ UNKNOWN |
| `mergeStateStatus` | mergeable + check states + draft | CLEAN / DIRTY / BLOCKED / UNSTABLE / DRAFT / UNKNOWN (spec needed) |
| `reviewDecision` | reviews + branch protection required approvals | APPROVED / CHANGES_REQUESTED / REVIEW_REQUIRED / null |
| `statusCheckRollup.contexts` | `commits/{sha}/statuses` (+ Actions tasks for workflow and event) | Forgejo Actions reports commit statuses. Emit `CheckRun` when the status comes from Actions, else `StatusContext` (spec needed) |
| `autoMergeRequest` | `merge_when_checks_succeed` scheduled merge | `GET pulls/{n}/merge` 204/404 semantics, to verify |
| `PullRequestMerge(expectedHeadOid)` | `head_commit_id` | `--match-head-commit` |
| auto-merge enable/disable | `merge_when_checks_succeed` / `DELETE pulls/{n}/merge` | |

## Testing

1. **Contract cases from real GitHub.** The spike recorder captures gh's exact requests and GitHub's
   responses per command. These are committed as fixtures, and they define which fields must exist,
   not their values.
2. **Hermetic integration.** The gateway runs against a **Forgejo container** seeded by a scenario
   script (PR states, checks via the commit-status API). Real `gh` runs in the same test environment
   against the gateway, and assertions are on gh's stdout and exit code. This runs in GitHub Actions
   with Forgejo as a service container.
3. **Live acceptance**, run by hand: the twin repo `git.dirtydishes.dev/dirtydishes/gfj-oracle`, with
   Claude Code, Claude desktop and ChatGPT on a test Mac. The badges must match the GitHub twin's.

Route tests send the client's exact request, taken from the client's source or a capture, never a
shape written from memory. #112's branch-delete route matched no request gh sends (gh escapes
`heads/{branch}` as one segment), and its own tests used the remembered shape, so four review rounds
missed it; #118 fixed it after the launcher's twin cases sent real gh.

## Milestones

- **G1, thin slice:** `/`, `/meta`, `/user`, `/repos/{o}/{r}`, `/pulls?head=`, GraphQL `UserCurrent`,
  and `PullRequestByNumber` with core fields. Real gh `pr view N --json number,title,state,url,isDraft,headRefName,baseRefName`
  works against a Forgejo container, and the Claude Code footer shows the PR on a test Mac.
- **G2, gh read surface (T1):** list, search, status, checks with `statusCheckRollup`, diff, runs, and
  issue list, after A3.
- **G3, gh write surface (T2):** the 6 mutations.
- **G4, ChatGPT's documents.**
- **G5, packaging:** image, proxy snippets, README.
- **G6, acceptance on a test Mac.**
