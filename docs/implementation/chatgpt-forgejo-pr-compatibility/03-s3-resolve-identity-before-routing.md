# Phase 03: S3: Resolve identity before routing

Canonical Beads issue: `gh-forgejo-shim-c03`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

The original `--repo 127.0.0.1/...` command targets canonical Forgejo, while unknown Forgejo operations fail locally.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S2.

## Scope

Allowed:

- Update `src/routing.rs`, `src/repo.rs`, and the repository context passed into `src/read_only.rs`. Remove duplicate host parsing from routing and use `HostRegistry` as the sole provider decision source.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert S2 and S3 together if the registry contract must change. The new profile data remains inert until routing uses it.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop if any Forgejo repository can fall through to GitHub, if GitHub stops delegating, or if C1 fails after three repair passes.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c02`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- The exact Dirtypages command never targets loopback HTTPS and never invokes real `gh`. GitHub repositories still delegate. An unsupported command for an allowlisted Forgejo alias exits nonzero with a clear error.
- A route matrix covers canonical Forgejo, alias Forgejo, unknown Forgejo operation, unconfigured host, `github.com`, `GH_REPO`, `GH_HOST`, and Git remotes.

## Execution Boundary

- Acceptance boundary: The original `--repo 127.0.0.1/...` command targets canonical Forgejo, while unknown Forgejo operations fail locally.
- Module surface: repository identity resolution and provider-first command routing
- Review boundary: S3 integration PR diff, named tests, fixtures, command evidence, and review gate: C1 requires both thermonuclear and adversarial review, followed by the combined repair process.

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
- A route matrix covers canonical Forgejo, alias Forgejo, unknown Forgejo operation, unconfigured host, `github.com`, `GH_REPO`, `GH_HOST`, and Git remotes.

## Replanning Triggers

- Stop if any Forgejo repository can fall through to GitHub, if GitHub stops delegating, or if C1 fails after three repair passes.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S3 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
