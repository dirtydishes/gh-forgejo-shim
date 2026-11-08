# gh 2.103.3's documents with every field

The GraphQL documents gh 2.103.3 sends when asked for every `--json` field it offers, captured with
`GH_DEBUG=api` against a closed local port (gh prints each document before the request fails):

| File | Command |
|---|---|
| `pr-view-all.graphql` | `gh pr view 1 --json <every pr view field>` |
| `pr-list-all.graphql` | `gh pr list --limit 1000 --json <every pr list field>` |
| `issue-view-all.graphql` | `gh issue view 1 --json <every issue view field>` |
| `issue-list-all.graphql` | `gh issue list --limit 1000 --json <every issue list field>` |
| `repo-view-all.graphql` | `gh repo view --json <every repo view field>` |
| `pr-view-comments.graphql` | `gh pr view 1 --comments` |
| `pr-checks.graphql` | `gh pr checks 1` |

They are the largest documents gh builds for these commands. No query limit may refuse any of them:
see `crates/fga-core/src/graphql/tests.rs`. `repo-view-all.graphql` fails validation anyway, as it
asks for a field the gateway doesn't serve (`archivedAt`).
