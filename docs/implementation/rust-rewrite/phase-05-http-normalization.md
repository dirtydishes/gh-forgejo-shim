# Phase 05: Port Forgejo HTTP And Normalization Core

- Bead: `gh-forgejo-shim-98v`
- Parent epic: `gh-forgejo-shim-d60`
- Status: open
- Priority: P2
- Depends on: phase 02
- Blocks: phases 06 and 08
- Parallel work: phases 03 and 04

## Plain English Goal

Move the Forgejo API client and GitHub-shaped output conversion to Rust. This is the data layer used by almost every routed command.

## Scope

- Port the blocking Forgejo HTTP client.
- Preserve headers, token behavior, user agent, request paths, query params, and error mapping.
- Port response models for repos, pulls, issues, labels, statuses, users, files, and comments.
- Port GitHub-shaped normalization for:
  - pull requests
  - repositories
  - issues
  - status check rollups
  - `gh pr checks`
- Port JSON field filtering.
- Port the small supported subset of `--jq` and `--template` behavior.

## Subagents

This phase is a good place for parallel workers.

Recommended split:

- Worker A owns the Forgejo HTTP client and fake-server tests.
- Worker B owns PR/repo/issue/check normalization.
- Worker C owns field filtering plus the tiny `--jq` and `--template` compatibility layer.

The workers must agree on shared response/value types before editing. If those types are not stable, pause parallel work and let one integrator settle the interface first.

## Deliverables

- Rust Forgejo client with fake HTTP server tests.
- Rust normalization fixtures that match Python golden outputs.
- Clear module interfaces for command phases to call.

## Acceptance Criteria

- Normalized JSON matches phase 01 fixtures.
- HTTP failures produce established error text and exit behavior.
- The implementation does not require Python for API calls or normalization.

## Risks

- Rust HTTP/TLS errors may not look like Python `urllib` errors.
- Over-modeling Forgejo responses can make the client brittle. Prefer tolerant deserialization where Forgejo may omit fields.
