# gh-forgejo-shim

`gh-forgejo-shim` is a small, stdlib-only Python CLI for Codex.app, T3 Code, and other GitHub-oriented tools used with Forgejo repositories.

It installs a durable management command named `gh-forgejo-shim`, plus a shorter daily-use alias named `gfj`. When you opt in, it can also place a user-local `gh` wrapper in front of the real GitHub CLI. Real GitHub repositories still go to the real `gh`; allowlisted Forgejo repositories route a narrow set of repository and pull request commands through Forgejo-friendly behavior.

V1 is not full `gh` emulation. It exists to keep GitHub-style development tools from treating Forgejo repositories like broken GitHub repositories.

## Install

```sh
pipx install gh-forgejo-shim
```

From inside a Forgejo checkout, run the bootstrap command:

```sh
gfj bootstrap
```

`bootstrap` detects the current repository, adds its host to the allowlist, installs the user-local `gh` shim, checks whether PATH resolves to the shim, verifies that Forgejo auth can be discovered, checks `origin` and `origin/HEAD`, and prints exact repair commands for anything it cannot fix automatically.

The long command name works the same way:

```sh
gh-forgejo-shim bootstrap
```

If you prefer to do the setup manually, add at least one Forgejo host:

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

## Quickstart For GitHub-Style Tools

1. Install the package with `pipx`.
2. Open a terminal inside your Forgejo repository.
3. Run `gfj bootstrap`.
4. Copy and run any repair commands it prints.
5. On macOS, run `gfj install-gui-path` if the tool was launched from Finder, Dock, Spotlight, or another GUI launcher.
6. Confirm the setup:

```sh
gfj doctor
```

7. Restart the GUI tool, open the Forgejo repository, and use the normal repository, branch, commit, push, and pull request workflows.

For scripted setup or documentation, use `gh-forgejo-shim`. For day-to-day typing, `gfj` is the same command with a shorter name.

## What This Setup Covers

GitHub-style tools do not rely only on `gh pr ...`. They commonly combine plain Git commands with GitHub CLI commands. The shim helps with the GitHub CLI side, while the repository remote shape below helps the tool recognize the repository before it invokes `gh`.

The setup is intended to cover:

- Repository discovery from local Git remotes.
- Default branch discovery through `origin/HEAD`.
- Current branch and upstream tracking for branch status, ahead/behind counts, and commit or push UI.
- Repository metadata through `gh repo view`.
- Pull request creation, listing, viewing, and current-branch status through `gh pr ...`.
- GUI-launched macOS tools that need a usable PATH to find the shim and the real `gh`.
- Forgejo auth discovery from environment variables or common `fj`, `tea`, and `gitea` config files.

## Repository Remote Shape For GitHub-Style Tools

Codex.app, T3 Code, and similar GitHub-oriented tools usually expect the local repository to look like a conventional GitHub checkout. Even when the shim can answer Forgejo-backed `gh` commands, those tools may still show incomplete repository state, disabled commit or push controls, or unavailable pull request status if the repository only has a remote named `forgejo`, if `origin/HEAD` has not been populated, or if the current branch does not track an `origin/*` branch.

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
git status --short --branch
gh repo view --json nameWithOwner,url,defaultBranchRef,sshUrl
gh pr status --json number,title,url,headRefName,state
```

`gfj bootstrap` checks these same basics and prints the matching commands when something is missing.

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
gfj uninstall-shim
```

Remove the macOS GUI PATH LaunchAgent:

```sh
gfj uninstall-gui-path
```

See [docs/rollback.md](docs/rollback.md) for PATH troubleshooting and recovery steps.

## Development

Run tests with:

```sh
python3 -m unittest
```

This project intentionally has no runtime dependencies outside the Python standard library.
