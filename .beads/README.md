# Beads workflow for gh-forgejo-shim

This repository uses Beads (`bd`) for project issue tracking. The tracker is
part of the repo so agents and developers can inspect current work, record
follow-up issues, and keep implementation context close to the code.

`gh-forgejo-shim` is now a stdlib-only Python CLI that gives GitHub-oriented
tools enough GitHub CLI compatibility to work in allowlisted Forgejo
repositories. Beads should describe that compatibility precisely: name the
command, flag, JSON field, setup step, or fallback that changed.

## Tracker storage and sync

- The source of truth is the local Dolt database under `.beads/dolt/`.
- Cross-machine Beads sync uses the configured Dolt remote:
  `http://dolt.deltaisland.io/gh_forgejo_shim`.
- `.beads/issues.jsonl` is a passive export for review and recovery. Do not use
  `bd import` during normal development.
- Pull Beads state at the start of a session and push Beads state before
  handoff:

```bash
bd prime
bd dolt pull
bd ready
```

```bash
bd dolt push
```

## Daily commands

```bash
# Full local workflow context and session rules
bd prime

# Find available work
bd ready
bd list --status=open
bd show <issue-id>

# Create and claim work
bd create --title="Short concrete title" --description="Why this issue exists and what needs to be done" --type=task --priority=2
bd update <issue-id> --claim

# Close finished work
bd close <issue-id> --reason="Completed and validated"
```

Use `bd` for task tracking. Do not create markdown TODO lists or separate
memory files for project state.

## What has recently landed

The tracker now reflects a much broader project than the original bootstrap:

- `gh-forgejo-shim` v1 package scaffolding, tests, documentation, and a
  generated user-local `gh` wrapper that delegates real GitHub repositories to
  the real GitHub CLI.
- `gfj` as the short daily-use command alias for the same management CLI.
- `gfj bootstrap`, which detects the current Forgejo checkout, adds host
  allowlisting, installs the shim, checks PATH, verifies auth discovery, checks
  `origin` and `origin/HEAD`, and prints repair commands for anything it cannot
  fix automatically.
- Robust GitHub CLI discovery for GUI-launched macOS tools, including
  `gfj install-gui-path` and `gfj doctor` checks for shell PATH versus GUI PATH.
- Repository remote guidance for GitHub-style tools: `origin.url`,
  `origin.pushurl`, `origin/HEAD`, and branch upstream tracking.
- Forgejo routing for the GitHub CLI commands this project currently supports:
  `gh repo view`, `gh issue create/list/new/view`, and the supported
  `gh pr checks/checkout/comment/create/diff/list/new/status/view` surface.
- GitHub-shaped JSON output for repository metadata, issues, pull requests,
  pull request status, and pull request checks.
- Forgejo commit status mapping into GitHub-style check rollups, including
  pass/fail/pending buckets and latest-status-per-context behavior.
- Empty-current-branch behavior that keeps tools from failing when a Forgejo
  branch has no pull request yet.
- Native Forgejo auth helpers: `auth login`, `auth import`, `auth status`, and
  `auth logout`, with macOS Keychain storage when available and a restricted
  config-file fallback otherwise.
- Auth discovery from shim storage, token environment variables, and common
  `fj`, `tea`, and `gitea` configuration files.

Turn records for these changes live in `docs/turns/`.

## Current open work

As of this README refresh, the remaining open Beads issues are:

- `gh-forgejo-shim-9r8`: implement real filters for shimmed
  `gh pr list` search flags.
- `gh-forgejo-shim-mte`: add live Forgejo integration coverage.
- `gh-forgejo-shim-xq1`: add package release automation.

Create new issues for discovered gaps before ending a session. Close issues only
after the relevant code, docs, and validation are done.

## Session close checklist

Before handoff:

1. Run relevant quality gates, such as `python -m pytest`, linters, or focused
   tests for the files changed.
2. Update Beads issue state with `bd close` or `bd update` notes.
3. Pull and push Beads data:

```bash
bd dolt pull
bd dolt push
```

4. Commit the code, docs, and Beads export changes that belong to the task.
5. Push the git branch when the active branch has an upstream or the current
   session instructions require publication.
6. Confirm `git status --short --branch` is clean or explain any remaining
   unrelated changes clearly.

## Project references

- `README.md`: user-facing install, setup, supported command, JSON, and auth
  documentation.
- `docs/configuration.md`: configuration, host allowlisting, GUI PATH, auth, and
  repository detection details.
- `PRODUCT.md`: product purpose, users, tone, and documentation principles.
- `AGENTS.md`: repository-specific agent workflow, including Beads requirements.
