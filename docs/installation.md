# Installation

`gh-forgejo-shim` is distributed as native Rust binaries. Python packaging is
not part of the supported product runtime.

## Requirements

The `gh` wrapper hands GitHub work to the real GitHub CLI, so it needs `gh`
2.103.3 or newer ([installation](https://github.com/cli/cli#installation)).
`gfj bootstrap` and `gfj install-shim` check the `gh` they find, by running
`gh --version`, and stop without changing anything when it is missing, older,
or a pre-release of 2.103.3. `gfj doctor` reports the same check.

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

## Rollback

To roll back the shim integration without uninstalling the native binary:

```sh
gfj uninstall-shim
gfj uninstall-gui-path
```

To roll back the binary version, install an earlier Homebrew formula revision or
download an older GitHub release tarball and replace the two binaries in your
install directory. Configuration and auth live outside the binary install path.
