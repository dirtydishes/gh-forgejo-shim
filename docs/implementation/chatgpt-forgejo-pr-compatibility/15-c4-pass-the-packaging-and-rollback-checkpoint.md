# Phase 15: C4: Pass the packaging and rollback checkpoint

Canonical Beads issue: `gh-forgejo-shim-c15`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

The reviewed integration commit can be packaged, installed, and removed without changing the system GitHub CLI or unrelated host state.

## Why This Phase Exists

This accepted checkpoint keeps later slices from building on an unreviewed contract.

## Scope

Allowed:

- Review the S10 and S11 integration PR diff, two-host proof, package files, managed install paths, checksum policy, permissions, uninstall, and rollback. Apply at most three combined repair passes.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert checkpoint repair commits and use the S10 and S11 rollback procedures.

## Constraints

- Reviewers do not repair their own findings.
- Keep one external integration PR.
- Stop after the third combined repair pass if a blocker, required finding, red test, red CI result, or rollback defect remains.

## Settled Decisions

- Use the review roles and combined repair policy stated in the accepted plan.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c13`, `gh-forgejo-shim-c14`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- Thermonuclear and adversarial reviewers report no unresolved blocker or required finding. The two-host test, package smoke, install simulation, and rollback proof pass.
- Both exact-commit GitHub Actions jobs are green, including release package smoke.

## Execution Boundary

- Acceptance boundary: The reviewed integration commit can be packaged, installed, and removed without changing the system GitHub CLI or unrelated host state.
- Module surface: S10 and S11 integration PR diff, multi-host tests, package archive, install commands, and rollback docs
- Review boundary: C4 integration PR diff, named tests, fixtures, command evidence, and review gate: Thermonuclear review covers code and package module shape. Adversarial review checks host isolation, every filesystem write, checksums, permissions, uninstall, and rollback.

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
- Both exact-commit GitHub Actions jobs are green, including release package smoke.

## Replanning Triggers

- Stop after the third combined repair pass if a blocker, required finding, red test, red CI result, or rollback defect remains.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Group findings by distinct failure path before appointing one repair owner.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
