# Client captures

The `gh` calls agent apps made, recorded by a transparent `gh` wrapper while each app ran against a
real checkout, then redacted with `scripts/capture/redact.py`. They show what each app sends and
what GitHub answered, so tests can hold the shim to the same calls.

| File | Client | What it shows |
|---|---|---|
| `chatgpt-22943-pr-card-s-pass.jsonl` | ChatGPT desktop build 22943 (Codex framework 156.0.8078.48), gh 2.103.3 | Opening the PR card and pane for PR 1 (`s/pass`) in a GitHub checkout |
| `chatgpt-22943-states.jsonl` | the same | Each PR state in turn (`mark`: fail, pending, draft, nochecks, merged, closed), then the Merge button (`pr merge 8`), the merge pane, a branch with no PR, and ChatGPT's agent opening PR 9 (`pr create`) |
| `chatgpt-22943-fj-probe.jsonl` | the same | A checkout whose origin is a Forgejo host, before the gateway existed. The wrapper answered some calls from `chatgpt-22943-fj-probe.stubs.json`; a record's `stubbed` is the stub's index |
| `chatgpt-22943-documents/` | the same | ChatGPT's GraphQL documents, one per file, with their flags in `.vars` (below) |
| `claude-code-cli-2.1.309-314.jsonl` | Claude Code CLI 2.1.309 to 2.1.314, in a terminal | A GitHub checkout, then a Forgejo checkout where the wrapper handed out a fake token (`faked`) |
| `claude-code-cli-forgejo-requests.log` | Claude Code CLI 2.1.314 | The Forgejo host's access log for that fake token: Claude's own REST lookups, refused with 401 |
| `claude-desktop-4.4212.jsonl` | Claude desktop 4.4212, running Claude Code 2.1.309 | The same flow from the desktop app: no token for the Forgejo host, then the fake one |
| `claude-desktop-4.4212-forgejo-requests.log` | the same | The Forgejo host's access log for it |

## Records

Each `.jsonl` line is one `gh` call:

| Field | Meaning |
|---|---|
| `argv` | The arguments after `gh`, as sent |
| `chain` | The parent processes, nearest first, which tell the clients apart |
| `cwd`, `env` | The working directory and the gh-related environment (`GH_*`, `NO_COLOR`, `TERM` and the like) |
| `env_secrets_present` | Token variables that were set, by name only |
| `stdin_tty`, `stdout_tty` | Whether those were terminals |
| `exit`, `duration_ms` | gh's exit code and run time |
| `stdout`, `stderr` | gh's output. `{"bytes": N, "redacted": true}` for output that can hold a token, never stored; `{"same_as": I}` for output equal to the record at 0-based line I |
| `t` | Seconds since the file's first call |
| `mark` | The step of the capture the call belongs to, when the capture marked its steps |
| `faked`, `stubbed` | The wrapper answered instead of gh: a fake token, or the stub at that index |

## Documents

ChatGPT sends each document as `gh api graphql --hostname <host> -f query=<document> <vars>`, the
host following the checkout's `origin`. In a `.vars` file, `-F` is a typed value (Int or Boolean)
and `-f` a string. A file ends with a newline the sent query may lack; the records hold the exact
argv.

| File | Used for |
|---|---|
| `body.graphql` | The PR description: `body`, `bodyHTML` |
| `reviews.graphql` | Review state: `reviewDecision`, `latestReviews`, `reviewRequests`, paged `reviews` |
| `permissions.graphql` | The merge button: `viewerPermission`, merge methods allowed, diff stats, `viewerCan*`, `autoMergeRequest` |
| `mergeability.graphql` | Head and base IDs, state, `isDraft`, `mergeable`, `mergeStateStatus` |
| `timeline.graphql` | The PR pane: summary, commits, comments, reviews and review threads, behind five `@include` flags |
| `batch-1.graphql`, `batch-8.graphql` | The PR card and checks: one `pN:` alias per PR, one PR while polling a thread and up to eight at startup. The records hold the counts between |

## Placeholders

| Placeholder | Stands for |
|---|---|
| `<user>` | The account that made the captures, which owns the checkout's repository |
| `<repo>` | That repository: the GitHub twin whose PR states `contract/scenarios.json` records |
| `forge.test` | The Forgejo host. `github.com` stays as sent |
| `<home>` | The home directory |
| `<ip>`, `<email>`, `<avatar-url>`, `<node-id>` | An IP address, an email, an avatar URL, a GraphQL or REST node ID |
| `<token>`, `<redacted>` | A token or credential, and any other hidden value: a profile field, a value under a sensitive key |
| `0000000000000000000000000000000000000001` | A commit ID, numbered in order of first appearance |
| `/actions/runs/1/job/1` | Actions run and job IDs, numbered the same way |
| `2000-01-01T00:00:00Z` | A timestamp. They keep their order, one second apart |
| `[+9s]` | An access log line's time, since the file's first line |

A user's numeric ID and follower counts are `0`. Screenshots taken during the captures aren't kept:
a script can't redact an image.

## Adding a capture

```sh
python3 scripts/capture/redact.py --out fixtures/clients \
    --name <your login>='<user>' --name <repository>='<repo>' --name <forge host>=forge.test \
    <capture files and their .marks>
python3 scripts/capture/leaks.py fixtures/clients
```

`redact.py` also finds logins in the answers, but only `--name` covers a repository or host it
can't recognize. `leaks.py`, which CI runs, fails on any token, IP address, email, home directory,
avatar URL or host other than GitHub's and reserved test names, and prints where without printing
the value. It can't recognize a name, a bio or other free text: read a capture before committing it.
