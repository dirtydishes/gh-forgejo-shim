# Contributing

Thanks for helping with `gh-forgejo-shim`.

The V1 goal is narrow compatibility for Codex.app workflows in Forgejo repositories. Prefer small, testable behavior over broad `gh` emulation.

## Development Setup

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Run the native commands from the checkout while iterating:

```sh
cargo run -p gh-forgejo-shim --bin gh-forgejo-shim -- --help
cargo run -p gh-forgejo-shim --bin gfj -- doctor
```

Installed runtime changes should be verified from a release-style archive built
with `scripts/package-release.sh`. The product runtime is Rust-only; do not add
Python source, setuptools metadata, PyPI packaging, or pipx install paths unless
a separate decision bead records a temporary migration exception.

## Compatibility Goals

- Real GitHub repositories delegate to the real GitHub CLI.
- Forgejo routing is opt-in through explicit host allowlisting.
- `gh-forgejo-shim` remains the stable management command.
- The generated `gh` wrapper is reversible.
- Unsupported GitHub-only PR metadata flags fail clearly.
- Native auth remains scoped to Forgejo access tokens and never prints stored secrets.

## Release Process

1. Update the shared Cargo workspace version.
2. Run Rust fmt, clippy, tests, and release archive smoke checks.
3. Build release tarballs with `scripts/package-release.sh` or the tag release
   workflow.
4. Verify checksums and smoke `gfj --version` plus `gfj doctor` from the
   unpacked tarball.
5. Publish the GitHub release tarballs and update the Homebrew formula. Do not
   add a PyPI release path except as a separately tracked temporary migration
   bridge with an owner and removal condition.

## Beads Workflow

This repository uses Beads for issue tracking. Run:

```sh
bd prime
```

before starting work, and follow the repository session close protocol before handoff.
