# Rollback And Recovery

`gh-forgejo-shim` is opt-in. Removing the generated wrapper returns normal `gh` behavior.

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

## Disable Forgejo Routing Temporarily

Run with an empty host override:

```sh
FJ_SHIM_HOSTS= gh pr view
```

Or remove a host permanently:

```sh
gh-forgejo-shim config remove-host git.example.com
```
