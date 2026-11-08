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

[host."git.example.com"]
api_url = "https://git.example.com"
token_env = "EXAMPLE_FORGEJO_TOKEN"
ca_file = "/path/to/ca.pem"
wip_prefixes = ["WIP:", "[WIP]"]

[paths]
gh = "/opt/homebrew/bin/gh"
fj = "/opt/homebrew/bin/fj"
```

A `[host."NAME"]` table sets `api_url`, `token_env`, `ca_file` and `wip_prefixes`
for one host.

The optional `[host."<hostname>"]` tables also configure hosts without an array entry. The singular `host` lets tables coexist with existing `hosts = [...]` files, including multi-line arrays. `api_url` defaults to `https://<hostname>` and accepts HTTP for tests or LAN servers; API requests append `/api/v1/...`. Omit `ca_file` unless the host needs an extra root CA. Its certificates and `FJ_SHIM_CA_FILE` both add to the normal trust store. `wip_prefixes` lists the title prefixes the forge marks drafts with, and should match the forge's own WIP setting: commands that launch real gh strip them from a draft's title, and the first one marks a new draft. It defaults to `["WIP:", "[WIP]"]`; the list can't be empty, and a prefix can't contain a comma.

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

`config add-host` refuses a known GitHub host: `github.com`, `ghe.com`, and every
name under them, such as `api.github.com` or `acme.ghe.com`. Such a host is also
ignored if it appears in the allowlist anyway. This preserves native GitHub CLI
behavior for GitHub repositories and keeps a stale or accidental `github.com`
entry from being treated as Forgejo.

## Environment Overrides

The shim reads these variables:

```text
FJ_SHIM_HOSTS
FJ_SHIM_REAL_GH
FJ_SHIM_ALLOW_UNKNOWN_GH
FJ_SHIM_REAL_FJ
FJ_SHIM_CA_FILE
FJ_SHIM_TRACE
FJ_SHIM_TRACE_BODY
FJ_SHIM_REAL_GIT
FJ_SHIM_TOKEN
FORGEJO_TOKEN
GITEA_TOKEN
FJ_TOKEN
GH_REPO
GH_HOST
```

`FJ_SHIM_TRACE` and `FJ_SHIM_TRACE_BODY` turn on [tracing](tracing.md).

`FJ_SHIM_HOSTS` replaces the configured host list for the current process:

```sh
FJ_SHIM_HOSTS=git.example.com,code.example.org gh pr view
```

`FJ_SHIM_ALLOW_UNKNOWN_GH=1` lets install and bootstrap go ahead with a real `gh`
whose version can't be read, such as one built from source; see
[Installation](installation.md#requirements).

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

## Private CA Certificates

`FJ_SHIM_CA_FILE` points to a PEM-encoded CA certificate or bundle to trust for
Forgejo API HTTPS connections, including token validation during `auth login`.
It adds to the bundled public roots; certificate and hostname
verification remain enabled. A missing or invalid file causes an error.

```sh
export FJ_SHIM_CA_FILE=/path/to/ca.pem
```

Set this variable in the environment of the process running the shim. GUI apps
must inherit it too; saving a token with `auth import` does not persist the CA
setting.

## macOS GUI PATH

Some GUI apps, including Codex.app when launched from Finder, Dock, or Spotlight, can start with only the system PATH:

```text
/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin
```

That PATH may miss both the generated shim at `~/.local/bin/gh` and the real Homebrew GitHub CLI at `/opt/homebrew/bin/gh`. The usual sign is Codex.app or another GUI-launched tool saying `GitHub CLI (gh) is not installed` even though `gh --version` works in your shell.

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

To only write the LaunchAgent and wait until the next login:

```sh
gh-forgejo-shim install-gui-path --no-apply
```

`gh-forgejo-shim doctor` reports separate checks for current process `PATH` and
macOS GUI PATH so shell and GUI launcher problems are easier to tell apart.

## Auth

Native auth commands are:

```sh
gh-forgejo-shim auth login [host]
gh-forgejo-shim auth import --from fj|tea [--hostname HOST]
gh-forgejo-shim auth status [host]
gh-forgejo-shim auth logout [host]
```

`auth login` prompts for a Forgejo host when one is not supplied, prompts for an access token without echoing it, validates the token with `GET /api/v1/user`, and saves it in shim-owned auth storage. It also adds the host to the allowlist, so routed `gh pr ...`, `gh issue ...`, and `gh repo view` commands can use the saved token immediately.

`auth import --from fj` reads forgejo-cli's `keys.json`; `--from tea` reads tea's YAML config. It copies the host's token into shim-owned storage and adds the host to the allowlist, without contacting the server. This is useful when Terminal already has auth through another Forgejo CLI, but GUI-launched apps such as Codex.app don't inherit that shell's environment. Omit `--hostname` only when the file has one entry. Missing or ambiguous entries cause an error without a write. The command prints the host and source path, never the token. Imports do not refresh OAuth tokens or read another tool's Keychain entries.

`auth status` reports whether auth is available for the host without printing the token. `auth logout` removes only the shim-owned stored auth for that host.

On macOS, the shim first tries to store tokens in Keychain with the system `security` tool. If Keychain storage is unavailable or fails, it falls back to:

```text
~/.config/gh-forgejo-shim/auth.json
```

The fallback file is written with owner-only permissions.

Stored tokens are protected at rest and from other users on the machine. They
aren't protected from programs running as your own user, which is also true of
the GitHub token plain `gh` keeps. The Keychain item trusts the `security` tool
that created it, as `gh`'s does, so any process running as you can read it with
`security find-generic-password` without a prompt. `gh auth token` also prints
the token for a Forgejo host to any caller, as plain `gh` does for GitHub,
because agents and scripts rely on it.

For each host, the shim first checks its stored token, then the variable named by
that host's `token_env`. If exactly one Forgejo host is configured, it also checks
these generic variables in order:

```text
FJ_SHIM_TOKEN
FORGEJO_TOKEN
GITEA_TOKEN
FJ_TOKEN
```

Empty variables count as unset. With more than one host, generic variables never
supply a token. Normal commands never read other tools' credential files; run
`auth import` once to copy a token.

On macOS, the current `fj` CLI stores auth in:

```text
~/Library/Application Support/forgejo-cli.forgejo-cli/keys.json
```

Import also accepts fj's older `Cyborus.forgejo-cli/keys.json` location. On Linux, fj uses `${XDG_DATA_HOME:-~/.local/share}/forgejo-cli/keys.json`. Tea uses `${XDG_CONFIG_HOME}/tea/config.yml`, defaulting to `~/Library/Application Support/tea/config.yml` on macOS or `~/.config/tea/config.yml` on Linux, with `~/.tea/tea.yml` as its legacy fallback.

The token needs the four scopes in the README's
[Token scopes](../README.md#token-scopes) table.

After import, GUI apps such as Codex.app find Forgejo auth without inheriting
shell-only environment variables.

## Repository Detection

Forgejo repository detection checks:

- `-R` or `--repo`
- `GH_REPO`
- `GH_HOST`
- local git remotes

Supported repository formats include HTTPS URLs, SSH URLs, scp-style SSH, `HOST/OWNER/REPO`, plain `OWNER/REPO` with `GH_HOST`, and `.git` suffixes.
