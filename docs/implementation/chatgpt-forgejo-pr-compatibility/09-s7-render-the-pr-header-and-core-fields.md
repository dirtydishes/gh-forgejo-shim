# Phase 09: S7: Render the PR header and core fields

Canonical Beads issue: `gh-forgejo-shim-c09`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

`gh pr view` renders title, body, branches, author, merge state, timestamps, counts, and links with GitHub-compatible shapes.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S6 and C2.

## Scope

Allowed:

- Add `src/pull_requests/view.rs` and `src/pull_requests/projection.rs`. Move only the required projection logic from `src/normalize.rs`. Track whether projected values are exact, derived, or unavailable inside the module.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert S7. S6 discovery remains usable.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop if a field's source is unknown or its empty shape differs from real `gh`.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c08`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- A core-field request matches the GitHub 2.96 fixture for nulls, strings, objects, arrays, field filtering, and ordering. No unsupported value is invented.
- Golden core-view projection and a redacted live Islandflow fixture.

## Execution Boundary

- Acceptance boundary: `gh pr view` renders title, body, branches, author, merge state, timestamps, counts, and links with GitHub-compatible shapes.
- Module surface: pull request core-field source and projection modules
- Review boundary: S7 integration PR diff, named tests, fixtures, command evidence, and review gate: Compare every implemented field with its Forgejo source and the GitHub fixture.

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
- Golden core-view projection and a redacted live Islandflow fixture.

## Replanning Triggers

- Stop if a field's source is unknown or its empty shape differs from real `gh`.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S7 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
