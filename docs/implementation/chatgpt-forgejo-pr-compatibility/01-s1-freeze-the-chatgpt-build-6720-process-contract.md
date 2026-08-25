# Phase 01: S1: Freeze the ChatGPT build-6720 process contract

Canonical Beads issue: `gh-forgejo-shim-c01`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

A maintainer can run one suite and see the exact process behavior ChatGPT expects, including native GitHub delegation.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S0.

## Scope

Allowed:

- Add `tests/chatgpt_build_6720.rs`, `tests/fixtures/chatgpt/build-6720/`, and narrow fake-server support under `tests/support/`. Store exact argv, exit status, stdout shape, stderr shape, empty values, and the GitHub 2.96 control.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert the S1 test and fixture commits.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop if a fresh desktop trace differs from the recorded command contract. Replan against the new trace before implementation.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c00`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- The GitHub control passes. Forgejo cases fail only at the known gaps: alias identity, `@me`, missing fields, and checks.
- Green `cargo test --test chatgpt_build_6720` attached to the red fixture commit and later green commits.

## Execution Boundary

- Acceptance boundary: A maintainer can run one suite and see the exact process behavior ChatGPT expects, including native GitHub delegation.
- Module surface: build-6720 process fixtures, fake-server support, and the real GitHub delegation control
- Review boundary: S1 integration PR diff, named tests, fixtures, command evidence, and review gate: Compare every fixture with the report's captured command and GitHub control. Fixtures describe external behavior, not Rust implementation details.

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
- Green `cargo test --test chatgpt_build_6720` attached to the red fixture commit and later green commits.

## Replanning Triggers

- Stop if a fresh desktop trace differs from the recorded command contract. Replan against the new trace before implementation.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S1 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
