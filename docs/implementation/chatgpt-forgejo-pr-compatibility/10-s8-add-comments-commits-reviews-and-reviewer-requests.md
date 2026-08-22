# Phase 10: S8: Add comments, commits, reviews, and reviewer requests

Canonical Beads issue: `gh-forgejo-shim-c10`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

ChatGPT's PR activity view shows the complete discussion and review state.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S7.

## Scope

Allowed:

- Extend `src/pull_requests/view.rs`, `src/pull_requests/projection.rs`, and `src/forgejo.rs`. Add paginated reads for commits, issue comments, reviews, review comments, and requested reviewers.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert S8. Core view from S7 remains.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop if the fixture has drifted without a replacement or if Forgejo cannot support a claimed semantic. Do not fake review-thread capabilities.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c09`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- The exact 25-field build-6720 `pr view` request matches the golden shape. Islandflow PR 36, or a verified replacement, shows its body, commits, two comments, reviews, and requests in stable order.
- Golden 25-field test, ordering tests, and redacted live fixture record.

## Execution Boundary

- Acceptance boundary: ChatGPT's PR activity view shows the complete discussion and review state.
- Module surface: pull request activity pagination, ordering, and projection
- Review boundary: S8 integration PR diff, named tests, fixtures, command evidence, and review gate: Check pagination, chronological ordering, latest-review derivation, deleted users, empty arrays, and unsupported review-thread semantics.

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
- Golden 25-field test, ordering tests, and redacted live fixture record.

## Replanning Triggers

- Stop if the fixture has drifted without a replacement or if Forgejo cannot support a claimed semantic. Do not fake review-thread capabilities.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S8 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
