# Phase 10: Cut Over And Remove Python Runtime

- Bead: `gh-forgejo-shim-k1e`
- Parent epic: `gh-forgejo-shim-d60`
- Status: open
- Priority: P4
- Depends on: phases 07, 08, and 09
- Blocks: epic completion
- Parallel work: none

## Plain English Goal

Make Rust the product. Remove Python from the installed runtime and delete Python implementation code. The target is zero Python.

## Scope

- Switch installed `gh`, `gfj`, and `gh-forgejo-shim` paths to Rust.
- Verify GitHub delegation in real GitHub repos.
- Verify Forgejo routing in real Forgejo repos.
- Verify Codex branch picker, PR status, PR creation, issue views, and trace workflows.
- Remove Python source modules from the runtime path.
- Remove setuptools/pipx packaging.
- Update README, configuration docs, rollback docs, and development docs.

## Subagents

Do not use parallel implementers for the deletion/cutover itself. This phase needs one owner because it removes old runtime code and changes install behavior.

Good subagent use:

- Verifier A runs GitHub repo smoke checks.
- Verifier B runs Forgejo repo smoke checks.
- Verifier C audits the repository for remaining Python and checks whether each file has a decision bead.
- Verifier D reviews docs for stale pipx/Python instructions.

The main owner performs the actual deletion and final integration.

## Python Exception Rule

The intended result is no Python. If someone proposes keeping any Python file after this phase, they must first create a separate `decision` bead. The decision must prove all of these:

- It is not used by `gh`, `gfj`, or `gh-forgejo-shim` at runtime.
- It is not installed as part of the product.
- It has a named owner.
- It has a removal condition or a documented permanent reason.

Examples that might pass only with a decision bead:

- Historical parity fixtures.
- Test-only scripts that are not installed.

## Deliverables

- Rust-only installed runtime.
- Python implementation removed.
- Final migration and rollback documentation.
- Closed epic with verification notes.

## Acceptance Criteria

- `command -v gh` points at the managed Rust shim/binary as intended.
- `gh --version` in a GitHub repo delegates without Python startup.
- Forgejo routed commands pass Codex smoke checks without Python.
- The full test suite passes.
- Any remaining Python has a separate decision bead and is not installed runtime code.

## Risks

- Cutover can strand users if migration is not tested.
- Removing Python before parity is complete will break Forgejo compatibility.
