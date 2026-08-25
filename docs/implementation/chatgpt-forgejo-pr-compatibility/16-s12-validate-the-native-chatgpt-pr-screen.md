# Phase 16: S12: Validate the native ChatGPT PR screen

Canonical Beads issue: `gh-forgejo-shim-c16`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

The desktop app opens and keeps the Forgejo PR screen current without harming GitHub repositories.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S10, S11, and C4.

## Scope

Allowed:

- Make no source changes unless validation finds a defect. Install the approved build through the existing managed path, then run read-only desktop checks.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Remove the managed wrapper or reinstall the prior binary. Do not restart unrelated services or active tasks.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop on unavailable status, stale data, timeout, token leak, Forgejo-to-GitHub request, GitHub regression, or a C5 failure after three repair passes. Optional GraphQL enrichment and mutations require separate accepted plans.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c15`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- All of the following must pass: - Dirtypages `main` reports no matching PR without "Pull request status unavailable." - Islandflow PR 36, or a verified replacement, shows header, body, activity, and checks. - Lyricslab PR 31 still opens through real `gh` 2.96. - A no-check PR displays the native nonfatal warning. - Repeated polling stays within five seconds per process. - Trace data contains no token and no Forgejo request in GitHub traffic.
- Exact green S12 commit plus a redacted desktop trace and screenshots tied to that commit.

## Execution Boundary

- Acceptance boundary: The desktop app opens and keeps the Forgejo PR screen current without harming GitHub repositories.
- Module surface: the approved managed install, native ChatGPT pull request screen, and redacted process trace
- Review boundary: S12 integration PR diff, named tests, fixtures, command evidence, and review gate: C5 requires adversarial review and manual product review. The adversarial reviewer must not invoke the thermonuclear skill. Compare the UI, process trace, and API truth.

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
- Exact green S12 commit plus a redacted desktop trace and screenshots tied to that commit.

## Replanning Triggers

- Stop on unavailable status, stale data, timeout, token leak, Forgejo-to-GitHub request, GitHub regression, or a C5 failure after three repair passes. Optional GraphQL enrichment and mutations require separate accepted plans.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S12 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
