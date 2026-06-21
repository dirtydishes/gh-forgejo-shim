# Migrate From pipx To The Rust Binary

This guide is for existing users who installed `gh-forgejo-shim` with `pipx`.

Phase 09 makes the release path Rust-first. Use the Rust-first install method named in the release notes for the version you are installing. Do not treat PyPI as the long-term distribution channel for new installs. A PyPI release may exist only as a short migration bridge, and any such bridge should point users to the Rust binary rather than keeping Python as the product runtime.

These steps only migrate an existing user install. They do not remove Python source, tests, or development tooling from the repository.

## Before You Start

Check the currently active commands:

```sh
which -a gh-forgejo-shim
which -a gfj
which -a gh
```

If `gh-forgejo-shim` or `gfj` resolves inside a pipx venv, keep the pipx install in place until the Rust binary has been installed and verified.

## Install The Rust Binary

Install the Rust binary through the release's selected Rust-first path. For example, use the Homebrew formula, release tarball, or other native binary channel documented for that release:

```sh
# Example only: use the exact command from the release notes.
brew install gh-forgejo-shim
```

After installation, confirm that your shell can see the Rust-installed command before the pipx command:

```sh
which -a gfj
gfj version
```

If `gfj` still points at pipx, either move the Rust install directory earlier in `PATH`, open a new shell, or run the Rust binary by its absolute path for the next step.

## Refresh The Managed `gh` Wrapper

Rewrite the managed `gh` wrapper so it points at the Rust binary instead of the old pipx venv:

```sh
gfj install-shim --force
```

Use `--force` only after `which -a gh` shows that the target `gh` is the old shim-managed wrapper you intend to replace. Without `--force`, `install-shim` refuses to overwrite an unrelated file.

Verify the migrated install:

```sh
gfj doctor
```

`doctor` should report that the real `gh` can be found, the managed wrapper is first in `PATH`, and your Forgejo auth and allowlisted hosts are still available.

## Remove The Old pipx Package

Only remove pipx after the Rust-installed `gfj doctor` succeeds:

```sh
pipx uninstall gh-forgejo-shim
```

Then clear the shell command cache and re-check command resolution:

```sh
hash -r
which -a gh-forgejo-shim
which -a gfj
which -a gh
gfj doctor
```

Your existing config and auth live under `~/.config/gh-forgejo-shim` or the macOS Keychain and are shared by the Rust binary. Removing the pipx package should not delete that state.

## Roll Back To pipx

If the Rust binary fails after migration, remove only the managed wrapper first:

```sh
gfj uninstall-shim
```

Then reinstall the old pipx package and recreate the wrapper from pipx:

```sh
pipx install gh-forgejo-shim
gfj install-shim --force
gfj doctor
```

If `gfj` is unavailable because `PATH` changed, call the intended binary by absolute path or open a new shell before running the commands again.

To bypass Forgejo routing temporarily without uninstalling anything, run a single command with an empty host override:

```sh
FJ_SHIM_HOSTS= gh pr view
```
