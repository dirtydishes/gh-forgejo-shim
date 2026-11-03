# Gateway: fga-server and fga-core

A stateless gateway that serves GitHub Enterprise API routes for Forgejo. It forwards each client's
Forgejo token to the upstream server and maps Forgejo responses to GitHub's JSON and GraphQL types.
It was built as the separate forgejo-github-api repository and moved here with its history.

The gateway supports `gh auth login`, `gh api user`, `gh pr view` with wide JSON field sets,
`gh pr checks`, and `gh pr status` with gh 2.103.3. It also serves the repository and branch-to-PR
REST lookups Claude Code uses. [G2a findings](G2A-FINDINGS.md) records the draft PR differences
and Forgejo API limits. See [the design](DESIGN.md) for later milestones.

Run with Rust 1.97.1:

```sh
FGA_FORGEJO_URL=http://127.0.0.1:3000 \
FGA_PUBLIC_URL=https://forge.example.com \
cargo run --locked -p fga-server
```

Mount `/api/v3/*` and `/api/graphql` on the Forgejo hostname through your reverse proxy. The gateway
listens on `127.0.0.1:8080` by default. Set `FGA_LISTEN` to change it.

The gateway answers only requests addressed to `FGA_PUBLIC_URL`'s host and port, so the proxy must pass
the client's `Host` through. Caddy and Traefik do by default; nginx needs
`proxy_set_header Host $http_host`, because `$host` drops the port. Any other host, or a request without
one, gets `421 Misdirected Request` before its token is used. gh can send every host's requests to one
gateway, each with that host's token, and this check keeps a GitHub or second forge's token from
reaching Forgejo. `X-Forwarded-Host` is ignored.

Embedded in a launcher, the router serves several forges at once (`fga_server::Host`), each with its
own upstream, public URL and, optionally, the one token it accepts: any other credential for that
host gets 401 without reaching Forgejo. With `.upstream_token(token)`, a host
sends that token to Forgejo in place of the admitted one: a launcher gives gh a random per-call token
for a forge on a custom port and keeps the real one for git. A host can't substitute a token without
expecting one. A host can also answer to a second name
(`Host::answering_to`): gh drops a remote's port, so a forge on `:3443` is asked for as plain
`forge.test`, or with the port when the user writes it, while its links keep the port. The gateway libraries never print to stdout or stderr; a panic still reaches stderr through the
panic hook, so an embedding launcher installs its own.

`FGA_GHES_VERSION` defaults to `3.19.0`. For direct HTTPS, set both `FGA_TLS_CERT` and
`FGA_TLS_KEY` to PEM file paths.
`FGA_WIP_PREFIXES` sets the comma-separated draft title prefixes, default `WIP:,[WIP]`.
REST and GraphQL strip the first matching prefix, ignoring case, and any following whitespace
when the PR is a draft. An empty list disables stripping. Set it to the forge's own
`[repository.pull-request] WORK_IN_PROGRESS_PREFIXES`, which no API exposes. When they differ,
`gh pr ready` and `gh pr ready --undo` fail rather than report a change Forgejo didn't make.
`gh pr ready` fails before writing anything. `gh pr ready --undo` has already retitled the PR when
Forgejo shows it isn't a draft, and the error names the title the PR now has. The gateway doesn't
put the old title back, because Forgejo's edit has no conditional write and could overwrite someone
else's edit. Draft mergeability stays `UNKNOWN`
because Forgejo's boolean cannot prove whether a draft has conflicts.
The upstream and public URLs must use HTTP or HTTPS and contain no credentials, query or fragment.
Upstream requests do not follow redirects. Each authenticated request shares a ten-second deadline
across its upstream reads, including the mergeability retry.
The gateway reads at most 16 MiB of each upstream reply and 64 MiB across one incoming request's reads. Past
either, the request fails with `502` (REST) or an `INTERNAL` GraphQL error saying so, which is what
a very large `gh pr diff` gets.

Use a Forgejo token with `read:user`, `write:repository`, `write:issue`, and `read:organization`.
The gateway accepts `token` and `bearer` authorization schemes. It advertises the GitHub scopes
`repo, read:org, workflow` for gh login; Forgejo enforces the token's actual permissions.
A bounded cache holds up to 256 viewer identities for 60 seconds, keyed by SHA-256 of the token.
It stores no plaintext tokens. A revoked token can retain its cached identity until expiry, but
Forgejo still checks each data request. Credentials and memoized data remain inside one incoming
request. Concurrent GraphQL fields share each upstream response, including errors.

