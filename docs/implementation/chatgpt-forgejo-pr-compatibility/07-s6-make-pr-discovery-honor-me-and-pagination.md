# Phase 07: S6: Make PR discovery honor `@me` and pagination

Canonical Beads issue: `gh-forgejo-shim-c07`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

ChatGPT can truthfully decide whether the current branch has a pull request.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S4 and S5.

## Scope

Allowed:

- Add `src/pull_requests/mod.rs` and `src/pull_requests/list.rs`. Update `src/read_only.rs`, `src/forgejo.rs`, `src/normalize.rs`, and `src/lib.rs`. Establish a private `PullRequestSource` seam with real and fake adapters.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert S6. The earlier routing and auth repairs remain.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop if the command returns partial data after timeout or cap, compares display names, ignores `--author`, or C2 fails after three repair passes.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c05`, `gh-forgejo-shim-c06`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- Resolve `@me` through `GET /user`, filter by stable login and head branch, handle `state=all`, paginate 50 entries at a time, and return at most the real `gh` default of 30 unless `--limit` changes it. Dirtypages `main` returns an authoritative empty list.
- Contract tests cover a match on page 2, no match after the terminal page, explicit limits, more than 1,000 candidates, and a redacted live Dirtypages replay.

## Execution Boundary

- Acceptance boundary: ChatGPT can truthfully decide whether the current branch has a pull request.
- Module surface: pull request discovery, author identity, pagination, and list projection
- Review boundary: S6 integration PR diff, named tests, fixtures, command evidence, and review gate: C2 requires thermonuclear and adversarial review, followed by the combined repair process. Review author semantics, terminal-page detection, the page cap, shared deadline, and empty-result truthfulness.

## Quality Gates

- Red behavior evidence precedes each green implementation in source-changing slices.
- The narrow acceptance tests pass.
- `cargo fmt --all -- --check` passes.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` passes.
- `cargo test --workspace --locked` passes.
- The GitHub Actions `rust` job is green for the exact commit.
- `release package smoke` is green when packaging changes.
- Required review findings are resolved within the three-pass limit.
- The branch is pushed and remote parity is proved before the slice closes.
- Contract tests cover a match on page 2, no match after the terminal page, explicit limits, more than 1,000 candidates, and a redacted live Dirtypages replay.

## Replanning Triggers

- Stop if the command returns partial data after timeout or cap, compares display names, ignores `--author`, or C2 fails after three repair passes.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S6 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
