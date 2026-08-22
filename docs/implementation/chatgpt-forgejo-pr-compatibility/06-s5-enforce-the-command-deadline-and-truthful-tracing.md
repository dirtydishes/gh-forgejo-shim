# Phase 06: S5: Enforce the command deadline and truthful tracing

Canonical Beads issue: `gh-forgejo-shim-c06`

Epic: `gh-forgejo-shim-6y9`

Status is tracked in Beads. This document preserves accepted intent and is decision-complete, implementation-open.

## Outcome

A user gets a bounded result or clear timeout, and the trace states which output it observed.

## Why This Phase Exists

This slice owns one independently verifiable result in the accepted dependency graph. S3 and C1. S4 and S5 may run in parallel in separate worktrees because their owned implementation files do not overlap.

## Scope

Allowed:

- Update `src/routing.rs`, `src/forgejo.rs`, `src/external.rs`, and `src/trace.rs`. Add a shared `CommandDeadline`. Count Forgejo output through counting writers. Capture and forward output only for the observed noninteractive ChatGPT command set. Mark inherited output as unknown when the shim cannot count it.

Out of scope:

- Work assigned to another slice or checkpoint.
- General GraphQL support, new mutation behavior, caching, a daemon, an HTTP proxy, and ChatGPT cloud connector work.
- Revert S5. Persistent effects remain limited to opt-in trace files.

## Constraints

- Use the accepted external process or management seam and one slice-owned worktree.
- Keep one external integration PR.
- Stop if the deadline resets between requests, output order changes, interactive behavior regresses, or trace data claims false precision.

## Settled Decisions

- Use a vertical red-to-green cycle at a confirmed TDD seam.
- Do not widen the slice when evidence exposes adjacent work.
- One shared five-second monotonic deadline covers pagination and enrichment for an observed command.

## Open Questions

- None. New evidence can trigger the listed stop condition or a user-visible plan amendment.

## Dependencies

- Depends on: `gh-forgejo-shim-c04`
- Parallel-safe: `Yes, with S4 after C1 in a separate worktree.`

## Acceptance Evidence

- A delayed multi-request command exits within five seconds. Trace records provider, command class, duration, exit, exact observed byte counts, and an explicit unknown state where capture was impossible.
- Fake-clock or bounded-delay tests plus literal trace schema assertions.

## Execution Boundary

- Acceptance boundary: A user gets a bounded result or clear timeout, and the trace states which output it observed.
- Module surface: shared CommandDeadline, Forgejo and external process I/O, and trace records
- Review boundary: S5 integration PR diff, named tests, fixtures, command evidence, and review gate: Confirm interactive and large existing commands keep inherited streaming and secrets remain redacted.

## Quality Gates

- Red behavior evidence precedes each green implementation in source-changing slices.
- The narrow acceptance tests pass.
- `cargo fmt --all -- --check` passes.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` passes.
- `cargo test --workspace --locked` passes.
- The GitHub Actions `rust` job is green for the exact commit.
- `release package smoke` is green when packaging changes.
- Required review findings are resolved within the three-pass limit.
- The branch is pushed and remote parity is proved before the slice closes.
- Fake-clock or bounded-delay tests plus literal trace schema assertions.

## Replanning Triggers

- Stop if the deadline resets between requests, output order changes, interactive behavior regresses, or trace data claims false precision.

## Implementation Hypotheses

These are suggestions to validate against repository evidence, not mandatory choreography.

- Start with the narrowest named test at the S5 process, CLI, HTTP, delegation, or product seam that can prove this outcome.

## Follow-Up Policy

Do not widen this phase. File Beads follow-ups for adjacent discoveries.
