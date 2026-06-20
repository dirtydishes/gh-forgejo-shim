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

Until the Rust cutover, installed runtime behavior still comes from the Python package. Python runtime code must stay stdlib-only. Test code should also stay stdlib-only unless the project deliberately changes that policy later.

The Rust rewrite scaffold is non-installed until cutover. Use these commands when touching Rust code:

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

1. Update the version in `pyproject.toml` and `src/gh_forgejo_shim/__init__.py`.
2. Run `python3 -m unittest`.
3. Review `gh-forgejo-shim doctor` output in a local install.
4. Build and publish with the standard Python packaging flow for the chosen release environment.

## Beads Workflow

This repository uses Beads for issue tracking. Run:

```sh
bd prime
```

before starting work, and follow the repository session close protocol before handoff.
