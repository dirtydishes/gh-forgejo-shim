# gh-forgejo-shim

> **Early beta.** Version 0.2 rebuilt the shim around a new gateway. Everyday pull request, issue
> and check commands work, but gaps remain: some `gh` commands and fields still fail, and a few app
> features work only with GitHub itself. We're closing these gaps release by release.
> [Known gaps](#known-gaps) lists what doesn't work yet. If something breaks, please
> [open an issue](https://github.com/dirtydishes/gh-forgejo-shim/issues/new). Say which app you
> were using and which action you were attempting, and paste the output of `gfj doctor`. To show us
> the exact commands, turn on [tracing](#codex-probe-diagnostics) and attach the trace, but read it
> first: redaction hides most credentials, not every form. [Rollback](#rollback) removes only the
> shim's own files.

`gh-forgejo-shim` is a native Rust CLI that helps Codex.app and other GitHub-oriented tools work inside Forgejo repositories without putting Python in the installed `gh`, `gfj`, or `gh-forgejo-shim` runtime path.

It installs a durable management command named `gh-forgejo-shim` plus a shorter daily-use alias named `gfj`. When you opt in, it can also place a user-local `gh` wrapper in front of the real GitHub CLI. Real GitHub repositories still delegate to the real `gh`. In allowlisted Forgejo repositories, the pull request, issue, run and API commands that coding tools use run the real `gh` too, against a gateway the wrapper starts for the call; the shim's own code answers the rest of a narrow set of repository, pull request, issue and check commands. The wrapper needs `gh` 2.103.3 or newer ([Requirements](#requirements)).

This is not full `gh` emulation. It is compatibility glue for the `gh` calls and repository probes that GitHub-style development tools commonly use.

## Two Ways To Run It

- **Client-only:** the `gh` wrapper on each machine. Each `gh` call in a Forgejo repository starts a
  private gateway for that call alone, and nothing changes on the forge. It covers Codex and any
  agent or tool that runs `gh`.
- **Server mode (new in 0.2.0):** the gateway, `fga-server`, runs beside Forgejo on the forge's own
  hostname, so the forge itself answers GitHub's API. Any `gh` logged in to the forge works with no
  wrapper, and so do tools that call the API without `gh`. **Use it if you use Claude Code or Claude
  desktop:** they look up a branch's pull request themselves, outside `gh`, and only a gateway on the
  forge can answer. You need control of the forge's reverse proxy, so a hosted forge such as Codeberg
  can't use it.

You can use both. [Server Mode](#server-mode), further down, has the setup.

## What It Covers

- Repository discovery from `-R/--repo`, `GH_REPO`, `GH_HOST`, and local Git remotes.
- Opt-in Forgejo host allowlisting so unrelated GitHub work keeps using the real GitHub CLI.
- Bootstrap setup that detects the current repo, installs the wrapper, allowlists the host, checks auth, and prints repair commands.
- macOS GUI PATH setup for apps launched from Finder, Dock, Spotlight, or other GUI launchers.
- Native Forgejo auth login, import, status, and logout commands with macOS Keychain support.
- Auth from shim-owned storage, a host's `token_env`, or generic token variables, with a one-time import from `fj` or `tea`.
- GitHub-shaped `gh repo view`, `gh pr ...`, and `gh issue ...` output for the fields tools usually probe.
- Pull request checks mapped from Forgejo commit statuses into GitHub-style `statusCheckRollup` and `gh pr checks` output.
- Safe rollback commands that remove only shim-managed files.

## Known gaps

- `gh run view --log` and `--log-failed` fail with a 404: the gateway doesn't serve job logs yet
  ([#161](https://github.com/dirtydishes/gh-forgejo-shim/issues/161)).
- Some `gh pr view --json` and `gh repo view --json` fields, and some `gh api` routes, aren't served
  by the gateway yet
  ([#146](https://github.com/dirtydishes/gh-forgejo-shim/issues/146),
  [#189](https://github.com/dirtydishes/gh-forgejo-shim/issues/189)).
- ChatGPT desktop's pull request overview pane needs a GitHub account connected to ChatGPT and
  can't open a Forgejo pull request. That's how ChatGPT is built, not a shim bug. The sidebar
  status, the pull request menu, review comments and the agent's `gh` commands work.
- In a Forgejo checkout, `gh pr create` checks where git will push before it runs. The check is best
  effort: a few unusual git or SSH setups can still send the push to another host
  ([#255](https://github.com/dirtydishes/gh-forgejo-shim/issues/255)).

## Install

### Requirements

The `gh` wrapper runs the real GitHub CLI, so it needs `gh` 2.103.3 or newer. `gfj bootstrap` and `gfj install-shim` stop without changing anything when the `gh` they find is missing or older, and `gfj doctor` reports the same check. Many distributions ship an older `gh`; on Ubuntu and Debian, add GitHub's own apt repository as [gh's Linux install guide](https://github.com/cli/cli/blob/trunk/docs/install_linux.md) shows, or use Homebrew. A `gh` you built yourself that reports `DEV` needs `FJ_SHIM_ALLOW_UNKNOWN_GH=1`. See [docs/installation.md](docs/installation.md#requirements).

### Install paths

The steps below set up the client-only wrapper. For server mode, see
[Server Mode](#server-mode).

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

From inside a Forgejo checkout, run:

```sh
gfj bootstrap
```

`bootstrap` does the practical setup work and tells you exactly what still needs attention. It:

- Detects the current Forgejo repository from Git remotes.
- Adds the repository host to the shim allowlist.
- Installs the managed `gh` wrapper into the first safe writable directory already visible to the current process, falling back to `~/.local/bin` when needed.
- Checks the current process `PATH`, the real GitHub CLI path, and whether managed `gh` is first.
- On macOS, updates the launchd GUI PATH for newly opened GUI apps unless `--no-gui-path` is passed.
- Checks the real GitHub CLI's version (2.103.3 or newer) before it writes anything.
- Checks whether Forgejo auth can be found from shim storage or token variables.
- Checks `origin`, `origin/HEAD`, and current-branch upstream tracking.
- Prints repair commands for anything it cannot fix automatically.

The long command name works the same way:

```sh
gh-forgejo-shim bootstrap
```

If the wrapper path already contains an unrelated `gh`, bootstrap refuses to overwrite it unless you opt in:

```sh
gfj bootstrap --force
```

To force the older user-local target, preview actions, or skip macOS GUI PATH repair:

```sh
gfj bootstrap --target user-local
gfj bootstrap --dry-run
gfj bootstrap --no-gui-path
```

When bootstrap changes the macOS GUI PATH, restart already-running GUI apps before expecting them to inherit it.

## Quickstart For GUI Coding Tools

1. Install the native package with Homebrew or a GitHub release tarball.
2. Open a terminal inside your Forgejo repository.
3. Run `gfj bootstrap`.
4. Run `gfj auth login HOST`, or use `gfj auth import --from fj|tea --hostname HOST` to copy a token from that tool's config.
5. Copy and run any repair commands `bootstrap` prints.
6. On macOS, restart the GUI tool if `bootstrap` reports that it changed the GUI PATH.
7. Confirm the setup:

```sh
gfj doctor
```

8. Restart the GUI tool, open the Forgejo repository, and use the normal repository, branch, commit, push, pull request, issue, and check workflows.

For scripted setup or documentation, use `gh-forgejo-shim`. For day-to-day typing, `gfj` is the same command with a shorter name.

## Manual Setup

If you prefer not to use `bootstrap`, allowlist at least one Forgejo host:

```sh
gh-forgejo-shim config add-host git.example.com
```

Known GitHub hosts are ignored even if they appear in the allowlist: `github.com`,
`ghe.com`, and every name under them, such as `api.github.com` or `acme.ghe.com`.
This keeps normal GitHub repositories delegated to the real GitHub
CLI if `bootstrap` is accidentally run from a GitHub checkout or an old config
contains `github.com`.

Install the opt-in wrapper:

```sh
gh-forgejo-shim install-shim
```

The wrapper is written to:

```text
~/.local/bin/gh
```

Make sure `~/.local/bin` appears before the real `gh` location in `PATH`. You can confirm with:

```sh
command -v gh
which -a gh
```

If you need a different install location:

```sh
gh-forgejo-shim install-shim --bin-dir ~/.local/bin
```

## macOS GUI PATH

macOS GUI apps often start with a small launchd PATH such as:

```text
/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin
```

That PATH may miss both `~/.local/bin/gh` and Homebrew tools such as `/opt/homebrew/bin/gh`. If Codex.app or another GUI-launched tool says `GitHub CLI (gh) is not installed` even though `gh --version` works in your shell, persist a GUI-friendly PATH:

```sh
gfj install-gui-path
```

This writes:

```text
~/Library/LaunchAgents/com.gh-forgejo-shim.user-gui-path.plist
```

It also applies the PATH to the current user launchd session for newly opened GUI apps. Restart existing GUI apps after running it.

To provide an exact PATH:

```sh
gfj install-gui-path --path "$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
```

To only write the LaunchAgent and wait until the next login:

```sh
gfj install-gui-path --no-apply
```

## Repository Remote Shape

GitHub-style tools usually inspect the local Git checkout before they invoke `gh`. The shim can answer Forgejo-backed `gh` commands, but tools may still show incomplete repository state, disabled commit or push controls, or unavailable pull request status if the checkout does not look like a conventional GitHub clone.

For best compatibility, each Forgejo checkout should have:

```text
origin.url      https://git.example.com/owner/repo.git
origin.pushurl  git@ssh.example.com:owner/repo.git
origin/HEAD     refs/remotes/origin/main
branch upstream origin/current-branch
```

If your checkout already has a Forgejo remote under another name, keep it and add `origin` as a compatibility alias:

```sh
git remote add origin https://git.example.com/owner/repo.git
git remote set-url --push origin git@ssh.example.com:owner/repo.git
git fetch origin
git remote set-head origin -a
git branch --set-upstream-to=origin/your-branch your-branch
```

If the server does not report the default branch clearly, set it explicitly:

```sh
git remote set-head origin main
```

Useful checks:

```sh
git config --get remote.origin.url
git symbolic-ref --quiet refs/remotes/origin/HEAD
git rev-parse --abbrev-ref --symbolic-full-name @{u}
git status --short --branch
gh repo view --json nameWithOwner,url,defaultBranchRef,sshUrl
gh pr status --json number,title,url,headRefName,state
gh issue list --json number,title,state,url
```

If your tool connects to a remote SSH workspace, apply the same remote setup inside that remote checkout too. Fixing the local Mac clone does not change a separate remote clone.

## Codex Probe Diagnostics

When Codex.app reports `GitHub CLI unavailable`, `Pull request status unavailable`, or stale branch data, enable tracing before guessing at the fix. The shim can record the exact `gh` commands it handles or delegates:

```sh
export FJ_SHIM_TRACE="$PWD/codex-probe-trace.jsonl"
export FJ_SHIM_TRACE_BODY=1
```

`FJ_SHIM_TRACE` is opt-in. With it unset, runtime behavior is unchanged. Trace records include timestamp, cwd, argv, route decision, host/repo, duration, exit code, stdout/stderr byte counts, relevant environment keys, and redacted excerpts when `FJ_SHIM_TRACE_BODY=1` is also set. Redaction hides credentials in common forms but not every form (see [#232](https://github.com/dirtydishes/gh-forgejo-shim/issues/232) and [#233](https://github.com/dirtydishes/gh-forgejo-shim/issues/233)), so read a trace before you share it; `gh auth token` stdout is always suppressed in trace excerpts. A new trace file is created owner-only (0600), and an existing one is narrowed to 0600 before records are added. If the file belongs to another user, so it can't be narrowed, no records are written to it.

Redaction errs toward hiding. Any value under a key with `password`, `secret`, `token`, `credential`, `api_key`, `apikey` or `private_key` in its name is hidden (brackets count as part of a name, as in `config[secret]`), and so is any key with `pass` anywhere in it, unless the key part is exactly one of a few ordinary words such as `bypass`, `compass` or `lowpass`. Outside JSON, a hidden value takes the rest of its line with it, and runs past the line's end while a quote is still open. In JSON, a quoted value ends at its closing quote when a comma, a bracket or the line's end follows, and an array or object runs to its closing bracket, across lines. In the gh trace, a flag with one of those words in its name hides its value, and so do `--body` and `-b` for `gh secret`. URL passwords are hidden too, and so are tokens that start with a known prefix (`ghp_`, `github_pat_`, `glpat-`, `gitea_` and others). The cost: some harmless text shows as `<redacted>` (`[REDACTED]` in git traces), such as `byPassEnabled=true`, or `status=200` after `password=x` on the same line. A hidden value costs a little readability; a leaked one costs a credential.

Summarize a trace file:

```sh
gfj trace summarize codex-probe-trace.jsonl
```

Run the local Codex-style smoke probes directly:

```sh
gfj trace smoke
```

The smoke command runs the observed read-only probe family: `gh --version`, `gh auth status`, `gh api user`, PR status/list JSON checks, and the local Git repo/default/upstream/current-branch/ref checks that branch and PR UI usually depend on.

Some Codex branch UI checks use raw `git`, not `gh`. To capture those calls for one launch session, create a temporary `git` recorder wrapper and use the printed launch environment for that Codex session:

```sh
gfj trace git-recorder create "$PWD/codex-probe-trace.jsonl"
```

Remove it when the session is done:

```sh
gfj trace git-recorder remove /path/to/printed/wrapper-dir
```

The recorder wrapper is temporary, prepends one directory to `PATH`, forwards real `git` stdout/stderr, and appends JSONL records to the same trace file.

## Routed Commands

Only commands in allowlisted Forgejo repositories are routed. Everything else delegates to the real GitHub CLI unchanged.

These run the real `gh`, against a gateway the wrapper starts for the call, with the flags the gateway serves:

```sh
gh pr view | checks | list | create | comment | edit | merge | ready | close | reopen | checkout
gh issue list | view | create | comment
gh run list | view
gh api
gh auth token | status
```

With any other flag, those commands, and these, are answered by the shim's own code:

```sh
gh pr diff | status
gh repo view
```

gh's short forms (`pr co`, `pr new`, `issue ls`, `issue new`) are routed too.

Unsupported commands and unsupported flags fail explicitly when routed, or delegate to the real `gh` when they are outside the shimmed Forgejo surface.

## Pull Request Support

`gh pr create` and `gh pr new` support the common flags coding tools tend to use:

```text
--title/-t
--body/-b
--body-file/-F
--base/-B
--head/-H
--repo/-R
--json
--fill
--fill-first
--fill-verbose
--web/-w
--draft/-d
```

GitHub-only metadata flags such as reviewers, labels, assignees, projects, milestones, templates, recover, and maintainer-edit controls fail with a Forgejo-specific error.

By default, `gh pr create` prints the created pull request's URL.

### Pull request links

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

`gh pr list` runs the real `gh`, and the gateway applies its filters, including state, limit, head, base, author, assignee and label. `--search` takes GitHub's query syntax, and qualifiers that Forgejo has no equivalent for aren't applied.

`gh pr view` and `gh pr status` return successfully when the current branch has no open PR. For JSON output, `gh pr view --json ...` returns `{}`, and `gh pr status --json ...` returns the GitHub CLI status envelope with `"currentBranch": null`.

`gh pr diff` supports pull number or branch resolution, `--repo`, `--web`, `--name-only`, `--patch`, `--color`, and `--exclude`.

`gh pr comment` supports pull number or branch resolution, `--repo`, `--body`, `--body-file`, and `--web`.

`gh pr checkout` supports pull number or branch resolution, `--repo`, `--branch`, `--detach`, `--force`, and `--recurse-submodules`.

## Checks And Statuses

When `statusCheckRollup` is requested, the shim fetches Forgejo commit statuses for the pull request head SHA and maps them into GitHub-style status context objects.

The same Forgejo status data powers:

```sh
gh pr checks
gh pr checks --json bucket,completedAt,conclusion,description,detailsUrl,link,name,startedAt,state,workflow
```

Forgejo status states are normalized into `pass`, `fail`, or `pending` buckets. When Forgejo returns repeated updates for the same context, the latest status per context is shown.

## Issue Support

`gh issue create` and `gh issue new` support:

```text
--title/-t
--body/-b
--body-file/-F
--repo/-R
--web/-w
--assignee/-a
--label/-l
--milestone/-m
```

Labels are resolved to Forgejo label IDs before issue creation. Milestones are passed through as numeric IDs.

`gh issue list` supports:

```text
--json
--repo/-R
--jq/-q
--template/-t
--state/-s
--limit/-L
--label/-l
--search/-S
--author/-A
--assignee/-a
--mention
--milestone/-m
--web/-w
```

`gh issue view` supports issue numbers or URLs, JSON output, repo selection, `--jq`, `--template`, `--comments`, and `--web`.

## JSON Output

For `--json`, the shim emits GitHub-shaped subsets when Forgejo data is available.

Pull request fields:

```text
additions, deletions, number, title, state, isDraft, url,
headRefName, baseRefName, author, createdAt, updatedAt,
mergeable, mergeStateStatus, reviewDecision, reviewRequests,
statusCheckRollup
```

Check fields:

```text
bucket, completedAt, conclusion, description, detailsUrl, link,
name, startedAt, state, workflow
```

Issue fields:

```text
assignees, author, body, closed, closedAt,
closedByPullRequestsReferences, comments, createdAt, id, isPinned,
labels, milestone, number, projectCards, projectItems, reactionGroups,
state, stateReason, title, updatedAt, url
```

Repository fields:

```text
archivedAt, assignableUsers, codeOfConduct, contactLinks, createdAt,
defaultBranchRef, deleteBranchOnMerge, description, diskUsage, forkCount,
fundingLinks, hasDiscussionsEnabled, hasIssuesEnabled, hasProjectsEnabled,
hasWikiEnabled, homepageUrl, id, isArchived, isBlankIssuesEnabled,
isEmpty, isFork, isInOrganization, isMirror, isPrivate,
isSecurityPolicyEnabled, isTemplate, isUserConfigurationRepository,
issueTemplates, issues, labels, languages, latestRelease, licenseInfo,
mentionableUsers, mergeCommitAllowed, milestones, mirrorUrl, name,
nameWithOwner, openGraphImageUrl, owner, parent, primaryLanguage,
projects, projectsV2, pullRequestTemplates, pullRequests, pushedAt,
rebaseMergeAllowed, repositoryTopics, securityPolicyUrl,
squashMergeAllowed, sshUrl, stargazerCount, templateRepository,
updatedAt, url, usesCustomOpenGraphImage, viewerCanAdminister,
viewerDefaultCommitEmail, viewerDefaultMergeMethod, viewerHasStarred,
viewerPermission, viewerPossibleCommitEmails, viewerSubscription,
visibility, watchers
```

`gh pr status --json ...` follows the GitHub CLI status envelope:

```json
{
  "currentBranch": {
    "number": 7,
    "title": "Add Forgejo support",
    "url": "https://git.example.com/owner/repo/pull/7"
  },
  "createdBy": [],
  "needsReview": []
}
```

## Auth

Native auth commands:

```sh
gh-forgejo-shim auth login [host]
gh-forgejo-shim auth import --from fj|tea [--hostname HOST]
gh-forgejo-shim auth status [host]
gh-forgejo-shim auth logout [host]
```

`auth login` prompts for a host when needed, prompts for an access token without echoing it, validates the token with `GET /api/v1/user`, stores it in shim-owned auth storage, and adds the host to the allowlist.

`auth import --from fj` reads forgejo-cli's `keys.json`; `--from tea` reads tea's YAML config. It copies the requested host's token into shim storage and adds the host to the allowlist, without contacting the server. Omit `--hostname` only when the file has one entry. Missing or ambiguous entries cause an error without a write. The command prints the host and source path, never the token. Imports do not refresh OAuth tokens or read another tool's Keychain entries.

`auth status` reports whether shim-owned auth exists for the host without printing secrets. `auth logout` removes only shim-owned stored auth for that host.

On macOS, the shim first tries to store tokens in Keychain with the system `security` tool. If Keychain storage is unavailable or fails, it falls back to:

```text
~/.config/gh-forgejo-shim/auth.json
```

The fallback file is written with owner-only permissions.

For each host, the shim first checks its stored token, then the variable named by that host's `token_env`. If exactly one Forgejo host is configured, it also checks these generic variables in order:

```text
FJ_SHIM_TOKEN
FORGEJO_TOKEN
GITEA_TOKEN
FJ_TOKEN
```

Empty variables count as unset. With multiple hosts, generic variables never supply a token. Normal commands never read other tools' credential files; use the explicit import command once to copy a token.

On macOS, the current `fj` CLI stores auth in:

```text
~/Library/Application Support/forgejo-cli.forgejo-cli/keys.json
```

Import also accepts fj's older `Cyborus.forgejo-cli/keys.json` location. On Linux, fj uses `${XDG_DATA_HOME:-~/.local/share}/forgejo-cli/keys.json`. Tea uses `${XDG_CONFIG_HOME}/tea/config.yml`, defaulting to `~/Library/Application Support/tea/config.yml` on macOS or `~/.config/tea/config.yml` on Linux, with `~/.tea/tea.yml` as its legacy fallback.

## Configuration

Persistent configuration lives at:

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

The optional `[host."<hostname>"]` tables also configure hosts without an array entry. The singular `host` lets tables coexist with existing `hosts = [...]` files, including multi-line arrays. `api_url` defaults to `https://<hostname>` and accepts HTTP for tests or LAN servers; API requests append `/api/v1/...`. Omit `ca_file` unless the host needs an extra root CA. Its certificates and `FJ_SHIM_CA_FILE` both add to the normal trust store. `wip_prefixes` lists the title prefixes the forge marks drafts with, and should match the forge's own WIP setting: commands that launch real gh strip them from a draft's title, and the first one marks a new draft. It defaults to `["WIP:", "[WIP]"]`; the list can't be empty, and a prefix can't contain a comma.

Config commands:

```sh
gh-forgejo-shim config add-host git.example.com
gh-forgejo-shim config remove-host git.example.com
gh-forgejo-shim config list
```

Environment overrides:

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

`FJ_SHIM_HOSTS` replaces the configured host list for the current process:

```sh
FJ_SHIM_HOSTS=git.example.com,code.example.org gh pr view
```

For private CA setup with `FJ_SHIM_CA_FILE`, see
[Private CA Certificates](docs/configuration.md#private-ca-certificates).

`FJ_SHIM_REAL_GH` and `FJ_SHIM_REAL_FJ` override configured executable paths:

```sh
FJ_SHIM_REAL_GH=/opt/homebrew/bin/gh
FJ_SHIM_REAL_FJ=/opt/homebrew/bin/fj
```

When no explicit executable path is configured, the shim searches the inherited `PATH` first and then checks common user and package-manager directories, including `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, and `/opt/local/bin`.

See [docs/configuration.md](docs/configuration.md) for more detail.

## Doctor

Run:

```sh
gfj doctor
```

`doctor` checks:

- Whether the current process `PATH` can resolve bare `gh`.
- Whether the real GitHub CLI can be found from config, env, current `PATH`, or common install dirs, and whether it is 2.103.3 or newer.
- Whether managed `gh` is installed first and can delegate to real `gh`.
- Whether duplicate managed wrappers are present.
- Whether `bd` can be found for repo workflows. This is diagnostic only; `bd` is not part of the GitHub CLI shim.
- Whether `fj` can be found for legacy auth-import diagnostics. Missing `fj` is not a GitHub CLI readiness failure.
- Whether at least one Forgejo host is allowlisted.
- Whether an auth token can be discovered.
- On macOS, whether the user launchd PATH can expose the shim to new GUI apps.

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

## Server Mode

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
   **Settings → Applications** with `read:user`, `write:repository`, `write:issue` and
   `read:organization`, run `gh auth login --hostname forge.example.com`, and paste it. Claude and
   ChatGPT desktop take their token from `gh`.

For full ChatGPT and Claude PR views (comments, labels), give the token `read:issue`; without it the
gateway shows PRs and hides what Forgejo won't let the token read. A token that can merge but lacks
`read:issue` can't see or cancel a scheduled auto-merge from ChatGPT; use
`gh pr merge --disable-auto` or the web UI, or give the token `read:issue`.

### Upgrade and roll back

The gateway keeps no state. To upgrade, change the image tag in `compose.yaml` to the new release and
run `docker compose up -d` in the folder that holds your compose file. To roll back, change the tag
back and run it again.

To build the image yourself instead, run
`docker build -f packaging/gateway/Dockerfile -t fga-server:<version> .` in a checkout of that release,
and set `image: fga-server:<version>`.

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
