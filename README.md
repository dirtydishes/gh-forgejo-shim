# gh-forgejo-shim

**GitHub CLI compatibility for Codeberg and Forgejo.**

Many coding tools assume every repository lives on GitHub. In a Codeberg or
Forgejo checkout, Git still works, but pull requests, issues, checks, and repo
details may appear missing.

You should not have to move a repository to GitHub because a coding app expects
`gh`.

`gh-forgejo-shim` answers the supported `gh` calls with data from Codeberg or
another Forgejo host. GitHub repositories still use the real GitHub CLI. It is
built for ChatGPT desktop, Codex, T3 Code, and other tools that run `gh` behind
the scenes. Work to match the current ChatGPT desktop contract is documented in
the [compatibility report](docs/reports/chatgpt-forgejo-pr-compatibility.html).

This is a focused compatibility layer, not a full rewrite of `gh`.

## Start here

Install with Homebrew:

```sh
brew install dirtydishes/tap/gh-forgejo-shim
```

Open a terminal in your Codeberg or Forgejo checkout, then run:

```sh
gfj bootstrap
gfj auth login HOST
gfj doctor
```

For Codeberg, `HOST` is `codeberg.org`:

```sh
gfj auth login codeberg.org
```

Create a dedicated token in Codeberg's settings before login. Codeberg explains
the process in its
[access token guide](https://docs.codeberg.org/advanced/access-token/).

Restart an already-open GUI coding app if `bootstrap` changes its `PATH`. Then
open the repository normally. The app can keep calling `gh`; the shim handles
the supported Forgejo requests.

Linux and macOS release archives are covered in
[the installation guide](docs/installation.md).

## Codeberg support

Codeberg is a hosted Forgejo service. It uses the same host-configured path as:

- `codeberg.org`
- self-hosted Forgejo servers
- other public Forgejo instances

Each host has its own allowlist entry and credential. Nothing is hardcoded to a
private server, and the code does not special-case `codeberg.org`. GitHub hosts
remain outside Forgejo routing.

The project name stays `gh-forgejo-shim` because Forgejo is the software and API
family. The docs name Codeberg directly so its users do not need to know that
detail before they can find or install the shim.

## What bootstrap does

`gfj bootstrap`:

- detects the current repository and Forgejo host
- adds that host to the allowlist
- installs a managed `gh` wrapper ahead of the real GitHub CLI
- checks authentication, `PATH`, remotes, and branch tracking
- prints commands for anything it cannot repair
- updates the macOS GUI `PATH` for newly opened apps unless you pass
  `--no-gui-path`

Preview its work without changing anything:

```sh
gfj bootstrap --dry-run
```

The long command name works too:

```sh
gh-forgejo-shim bootstrap
```

## How routing works

- Supported commands in an allowlisted Forgejo repository use the Forgejo API.
- GitHub repositories use the real `gh` binary.
- Unsupported flags inside a routed command fail with a clear error.
- Commands outside the routed set currently pass to the real `gh` binary.
- Removing a host from the allowlist disables Forgejo routing for that host.

The ChatGPT compatibility work will make provider selection happen before
command support checks, so unknown Forgejo operations fail locally instead of
reaching GitHub. The shim emits GitHub-shaped JSON only where tools require it.

## Supported commands

Pull requests:

```text
gh pr checks
gh pr checkout
gh pr comment
gh pr create
gh pr diff
gh pr list
gh pr status
gh pr view
```

Issues and repositories:

```text
gh issue create
gh issue list
gh issue view
gh repo view
```

Common aliases such as `gh pr co`, `gh pr new`, `gh issue ls`, and
`gh issue new` also work.

Support is deliberately narrower than GitHub's CLI. Run a command with
`--help` to see its accepted flags. Most unsupported flags fail explicitly.
`gh pr list` currently accepts but does not apply `--author`, `--app`,
`--assignee`, `--label`, or `--search`. Implementing authoritative
`--author @me` behavior is part of the current ChatGPT compatibility work.

## Pull requests and checks

The pull request commands cover the flows coding tools probe most often:

- find the pull request for the current branch
- read GitHub-shaped pull request fields
- create, inspect, diff, comment on, and check out pull requests
- show Forgejo commit statuses through `gh pr checks`

Repeated status updates with the same context collapse to the latest result.
States are reported as `pass`, `fail`, or `pending` buckets where `gh` expects
them.

Some GitHub-only metadata, including projects and maintainer-edit controls, has
no Forgejo equivalent and is not faked.

## Repository setup

GitHub-oriented apps inspect Git remotes before they call `gh`. A conventional
`origin` gives them the clearest repository identity:

```text
origin.url      https://forge.example/owner/repo.git
origin.pushurl  git@forge.example:owner/repo.git
origin/HEAD     refs/remotes/origin/main
branch upstream origin/current-branch
```

For Codeberg:

```sh
git remote set-url origin https://codeberg.org/OWNER/REPO.git
git remote set-url --push origin git@codeberg.org:OWNER/REPO.git
git fetch origin
git remote set-head origin -a
```

If a coding app runs in a remote SSH workspace, apply the setup inside that
remote checkout. Changing the clone on your laptop does not change the clone on
the remote machine.

## Authentication

Manage credentials per Forgejo host:

```sh
gfj auth login HOST
gfj auth import HOST
gfj auth status HOST
gfj auth logout HOST
```

`auth login` validates the token with `GET /api/v1/user`, stores it without
printing it, and adds the host to the allowlist. `auth import` checks supported
environment variables and CLI config files for an existing token.

On macOS, the shim uses Keychain when available. Its file fallback is:

```text
~/.config/gh-forgejo-shim/auth.json
```

The shim writes that file with owner-only permissions.

## Configuration

Persistent configuration lives at:

```text
~/.config/gh-forgejo-shim/config.toml
```

Manage allowed hosts with:

```sh
gfj config add-host codeberg.org
gfj config add-host git.example.com
gfj config remove-host git.example.com
gfj config list
```

Useful overrides include:

```text
FJ_SHIM_HOSTS
FJ_SHIM_REAL_GH
FJ_SHIM_REAL_FJ
FJ_SHIM_TOKEN
FORGEJO_TOKEN
GITEA_TOKEN
FJ_TOKEN
GH_REPO
GH_HOST
```

See [the configuration guide](docs/configuration.md) for executable paths,
token lookup order, and manual setup.

## GUI apps and remote workspaces

GUI apps on macOS may not inherit `~/.local/bin` or Homebrew paths. Install a
launchd path for newly opened apps with:

```sh
gfj install-gui-path
```

If the app says `GitHub CLI unavailable` after setup, restart the app and run:

```sh
gfj doctor
command -v gh
which -a gh
```

Install the shim on the machine where the app runs commands. For a remote
workspace, that usually means the remote host, not only your laptop.

## Trace a broken integration

When an app reports stale branch data or unavailable pull request status, record
the commands it runs:

```sh
export FJ_SHIM_TRACE="$PWD/gh-probe-trace.jsonl"
export FJ_SHIM_TRACE_BODY=1
```

Summarize the trace or run the built-in read probes:

```sh
gfj trace summarize gh-probe-trace.jsonl
gfj trace smoke
```

Trace output redacts authentication material. The optional body mode records
bounded command excerpts, but never records `gh auth token` output.

## Roll back

Remove only the files managed by the shim:

```sh
gfj uninstall-shim
gfj uninstall-gui-path
gfj auth logout HOST
```

Temporarily bypass Forgejo routing for one command:

```sh
FJ_SHIM_HOSTS= gh pr view
```

See [the rollback guide](docs/rollback.md) for path repair and recovery.

## Develop

Run the Rust checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Run either binary from the checkout:

```sh
cargo run -p gh-forgejo-shim --bin gh-forgejo-shim -- --help
cargo run -p gh-forgejo-shim --bin gfj -- --version
```

## More documentation

- [Installation](docs/installation.md)
- [Configuration](docs/configuration.md)
- [Testing](docs/dev/testing.html)
- [Rollback](docs/rollback.md)
- [Migrating from pipx](docs/pipx-to-rust-migration.md)
