# Release notes: the rewrite

This release replaces most of the shim's own Forgejo code with real gh. It adds a second install
path, server mode, for tools that call the GitHub API without gh. Existing config and stored
tokens carry over.

## Before you upgrade: gh 2.103.3 or newer

The `gh` wrapper now hands most Forgejo work to the real GitHub CLI, so it needs gh 2.103.3 or
newer. `gfj bootstrap` and `gfj install-shim` check the gh they find, by running `gh --version`,
and stop without changing anything when it is missing, older, or a pre-release of 2.103.3.
`gfj doctor` reports the same check.

Many distributions ship an older gh. Ubuntu's own package, for one, was 2.103.2 when this release
was made. To get a current gh:

- **Ubuntu and Debian:** add GitHub's own apt repository (`cli.github.com/packages`), as
  [gh's Linux install guide](https://github.com/cli/cli/blob/trunk/docs/install_linux.md)
  shows, then `sudo apt install gh`.
- **macOS and Linux with Homebrew:** `brew install gh`, or `brew upgrade gh`. The Homebrew formula
  for this shim depends on gh, so `brew install dirtydishes/tap/gh-forgejo-shim` installs it too.
- **Anywhere else:** a release archive from
  [gh's releases page](https://github.com/cli/cli/releases).

Check with `gh --version`. A gh you built yourself may report `DEV` or a `git describe` version
that can't be compared. To go ahead with it, set `FJ_SHIM_ALLOW_UNKNOWN_GH=1`; `doctor` then
shows a warning. A gh that reports an older version is refused even then.

## Client-only: what changed

**Forgejo commands run real gh.** In an allowlisted Forgejo checkout, the commands that coding
tools use run the real gh. The wrapper starts a gateway inside its own process, on a private
socket, and points gh at it. Nothing runs in the background afterwards. Those commands are:

- `gh pr view`, `checks`, `list`, `create`, `comment`, `edit`, `merge`, `ready`, `close`, `reopen`
  and `checkout`;
- `gh issue list`, `view`, `create` and `comment`;
- `gh run list` and `view`;
- `gh api`, `gh auth token` and `gh auth status`.

Each runs real gh only with the flags the gateway serves. With any other flag, and for every
other Forgejo command (`pr status`, `pr diff`, `repo view` and others), the shim's built-in
code answers, as before. GitHub repositories, and commands such as `gh --version`, `gh config`
and `gh auth login`, go to real gh unchanged.

**Tokens.** For each host the shim reads, in order:

1. its stored token;
2. the variable named by the host's `token_env`;
3. `FJ_SHIM_TOKEN`, `FORGEJO_TOKEN`, `GITEA_TOKEN` and `FJ_TOKEN`, only when exactly one host is
   configured.

Normal commands no longer read `fj`, `tea` or `gitea` config files. To copy a token from one of
them, run `gfj auth import --from fj` or `--from tea` once. Without a token for the host, a
command exits 4 and says to run `gfj auth login HOST`.

**Configuration.** A `[host."NAME"]` table in `config.toml` sets `api_url`, `token_env`, `ca_file`
and `wip_prefixes` for one host. `gfj config add-host` refuses a GitHub host, and `bootstrap`
allowlists a remote's web port with its host (`git.example.com:3000`).

**Pull request links** use GitHub's `/{owner}/{repo}/pull/{n}` layout, and `--web` and
`gh browse` open Forgejo's `/pulls/{n}` page. The README has a redirect for the forge's reverse
proxy, so that tools opening the `/pull/` link reach the page.

**Traces.** Trace files are created owner-only (0600). Redaction errs toward hiding: some harmless
values show as `<redacted>`, so that a secret never shows. `gfj trace smoke` redacts the stderr it
reports.

**Other changes:**

- `gfj auth import` takes `--from fj|tea [--hostname HOST]`, and Gitea is no longer a source.
- `gfj uninstall-shim` also removes managed wrappers in `PATH` directories under `$HOME`.
- A `gh` that leads back to the shim stops with exit 127 and names `FJ_SHIM_REAL_GH`.

## Server mode: the gateway on your forge

This release adds a second install path. Server mode runs the gateway, `fga-server`, next to
Forgejo and mounts the GitHub Enterprise API on the forge's own hostname. The forge then answers
`https://forge.example.com/api/v3` and `/api/graphql`, and clients need no wrapper: a plain
`gh auth login --hostname forge.example.com` with a Forgejo token is enough.

**Who needs it.** Use server mode if you use Claude Code or Claude desktop. Their pull request
status calls the forge's GitHub API itself, outside any `gh` process, so only a gateway on the
forge can answer it. The client-only wrapper still covers agents that run `gh`, including
Claude's agent in a shell. You can run both. Server mode needs control of the forge's reverse
proxy, so a hosted forge such as Codeberg can't use it.

**What it serves.** The `gh` commands that coding tools use against pull requests, issues, checks
and Actions runs, through gh 2.103.3's own REST and GraphQL requests: `pr view`, `list`, `status`,
`checks`, `diff`, `checkout`, `create`, `edit`, `comment`, `ready`, `close`, `reopen` and `merge`
(including `--auto`); `issue list`, `view`, `create` and `comment`; `run list` (including
`--workflow`) and `run view`. It also serves Claude's REST lookups for a branch's pull request,
and the read documents ChatGPT desktop's pull request view sends, and its merge; the review pane's
own mutations (review threads, pending reviews) aren't served yet.

**Pull request links.** In server mode the gateway redirects `/{owner}/{repo}/pull/{n}` links to
Forgejo's `/pulls/{n}` page.

**Setting it up.** Build the image from the release's checkout, start it with the example compose
file (`packaging/gateway/compose.yaml`), and route `/api/v3/`, `/api/graphql` and
`/{owner}/{repo}/pull/{n}` to it in the forge's own site block. The repository ships an nginx
snippet, which also works in Nginx Proxy Manager, and a Caddy site block. See the README's
"Server Mode" section and `docs/gateway/server-mode.md`.

**Upgrading and rolling back.** The gateway keeps no state. Tag each image with its release and
keep the previous image and compose file: an upgrade switches to the new tag, and a rollback
switches back.

**Security.** The gateway forwards each client's own Forgejo token and stores none. It answers
only the forge's own host and port, and refuses any other host with `421` before a token is used,
so a GitHub or second forge's token never reaches Forgejo. Every API route needs a token.

## Upgrading the client

The binary names, the config file (`~/.config/gh-forgejo-shim/config.toml`) and stored tokens
(Keychain, or `~/.config/gh-forgejo-shim/auth.json`) stay where they were, and the managed `gh`
wrapper keeps working.

1. Make sure `gh --version` reports 2.103.3 or newer (above).
2. Upgrade the shim: `brew upgrade gh-forgejo-shim`, or unpack the new release archive in place of
   the old binaries.
3. From inside your Forgejo checkout, run `gfj bootstrap`, then `gfj doctor`.
4. If you relied on a token in an `fj`, `tea` or `gitea` config file, or on a generic token
   variable with more than one host configured, run `gfj auth login HOST`, or
   `gfj auth import --from fj|tea` once.

**Tested.** Before this release, the upgrade and the rollback were rehearsed on macOS in a
scratch home directory, against a local Forgejo 15, using a build of the previous release (0.1.2)
and of this one, with gh 2.103.3:

- The previous build installed the wrapper, listed the host and found its stored token.
- Copying this release's binaries over it left `config.toml` and the token file byte for byte
  unchanged. The wrapper kept working, `gfj bootstrap` kept the token, and `gfj doctor` passed
  the gh version check.
- `gh pr list`, `gh pr view`, `gh issue list` and `gh api user` then worked through the wrapper.
  That Forgejo served plain HTTP, so it needed a `[host]` table with an `api_url`.
- Putting the previous binaries back left the token file unchanged. The previous build still
  listed the host and found the token, ignoring the `[host]` table, and the wrapper kept working.

## Rolling back

Install the previous release's binaries in place of these; config and tokens live outside the
install path. A host configured only through a `[host."NAME"]` table loses its settings under the
previous release, which doesn't read those tables. To return to plain gh altogether, run
`gfj uninstall-shim`: it removes only the shim's own wrapper and never touches gh's own config.

## Known limits

**Client-only, in a Forgejo checkout:**

- `gh auth status` without `--hostname` shows your other hosts, github.com included, as failing.
- Changes gh makes to its own config during a routed call, such as an alias that runs
  `gh config set`, are lost.
- Installed gh extensions run through plain gh.
- Git started by a child of gh, such as an alias's shell or an editor, gets credentials only for
  the forge.
- `gh pr checkout` can't authenticate private GitHub submodules, may fetch from a remote named
  `github`, and with `--force` runs `git reset --hard`. A pull request made with AGit, or whose
  branch was deleted, checks out from its own ref.
- `pr ready` fails when `wip_prefixes` doesn't match the forge's own draft prefixes.
- Each gateway request has about 10 seconds, or about 20 when it must also authenticate.
- In client-only mode, ChatGPT's "Open pull request in browser" reaches a 404 unless the forge
  redirects `/pull/` links (see the README).

**Both modes:**

- Forgejo answers "still checking" and "conflicts" the same way, so a conflicting merge takes
  about 5 seconds to report.
- Actions runs have no jobs, logs or artifacts through Forgejo 15's API, and every run is
  attempt 1.
- `gh api rate_limit` answers with fixed numbers.
- Auto-merge can show as enabled after Forgejo has dropped the schedule.
- A closed or merged pull request, or one whose branch is no longer its head, checks out from its
  own ref (`refs/pull/{n}/head`).
- One gateway serves one forge.

Server mode's differences from GitHub are in `docs/gateway/MAPPING.md` and
`docs/gateway/DESIGN.md`.

## Follow-up work

Known gaps that didn't block this release are tracked in the repository's issues labelled
[`post-release`](https://github.com/dirtydishes/gh-forgejo-shim/issues?q=label%3Apost-release).
