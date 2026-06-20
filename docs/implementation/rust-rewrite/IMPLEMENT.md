# Implementing The Rust Rewrite

Read this file at the start of every Rust rewrite session.

## Loop

1. Run `bd prime`.
2. Run `bd show gh-forgejo-shim-d60`.
3. Run `bd ready` and choose the first ready child phase under the epic, unless the user directs otherwise.
4. Run `bd show <phase-id>`.
5. Read the linked phase document below.
6. Claim the bead with `bd update <phase-id> --claim`.
7. Implement only that phase's scope.
8. Run the phase's verification commands and any broader tests required by the changed surface.
9. Update the phase bead with notes if scope changes.
10. Close the phase bead only when its acceptance criteria are met.
11. Before ending the session, run the repo close protocol: tests, `git status`, commit, `git pull --rebase`, `git push`, and final `git status`.

## Epic

- Bead: `gh-forgejo-shim-d60`
- Type: epic
- Status: open
- Priority: P0
- Purpose: Replace the Python runtime with Rust for the installed `gh`, `gfj`, and `gh-forgejo-shim` command paths.

## Planning

| Bead | Status | Priority | Document | Purpose |
| --- | --- | --- | --- | --- |
| `gh-forgejo-shim-qbu` | in_progress | P0 | [README.md](README.md) | Create and maintain this implementation plan. This is not an implementation phase. |

## Phases

| Order | Bead | Status | Priority | Depends On | Can Run In Parallel With | Subagents | Full Document |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 01 | `gh-forgejo-shim-3sl` | open | P0 | none | none | Use explorers for contract inventory; one integrator owns final fixtures. | [phase-01-contract.md](phase-01-contract.md) |
| 02 | `gh-forgejo-shim-dqf` | open | P1 | phase 01 | none | Avoid parallel workers until layout is chosen; use at most one verifier. | [phase-02-workspace.md](phase-02-workspace.md) |
| 03 | `gh-forgejo-shim-gxo` | open | P1 | phase 02 | phases 04, 05 | Use one worker for dispatcher/routing and one verifier for timing/parity. | [phase-03-dispatcher.md](phase-03-dispatcher.md) |
| 04 | `gh-forgejo-shim-z9l` | open | P2 | phase 02 | phases 03, 05 | Use separate workers for config and auth if file ownership is clear. | [phase-04-config-auth.md](phase-04-config-auth.md) |
| 05 | `gh-forgejo-shim-98v` | open | P2 | phase 02 | phases 03, 04 | Use separate workers for HTTP client and normalization. | [phase-05-http-normalization.md](phase-05-http-normalization.md) |
| 06 | `gh-forgejo-shim-fon` | open | P2 | phases 03, 04, 05 | phase 08 | Use one worker per command family after shared output helpers are stable. | [phase-06-read-only-compat.md](phase-06-read-only-compat.md) |
| 07 | `gh-forgejo-shim-bk1` | open | P3 | phase 06 | phase 08, late phase 09 | Use workers by mutation family; keep checkout/git worker separate. | [phase-07-mutating-workflows.md](phase-07-mutating-workflows.md) |
| 08 | `gh-forgejo-shim-184` | open | P3 | phases 03, 04, 05 | phase 07 | Use workers for setup, trace, and macOS GUI PATH slices. | [phase-08-diagnostics-setup.md](phase-08-diagnostics-setup.md) |
| 09 | `gh-forgejo-shim-ppc` | open | P3 | phase 03 | phases 06, 07, 08 after packaging skeleton exists | Use workers for CI/release docs and installer artifacts. | [phase-09-packaging-release.md](phase-09-packaging-release.md) |
| 10 | `gh-forgejo-shim-k1e` | open | P4 | phases 07, 08, 09 | none | Use verifiers, not parallel implementers; one owner performs deletion/cutover. | [phase-10-cutover-remove-python.md](phase-10-cutover-remove-python.md) |

## Non-Negotiable Exit Criteria

- GitHub repositories delegate to the real `gh` without Python startup.
- Forgejo repositories do not use Python for routed `gh` behavior.
- `gfj` and `gh-forgejo-shim` are native Rust binaries.
- The target is zero Python. Any Python left after phase 10 requires a separate decision bead and must be test-only or historical, not installed runtime code.
- Existing config, auth, trace, and rollback behavior is preserved or intentionally migrated with clear release notes.
