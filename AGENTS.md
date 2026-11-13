# Agent instructions

gh-forgejo-shim lets `gh`, and the apps that drive it, work in Forgejo repositories.
[README.md](README.md) says what it does; [CONTRIBUTING.md](CONTRIBUTING.md) covers setup and
releases.

## Issues

Work is tracked in GitHub issues. Before you file, label or pick up an issue, read
[CONTRIBUTING.md#issues](CONTRIBUTING.md#issues): it defines the priority labels, `ready`,
`needs-maintainer` and the milestones.

## Gates

Run these before you push; CI runs them too.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI also runs the contract suite against a disposable Forgejo container: `scripts/contract/run.sh`
(needs Docker).

## Public repository

Commits, pull requests and issues are public. Keep private hostnames, local paths, IP addresses and
credentials out of them.
