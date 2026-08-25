# Phase 11: S9: Render current checks and real no-check behavior

Canonical Beads issue: `gh-forgejo-shim-c11`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

ChatGPT shows current Forgejo checks, and a PR with no checks produces the same nonfatal warning as real `gh`.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S8.

## Scope

Allowed:

- Add `src/pull_requests/checks.rs`. Update `src/forgejo.rs` and check projection code in `src/normalize.rs`. Read combined commit status and action runs.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert S9. PR discovery and view remain intact.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop on stale status, missing requested keys, different no-check process behavior, or a C3 failure after three repair passes.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c10`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- Return requested `event`, timing, workflow, state, conclusion, and links. Collapse duplicate contexts to current state. With no checks, exit 1, write empty stdout, and write `no checks reported` to stderr.
- Golden checks test, no-check contract test, stale-history regression, and a redacted live Islandflow PR 111 record or verified replacement.

## Execution Boundary

- Acceptance boundary: ChatGPT shows current Forgejo checks, and a PR with no checks produces the same nonfatal warning as real `gh`.
- Module surface: current check collection, stale-history collapse, and no-check process output
- Review boundary: S9 integration PR diff, named tests, fixtures, command evidence, and review gate: C3 requires adversarial review only. The adversarial reviewer must not invoke the thermonuclear skill. Apply the combined repair process to its findings.

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
- Golden checks test, no-check contract test, stale-history regression, and a redacted live Islandflow PR 111 record or verified replacement.

## Replanning Triggers

- Stop on stale status, missing requested keys, different no-check process behavior, or a C3 failure after three repair passes.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S9 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
