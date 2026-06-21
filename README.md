# gh-forgejo-shim

`gh-forgejo-shim` is a small CLI that helps Codex.app, T3 Code, and other GitHub-oriented tools work inside Forgejo repositories. The install path is moving to native Rust binaries; legacy Python code remains in this repository during the rewrite for compatibility testing and migration.

It installs a durable management command named `gh-forgejo-shim` plus a shorter daily-use alias named `gfj`. When you opt in, it can also place a user-local `gh` wrapper in front of the real GitHub CLI. Real GitHub repositories still delegate to the real `gh`; allowlisted Forgejo repositories route a narrow set of repository, pull request, issue, and check commands through Forgejo-friendly behavior.

This is not full `gh` emulation. It is compatibility glue for the `gh` calls and repository probes that GitHub-style development tools commonly use.

## What It Covers

- Repository discovery from `-R/--repo`, `GH_REPO`, `GH_HOST`, and local Git remotes.
- Opt-in Forgejo host allowlisting so unrelated GitHub work keeps using the real GitHub CLI.
- Bootstrap setup that detects the current repo, installs the wrapper, allowlists the host, checks auth, and prints repair commands.
- macOS GUI PATH setup for apps launched from Finder, Dock, Spotlight, or other GUI launchers.
- Native Forgejo auth login, import, status, and logout commands with macOS Keychain support.
- Auth discovery from shim-owned storage, environment variables, and common `fj`, `tea`, and `gitea` config files.
- GitHub-shaped `gh repo view`, `gh pr ...`, and `gh issue ...` output for the fields tools usually probe.
- Pull request checks mapped from Forgejo commit statuses into GitHub-style `statusCheckRollup` and `gh pr checks` output.
- Safe rollback commands that remove only shim-managed files.

## Install

Install the native Rust binary with Homebrew:

```sh
brew tap dirtydishes/gh-forgejo-shim
brew install gh-forgejo-shim
gfj --version
```

Phase 09 is moving releases to a Rust-first install path. Existing pipx users
should follow [Migrate From pipx To The Rust Binary](docs/pipx-to-rust-migration.md)
when installing a Rust release. PyPI is not intended to remain the long-term
distribution channel once the Rust binary release path is active.

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
- Installs the user-local `gh` wrapper.
- Checks whether `PATH` resolves `gh` to the shim.
- Checks whether Forgejo auth can be found from shim storage, env, or supported CLI config files.
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

## Quickstart For GUI Coding Tools

1. Install the native package with Homebrew or a GitHub release tarball.
2. Open a terminal inside your Forgejo repository.
3. Run `gfj bootstrap`.
4. Run `gfj auth login HOST`, or use `gfj auth import HOST` if a token already exists in env, `fj`, `tea`, or `gitea` config.
5. Copy and run any repair commands `bootstrap` prints.
6. On macOS, run `gfj install-gui-path` if your coding tool is launched from Finder, Dock, Spotlight, Raycast, Alfred, or another GUI launcher.
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

Known GitHub hosts such as `github.com` are ignored even if they appear in the
allowlist. This keeps normal GitHub repositories delegated to the real GitHub
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

`FJ_SHIM_TRACE` is opt-in. With it unset, runtime behavior is unchanged. Trace records include timestamp, cwd, argv, route decision, host/repo, duration, exit code, stdout/stderr byte counts, relevant environment keys, and redacted excerpts when `FJ_SHIM_TRACE_BODY=1` is also set. Sensitive auth material is not logged; `gh auth token` stdout is always suppressed in trace excerpts.

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

Only supported commands in allowlisted Forgejo repositories are routed through Forgejo. Everything else delegates to the real GitHub CLI.

Supported pull request commands:

```sh
gh pr checks
gh pr checkout
gh pr co
gh pr comment
gh pr create
gh pr diff
gh pr list
gh pr new
gh pr status
gh pr view
```

Supported issue commands:

```sh
gh issue create
gh issue list
gh issue ls
gh issue new
gh issue view
```

Supported repository commands:

```sh
gh repo view
```

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

By default, `gh pr create` prints the created Forgejo pull request URL. For Codex.app compatibility, Forgejo `/pulls/7` URLs include a harmless fragment marker like `#codex-pr=/pull/7`, because Codex.app's create-PR action recognizes GitHub-style `/pull/7` URL markers when surfacing the new PR link.

`gh pr list` supports JSON output, repo selection, state, limit, head, and base filters. It currently accepts but does not apply several GitHub search-style filters, including author, app, assignee, label, and search. Real filtering for those flags is tracked as follow-up work in Beads issue `gh-forgejo-shim-9r8`.

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
    "url": "https://git.example.com/owner/repo/pulls/7"
  },
  "createdBy": [],
  "needsReview": []
}
```

## Auth

Native auth commands:

```sh
gh-forgejo-shim auth login [host]
gh-forgejo-shim auth import [host]
gh-forgejo-shim auth status [host]
gh-forgejo-shim auth logout [host]
```

`auth login` prompts for a host when needed, prompts for an access token without echoing it, validates the token with `GET /api/v1/user`, stores it in shim-owned auth storage, and adds the host to the allowlist.

`auth import` finds an existing token from supported sources, validates it, and stores it in the same shim-owned storage. This is useful when Terminal already has auth through an environment variable or another Forgejo CLI, but GUI-launched apps do not inherit that shell environment.

`auth status` reports whether shim-owned auth exists for the host without printing secrets. `auth logout` removes only shim-owned stored auth for that host.

On macOS, the shim first tries to store tokens in Keychain with the system `security` tool. If Keychain storage is unavailable or fails, it falls back to:

```text
~/.config/gh-forgejo-shim/auth.json
```

The fallback file is written with owner-only permissions.

Token environment variables are checked in this order:

```text
FJ_SHIM_TOKEN
FORGEJO_TOKEN
GITEA_TOKEN
FJ_TOKEN
```

Environment variables take precedence for the current process. If none are set, the shim checks native shim storage and then tries common `fj`, `tea`, and `gitea` config files.

On macOS, the current `fj` CLI stores auth in:

```text
~/Library/Application Support/Cyborus.forgejo-cli/keys.json
```

The shim can import from that file when present.

## Configuration

Persistent configuration lives at:

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
FJ_SHIM_REAL_FJ
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

- Whether the real `gh` can be found.
- Whether `fj` can be found.
- Whether at least one Forgejo host is allowlisted.
- Whether an auth token can be discovered.
- Whether the managed wrapper exists and is the first `gh` in `PATH`.
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

## Development

Run tests with:

```sh
cargo test --workspace
python3 -m unittest
```

When running directly from a checkout without installing the package, set `PYTHONPATH`:

```sh
PYTHONPATH=src python3 -m gh_forgejo_shim --help
```

The Rust binary is the target runtime. The Python package remains in this
repository during the rewrite for compatibility testing and migration only.
