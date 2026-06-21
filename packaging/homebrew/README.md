# Homebrew packaging

This directory defines the native Homebrew installer path for the Rust
`gh-forgejo-shim` release. It is intentionally Rust-first: Homebrew installs
the prebuilt `gh-forgejo-shim` and `gfj` binaries from GitHub release tarballs
and verifies each tarball with the SHA256 embedded in the formula.

Do not add a long-lived PyPI path here. A temporary PyPI migration package, if
one is ever proposed, needs a separate decision bead and a dated removal plan.

## Release artifact contract

The formula template expects release assets named:

```text
gh-forgejo-shim-<version>-aarch64-apple-darwin.tar.gz
gh-forgejo-shim-<version>-x86_64-apple-darwin.tar.gz
gh-forgejo-shim-<version>-aarch64-unknown-linux-gnu.tar.gz
gh-forgejo-shim-<version>-x86_64-unknown-linux-gnu.tar.gz
```

Each tarball must unpack with the binaries at its root:

```text
gh-forgejo-shim
gfj
VERSION
LICENSE
README.md
docs/
```

Homebrew will download exactly one platform tarball, verify the matching
`sha256`, and install both binaries into `bin`.

## Maintainer update procedure

1. Build and publish the release tarballs from a clean tag, for example
   `v0.1.1`.
2. Compute the checksum for each uploaded tarball:

   ```sh
   shasum -a 256 dist/gh-forgejo-shim-0.1.1-aarch64-apple-darwin.tar.gz
   shasum -a 256 dist/gh-forgejo-shim-0.1.1-x86_64-apple-darwin.tar.gz
   shasum -a 256 dist/gh-forgejo-shim-0.1.1-aarch64-unknown-linux-gnu.tar.gz
   shasum -a 256 dist/gh-forgejo-shim-0.1.1-x86_64-unknown-linux-gnu.tar.gz
   ```

3. Copy `Formula/gh-forgejo-shim.rb.template` into the tap as
   `Formula/gh-forgejo-shim.rb`.
4. Replace `{{VERSION}}` with the release version without the leading `v`.
5. Replace every `{{SHA256_*}}` placeholder with the checksum for the matching
   tarball.
6. Validate the rendered formula locally:

   ```sh
   brew install ./Formula/gh-forgejo-shim.rb
   brew test gh-forgejo-shim
   gfj doctor
   ```

7. Commit the rendered tap formula only after `brew test` passes. Keep this
   repository's template in sync with any tap-side formula structure changes.

## Manual smoke install before tap publication

To validate a release asset before updating the tap, install directly from the
rendered formula file:

```sh
brew install ./Formula/gh-forgejo-shim.rb
gfj --version
gh-forgejo-shim --help
gfj doctor
```

If a checksum is wrong, Homebrew fails before install. Recompute the checksum
from the uploaded asset rather than from a local rebuild, because Homebrew
verifies the bytes it downloads from the GitHub release.
