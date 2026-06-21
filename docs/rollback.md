# Rollback And Recovery

`gh-forgejo-shim` is opt-in. Removing the generated wrapper returns normal `gh` behavior.

If you are migrating from an old pipx install to a Rust release, follow
[Migrate From pipx To The Rust Binary](pipx-to-rust-migration.md) first. That
guide covers the order-sensitive part of replacing the pipx-provided `gfj` and
`gh-forgejo-shim` commands with the Rust binary before uninstalling the pipx
package.

## Remove The Wrapper

```sh
gh-forgejo-shim uninstall-shim
```

By default this removes:

```text
~/.local/bin/gh
```

The uninstall command only removes files that look like a shim created by this project.

## If The Real `gh` Is Not Found

Check your PATH:

```sh
command -v gh
which -a gh
```

If `~/.local/bin/gh` still exists and you want to remove it manually:

```sh
rm -f ~/.local/bin/gh
```

Then open a new shell or refresh your shell command cache:

```sh
hash -r
```

## Keep The Wrapper But Pin Real Executables

If PATH ordering is unusual, set explicit paths:

```sh
FJ_SHIM_REAL_GH=/opt/homebrew/bin/gh
FJ_SHIM_REAL_FJ=/opt/homebrew/bin/fj
```

Or write them into:

```text
~/.config/gh-forgejo-shim/config.toml
```

```toml
[paths]
gh = "/opt/homebrew/bin/gh"
fj = "/opt/homebrew/bin/fj"
```

## Remove The macOS GUI PATH LaunchAgent

If you used the macOS GUI PATH helper, remove it with:

```sh
gh-forgejo-shim uninstall-gui-path
```

This deletes:

```text
~/Library/LaunchAgents/com.gh-forgejo-shim.user-gui-path.plist
```

Restart the login session to return newly launched GUI apps to the default launchd PATH. Existing apps keep the environment they already inherited until they restart.

## Remove Stored Forgejo Auth

Remove shim-owned auth for a Forgejo host with:

```sh
gh-forgejo-shim auth logout git.example.com
```

On macOS this removes the shim's Keychain item when one exists. On other platforms, or when Keychain storage was unavailable, it removes the matching entry from:

```text
~/.config/gh-forgejo-shim/auth.json
```

## Disable Forgejo Routing Temporarily

Run with an empty host override:

```sh
FJ_SHIM_HOSTS= gh pr view
```

Or remove a host permanently:

```sh
gh-forgejo-shim config remove-host git.example.com
```
