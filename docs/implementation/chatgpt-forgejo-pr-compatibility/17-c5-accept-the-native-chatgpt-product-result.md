# Phase 17: C5: Accept the native ChatGPT product result

Canonical Beads issue: `gh-forgejo-shim-c17`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

Independent review and manual product checks confirm the native ChatGPT PR screen works for Forgejo and still works for GitHub.

## Why This Phase Exists

This accepted checkpoint keeps later slices from building on an unreviewed contract.

## Scope

Allowed:

- Run adversarial review and manual product review against the S12 commit, native UI, redacted trace, API truth, screenshots, and rollback evidence.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Remove the managed wrapper or reinstall the prior binary. Do not restart unrelated services or active tasks.

## Constraints

- Reviewers do not repair their own findings.
- Keep one external integration PR.
- Invalidate a review that consults the thermonuclear skill. Stop after the third repair pass on any product, security, compatibility, CI, or rollback failure.

## Settled Decisions

- Use the review roles and combined repair policy stated in the accepted plan.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c16`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- The adversarial reviewer reports no unresolved blocker or required finding and does not use the thermonuclear skill. Manual review confirms every S12 acceptance case against API truth.
- The exact S12 commit is green and the redacted desktop evidence is tied to that commit.

## Execution Boundary

- Acceptance boundary: Independent review and manual product checks confirm the native ChatGPT PR screen works for Forgejo and still works for GitHub.
- Module surface: S12 commit, native UI screenshots, redacted process trace, API truth, and rollback command evidence
- Review boundary: C5 integration PR diff, named tests, fixtures, command evidence, and review gate: Adversarial review and manual product review inspect the S12 commit, UI screenshots, redacted process trace, API fixtures, and rollback command evidence.

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
- The exact S12 commit is green and the redacted desktop evidence is tied to that commit.

## Replanning Triggers

- Invalidate a review that consults the thermonuclear skill. Stop after the third repair pass on any product, security, compatibility, CI, or rollback failure.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Group findings by distinct failure path before appointing one repair owner.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
