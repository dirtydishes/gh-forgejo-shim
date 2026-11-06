# Unreleased

- **Server mode no longer sends host-less commands to github.com.** In a checkout of a forge's
  repository, with gh logged in to both the forge and github.com, commands such as
  `gh repo delete OWNER/REPO`, `gh repo clone OWNER/REPO`, `gh search` and `gh gist` went to
  github.com, while the wrapper's trace named the forge. They now go to the forge, and the trace
  names the host gh used. `gh gist` and other GitHub-only commands fail there; run them outside the
  checkout to reach github.com. `gh pr create` stops with an error when git would push the forge's
  commits to another host, through a remote's push URL, a `pushInsteadOf` rule or an SSH host alias.
  In a forge checkout, `gh repo create` with `--push`, or when it prompts, stops too: create the
  repository without `--push`, then push with git (#231).

The v0.2.2 notes follow.

# gh-forgejo-shim v0.2.2

- **The gateway ships as a ready-made image.** Each release publishes `fga-server` for
  linux/amd64 and linux/arm64 as `ghcr.io/dirtydishes/fga-server`, tagged with the version; the
  newest stable release also moves `latest`. Server mode no longer needs a source checkout or a
  local `docker build`.
- **Server-mode setup starts from the image.** The README and `docs/gateway/server-mode.md` list what
  you need, then the steps: copy the compose file, set two addresses and the network, start it,
  route three paths, check, log in. Upgrading means changing the image tag; rolling back means
  changing it back. The example compose file names a real release.
- **The trace docs describe redaction accurately.** Redaction hides credentials in common git and
  gh forms but not every form (#232, #233), so read a trace before you share it (#239).

The v0.2.1 notes follow.

# gh-forgejo-shim v0.2.1

A small follow-up to v0.2.0:

- **Linux downloads run on older systems.** They now need glibc 2.34 or newer, so they run on
  Ubuntu 22.04 and later, Debian 12 and later, and RHEL 9 and later. v0.2.0's Linux downloads
  needed glibc 2.39 and didn't start on those systems. macOS is unchanged.
- **The gateway keeps permission errors as errors.** It treats only Forgejo's refusal for a
  missing token scope as an empty answer; any other 401 or 403 stays an error (#219).
- **The README introduces server mode near the top** (#224).

The v0.2.0 notes follow.

# gh-forgejo-shim v0.2.0

## What's new: a rewrite around real gh

Until now the shim imitated gh by hand. It reimplemented each `gh pr` and `gh issue` command
against Forgejo's API and printed output shaped like gh's. Every flag and every field had to be
copied, and anything not copied was missing.

This release runs the real GitHub CLI instead, and translates GitHub's API into Forgejo's. gh sends
the same REST and GraphQL requests it sends to github.com; a gateway answers them from Forgejo. So
gh's own flags, output, JSON fields, prompts and exit codes come with it, because they are gh's.

The gateway runs in one of two places:

```text
client-only (the gh wrapper)              server mode (the gateway on the forge)

  your tool                                 your tool
     | gh pr view                              | gh pr view
  gh wrapper                                real gh, logged in to the forge
     | starts, for this call only              | HTTPS
  real gh --private socket--> gateway       forge.example.com /api/v3, /api/graphql
                                 |              |
                             Forgejo API     gateway --> Forgejo API
```

- **Client-only.** The `gh` wrapper on your machine starts the gateway inside its own process, on a
  private socket, points the real gh at it, and exits with gh's status. Nothing keeps running
  afterwards, and nothing changes on the forge.
- **Server mode.** The gateway, `fga-server`, runs beside Forgejo and answers on the forge's own
  hostname. Any gh logged in to the forge works with no wrapper, and so do tools that call the
  GitHub API without gh.

## What you need: gh 2.103.3 or newer

The wrapper runs the real gh, so it needs gh 2.103.3 or newer. `gfj bootstrap` and
`gfj install-shim` check the gh they find, by running `gh --version`, and stop without changing
anything when it is missing, older, or a pre-release of 2.103.3. `gfj doctor` reports the same
check.

Many distributions ship an older gh. Ubuntu's own package, for one, was 2.103.2 when this release
was made. To get a current gh:

- **Ubuntu and Debian:** add GitHub's own apt repository (`cli.github.com/packages`), as
  [gh's Linux install guide](https://github.com/cli/cli/blob/trunk/docs/install_linux.md) shows,
  then `sudo apt install gh`.
- **macOS and Linux with Homebrew:** `brew install gh`, or `brew upgrade gh`. The formula for this
  shim depends on gh, so `brew install dirtydishes/tap/gh-forgejo-shim` installs it too.
- **Anywhere else:** a release archive from
  [gh's releases page](https://github.com/cli/cli/releases).

Check with `gh --version`. A gh you built yourself may report `DEV` or a `git describe` version
that can't be compared. To go ahead with it, set `FJ_SHIM_ALLOW_UNKNOWN_GH=1`; `doctor` then
shows a warning. A gh that reports an older version is refused even then.

## What runs real gh now

In an allowlisted Forgejo checkout, these commands run the real gh through the gateway, in the
forms listed. `-R`/`--repo` works with all of them; a pull request or issue given as a URL keeps
the shim's own code.

| Command | Flags |
|---|---|
| `gh pr view`, `pr list` | all |
| `gh pr checks` | all but `--watch`, `--interval`, `--fail-fast` and `--web` |
| `gh pr create` | `--head`, `--base`, `--title`, `--body`, `--draft` |
| `gh pr comment`, `pr edit` | `--body`; `pr edit` also `--title` |
| `gh pr merge` | exactly one of `--merge`, `--squash`, `--rebase`, with `--auto`, `--match-head-commit`, `--delete-branch` |
| `gh pr ready` | `--undo` |
| `gh pr close` | `--delete-branch` |
| `gh pr reopen` | none |
| `gh pr checkout` | `--branch`, `--detach`, `--force` |
| `gh issue list` | `--state`, `--limit`, `--json`, `--jq`, `--author`, `--assignee`, `--label`, and `--mention` without `--label` |
| `gh issue view` | with `--json`, and `--jq` |
| `gh issue create` | `--title`, with `--body` or `--body-file` |
| `gh issue comment` | `--body`, `--body-file` |
| `gh run list` | `--json`, `--jq`, `--limit`, `--status`, `--branch`, `--event`, `--commit`, `--all` |
| `gh run view` | a run ID, with `--json`, `--jq` |
| `gh api` | `user`, `rate_limit`, and `graphql` in the shape ChatGPT desktop sends (one read-only `query`) |
| `gh auth token`, `gh auth status` | all |

A few forms stay on the shim's own code even within those commands: `gh pr merge --auto` with
`--delete-branch`, `issue list --search`, `issue view` without `--json`, a `--json` naming `jobs`,
`attempt`, `blockedBy`, `blocking` or `reactionGroups`, and `run view`'s text form. Every other Forgejo command (`pr status`,
`pr diff`, `repo view` and others) is also answered by the shim's own code, as before. GitHub
repositories, and commands such as `gh --version`, `gh config` and `gh auth login`, go to real gh
unchanged.

**Stepping aside to a logged-in gh.** When your gh is itself logged in to the forge, as it is in
server mode, the wrapper steps aside and runs plain gh, which talks to the forge's own gateway. It
removes `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN` and `GITHUB_ENTERPRISE_TOKEN` first, so
none of them reaches the forge, and a call naming another host never steps aside.

## Other changes

**Tokens.** For each host the shim reads, in order:

1. its stored token;
2. the variable named by the host's `token_env`;
3. `FJ_SHIM_TOKEN`, `FORGEJO_TOKEN`, `GITEA_TOKEN` and `FJ_TOKEN`, only when exactly one host is
   configured.

Normal commands no longer read `fj`, `tea` or `gitea` config files. To copy a token from one of
them, run `gfj auth import --from fj` or `--from tea` once. Without a token for the host, a
command exits 4 and says to run `gfj auth login HOST`.

**Configuration.** A `[host."NAME"]` table in `config.toml` sets `api_url`, `token_env`, `ca_file`
and `wip_prefixes` for one host. `gfj config add-host` refuses a GitHub host. `bootstrap`
allowlists a remote's web port with its host (`git.example.com:3000`) when every web URL in the
checkout for that host names the same one.

**Pull request links** use GitHub's `/{owner}/{repo}/pull/{n}` layout, and `--web` and `gh browse`
open Forgejo's `/pulls/{n}` page. The README has a redirect for the forge's reverse proxy, so that
tools opening the `/pull/` link reach the page.

**Commands:**

- `gfj auth import` takes `--from fj|tea [--hostname HOST]`; Gitea is no longer a source.
- `gfj uninstall-shim` also removes managed wrappers in `PATH` directories under `$HOME`.
- A `gh` that leads back to the shim stops with exit 127 and names `FJ_SHIM_REAL_GH`.
- Trace files are created owner-only (0600), and redaction errs toward hiding: some harmless
  values show as `<redacted>`, to keep secrets out.

## Upgrading

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
scratch home directory, against a local Forgejo 15, using a build of the previous version (0.1.2)
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

1. **Clear the generic token variables first** if you have more than one Forgejo host. The
   previous version uses `FJ_SHIM_TOKEN`, `FORGEJO_TOKEN`, `GITEA_TOKEN` and `FJ_TOKEN` before a
   host's stored token, for every host, so a token meant for one forge would go to another.
   Remove them from your shell profile, unset them in the shell you're using
   (`unset FJ_SHIM_TOKEN FORGEJO_TOKEN GITEA_TOKEN FJ_TOKEN`), and check that
   `env | grep -E '^(FJ_SHIM_TOKEN|FORGEJO_TOKEN|GITEA_TOKEN|FJ_TOKEN)='` prints nothing and that
   `gfj auth status HOST` finds a stored token for each host. Do the same in any app or service
   that sets them.
2. Install the previous version's binaries in place of these: an earlier Homebrew formula, or the
   previous archive. Config and tokens live outside the install path.
3. Recreate the managed wrapper from the rolled-back binary, because the wrapper names the
   binary by its full path, which under Homebrew includes the version:
   `gh-forgejo-shim install-shim --force --bin-dir DIR`, where `DIR` holds your managed `gh`.
   Check that the wrapper's `exec` line names the rolled-back binary.

A host configured only through a `[host."NAME"]` table loses its settings under the previous
version, which doesn't read those tables. To return to plain gh altogether, run
`gfj uninstall-shim`: it removes only the shim's own wrapper and never touches gh's own config.

## Server mode: the gateway on your forge

Server mode runs the gateway, `fga-server`, next to Forgejo and mounts the GitHub Enterprise API on
the forge's own hostname. The forge then answers `https://forge.example.com/api/v3` and
`/api/graphql`, and clients need no wrapper: a plain `gh auth login --hostname forge.example.com`
with a Forgejo token is enough.

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
- ChatGPT desktop's pull request card runs `gh pr list --head <branch> --author @me`. On a gateway
  older than this release, that search needed a Forgejo token with `read:issue`, and failed with
  "Resource not accessible by token (search)" without it. This release answers it from the pull
  list, so it works with repository scope alone. If you can't upgrade the gateway yet, give gh's
  Forgejo token issue scope. Other searches by label, keyword or assignee still need `read:issue`.
- With a token that can merge but lacks `read:issue`, ChatGPT can't see or cancel a scheduled
  auto-merge; use `gh pr merge --disable-auto` or the web UI, or give the token `read:issue`.
- Auto-merge can show as enabled after Forgejo has dropped the schedule.
- A closed or merged pull request, or one whose branch is no longer its head, checks out from its
  own ref (`refs/pull/{n}/head`).
- One gateway serves one forge.

Server mode's differences from GitHub are in `docs/gateway/MAPPING.md` and
`docs/gateway/DESIGN.md`. Known gaps that didn't block this release are the repository's issues
labelled
[`post-release`](https://github.com/dirtydishes/gh-forgejo-shim/issues?q=label%3Apost-release).

## Security fixes in this release

- Trace redaction hides credentials in common git and gh forms: URL passwords, Authorization
  headers, `-c http.extraHeader`, JSON bodies, `gh secret set --body` (also after `-R`) and gh's
  secret flags. Some forms still get through (#232, #233), so read a trace before you share it.
- Trace files, a new `config.toml` and the token file are owner-only, the directories the shim
  creates are writable only by their owner, and the installed wrapper is never group- or
  world-writable.
- `install-shim --force` never overwrites the real gh it found, and a dangling `gh` symlink needs
  `--force` like any other file.
- Discovery recognises a copy of this release's shim and doesn't take it for real gh, and
  recursion guards stop the wrapper calling itself through any other copy.
- The minimum-gh check runs `gh --version` without `GH_TOKEN`, `GITHUB_TOKEN`,
  `GH_ENTERPRISE_TOKEN` or `GITHUB_ENTERPRISE_TOKEN`, and with gh's update notices and telemetry
  off.
- A token is never sent to a GitHub host: `config add-host` and an `api_url` pointing at GitHub
  are refused.
- Stepping aside to a logged-in gh removes GitHub token variables first.
- `bootstrap` keeps a forge's web port, so the token goes to the server on that port, not the
  default one.
- In server mode, the gateway refuses any host but the forge's own with `421` before a token is
  used, stores no token, and needs a token on every API route.

## Thanks

[@pikos-apikos](https://github.com/pikos-apikos) contributed private CA support (#30):
`FJ_SHIM_CA_FILE`, which carries into this release alongside the per-host `ca_file` setting.
