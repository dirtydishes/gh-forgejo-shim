# Phase 04: Port Config And Auth

- Bead: `gh-forgejo-shim-z9l`
- Parent epic: `gh-forgejo-shim-d60`
- Status: open
- Priority: P2
- Depends on: phase 02
- Blocks: phases 06 and 08
- Parallel work: phases 03 and 05

## Plain English Goal

Move host configuration and token lookup to Rust without changing where users' existing config and auth live.

## Scope

- Preserve config path: `~/.config/gh-forgejo-shim/config.toml`.
- Preserve host normalization and the rule that GitHub hosts are never treated as Forgejo.
- Preserve env overrides:
  - `FJ_SHIM_HOSTS`
  - `FJ_SHIM_REAL_GH`
  - `FJ_SHIM_REAL_FJ`
  - token env vars such as `FJ_SHIM_TOKEN`, `FORGEJO_TOKEN`, `GITEA_TOKEN`, and `FJ_TOKEN`
- Preserve auth storage:
  - macOS Keychain service name
  - fallback auth JSON path and file mode
- Preserve import from common `fj`, `tea`, and `gitea` config files.
- Port `auth login`, `auth import`, `auth status`, and `auth logout` management commands.

## Subagents

This phase can use subagents if ownership is strict.

Recommended split:

- Worker A owns config file parsing, host normalization, and env overrides.
- Worker B owns token discovery from env, shim storage, and external CLI config files.
- Worker C owns macOS Keychain integration and fallback file permissions.

One owner should integrate the public auth interface so command handlers see one coherent token lookup module.

## Deliverables

- Rust config module with round-trip tests.
- Rust auth module with env, file, external config, and Keychain tests where possible.
- No secret printed in status or trace output.

## Acceptance Criteria

- Existing Python-created config and auth files work with Rust.
- Rust-created config and auth files are readable by the old Python implementation during transition.
- Missing token messages remain actionable.

## Risks

- Keychain behavior is platform-specific and needs macOS validation.
- Auth regressions can either block all Forgejo use or leak secrets, so tests must be strict.
