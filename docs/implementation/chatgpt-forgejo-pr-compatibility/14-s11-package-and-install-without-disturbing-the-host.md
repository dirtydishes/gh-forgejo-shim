# Phase 14: S11: Package and install without disturbing the host

Canonical Beads issue: `gh-forgejo-shim-c14`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

A fresh app-server child resolves the managed wrapper, while GitHub still runs through verified `gh` 2.96.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S9 and C3. S10 and S11 may run in parallel in separate worktrees because their owned implementation files do not overlap.

## Scope

Allowed:

- Update `src/setup.rs`, `src/bootstrap.rs`, `src/doctor.rs`, `src/shim.rs`, `src/external.rs`, `scripts/package-release.sh`, `.github/workflows/ci.yml`, `docs/installation.md`, and `docs/rollback.md`. Add `packaging/real-gh-compat.toml` with approved versions and target checksums.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Before live installation there is no runtime state. After installation, `gfj uninstall-shim` removes the managed wrapper. Reinstalling the prior binary rolls back its version. `FJ_SHIM_HOSTS=` disables Forgejo routing for one process.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop if the wrapper target is unmanaged, checksum verification fails, system `gh` would change, active ChatGPT work requires a restart, or C4 fails after three repair passes.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- Preserve the accepted process contract and GitHub delegation behavior.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c12`
- Parallel-safe: `Yes, with S10 after C3 in a separate worktree.`

## Acceptance Evidence

- Package smoke verifies archive checksum, wrapper precedence, explicit real-`gh` path, GitHub control, private config and auth permissions, and uninstall. Do not replace `/usr/bin/gh`.
- Both GitHub Actions jobs are green, including release archive install and rollback.

## Execution Boundary

- Acceptance boundary: A fresh app-server child resolves the managed wrapper, while GitHub still runs through verified `gh` 2.96.
- Module surface: managed wrapper packaging, verified real-gh selection, install, doctor, uninstall, CI, and rollback docs
- Review boundary: S11 integration PR diff, named tests, fixtures, command evidence, and review gate: C4 requires thermonuclear and adversarial review, followed by the combined repair process. Inspect every filesystem write, managed-file marker, checksum source, permission, and uninstall path.

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
- Both GitHub Actions jobs are green, including release archive install and rollback.

## Replanning Triggers

- Stop if the wrapper target is unmanaged, checksum verification fails, system `gh` would change, active ChatGPT work requires a restart, or C4 fails after three repair passes.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S11 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
