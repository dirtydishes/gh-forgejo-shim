# Phase 02: S2: Add typed, backward-compatible host profiles

Canonical Beads issue: `gh-forgejo-shim-c02`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

A user can configure one canonical Forgejo host with transport aliases and inspect the result with `gfj config list`.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S1.

## Scope

Allowed:

- Add `src/provider.rs`. Update `src/config.rs`, `src/cli.rs`, `src/lib.rs`, and `docs/configuration.md`. Introduce: ```rust HostProfile { canonical_host, aliases, api_root, credential_host, } HostRegistry::resolve(&RepoRef) -> ProviderResolution ``` Extend `config add-host` with repeatable `--alias`, optional `--api-root`, and optional `--credential-host` flags.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert S2. New writes retain the legacy canonical host list, so an older binary can still read it.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop on a lossy config rewrite, implicit host enablement, ambiguous resolution, or a format an older binary cannot read.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c01`
- Parallel-safe: `No parallel slice is accepted at this boundary.`

## Acceptance Evidence

- Legacy `hosts = ["codeberg.org"]` creates a default profile. A `git.dirtydishes.dev` fixture profile resolves both `127.0.0.1` and `127.0.0.1:2222`. Duplicate or ambiguous aliases fail.
- Unit and CLI round-trip tests for legacy config, new config, invalid roots, ambiguous aliases, and `FJ_SHIM_HOSTS`.

## Execution Boundary

- Acceptance boundary: A user can configure one canonical Forgejo host with transport aliases and inspect the result with `gfj config list`.
- Module surface: HostProfile, HostRegistry, configuration CLI, and configuration docs
- Review boundary: S2 integration PR diff, named tests, fixtures, command evidence, and review gate: C1 later reviews the pattern. Slice review confirms no default Dirtydishes entry, GitHub cannot be registered, URL schemes are restricted to HTTP and HTTPS, and old config remains readable.

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
- Unit and CLI round-trip tests for legacy config, new config, invalid roots, ambiguous aliases, and `FJ_SHIM_HOSTS`.

## Replanning Triggers

- Stop on a lossy config rewrite, implicit host enablement, ambiguous resolution, or a format an older binary cannot read.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S2 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
