# Phase 04: C1: Pass the host identity and routing checkpoint

Canonical Beads issue: `gh-forgejo-shim-c04`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

The host registry, canonical identity, and provider-first routing pattern can support later read behavior without unsafe GitHub fallthrough.

## Why This Phase Exists

This accepted checkpoint keeps later slices from building on an unreviewed contract.

## Scope

Allowed:

- Review the S2 and S3 integration PR diff, route matrix, host configuration contract, and module boundaries. Apply at most three combined repair passes.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert the checkpoint repair commits. S2 and S3 retain their own rollback boundary.

## Constraints

- Reviewers do not repair their own findings.
- Keep one external integration PR.
- Stop after the third combined repair pass if a blocker, required finding, red test, or red CI result remains.

## Settled Decisions

- Use the review roles and combined repair policy stated in the accepted plan.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c03`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- Thermonuclear and adversarial reviewers report no unresolved blocker or required finding. Each accepted repair passes the affected tests and the full local gate.
- The exact checkpoint commit has a green rust job and the route matrix passes.

## Execution Boundary

- Acceptance boundary: The host registry, canonical identity, and provider-first routing pattern can support later read behavior without unsafe GitHub fallthrough.
- Module surface: S2 and S3 integration PR diff, route matrix tests, HostRegistry interface, and routing contract
- Review boundary: C1 integration PR diff, named tests, fixtures, command evidence, and review gate: Thermonuclear review covers module depth and maintainability. Adversarial review covers host isolation, unsafe fallthrough, compatibility, and failure behavior.

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
- The exact checkpoint commit has a green rust job and the route matrix passes.

## Replanning Triggers

- Stop after the third combined repair pass if a blocker, required finding, red test, or red CI result remains.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Group findings by distinct failure path before appointing one repair owner.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
