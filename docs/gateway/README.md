# forgejo-github-api

A stateless gateway that serves GitHub Enterprise API routes for Forgejo. It forwards each client's
Forgejo token to the upstream server and maps Forgejo responses to GitHub's JSON and GraphQL types.

The gateway supports `gh auth login`, `gh api user`, `gh pr view` with wide JSON field sets,
`gh pr checks`, and `gh pr status` with gh 2.103.3. It also serves the repository and branch-to-PR
REST lookups Claude Code uses. [G2a findings](docs/G2A-FINDINGS.md) records the draft PR differences
and Forgejo API limits. See [the design](docs/DESIGN.md) for later milestones.

Run with Rust 1.97.1:

```sh
FGA_FORGEJO_URL=http://127.0.0.1:3000 \
FGA_PUBLIC_URL=https://forge.example.com \
cargo run --locked -p fga-server
```

Mount `/api/v3/*` and `/api/graphql` on the Forgejo hostname through your reverse proxy. The gateway
listens on `127.0.0.1:8080` by default. Set `FGA_LISTEN` to change it. `FGA_GHES_VERSION` defaults
to `3.19.0`. For direct HTTPS, set both `FGA_TLS_CERT` and `FGA_TLS_KEY` to PEM file paths.
`FGA_WIP_PREFIXES` sets the comma-separated draft title prefixes, default `WIP:,[WIP]`.
REST and GraphQL strip the first matching prefix, ignoring case, and any following whitespace
when the PR is a draft. An empty list disables stripping. Draft mergeability stays `UNKNOWN`
because Forgejo's boolean cannot prove whether a draft has conflicts.
The upstream and public URLs must use HTTP or HTTPS and contain no credentials, query or fragment.
Upstream requests do not follow redirects. Each authenticated request shares a ten-second deadline
across its upstream reads, including the mergeability retry.

Use a Forgejo token with `read:user`, `write:repository`, `write:issue`, and `read:organization`.
The gateway accepts `token` and `bearer` authorization schemes. It advertises the GitHub scopes
`repo, read:org, workflow` for gh login; Forgejo enforces the token's actual permissions.
A bounded cache holds up to 256 viewer identities for 60 seconds, keyed by SHA-256 of the token.
It stores no plaintext tokens. A revoked token can retain its cached identity until expiry, but
Forgejo still checks each data request. Credentials and memoized data remain inside one incoming
request. Concurrent GraphQL fields share each upstream response, including errors.

The GraphQL subset covers gh's PR read documents, introspection, PR node lookup, checks, reviews,
review requests, and empty project connections. `search` supports the repo-scoped author and
review-request queries from `pr status`. Other search qualifiers return an error. Mutations remain
outside this version. The [trimmed GitHub SDL](fixtures/github-g1.graphql) pins the source commit
and checksum. Its contract test checks type kinds, fields, nullability, arguments, defaults, enums,
interfaces, and unions. [The schema example](crates/fga-core/examples/schema.rs) emits the implemented
SDL, and [pin-schema.py](scripts/oracle/pin-schema.py) trims the independent GitHub reference to it.
The full schema download remains ignored under `.local/`.

The pull-list route filters `head=owner:branch` after reading Forgejo's pages, then applies GitHub
`page` and `per_page` pagination and emits `Link` headers. It compares owner names without case and
branch names with case. This first version reads all upstream pages per lookup, which costs more
requests on repositories with many PRs. It emits Forgejo's real web URLs, including `/pulls/N`.
Merged PRs have GraphQL state `MERGED`, REST state `closed`, and their upstream `merged_at`.

Run the gates from this checkout:

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
CI runs the same gates. The oracle comparison checks exit code, stdout, and stderr for 14 cases.
It fails on new differences and on an expected failure that starts passing.

To refresh the committed GitHub oracle, run `scripts/oracle/capture.sh` with a host gh login and
exactly gh 2.103.3. The script reads only `github.com/dirtydishes/gfj-oracle`. It obtains the token
from `gh auth token`, keeps it in the process environment, and accepts no command arguments.
The runner checks a read-only allowlist and verifies the expected exit codes before the script
replaces fixtures. It scrubs email addresses from stdout, stderr, and the seed snapshot before
writing them. The comparison uses the same email placeholder for both hosts. This capture is
separate from e2e, which needs no host GitHub credentials.
[The findings note](docs/G2A-FINDINGS.md) lists the normalizers and every expected difference.

Related: [gh-forgejo-shim](https://github.com/dirtydishes/gh-forgejo-shim), a CLI alternative for
hosts where you cannot add gateway routes.
