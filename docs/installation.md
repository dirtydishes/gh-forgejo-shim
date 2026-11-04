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

The check runs `gh --version` with no token in its environment and with gh's
update notices and telemetry turned off.

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
docs/
```

Manual install example:

```sh
version=0.1.2
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

Useful setup variants:

```sh
gfj bootstrap --target user-local
gfj bootstrap --dry-run
gfj bootstrap --no-gui-path
gfj doctor
```

On macOS, restart already-running GUI apps after `bootstrap` reports that it
changed the GUI PATH. Existing processes cannot inherit a new launchd
environment in place.

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

1. With more than one Forgejo host, unset `FJ_SHIM_TOKEN`, `FORGEJO_TOKEN`,
   `GITEA_TOKEN` and `FJ_TOKEN` first, and check that `gfj auth status HOST`
   finds a stored token for each host. Releases before this one use those
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