The GraphQL subset covers gh's PR read documents, introspection, PR node lookup, checks, reviews,
review requests, an issue's pin (`isPinned`) and the open or merged PRs that close it on merge
(`closedByPullRequestsReferences`: each PR's newest `closes` or `reopens` reference on the issue's
timeline, as Forgejo applies it), and empty project connections: `projectItems`, and `projectCards`
for classic projects, which Forgejo lacks. Its mutations cover `gh pr create`, `comment`, `edit`,
`close`, `reopen`, `ready` (both ways) and `merge`, including auto-merge, `gh issue comment`, and a
non-interactive `gh issue create` with a title and body ([DESIGN.md](DESIGN.md) lists the flags it
refuses).
`search` supports the repo-scoped author and review-request queries from `pr status`. Other search
qualifiers return an error. The [trimmed GitHub SDL](../../fixtures/github-g1.graphql) pins the
source commit and checksum. Its contract test checks type kinds, fields, nullability, arguments,
defaults, enums, interfaces, and unions. [The schema
example](../../crates/fga-core/examples/schema.rs) emits the implemented SDL, and
[pin-schema.py](../../scripts/oracle/pin-schema.py) trims the independent GitHub reference to it.
The full schema download remains ignored under `.local/`.

The pull-list route filters `head=owner:branch` after reading Forgejo's pages, then applies GitHub
`page` and `per_page` pagination and emits `Link` headers. It compares owner names without case and
branch names with case. This first version reads all upstream pages per lookup, which costs more
requests on repositories with many PRs. PR web URLs, and URLs under a PR, use GitHub's
`/pull/N` layout ([MAPPING §8](MAPPING.md#8-web-urls-v11)), which ChatGPT expects.
Forgejo has no `/pull/N` route, so links open only where the forge redirects them: server mode does
([server-mode.md](server-mode.md)).
Merged PRs have GraphQL state `MERGED`, REST state `closed`, and their upstream `merged_at`.
`GET /api/v3/rate_limit` answers GitHub's shape with fixed numbers, because Forgejo has no
rate-limit API and the gateway sets no limits of its own. It reports GitHub's per-user ceilings with
nothing used: 5000 an hour for `core` and `graphql`, and 30 and 10 a minute for `search` and
`code_search`, each with a reset at the end of its window. `rate` repeats `core`. A script that
paces itself by `remaining` never waits.

Run the gates from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
scripts/e2e.sh
```

The e2e script requires Docker, starts Forgejo 15 with SQLite, and runs the G1 assertions with v1's draft title.
It then seeds PRs 1 through 10 from the committed GitHub snapshot in a second private repository,
including passing, failing, draft, unchecked, merged, and closed PRs. It builds a test-only image with the
checksum-verified official gh 2.103.3 Linux release for the container architecture. A self-signed
certificate lets gh use the gateway as `https://forge.test`. No host gh config or credentials enter
the containers. Results and gh's query capture go under `.local/e2e/`; the script removes its
containers, network, and certificate volume on exit. Every test resource name starts with `fga-`.
CI runs the same gates. The oracle comparison checks exit code, stdout, and stderr for 42 read
cases and 14 write cases.
It fails on new differences and on an expected failure that starts passing.

To refresh the committed GitHub oracle, run `scripts/oracle/capture.sh` with a host gh login and
exactly gh 2.103.3. The script reads only `github.com/dirtydishes/gfj-oracle`. It obtains the token
from `gh auth token`, keeps it in the process environment, and accepts no command arguments.
The runner checks a read-only allowlist and verifies the expected exit codes before the script
replaces fixtures. It scrubs email addresses from stdout, stderr, and the seed snapshot before
writing them. The comparison uses the same email placeholder for both hosts. This capture is
separate from e2e, which needs no host GitHub credentials.
[The findings note](G2A-FINDINGS.md) lists the normalizers and every expected difference.

The shim in this repository (`crates/gh-forgejo-shim`) serves Forgejo hosts where you can't add
gateway routes. [The rewrite plan](../rewrite/plan-review-2026-11-01.md) makes this gateway its core.
