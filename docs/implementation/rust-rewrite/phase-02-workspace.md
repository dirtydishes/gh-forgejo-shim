# Phase 02: Establish Rust Workspace

- Bead: `gh-forgejo-shim-dqf`
- Parent epic: `gh-forgejo-shim-d60`
- Status: open
- Priority: P1
- Depends on: phase 01
- Blocks: phases 03, 04, and 05
- Parallel work: none

## Plain English Goal

Create the Rust project skeleton and test harness before porting behavior. This gives every later phase a shared place to put code and tests.

## Scope

- Add a Cargo workspace.
- Add a Rust package named `gh-forgejo-shim`.
- Build native binaries for:
  - `gh-forgejo-shim`
  - `gfj`
- Keep the existing user install untouched until cutover.
- Add a test harness that can run Rust CLI commands under temporary homes, temporary repos, and fake environments.
- Add formatting, linting, and test commands for Rust.

## Subagents

Avoid broad parallel implementation in this phase. Workspace layout is a shared seam, so one owner should make the scaffold decisions.

Useful subagent work:

- One explorer can review Rust packaging patterns and report recommendations without editing.
- One verifier can run the scaffold on a clean checkout after the owner creates it.

Do not split `Cargo.toml`, crate layout, and binary entrypoints across multiple workers. That causes merge churn and unclear ownership.

## Suggested Layout

```text
Cargo.toml
crates/
  gh-forgejo-shim/
    Cargo.toml
    src/
      main.rs
      lib.rs
      cli.rs
      routing.rs
      repo.rs
      config.rs
      auth.rs
      forgejo.rs
      normalize.rs
      create.rs
      trace.rs
      trace_summary.rs
      codex_smoke.rs
      doctor.rs
      bootstrap.rs
      shim.rs
      gui_path.rs
      git_recorder.rs
      external.rs
tests/
  cli_parity/
  fixtures/
```

## Deliverables

- `cargo test`, `cargo fmt`, and `cargo clippy` run locally.
- Rust binaries can print version/help without Python.
- Initial integration test helpers exist.
- No installed `gh` behavior changes yet unless explicitly opted in for testing.

## Acceptance Criteria

- Rust workspace builds on the developer machine.
- Existing Python tests still pass.
- Beads and docs explain that this phase is scaffold only, not behavior parity.

## Risks

- Adding a workspace can disturb packaging files. Keep changes narrow and reversible.
- Do not start porting command behavior before the test harness exists.
