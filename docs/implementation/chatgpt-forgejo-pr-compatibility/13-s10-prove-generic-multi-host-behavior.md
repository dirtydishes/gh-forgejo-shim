# Phase 13: S10: Prove generic multi-host behavior

Canonical Beads issue: `gh-forgejo-shim-c13`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

The same binary handles two unrelated Forgejo identities without sharing aliases or credentials.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S9 and C3.

## Scope

Allowed:

- Add `tests/multi_host.rs` and isolated two-host HTTP fixtures. Do not change product files unless the test exposes a defect.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert test files. Live work is read-only.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop if no authorized live second-host credential or repository is available. Do not create one without separate approval.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c12`
- Parallel-safe: `Yes, with S11 after C3 in a separate worktree.`

## Acceptance Evidence

- Two profiles use distinct aliases, API roots, tokens, repositories, and links. Requests and credentials never cross. Run a read-only smoke test against Codeberg or another unrelated live Forgejo host.
- Green two-host integration test. Attach the live smoke record separately because CI must not depend on a public service.

## Execution Boundary

- Acceptance boundary: The same binary handles two unrelated Forgejo identities without sharing aliases or credentials.
- Module surface: two-host integration fixtures and one authorized read-only live smoke record
- Review boundary: S10 integration PR diff, named tests, fixtures, command evidence, and review gate: Search product and fixture files for Dirtydishes defaults and hidden single-host assumptions.

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
- Green two-host integration test. Attach the live smoke record separately because CI must not depend on a public service.

## Replanning Triggers

- Stop if no authorized live second-host credential or repository is available. Do not create one without separate approval.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S10 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
