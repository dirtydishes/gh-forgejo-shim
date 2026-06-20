# Phase 09: Package And Release Rust Binaries

- Bead: `gh-forgejo-shim-ppc`
- Parent epic: `gh-forgejo-shim-d60`
- Status: open
- Priority: P3
- Depends on: phase 03
- Blocks: phase 10
- Parallel work: can run alongside later command phases after the binary shape exists

## Plain English Goal

Create a Rust-first install and release path. Users should not need Python for the final product.

## Scope

- Decide primary distribution:
  - GitHub release tarballs
  - Homebrew formula
  - crates.io or cargo-binstall
- Avoid PyPI unless there is a short migration release that only installs or locates the Rust binary and is removed by a dated follow-up.
- Build CI for:
  - Rust tests
  - formatting
  - linting
  - release builds
  - checksums
  - smoke tests
- Define upgrade behavior from old pipx installs.
- Define rollback behavior.
- Keep version numbers synchronized.

## Subagents

Use subagents for independent release slices.

Recommended split:

- Worker A owns Cargo release metadata and binary artifact layout.
- Worker B owns GitHub Actions build/test/release workflows.
- Worker C owns Homebrew or other native installer path.
- Worker D owns migration documentation for old pipx installs.

The main agent owns the final packaging decision. Any proposal to keep PyPI must create a separate decision bead.

## Deliverables

- Release workflow design and implementation.
- Native binary install documentation.
- Migration notes for existing users.
- A decision bead if a temporary PyPI migration package is proposed.

## Acceptance Criteria

- A clean machine can install the Rust binary and run `gfj doctor`.
- Existing users have a documented migration path.
- Release artifacts are reproducible enough to verify checksums.

## Risks

- Dropping pipx abruptly can strand existing users.
- Keeping a PyPI package too long can violate the zero-Python goal.
