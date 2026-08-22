# Phase 08: C2: Pass the PR discovery checkpoint

Canonical Beads issue: `gh-forgejo-shim-c08`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

The pull request module, test seam, shared deadline, and pagination rules are sound before view projections grow around them.

## Why This Phase Exists

This accepted checkpoint keeps later slices from building on an unreviewed contract.

## Scope

Allowed:

- Review the S4 through S6 integration PR diff, authentication isolation, deadline behavior, list contract, and pagination tests. Apply at most three combined repair passes.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert the checkpoint repair commits. S4 through S6 retain their own rollback boundaries.

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

- Depends on: `gh-forgejo-shim-c07`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- Thermonuclear and adversarial reviewers report no unresolved blocker or required finding. The exact contract and deadline tests pass after repairs.
- The exact checkpoint commit has a green rust job, including the ChatGPT contract suite.

## Execution Boundary

- Acceptance boundary: The pull request module, test seam, shared deadline, and pagination rules are sound before view projections grow around them.
- Module surface: S4 through S6 integration PR diff, auth tests, deadline tests, and pull request list contract
- Review boundary: C2 integration PR diff, named tests, fixtures, command evidence, and review gate: Thermonuclear review covers module depth and seams. Adversarial review covers credentials, timeouts, pagination, partial results, and empty-result truthfulness.

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
- The exact checkpoint commit has a green rust job, including the ChatGPT contract suite.

## Replanning Triggers

- Stop after the third combined repair pass if a blocker, required finding, red test, or red CI result remains.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Group findings by distinct failure path before appointing one repair owner.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
