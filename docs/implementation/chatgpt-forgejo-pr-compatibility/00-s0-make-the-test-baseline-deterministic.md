# Phase 00: S0: Make the test baseline deterministic

Canonical Beads issue: `gh-forgejo-shim-c00`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

Contributors get the same test result whether or not the host already has `/usr/bin/gh`.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. None.

## Scope

Allowed:

- Change only `crates/gh-forgejo-shim/tests/support/mod.rs` and the two affected cases in `tests/cli_scaffold.rs`. Give test processes a fixture-only `PATH` with an explicit Git fixture instead of inherited executable directories.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert the S0 commits. No product or user state changes.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop if isolation requires a product behavior change, skips an assertion, or relies on the developer machine's executable layout.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `none`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- All 25 CLI integration tests and all 124 current unit tests pass on a machine that has a system `gh`. Add a named regression proving that an installed system `gh` cannot affect the fixture.
- Green `cargo test --workspace --locked` and the named isolation regression in the `rust` job.

## Execution Boundary

- Acceptance boundary: Contributors get the same test result whether or not the host already has `/usr/bin/gh`.
- Module surface: CLI test support and the two environment-sensitive scaffold cases
- Review boundary: S0 integration PR diff, named tests, fixtures, command evidence, and review gate: Confirm the fix isolates the environment and does not weaken either existing assertion.

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
- Green `cargo test --workspace --locked` and the named isolation regression in the `rust` job.

## Replanning Triggers

- Stop if isolation requires a product behavior change, skips an assertion, or relies on the developer machine's executable layout.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S0 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
