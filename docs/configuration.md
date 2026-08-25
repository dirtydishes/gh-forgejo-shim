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

[[host_profiles]]
canonical_host = "git.example.com"
aliases = ["git.internal", "127.0.0.1:2222"]
api_root = "https://git.example.com/api/v1"
credential_host = "git.example.com"

[paths]
gh = "/opt/homebrew/bin/gh"
fj = "/opt/homebrew/bin/fj"
```

The `hosts` list remains the allowlist and keeps the file readable by older
versions. A `host_profiles` table adds transport aliases, a REST API root, and
the host name used for credentials. A profile table does not enable a host that
is absent from `hosts`.

## Host Allowlist

Add a host:

```sh
gh-forgejo-shim config add-host git.example.com
```

Add aliases or override the default API and credential hosts:

```sh
gfj config add-host git.example.com \
  --alias git.internal \
  --alias 127.0.0.1:2222 \
  --api-root https://git.example.com/api/v1 \
  --credential-host git.example.com
```

`--alias` may appear more than once. API roots must use HTTP or HTTPS. The
default API root is `https://HOST/api/v1`, and the default credential host is
the canonical host. The command rejects duplicate aliases, aliases assigned to
more than one profile, and attempts to register `github.com` as Forgejo.

Remove a host:

```sh
gh-forgejo-shim config remove-host git.example.com
```

List configured hosts:

```sh
gh-forgejo-shim config list
```

Profiles with custom values show their details:

```text
git.example.com
  aliases: git.internal, 127.0.0.1:2222
  api root: https://git.example.com/api/v1
  credential host: git.example.com
```

The shim routes Forgejo commands only when the detected repository host is in the allowlist.

Known GitHub hosts such as `github.com` are ignored even if they appear in the
allowlist. This preserves native GitHub CLI behavior for GitHub repositories and
prevents a stale or accidental `github.com` entry from being treated as Forgejo.

## Environment Overrides

`FJ_SHIM_HOSTS` replaces the configured host list for the current process:

```sh
FJ_SHIM_HOSTS=git.example.com,code.example.org gh pr view
```

Hosts from this environment variable use default profiles for that process.
Stored aliases, API roots, and credential hosts do not carry into the override.

`FJ_SHIM_REAL_GH` and `FJ_SHIM_REAL_FJ` override configured executable paths:

```sh
FJ_SHIM_REAL_GH=/opt/homebrew/bin/gh
FJ_SHIM_REAL_FJ=/opt/homebrew/bin/fj
```

When no explicit real-`gh` path is configured, the shim searches the inherited
`PATH` first and then checks common user and package-manager directories such as:

```text
~/.local/bin
/opt/homebrew/bin
/usr/local/bin
/opt/local/bin
```

This lets a managed wrapper in one directory delegate to a real GitHub CLI that
lives somewhere else, such as Homebrew. `gfj doctor` reports `bd` separately
because this repository's workflows may need Beads, but `bd` is not installed,
wrapped, or used for GitHub CLI routing.

## macOS GUI PATH

Some GUI apps, including Codex.app when launched from Finder, Dock, or Spotlight, can start with only the system PATH:

```text
/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin
```

That PATH may miss both the generated shim at `~/.local/bin/gh` and the real Homebrew GitHub CLI at `/opt/homebrew/bin/gh`.

`gfj bootstrap` repairs this by default on macOS. To run only the GUI PATH
repair, use:

```sh
gh-forgejo-shim install-gui-path
```

The command writes:

```text
~/Library/LaunchAgents/com.gh-forgejo-shim.user-gui-path.plist
```

It also runs `launchctl setenv PATH ...` for the current login session. Existing
GUI apps need to be restarted before they inherit the new PATH; already-running
processes cannot be repaired in place.

To provide an exact value instead of the default:

```sh
gh-forgejo-shim install-gui-path --path "$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
```

`gh-forgejo-shim doctor` reports separate checks for current process `PATH` and
macOS GUI PATH so shell and GUI launcher problems are easier to tell apart.

## Auth

Native auth commands are:

```sh
gh-forgejo-shim auth login [host]
gh-forgejo-shim auth import [host]
gh-forgejo-shim auth status [host]
gh-forgejo-shim auth logout [host]
```

`auth login` prompts for a Forgejo host when one is not supplied, prompts for an access token without echoing it, validates the token with `GET /api/v1/user`, and saves it in shim-owned auth storage. It also adds the host to the allowlist, so routed `gh pr ...`, `gh issue ...`, and `gh repo view` commands can use the saved token immediately.

`auth import` finds an existing token from supported sources, validates it, and saves it to the same shim-owned storage. This is useful when Terminal already has auth through an environment variable or another Forgejo CLI, but GUI-launched apps such as Codex.app do not inherit that shell environment.

`auth status` reports whether auth is available for the host without printing the token. `auth logout` removes only the shim-owned stored auth for that host.

On macOS, the shim first tries to store tokens in Keychain with the system `security` tool. If Keychain storage is unavailable or fails, it falls back to:

```text
~/.config/gh-forgejo-shim/auth.json
```

The fallback file is written with owner-only permissions.

The shim checks token environment variables in this order:

```text
FJ_SHIM_TOKEN
FORGEJO_TOKEN
GITEA_TOKEN
FJ_TOKEN
```

Environment variables still take precedence for the current process. If none are set, the shim checks native shim storage and then tries a best-effort read from common `fj`, `tea`, and `gitea` config files.

On macOS, the current `fj` CLI stores auth in:

```text
~/Library/Application Support/Cyborus.forgejo-cli/keys.json
```

The shim can import from this file when present. After import, GUI apps such as Codex.app can find Forgejo auth without inheriting shell-only environment variables.

## Repository Detection

Forgejo repository detection checks:

- `-R` or `--repo`
- `GH_REPO`
- `GH_HOST`
- local git remotes

Supported repository formats include HTTPS URLs, SSH URLs, scp-style SSH, `HOST/OWNER/REPO`, plain `OWNER/REPO` with `GH_HOST`, and `.git` suffixes.
