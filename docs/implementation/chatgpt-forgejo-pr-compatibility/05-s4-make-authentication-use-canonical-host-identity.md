# Phase 05: S4: Make authentication use canonical host identity

Canonical Beads issue: `gh-forgejo-shim-c05`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

`gh auth status --active --hostname ALIAS` finds the canonical host credential without exposing the token.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S3 and C1.

## Scope

Allowed:

- Update `src/auth.rs` and auth-specific tests. Add Linux `fj` discovery at `~/.local/share/forgejo-cli/keys.json`. Key lookup and stored auth by `credential_host`.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert S4. No credential migration or copying occurs.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop on any cross-host credential use, token disclosure, or change that copies an external token into a project file.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c04`
- Parallel-safe: `Yes, with S5 after C1 in a separate worktree.`

## Acceptance Evidence

- Alias and canonical host return the same logged-in identity. An unrelated host cannot read that credential. Linux `fj`, shim storage, environment variables, and macOS Keychain retain their documented precedence.
- Auth tests for matching host, unrelated host, aliases, precedence, owner-only storage, and secret redaction.

## Execution Boundary

- Acceptance boundary: `gh auth status --active --hostname ALIAS` finds the canonical host credential without exposing the token.
- Module surface: canonical-host authentication lookup and auth-specific tests
- Review boundary: S4 integration PR diff, named tests, fixtures, command evidence, and review gate: Inspect path permissions, host matching, logs, errors, stdout, stderr, and traces for token leakage.

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
- Auth tests for matching host, unrelated host, aliases, precedence, owner-only storage, and secret redaction.

## Replanning Triggers

- Stop on any cross-host credential use, token disclosure, or change that copies an external token into a project file.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S4 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
