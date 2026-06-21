# Installation

`gh-forgejo-shim` is distributed as native Rust binaries. Python packaging is
legacy only and is not the long-term install path.

## Homebrew

On macOS and Linux, the intended native installer is Homebrew:

```sh
brew tap dirtydishes/gh-forgejo-shim
brew install gh-forgejo-shim
gfj --version
gfj doctor
```

`gfj doctor` may print setup warnings on a clean machine before you have
installed the managed `gh` wrapper, allowlisted a Forgejo host, or imported
auth. That still verifies the native binary starts without Python.

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
version=0.1.1
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
tar -xzf "gh-forgejo-shim-${version}-${target}.tar.gz" -C ~/.local/bin gh-forgejo-shim gfj
gfj --version
gfj doctor
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
shape problems it cannot fix automatically.

## Rollback

To roll back the shim integration without uninstalling the native binary:

```sh
gfj uninstall-shim
gfj uninstall-gui-path
```

To roll back the binary version, install an earlier Homebrew formula revision or
download an older GitHub release tarball and replace the two binaries in your
install directory. Configuration and auth live outside the binary install path.
