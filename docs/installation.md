# Installation

`gh-forgejo-shim` is distributed as native Rust binaries. Python packaging is
not part of the supported product runtime.

## Requirements

The `gh` wrapper runs the real GitHub CLI, for GitHub work and for most Forgejo
commands, so it needs `gh` 2.103.3 or newer. `gfj bootstrap` and
`gfj install-shim` check the `gh` they find, by running `gh --version`, and stop
without changing anything when it is missing, older, or a pre-release of
2.103.3. `gfj doctor` reports the same check.

Many distributions ship an older `gh`. To get a current one:

- **Ubuntu and Debian:** add GitHub's own apt repository
  (`cli.github.com/packages`), as
  [gh's Linux install guide](https://github.com/cli/cli/blob/trunk/docs/install_linux.md)
  shows, then `sudo apt install gh`.
- **Homebrew:** `brew install gh`; the shim's formula installs it as a
  dependency.
- **Anywhere else:** a release archive from
  [gh's releases page](https://github.com/cli/cli/releases).

Check with `gh --version`.

A `gh` you built yourself may report `DEV` or a `git describe` version that
can't be compared. To go ahead with it, set `FJ_SHIM_ALLOW_UNKNOWN_GH=1`;
`doctor` then shows a warning instead. A `gh` that reports an older version,
or that fails, hangs or floods its output instead of answering, is refused even
then.

The check runs `gh --version` without `GH_TOKEN`, `GITHUB_TOKEN`,
`GH_ENTERPRISE_TOKEN` or `GITHUB_ENTERPRISE_TOKEN`, and with gh's update
notices and telemetry turned off.

## Homebrew

On macOS and Linux, the intended native installer is Homebrew:

```sh
brew install dirtydishes/tap/gh-forgejo-shim
gfj --version
gfj bootstrap
```

Run `gfj bootstrap` from inside the Forgejo checkout you want GUI tools to use.
It installs the managed `gh` wrapper, repairs macOS GUI PATH for newly opened
apps, and prints any remaining setup commands.

## GitHub Release Tarballs

Release assets are named by version and Rust target triple:

```text
gh-forgejo-shim-<version>-aarch64-apple-darwin.tar.gz
gh-forgejo-shim-<version>-x86_64-apple-darwin.tar.gz
gh-forgejo-shim-<version>-aarch64-unknown-linux-gnu.tar.gz
gh-forgejo-shim-<version>-x86_64-unknown-linux-gnu.tar.gz
```

Each tarball contains root-level binaries and release metadata:

```text
gh-forgejo-shim
gfj
VERSION
LICENSE
README.md
CONTRIBUTING.md
docs/
```

Manual install example:

```sh
version=0.5.0
case "$(uname -s):$(uname -m)" in
  Darwin:arm64) target=aarch64-apple-darwin ;;
  Darwin:x86_64) target=x86_64-apple-darwin ;;
  Linux:aarch64) target=aarch64-unknown-linux-gnu ;;
  Linux:x86_64) target=x86_64-unknown-linux-gnu ;;
  *) echo "unsupported platform" >&2; exit 1 ;;
esac

curl -LO "https://github.com/dirtydishes/gh-forgejo-shim/releases/download/v${version}/gh-forgejo-shim-${version}-${target}.tar.gz"
curl -LO "https://github.com/dirtydishes/gh-forgejo-shim/releases/download/v${version}/gh-forgejo-shim-${version}-${target}.tar.gz.sha256"
shasum -a 256 -c "gh-forgejo-shim-${version}-${target}.tar.gz.sha256"

mkdir -p ~/.local/bin
tar -xzf "gh-forgejo-shim-${version}-${target}.tar.gz" -C ~/.local/bin ./gh-forgejo-shim ./gfj
gfj --version
gfj bootstrap
```

On Linux systems without `shasum`, use `sha256sum -c` instead.

## First Setup

From inside a Forgejo checkout, run:

```sh
gfj bootstrap
gfj doctor
```

`bootstrap` installs the managed `gh` wrapper, allowlists the current Forgejo
host when one is detected, and prints repair commands for PATH, auth, or remote
shape problems it cannot fix automatically. By default it targets the first safe
writable directory that is already visible in the current process `PATH`; if no
safe visible directory exists, it falls back to `~/.local/bin`.

In full, `bootstrap`:

- Detects the current Forgejo repository from Git remotes.
- Adds the repository host to the shim allowlist.
- Installs the managed `gh` wrapper into the first safe writable directory already visible to the current process, falling back to `~/.local/bin` when needed.
- Checks the current process `PATH`, the real GitHub CLI path, and whether managed `gh` is first.
- On macOS, updates the launchd GUI PATH for newly opened GUI apps unless `--no-gui-path` is passed.
- Checks the real GitHub CLI's version (2.103.3 or newer) before it writes anything.
- Checks whether Forgejo auth can be found from shim storage or token variables.
- Checks `origin`, `origin/HEAD`, and current-branch upstream tracking.
- Prints repair commands for anything it cannot fix automatically.

The long command name works the same way:

```sh
gh-forgejo-shim bootstrap
```

If the wrapper path already contains an unrelated `gh`, bootstrap refuses to overwrite it unless you opt in:

```sh
gfj bootstrap --force
```

To force the older user-local target, preview actions, or skip macOS GUI PATH repair:

```sh
gfj bootstrap --target user-local
gfj bootstrap --dry-run
gfj bootstrap --no-gui-path
gfj doctor
```

On macOS, restart already-running GUI apps after `bootstrap` reports that it
changed the GUI PATH. Existing processes cannot inherit a new launchd
environment in place.

## Manual Setup

If you prefer not to use `bootstrap`, allowlist at least one Forgejo host:

```sh
gh-forgejo-shim config add-host git.example.com
```

Known GitHub hosts are ignored even if they appear in the allowlist; see
[Host Allowlist](configuration.md#host-allowlist).

Install the opt-in wrapper:

```sh
gh-forgejo-shim install-shim
```

The wrapper is written to:

```text
~/.local/bin/gh
```

Make sure `~/.local/bin` appears before the real `gh` location in `PATH`. You can confirm with:

```sh
command -v gh
which -a gh
```

If you need a different install location:

```sh
gh-forgejo-shim install-shim --bin-dir ~/.local/bin
```

On macOS, GUI apps may not see that `PATH`; see
[macOS GUI PATH](configuration.md#macos-gui-path).

## Doctor

`gfj doctor` checks:

- Whether the current process `PATH` can resolve bare `gh`.
- Whether the real GitHub CLI can be found from config, env, current `PATH`, or common install dirs, and whether it is 2.103.3 or newer.
- Whether managed `gh` is installed first and can delegate to real `gh`.
- Whether duplicate managed wrappers are present.
- Whether `bd` can be found for repo workflows. This is diagnostic only; `bd` is not part of the GitHub CLI shim.
- Whether `fj` can be found for legacy auth-import diagnostics. Missing `fj` is not a GitHub CLI readiness failure.
- Whether at least one Forgejo host is allowlisted.
- Whether an auth token can be discovered.
- Whether each host's token has the [scopes it needs](../README.md#token-scopes). doctor asks that host's Forgejo, naming an owner that can't exist, so the check reads and changes nothing. It never prints the token.
- On macOS, whether the user launchd PATH can expose the shim to new GUI apps.

## Repository Remote Shape

GitHub-style tools usually inspect the local Git checkout before they invoke `gh`. The shim can answer Forgejo-backed `gh` commands, but tools may still show incomplete repository state, disabled commit or push controls, or unavailable pull request status if the checkout does not look like a conventional GitHub clone.

For best compatibility, each Forgejo checkout should have:

```text
origin.url      https://git.example.com/owner/repo.git
origin.pushurl  git@ssh.example.com:owner/repo.git
origin/HEAD     refs/remotes/origin/main
branch upstream origin/current-branch
```

If your checkout already has a Forgejo remote under another name, keep it and add `origin` as a compatibility alias:

```sh
git remote add origin https://git.example.com/owner/repo.git
git remote set-url --push origin git@ssh.example.com:owner/repo.git
git fetch origin
git remote set-head origin -a
git branch --set-upstream-to=origin/your-branch your-branch
```

If the server does not report the default branch clearly, set it explicitly:

```sh
git remote set-head origin main
```

Useful checks:

```sh
git config --get remote.origin.url
git symbolic-ref --quiet refs/remotes/origin/HEAD
git rev-parse --abbrev-ref --symbolic-full-name @{u}
git status --short --branch
gh repo view --json nameWithOwner,url,defaultBranchRef,sshUrl
gh pr status --json number,title,url,headRefName,state
gh issue list --json number,title,state,url
```

If your tool connects to a remote SSH workspace, apply the same remote setup inside that remote checkout too. Fixing the local Mac clone does not change a separate remote clone.

## Upgrading

The config file and stored tokens stay where they are, and the managed `gh`
wrapper keeps working across an upgrade.

1. Check that `gh --version` reports 2.103.3 or newer ([Requirements](#requirements)).
2. Upgrade the shim: `brew upgrade gh-forgejo-shim`, or unpack the new release
   archive over the old binaries.
3. From inside your Forgejo checkout, run `gfj bootstrap`, then `gfj doctor`.

If you relied on a token in an `fj`, `tea` or `gitea` config file, or on a
generic token variable with more than one host configured, run
`gfj auth login HOST`, or `gfj auth import --from fj|tea` once: normal commands
no longer read other tools' files.

## Rollback

To roll back the shim integration without uninstalling the native binary:

```sh
gfj uninstall-shim
gfj uninstall-gui-path
```

To roll back the binary version:

1. With more than one Forgejo host, remove `FJ_SHIM_TOKEN`, `FORGEJO_TOKEN`,
   `GITEA_TOKEN` and `FJ_TOKEN` from your shell profile and unset them in the
   shell you're using, check that `env` shows none of them, and check that
   `gfj auth status HOST` finds a stored token for each host. Releases before this one use those
   variables before a host's stored token, for every host, so a token meant for
   one forge would go to another.
2. Install an earlier Homebrew formula revision, or replace the two binaries in
   your install directory with an older release's. Configuration and auth live
   outside the binary install path.
3. Recreate the managed wrapper from the rolled-back binary, since the wrapper
   names the binary by its full path, which under Homebrew includes the
   version: `gh-forgejo-shim install-shim --force --bin-dir DIR`, where `DIR`
   holds your managed `gh`. Check that the wrapper's `exec` line names the
   rolled-back binary.

A release before this one doesn't read `[host."NAME"]` tables in `config.toml`,
so a host set up only through one loses those settings after a rollback.
