# Configuration

`gh-forgejo-shim` uses explicit host allowlisting. No Forgejo hosts are enabled by default.

## Config File

The default config path is:

```text
~/.config/gh-forgejo-shim/config.toml
```

Example:

```toml
hosts = ["git.example.com"]

[paths]
gh = "/opt/homebrew/bin/gh"
fj = "/opt/homebrew/bin/fj"
```

## Host Allowlist

Add a host:

```sh
gh-forgejo-shim config add-host git.example.com
```

Remove a host:

```sh
gh-forgejo-shim config remove-host git.example.com
```

List configured hosts:

```sh
gh-forgejo-shim config list
```

The shim routes Forgejo commands only when the detected repository host is in the allowlist.

## Environment Overrides

`FJ_SHIM_HOSTS` replaces the configured host list for the current process:

```sh
FJ_SHIM_HOSTS=git.example.com,code.example.org gh pr view
```

`FJ_SHIM_REAL_GH` and `FJ_SHIM_REAL_FJ` override configured executable paths:

```sh
FJ_SHIM_REAL_GH=/opt/homebrew/bin/gh
FJ_SHIM_REAL_FJ=/opt/homebrew/bin/fj
```

## Auth

The shim checks token environment variables in this order:

```text
FJ_SHIM_TOKEN
FORGEJO_TOKEN
GITEA_TOKEN
FJ_TOKEN
```

If none are set, it tries a best-effort read from common `fj`, `tea`, and `gitea` config files. V1 does not implement a native login command.

On macOS, the current `fj` CLI stores auth in:

```text
~/Library/Application Support/Cyborus.forgejo-cli/keys.json
```

The shim reads this file when present, which helps GUI apps such as Codex.app find Forgejo auth without inheriting shell-only environment variables.

## Repository Detection

Forgejo repository detection checks:

- `-R` or `--repo`
- `GH_REPO`
- `GH_HOST`
- local git remotes

Supported repository formats include HTTPS URLs, SSH URLs, scp-style SSH, `HOST/OWNER/REPO`, plain `OWNER/REPO` with `GH_HOST`, and `.git` suffixes.
