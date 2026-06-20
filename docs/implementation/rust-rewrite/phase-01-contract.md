# Phase 01: Freeze Compatibility Contract

- Bead: `gh-forgejo-shim-3sl`
- Parent epic: `gh-forgejo-shim-d60`
- Status: open
- Priority: P0
- Depends on: none
- Blocks: phase 02
- Parallel work: none

## Plain English Goal

Before rewriting anything, capture what the Python version currently promises. The Rust rewrite should not guess at behavior. It should have fixtures that say, "for this command, in this repo shape, this is the exit code, stdout, stderr, trace record, and API request."

## Scope

- Inventory the public command surface:
  - `gh-forgejo-shim`, `gfj`, and managed `gh`.
  - Management commands: `install-shim`, `uninstall-shim`, `doctor`, `bootstrap`, `install-gui-path`, `uninstall-gui-path`, `config`, `auth`, `trace`, and `version`.
  - Routed `gh` commands: supported `auth`, `api`, `repo`, `pr`, and `issue` subsets.
- Convert current Python tests and traces into contract fixtures.
- Capture exit codes, stdout, stderr, JSON field shapes, trace JSONL records, and important error text.
- Define performance budgets for GitHub delegation and Forgejo routed commands.
- Mark intentionally incomplete behavior, such as accepted but ignored flags, so Rust preserves the contract deliberately.

## Subagents

Use explorer subagents in this phase. Good parallel assignments:

- Inventory routed `gh pr`, `gh issue`, and `gh repo` behavior from `routing.py` and `tests/test_routing.py`.
- Inventory config/auth/setup behavior from `config.py`, `auth.py`, `doctor.py`, `bootstrap.py`, and their tests.
- Inventory trace, smoke, and git-recorder behavior from `trace.py`, `trace_summary.py`, `codex_smoke.py`, `git_recorder.py`, and their tests.

Do not let subagents edit the final fixture format independently. One integrator should merge the findings into the contract fixtures so the test surface stays coherent.

## Deliverables

- A command matrix for all supported and delegated commands.
- Golden fixtures for Codex-critical probes:
  - `gh --version`
  - `gh auth status`
  - `gh api user`
  - `gh repo view`
  - `gh pr status`
  - `gh pr list`
  - `gh pr view`
  - `gh pr checks`
  - raw git branch/ref probe traces
- A clear list of unsupported commands and expected delegation behavior.
- Baseline timing numbers comparing the Python shim and real `gh`.

Frozen contract artifact:

- [contracts/compatibility-contract.v1.json](contracts/compatibility-contract.v1.json)

## Acceptance Criteria

- A future Rust command can be tested against the frozen fixtures without launching Codex.
- The contract includes both GitHub delegation and Forgejo routing.
- Trace redaction behavior is covered.
- The current Python test suite still passes.

## Risks

- Codex may depend on small output details that are not obvious from normal terminal use.
- The current Python behavior includes quirks. The rewrite must either preserve them or explicitly decide to migrate them.
