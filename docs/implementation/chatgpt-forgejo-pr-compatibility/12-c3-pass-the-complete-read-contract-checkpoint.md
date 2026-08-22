# Phase 12: C3: Pass the complete read-contract checkpoint

Canonical Beads issue: `gh-forgejo-shim-c12`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

The full read contract returns current, truthful PR and check data without leaking Forgejo work to GitHub.

## Why This Phase Exists

This accepted checkpoint keeps later slices from building on an unreviewed contract.

## Scope

Allowed:

- Run adversarial review on the S7 through S9 integration PR diff, golden projections, current-check logic, timeout behavior, and GitHub delegation. Apply at most three combined repair passes.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert the checkpoint repair commits. S7 through S9 retain their own rollback boundaries.

## Constraints

- Reviewers do not repair their own findings.
- Keep one external integration PR.
- Invalidate a review that consults the thermonuclear skill. Stop after the third repair pass if a blocker, required finding, red test, or red CI result remains.

## Settled Decisions

- Use the review roles and combined repair policy stated in the accepted plan.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c11`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- The adversarial reviewer reports no unresolved blocker or required finding and does not use the thermonuclear skill. Golden view, checks, no-check, and stale-history tests pass.
- The exact checkpoint commit has a green rust job and the full build-6720 read suite passes.

## Execution Boundary

- Acceptance boundary: The full read contract returns current, truthful PR and check data without leaking Forgejo work to GitHub.
- Module surface: S7 through S9 integration PR diff, golden projection fixtures, checks tests, and process-output contract
- Review boundary: C3 integration PR diff, named tests, fixtures, command evidence, and review gate: Adversarial review attacks the complete read-contract diff, stale check handling, timeouts, failure output, and GitHub fallthrough.

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
- The exact checkpoint commit has a green rust job and the full build-6720 read suite passes.

## Replanning Triggers

- Invalidate a review that consults the thermonuclear skill. Stop after the third repair pass if a blocker, required finding, red test, or red CI result remains.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Group findings by distinct failure path before appointing one repair owner.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
