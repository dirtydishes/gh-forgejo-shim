# gh-forgejo-shim

> **Beta.** The shim covers the `gh` calls ChatGPT desktop, Codex and Claude make for day-to-day
> pull request work. A few less common commands aren't covered yet; [Known gaps](#known-gaps) lists
> them. If something breaks, please
> [open an issue](https://github.com/dirtydishes/gh-forgejo-shim/issues/new). Say which app you
> were using and which action you were attempting, and paste the output of `gfj doctor`. To show us
> the exact commands, turn on [tracing](#codex-probe-diagnostics) and attach the trace, but read it
> first: redaction hides most credentials, not every form. [Rollback](#rollback) removes only the
> shim's own files.

Open a Forgejo repository in ChatGPT desktop or Codex, and its pull requests work as they do on
GitHub. The shim answers the GitHub CLI (`gh`) calls these apps make with data from your Forgejo
server, and leaves your GitHub repositories to the real `gh`.

![Codex in ChatGPT desktop, open on a Forgejo repository named gfj-demo. Asked "Is PR #1 ready to merge?", the agent answers that Forgejo reports the PR can merge into main without conflicts, that lint, tests and build passed, and that it has one review comment and no approvals. The sidebar shows a pull request status icon beside each chat.](docs/images/pr-ready.png)

## What works

In a Forgejo repository, ChatGPT desktop and Codex give you:

- A pull request status icon beside each chat in the sidebar.
- The pull request menu: view the PR, its code changes and status, repair, and merge.
- Review comments: add a reviewer's comment to the chat and tell the agent to deal with it.
- The agent's own `gh` calls, so it can read checks, post comments and tell whether a PR is ready
  to merge.

<img src="docs/images/pr-menu.png" width="480" alt="The pull request menu in Codex for a Forgejo pull request: View PR, Code changes (+225 −3), Status (Open), Repair, Merge, and Add to chat. Beside it, the repository panel lists the pull request with a green status dot.">

![A Forgejo review comment on src/cli/commands.ts, line 45, attached to the Codex message box as "1 comment". Below it, the user has typed "Fix this, push, and reply to the reviewer."](docs/images/review-comment.png)

ChatGPT's pull request overview pane can't open a Forgejo pull request; [Known gaps](#known-gaps)
says why and lists what else is missing. Claude Code and Claude desktop need
[server mode](#server-mode).

## Works with

| App | PR status | CI checks | Merge | Issues | Mode |
|---|---|---|---|---|---|
| ChatGPT desktop (Codex in the app) | yes, except the overview pane | yes | yes | untested | either |
| Codex CLI | untested | untested | untested | untested | either |
| Claude Code | API tested | untested | untested | untested | server mode |
| Claude desktop | API tested | API tested | untested | untested | server mode |

"Yes" means it has been run in the app. "API tested" means CI sends the app's own requests to the
gateway, but no run in the app is on record. "Either" means client-only or server mode. In
client-only mode, Claude's agent can still run `gh` itself.

## Contents

- [Works with](#works-with)
- [Two ways to run it](#two-ways-to-run-it)
- [Install](#install) and [Quickstart](#quickstart)
- [Server mode](#server-mode) and [pull request links](#pull-request-links)
- [Known gaps](#known-gaps)
- When something breaks: [Doctor](#doctor), [Codex probe diagnostics](#codex-probe-diagnostics),
  [Rollback](#rollback)
- [Reference](#reference): commands and JSON output, configuration and auth, setup details
- [Development](#development)

## Two ways to run it

- **Client-only:** the `gh` wrapper on each machine. Each `gh` call in a Forgejo repository starts a
  private gateway for that call alone, and nothing changes on the forge. It covers Codex and any
  agent or tool that runs `gh`.
- **Server mode (new in 0.2.0):** the gateway, `fga-server`, runs beside Forgejo on the forge's own
  hostname, so the forge itself answers GitHub's API. Any `gh` logged in to the forge works with no
  wrapper, and so do tools that call the API without `gh`. **Use it if you use Claude Code or Claude
  desktop:** they look up a branch's pull request themselves, outside `gh`, and only a gateway on the
  forge can answer. You need control of the forge's reverse proxy, so a hosted forge such as Codeberg
  can't use it.

You can use both. [Server mode](#server-mode), further down, has the setup.

## Install

### Requirements

The `gh` wrapper runs the real GitHub CLI, so it needs `gh` 2.103.3 or newer. `gfj bootstrap` and `gfj install-shim` stop without changing anything when the `gh` they find is missing or older, and `gfj doctor` reports the same check. Many distributions ship an older `gh`; on Ubuntu and Debian, add GitHub's own apt repository as [gh's Linux install guide](https://github.com/cli/cli/blob/trunk/docs/install_linux.md) shows, or use Homebrew. A `gh` you built yourself that reports `DEV` needs `FJ_SHIM_ALLOW_UNKNOWN_GH=1`. See [docs/installation.md](docs/installation.md#requirements).

### Install paths

The steps below set up the client-only wrapper. For server mode, see
[Server mode](#server-mode).

Install the native Rust binary with Homebrew:

```sh
brew install dirtydishes/tap/gh-forgejo-shim
gfj --version
```

Existing pipx users should follow
[Migrate From pipx To The Rust Binary](docs/pipx-to-rust-migration.md) when
installing a native Rust release. PyPI is not a supported product runtime.

GitHub release tarballs are also available for macOS and Linux. See
[docs/installation.md](docs/installation.md) for manual tarball installs,
checksum verification, rollback, and maintainer release details.

## Quickstart

1. Install the native package with Homebrew or a GitHub release tarball.
2. Open a terminal inside your Forgejo repository.
3. Run `gfj bootstrap`.
4. Run `gfj auth login HOST` with a token that has the [four scopes](#token-scopes), or use `gfj auth import --from fj|tea --hostname HOST` to copy a token from that tool's config.
5. Copy and run any repair commands `bootstrap` prints.
6. On macOS, restart the GUI tool if `bootstrap` reports that it changed the GUI PATH.
7. Confirm the setup:

```sh
gfj doctor
```

8. Restart the GUI tool, open the Forgejo repository, and use the normal repository, branch, commit, push, pull request, issue, and check workflows.

`bootstrap` adds the repository's host to the allowlist, installs the managed `gh` wrapper, and on
macOS updates the PATH that apps opened from Finder, Dock or Spotlight get. It checks the real
`gh`'s version before it writes anything, and prints repair commands for anything it can't fix.
[docs/installation.md](docs/installation.md#first-setup) lists each step and its flags
(`--force`, `--dry-run`, `--target user-local`, `--no-gui-path`), and
[how to set up by hand](docs/installation.md#manual-setup).

For scripted setup or documentation, use `gh-forgejo-shim`. For day-to-day typing, `gfj` is the same command with a shorter name.

### Token scopes

Create the Forgejo token under **Settings → Applications** with these four scopes. The wrapper and server mode need the same ones. In Forgejo a `write:` scope includes its `read:` scope, so a token with these four needs no other.

| Scope | Without it |
|---|---|
| `read:user` | Every gh call fails, since the gateway can't tell whose token it is. |
| `write:repository` | Pull requests, branches and checks can't be read. With `read:repository` only, pull requests can't be created, edited or merged. |
| `write:issue` | Issues can't be read, and a pull request's comments and scheduled auto-merge read as none. With `read:issue` only, issues and comments can't be created or edited. |
| `read:organization` | A repository an organization owns reads as owned by a user. |

`gfj doctor` asks each configured host's Forgejo which of these its token has, and names any that's missing. Where a missing scope hides data rather than failing a call, as in the last two rows, the gateway answers without that data and logs one warning per scope: in fga-server's log, or, for the wrapper, as a `warning` record in the trace when `FJ_SHIM_TRACE` is set.

## Server mode

Server mode runs the gateway, `fga-server`, next to Forgejo on the forge's own hostname, so the forge
answers GitHub's API. Any `gh` logged in to the forge then works with no wrapper, and so do Claude
Code and Claude desktop, which look up a branch's pull request themselves, outside `gh`.

**You need:**

- your own Forgejo;
- a reverse proxy in front of it that you control (nginx, Caddy or Nginx Proxy Manager); a hosted
  forge such as Codeberg can't use server mode;
- Docker with Compose, on a machine that can reach Forgejo.

**Set it up:**

1. Copy [packaging/gateway/compose.yaml](packaging/gateway/compose.yaml) into a folder of its own.
2. In it, set `FGA_PUBLIC_URL` to the forge's address as users type it, `FGA_FORGEJO_URL` to how the
   gateway reaches Forgejo, and the network name to the Docker network Forgejo and the proxy share.
   If they don't share one yet, [server-mode.md](docs/gateway/server-mode.md#1-run-the-gateway) shows
   how to create one and attach them.
3. In that folder, run `docker compose up -d`. It pulls the published image,
   `ghcr.io/dirtydishes/fga-server`.
4. Route three paths on the forge's hostname to `fga-server:8080`, and everything else to Forgejo as
   before: `/api/v3/`, `/api/graphql`, and `/{owner}/{repo}/pull/{n}` (GitHub-style pull request
   links, which the gateway redirects to Forgejo's `/pulls/{n}`). Pass the client's `Host` header
   through unchanged. [docs/gateway/server-mode.md](docs/gateway/server-mode.md) has the nginx, Caddy
   and Nginx Proxy Manager steps.
5. Check it. Without a token the API answers `401`; `421` means the proxy changed `Host`. A pull
   request link redirects to Forgejo's page:

   ```sh
   curl -s -o /dev/null -w '%{http_code}\n' https://forge.example.com/api/v3/meta
   curl -s -o /dev/null -w '%{http_code} %{redirect_url}\n' https://forge.example.com/<owner>/<repo>/pull/1
   ```

6. Log in. gh's browser login doesn't work with Forgejo, so create a token in Forgejo under
   **Settings → Applications** with the [four scopes](#token-scopes), run
   `gh auth login --hostname forge.example.com`, and paste it. Claude and ChatGPT desktop take their
   token from `gh`.

### Upgrade and roll back

The gateway keeps no state. To upgrade, change the image tag in `compose.yaml` to the new release and
run `docker compose up -d` in the folder that holds your compose file. To roll back, change the tag
back and run it again.

To build the image yourself instead, run
`docker build -f packaging/gateway/Dockerfile -t fga-server:<version> .` in a checkout of that release,
and set `image: fga-server:<version>`.

## Pull request links

Every pull request URL the wrapper and the gateway print uses GitHub's layout,
`https://<forge>/<owner>/<repo>/pull/7`, not Forgejo's `/pulls/7`. ChatGPT and Codex.app find a PR
only by that `/pull/<number>` shape. Forgejo itself has no `/pull/` page:

- **Server mode** redirects `/pull/7` to Forgejo's `/pulls/7`, so every link opens.
- **Client-only installs:** the wrapper's own `--web` commands (`gh pr view --web`, `gh pr diff --web`,
  `gh pr comment --web`, and `gh issue view --web` for a PR) give Forgejo's `/pulls/7` page. A `/pull/7` link opened anywhere else, such as
  ChatGPT's "Open pull request in browser", gets a 404 unless the forge redirects it.

If you run the forge, add the redirect to its reverse proxy. For nginx, in the forge's `server` block:

```nginx
location ~ "^/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)/pull/([0-9]+)(/(?:files|commits|checks))?/?$" {
    return 301 /$1/$2/pulls/$3$4$is_args$args;
}
```

For Nginx Proxy Manager, paste the same block into the forge's proxy host, under **Advanced**, below
anything already there.

For Caddy, in the forge's site block:

```caddyfile
@pull_link path_regexp pull ^/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)/pull/([0-9]+)(/(?:files|commits|checks))?/?$
redir @pull_link /{re.pull.1}/{re.pull.2}/pulls/{re.pull.3}{re.pull.4}{?query} 301
```

Both match only plain owner and repository names, so a crafted path can't redirect to another site.
They assume the forge is served at the root of its hostname; under a subpath, add the prefix to both
the pattern and the target.
Server mode's gateway does the same, and also redirects deeper paths under a PR.

## Known gaps

- `gh run view --log` and `--log-failed` fail with a 404: the gateway doesn't serve job logs yet
  ([#161](https://github.com/dirtydishes/gh-forgejo-shim/issues/161)).
- Some `gh repo view --json` fields and some `gh api` routes aren't served by the gateway yet
  ([#189](https://github.com/dirtydishes/gh-forgejo-shim/issues/189)).
- In server mode without the wrapper, a `gh api` call with no `--hostname` goes to github.com when
  gh is also logged in there; that's how gh picks a host. Agents run such calls, so install the
  wrapper too, or tell the agent to pass `--hostname`
  ([#265](https://github.com/dirtydishes/gh-forgejo-shim/issues/265)).
- ChatGPT desktop's pull request overview pane needs a GitHub account connected to ChatGPT and
  can't open a Forgejo pull request. That's how ChatGPT is built, not a shim bug. The sidebar
  status, the pull request menu, review comments and the agent's `gh` commands work.
- In a Forgejo checkout, `gh pr create` checks where git will push before it runs. The check is best
  effort: a few unusual git or SSH setups can still send the push to another host
  ([#255](https://github.com/dirtydishes/gh-forgejo-shim/issues/255)).

## Doctor

Run:

```sh
gfj doctor
```

It checks that `gh` resolves to the managed wrapper and that the real `gh` is 2.103.3 or newer, that
a Forgejo host is allowlisted, that a token can be found and has the [scopes it needs](#token-scopes),
and, on macOS, the PATH new GUI apps get. [docs/installation.md](docs/installation.md#doctor) lists
every check.

## Codex probe diagnostics

When Codex.app reports `GitHub CLI unavailable`, `Pull request status unavailable`, or stale branch
data, turn on tracing before guessing at the fix. The shim then records the exact `gh` commands it
handles or delegates:

```sh
export FJ_SHIM_TRACE="$PWD/codex-probe-trace.jsonl"
export FJ_SHIM_TRACE_BODY=1
```

Redaction hides credentials in common forms but not every form, so read a trace before you share
it. Summarize one with:

```sh
gfj trace summarize codex-probe-trace.jsonl
```

[docs/tracing.md](docs/tracing.md) covers what a trace records, how redaction works, the smoke probes
(`gfj trace smoke`) and recording Codex's raw `git` calls.

## Rollback

Remove the generated wrapper:

```sh
gfj uninstall-shim
```

Remove the macOS GUI PATH LaunchAgent:

```sh
gfj uninstall-gui-path
```

Remove shim-owned auth for a host:

```sh
gfj auth logout git.example.com
```

Temporarily bypass Forgejo routing for a process:

```sh
FJ_SHIM_HOSTS= gh pr view
```

See [docs/rollback.md](docs/rollback.md) for PATH troubleshooting and recovery steps.
For old pipx installs moving to Rust, see
[docs/pipx-to-rust-migration.md](docs/pipx-to-rust-migration.md).

## Reference

- [Commands and output](docs/commands.md): which `gh` commands the shim routes, the flags it
  supports for pull requests and issues, how it maps Forgejo statuses to checks, and the JSON
  fields it returns.
- [Configuration](docs/configuration.md): `config.toml` and each host's keys, environment
  variables, auth storage and token lookup, private CA certificates, and the macOS GUI PATH.
- [Installation](docs/installation.md): release archives, what `bootstrap` does, manual setup,
  the doctor checks, the remote layout tools expect, upgrading and rolling back.
- [Tracing](docs/tracing.md): trace records, redaction, smoke probes and the `git` recorder.
- [Server mode in depth](docs/gateway/server-mode.md): proxy setup for nginx, Caddy and Nginx Proxy
  Manager.

## Development

Run tests with:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Run the native binaries directly from the checkout:

```sh
cargo run -p gh-forgejo-shim --bin gh-forgejo-shim -- --help
cargo run -p gh-forgejo-shim --bin gfj -- --version
```

For installed-runtime validation, build the release archive with
`scripts/package-release.sh`, unpack it into a temporary directory, and run the
`gh-forgejo-shim`, `gfj`, and managed `gh` wrapper smoke checks from there.
