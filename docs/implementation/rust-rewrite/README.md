# Rust Rewrite Implementation Plan

This directory is the implementation plan for `gh-forgejo-shim-d60`, the epic for a full Rust rewrite of the `gh-forgejo-shim` runtime.

The goal is not to wrap Python with Rust. The target is zero Python in the final product. Python may be used temporarily as a parity oracle while Rust is being built, but it should not remain in the installed `gh`, `gfj`, or `gh-forgejo-shim` runtime paths. Any proposal to keep Python after phase 10 requires a separate `decision` bead and must prove that the retained code is not installed runtime code.

## Tracker

- Epic: `gh-forgejo-shim-d60` - `epic: full Rust rewrite of gh-forgejo-shim runtime`
- Planning task: `gh-forgejo-shim-qbu` - `planning: document Rust rewrite implementation plan`

## Phase Graph

The dependencies below are encoded in beads. Parent-child links attach each phase to the epic; `blocks` dependencies control execution order.

```text
phase 01
  -> phase 02
       -> phase 03
       -> phase 04
       -> phase 05

phase 03 + phase 04 + phase 05
  -> phase 06
  -> phase 08

phase 06
  -> phase 07

phase 03
  -> phase 09

phase 07 + phase 08 + phase 09
  -> phase 10
```

## Parallel Work

After phase 02 is complete, phases 03, 04, and 05 can run in parallel because they own different parts of the implementation.

After phases 03, 04, and 05 are complete, phase 06 and phase 08 can run in parallel. Phase 07 waits for phase 06 because mutation commands should reuse the read-only routing, client, normalization, and output behavior.

Phase 09 can begin after phase 03 because packaging can validate the native binary shape before all command families are ported. It cannot be considered release-ready until phase 10.

## Files

- [IMPLEMENT.md](IMPLEMENT.md) is the loop document for agents.
- [PLAN.html](PLAN.html) is the consolidated readable plan.
- [phase-01-contract.md](phase-01-contract.md)
- [phase-02-workspace.md](phase-02-workspace.md)
- [phase-03-dispatcher.md](phase-03-dispatcher.md)
- [phase-04-config-auth.md](phase-04-config-auth.md)
- [phase-05-http-normalization.md](phase-05-http-normalization.md)
- [phase-06-read-only-compat.md](phase-06-read-only-compat.md)
- [phase-07-mutating-workflows.md](phase-07-mutating-workflows.md)
- [phase-08-diagnostics-setup.md](phase-08-diagnostics-setup.md)
- [phase-09-packaging-release.md](phase-09-packaging-release.md)
- [phase-10-cutover-remove-python.md](phase-10-cutover-remove-python.md)
