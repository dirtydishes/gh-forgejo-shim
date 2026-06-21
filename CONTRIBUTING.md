# Contributing

Thanks for helping with `gh-forgejo-shim`.

The V1 goal is narrow compatibility for Codex.app workflows in Forgejo repositories. Prefer small, testable behavior over broad `gh` emulation.

## Development Setup

```sh
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install -e .
python3 -m unittest
```

Native release installs are Rust-first. The Python package remains in the
repository for legacy pipx migration and compatibility testing until the Phase
10 runtime cutover removes it. Python runtime code must stay stdlib-only. Test
code should also stay stdlib-only unless the project deliberately changes that
policy later.

Use these commands when touching Rust code:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Compatibility Goals

- Real GitHub repositories delegate to the real GitHub CLI.
- Forgejo routing is opt-in through explicit host allowlisting.
- `gh-forgejo-shim` remains the stable management command.
- The generated `gh` wrapper is reversible.
- Unsupported GitHub-only PR metadata flags fail clearly.
- Native auth remains scoped to Forgejo access tokens and never prints stored secrets.

## Release Process

1. Update the shared Cargo workspace version, `pyproject.toml`, and
   `src/gh_forgejo_shim/__init__.py` while the legacy Python package remains.
2. Run Rust fmt, clippy, tests, and the legacy Python unittest/compileall gates.
3. Build release tarballs with `scripts/package-release.sh` or the tag release
   workflow.
4. Verify checksums and smoke `gfj --version` plus `gfj doctor` from the
   unpacked tarball.
5. Publish the GitHub release tarballs and update the Homebrew formula. Do not
   add a PyPI release path except as a separately tracked temporary migration
   bridge.

## Beads Workflow

This repository uses Beads for issue tracking. Run:

```sh
bd prime
```

before starting work, and follow the repository session close protocol before handoff.
