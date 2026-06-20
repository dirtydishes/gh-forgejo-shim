# Phase 08: Port Diagnostics And Setup Commands

- Bead: `gh-forgejo-shim-184`
- Parent epic: `gh-forgejo-shim-d60`
- Status: open
- Priority: P3
- Depends on: phases 03, 04, and 05
- Blocks: phase 10
- Parallel work: phase 07

## Plain English Goal

Move the setup and diagnosis tools to Rust so users can install, repair, trace, and roll back without Python.

## Scope

- Port shim lifecycle:
  - `install-shim`
  - `uninstall-shim`
- Port setup:
  - `bootstrap`
  - `doctor`
  - `install-gui-path`
  - `uninstall-gui-path`
- Port trace tools:
  - `trace summarize`
  - `trace smoke`
  - `trace git-recorder create`
  - `trace git-recorder remove`
- Preserve managed-file markers and refusal to overwrite unmanaged files without `--force`.
- Preserve macOS LaunchAgent behavior.

## Subagents

This phase can run in parallel with clear file ownership.

Recommended split:

- Worker A owns `install-shim`, `uninstall-shim`, and rollback marker behavior.
- Worker B owns `doctor` and `bootstrap`.
- Worker C owns trace summarize, trace smoke, and git-recorder.
- Worker D owns macOS GUI PATH LaunchAgent commands.

Use a verifier on macOS-specific behavior because CI may not cover launchd and GUI environment effects.

## Deliverables

- Rust diagnostic and setup commands.
- Tests for generated files, permissions, markers, and trace summaries.
- Manual macOS validation notes for GUI PATH and Keychain-adjacent flows.

## Acceptance Criteria

- A user can bootstrap a Forgejo checkout with only the Rust binary.
- `doctor` clearly reports the same categories as Python.
- Git recorder captures raw `git` probes and can remove itself safely.

## Risks

- macOS launchd behavior is hard to fully test in CI.
- The generated shim is the rollback path. Its marker and refusal rules must be conservative.
