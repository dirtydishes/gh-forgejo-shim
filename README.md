# gh-forgejo-shim

`gh-forgejo-shim` is a small, stdlib-only Python CLI for Codex.app users who work in Forgejo repositories.

It installs a durable management command named `gh-forgejo-shim`. When you opt in, it can also place a user-local `gh` wrapper in front of the real GitHub CLI. Real GitHub repositories still go to the real `gh`; allowlisted Forgejo repositories route a narrow set of pull request commands through Forgejo-friendly behavior.

V1 is not full `gh` emulation. It exists to keep Codex.app from treating Forgejo repositories like broken GitHub repositories.

## Install

```sh
pipx install gh-forgejo-shim
```

Then add at least one Forgejo host:

```sh
gh-forgejo-shim config add-host git.example.com
```

Install the opt-in wrapper:

```sh
gh-forgejo-shim install-shim
```

The wrapper is written to `~/.local/bin/gh` by default. Make sure `~/.local/bin` appears before the real `gh` location in `PATH`.

On macOS, GUI apps launched from Finder or Dock can inherit a very small `PATH` such as `/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin`. If Codex.app says `GitHub CLI (gh) is not installed` even though `gh --version` works in your shell, persist a GUI-friendly PATH and then restart Codex.app:

```sh
gh-forgejo-shim install-gui-path
```

This writes a LaunchAgent that places `~/.local/bin`, Homebrew, MacPorts, and system executable directories in the user launchd environment. It also applies the PATH to the current login session for newly opened GUI apps.

## Quickstart For Codex.app

1. Install the package with `pipx`.
2. Add each Forgejo host explicitly.
3. Install the shim.
4. On macOS, run `gh-forgejo-shim install-gui-path` if Codex.app was launched from Finder, Dock, Spotlight, or another GUI launcher.
5. Make sure the Forgejo repository has an `origin` remote, fetched `origin/*` refs, and an `origin/HEAD` default branch pointer. Codex.app, T3 Code, and other GitHub-style tools often probe conventional remote names before they ask `gh` for PR status.
6. Confirm the setup:

```sh
gh-forgejo-shim doctor
```

7. Restart Codex.app, open a Forgejo repository, and use the normal PR workflow.

## Repository Remote Shape For GitHub-Style Tools

Codex.app, T3 Code, and similar GitHub-oriented tools usually expect the local repository to look like a conventional GitHub checkout. Even when the shim can create and read Forgejo pull requests, those tools may still show unavailable pull request status if the repository only has a remote named `forgejo`, or if `origin/HEAD` has not been populated.

For the best compatibility, each Forgejo checkout should have:

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

Replace `git.example.com/owner/repo.git` and `your-branch` with the repository and branch you are using. If the default branch is not discoverable from the server, set it explicitly:

```sh
git remote set-head origin main
```

You can verify the local shape these tools commonly probe with:

```sh
git config --get remote.origin.url
git symbolic-ref --quiet refs/remotes/origin/HEAD
git rev-parse --abbrev-ref --symbolic-full-name @{u}
gh pr status --json number,title,url,headRefName,state
```

If your tool is connected to a remote SSH workspace, apply the same remote setup inside that remote checkout too. Fixing the local Mac checkout does not change a separate remote clone.

## Supported Wrapper Commands

Only these commands are routed for allowlisted Forgejo repositories:

```sh
gh pr create
gh pr list
gh pr new
gh pr status
gh pr view
gh repo view
```

Everything else delegates to the real GitHub CLI.

## Supported `pr create` Flags

The shim translates the common create flags Codex.app is likely to use:

```text
--title/-t
--body/-b
--body-file/-F
--base/-B
--head/-H
--repo/-R
--fill
--fill-first
--fill-verbose
--web/-w
--draft/-d
```

GitHub-only metadata flags, such as reviewers, labels, assignees, projects, milestones, templates, and maintainer-edit controls, fail with a clear Forgejo-specific error.

## JSON Output

For `--json`, the shim emits a GitHub-shaped subset when Forgejo data is available:

```text
number, title, state, isDraft, url, headRefName, baseRefName,
author, createdAt, updatedAt, mergeable, mergeStateStatus,
statusCheckRollup
```

`gh repo view --json ...` supports a small repository metadata subset:

```text
description, defaultBranchRef, isPrivate, name, nameWithOwner,
owner, sshUrl, url
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

When no current-branch PR exists, `gh pr view --json ...` returns `{}` with exit code `0`, and `gh pr status --json ...` returns the same status envelope with `"currentBranch": null`. This keeps automation from failing just because a Forgejo branch has no PR yet.

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

Environment overrides:

```sh
FJ_SHIM_HOSTS
FJ_SHIM_REAL_GH
FJ_SHIM_REAL_FJ
FJ_SHIM_TOKEN
FORGEJO_TOKEN
GITEA_TOKEN
FJ_TOKEN
```

See [docs/configuration.md](docs/configuration.md) for details.

## Rollback

Remove the generated wrapper:

```sh
gh-forgejo-shim uninstall-shim
```

Remove the macOS GUI PATH LaunchAgent:

```sh
gh-forgejo-shim uninstall-gui-path
```

See [docs/rollback.md](docs/rollback.md) for PATH troubleshooting and recovery steps.

## Development

Run tests with:

```sh
python3 -m unittest
```

This project intentionally has no runtime dependencies outside the Python standard library.
