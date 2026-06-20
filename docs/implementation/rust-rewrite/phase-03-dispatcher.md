# Phase 03: Native Dispatcher And GitHub Delegation

- Bead: `gh-forgejo-shim-gxo`
- Parent epic: `gh-forgejo-shim-d60`
- Status: open
- Priority: P1
- Depends on: phase 02
- Blocks: phases 06, 08, and 09
- Parallel work: phases 04 and 05

## Plain English Goal

Make the front door fast. When Codex runs `gh` in a normal GitHub repo, Rust should quickly exec the real GitHub CLI without starting Python. In Forgejo repos, Rust should choose the Forgejo route.

## Scope

- Parse argv enough to classify supported command families.
- Find the real `gh` without selecting the managed shim.
- Detect repository host from:
  - `-R` or `--repo`
  - command URLs
  - `GH_REPO`
  - `GH_HOST`
  - local git remotes
- Load the host allowlist.
- Always delegate known GitHub hosts to real `gh`.
- Delegate unsupported commands to real `gh`.
- Add minimal trace records for delegated commands without leaking secrets.

## Subagents

Use subagents carefully after the dispatcher interface is defined.

Recommended split:

- Worker A owns route decision and repo/host detection modules.
- Worker B owns real `gh` discovery, exec/delegation, and managed-shim avoidance.
- Verifier owns timing measurements and GitHub delegation parity tests.

Do not let multiple workers edit the same dispatcher entrypoint at the same time. The main agent should integrate the route decision interface.

## Deliverables

- Native Rust `gh-forgejo-shim gh ...` dispatcher.
- GitHub delegation path that avoids Python startup.
- Tests for route decisions and real-`gh` discovery.
- Performance comparison against the current Python shim.

## Acceptance Criteria

- `gh --version` through the Rust dispatcher is close to real `gh`, not Python startup speed.
- GitHub repos delegate directly.
- Forgejo allowlisted repos route to the Rust implementation path.
- Missing real `gh` exits with the established error code and message shape.

## Risks

- A routing mistake can break normal GitHub work. Keep the dispatcher conservative.
- Route detection must preserve the current order because `GH_REPO`, `GH_HOST`, and `-R` are used by tools.
