# Phase 07: Port Mutating Workflows

- Bead: `gh-forgejo-shim-bk1`
- Parent epic: `gh-forgejo-shim-d60`
- Status: open
- Priority: P3
- Depends on: phase 06
- Blocks: phase 10
- Parallel work: phase 08 and late phase 09

## Plain English Goal

Port commands that change remote or local state after the read-only behavior is stable.

## Scope

- Port PR write workflows:
  - `gh pr create`
  - `gh pr new`
  - `gh pr comment`
  - `gh pr checkout`
  - `gh pr co`
- Port issue write workflows:
  - `gh issue create`
  - `gh issue new`
- Port auth write workflows:
  - `gh-forgejo-shim auth login`
  - `gh-forgejo-shim auth import`
  - `gh-forgejo-shim auth logout`
- Preserve branch, base, head, fill, body-file, web, draft, and JSON behavior.
- Preserve Codex-compatible created PR URL behavior.

## Subagents

Use workers, but keep state-changing surfaces isolated.

Recommended split:

- Worker A owns PR create/new parsing, body handling, fill behavior, and created URL output.
- Worker B owns PR comment and issue create/new.
- Worker C owns PR checkout/co and all git subprocess behavior.
- Worker D owns auth login/import/logout write behavior.

Every worker must use fake Forgejo servers and temporary git repos. Do not run live mutating commands as verification unless the user explicitly asks.

## Deliverables

- Rust command handlers for mutating commands.
- Tests with fake Forgejo APIs and temporary git repos.
- Safety checks around stdin, files, local checkout changes, and token validation.

## Acceptance Criteria

- PR creation and issue creation match Python contract.
- Checkout behavior works in temporary repos.
- Commands fail clearly when required auth, branch, or title/body data is missing.

## Risks

- Mutating commands have higher blast radius. Use fake servers and temp repos by default.
- `pr checkout` shells out to git and must preserve error behavior.
