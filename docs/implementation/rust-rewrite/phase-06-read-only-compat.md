# Phase 06: Port Read-Only gh Compatibility

- Bead: `gh-forgejo-shim-fon`
- Parent epic: `gh-forgejo-shim-d60`
- Status: open
- Priority: P2
- Depends on: phases 03, 04, and 05
- Blocks: phase 07
- Parallel work: phase 08

## Plain English Goal

Port the commands that Codex uses to understand the repository without changing it. This should make Forgejo repos usable for status, branch, PR board, and issue views through the Rust runtime.

## Scope

- Port routed read-only commands:
  - `gh auth status`
  - `gh auth token`
  - `gh api user`
  - `gh repo view`
  - `gh pr list`
  - `gh pr status`
  - `gh pr view`
  - `gh pr checks`
  - `gh pr diff`
  - `gh issue list`
  - `gh issue view`
- Preserve supported flags and tolerated unsupported flags from the Python contract.
- Preserve text output and JSON output.
- Preserve empty-success behavior where the Python shim returns `{}` or empty output for no current PR.

## Subagents

Use workers by command family after phases 03, 04, and 05 have stable interfaces.

Recommended split:

- Worker A owns `auth status`, `auth token`, and `api user`.
- Worker B owns `repo view` and issue read commands.
- Worker C owns PR list/status/view.
- Worker D owns PR checks/diff and status rollup output.
- Verifier owns Codex smoke and golden output parity.

Each worker should own its tests and fixtures for its command family. The main agent should integrate shared CLI parsing/output helpers.

## Deliverables

- Rust command handlers for read-only command families.
- Parity tests for stdout, stderr, exit code, JSON fields, and trace output.
- Codex smoke command runs against the Rust path.

## Acceptance Criteria

- Codex-critical read-only probes pass without Python.
- Existing Python tests for equivalent behavior have Rust parity coverage.
- GitHub repos still delegate.

## Risks

- Codex may treat a nonzero exit as unavailable even if the message is correct for a human.
- Small JSON differences can break UI states.
