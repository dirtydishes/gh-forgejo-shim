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

When no explicit path is configured, the shim searches the inherited `PATH` first and then checks common user and package-manager directories such as:

```text
~/.local/bin
/opt/homebrew/bin
/usr/local/bin
/opt/local/bin
```

This helps GUI-launched tools that inherit a minimal macOS PATH but still execute the shim successfully.

## macOS GUI PATH

Some GUI apps, including Codex.app when launched from Finder, Dock, or Spotlight, can start with only the system PATH:

```text
/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin
```

That PATH may miss both the generated shim at `~/.local/bin/gh` and the real Homebrew GitHub CLI at `/opt/homebrew/bin/gh`.

Persist a GUI-friendly user launchd PATH with:

```sh
gh-forgejo-shim install-gui-path
```

The command writes:

```text
~/Library/LaunchAgents/com.gh-forgejo-shim.user-gui-path.plist
```

It also runs `launchctl setenv PATH ...` for the current login session. Existing GUI apps need to be restarted before they inherit the new PATH.

To provide an exact value instead of the default:

```sh
gh-forgejo-shim install-gui-path --path "$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
```

`gh-forgejo-shim doctor` reports a `macOS gui PATH` check on macOS so shell and GUI PATH problems are easier to tell apart.

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
