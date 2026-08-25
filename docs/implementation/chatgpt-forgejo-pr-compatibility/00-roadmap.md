# ChatGPT desktop Forgejo pull request compatibility Roadmap

Canonical tracker: Beads epic `gh-forgejo-shim-6y9`

## Plan Source

`docs/implementation/chatgpt-support/PLAN.md`

## Outcome

ChatGPT desktop opens and polls Codeberg and user-configured Forgejo pull requests through the installed `gh` process, while verified real GitHub CLI delegation remains unchanged.

## Phase Sequence

00. **S0.** Make the test baseline deterministic through Beads issue `gh-forgejo-shim-c00`.
01. **S1.** Freeze the ChatGPT build-6720 process contract through Beads issue `gh-forgejo-shim-c01`.
02. **S2.** Add typed, backward-compatible host profiles through Beads issue `gh-forgejo-shim-c02`.
03. **S3.** Resolve identity before routing through Beads issue `gh-forgejo-shim-c03`.
04. **C1.** Pass the host identity and routing checkpoint through Beads issue `gh-forgejo-shim-c04`.
05. **S4.** Make authentication use canonical host identity through Beads issue `gh-forgejo-shim-c05`.
06. **S5.** Enforce the command deadline and truthful tracing through Beads issue `gh-forgejo-shim-c06`.
07. **S6.** Make PR discovery honor `@me` and pagination through Beads issue `gh-forgejo-shim-c07`.
08. **C2.** Pass the PR discovery checkpoint through Beads issue `gh-forgejo-shim-c08`.
09. **S7.** Render the PR header and core fields through Beads issue `gh-forgejo-shim-c09`.
10. **S8.** Add comments, commits, reviews, and reviewer requests through Beads issue `gh-forgejo-shim-c10`.
11. **S9.** Render current checks and real no-check behavior through Beads issue `gh-forgejo-shim-c11`.
12. **C3.** Pass the complete read-contract checkpoint through Beads issue `gh-forgejo-shim-c12`.
13. **S10.** Prove generic multi-host behavior through Beads issue `gh-forgejo-shim-c13`.
14. **S11.** Package and install without disturbing the host through Beads issue `gh-forgejo-shim-c14`.
15. **C4.** Pass the packaging and rollback checkpoint through Beads issue `gh-forgejo-shim-c15`.
16. **S12.** Validate the native ChatGPT PR screen through Beads issue `gh-forgejo-shim-c16`.
17. **C5.** Accept the native ChatGPT product result through Beads issue `gh-forgejo-shim-c17`.

## Dependencies

- S0 (gh-forgejo-shim-c00) depends on no earlier loop issue.
- S1 (gh-forgejo-shim-c01) depends on `gh-forgejo-shim-c00`.
- S2 (gh-forgejo-shim-c02) depends on `gh-forgejo-shim-c01`.
- S3 (gh-forgejo-shim-c03) depends on `gh-forgejo-shim-c02`.
- C1 (gh-forgejo-shim-c04) depends on `gh-forgejo-shim-c03`.
- S4 (gh-forgejo-shim-c05) depends on `gh-forgejo-shim-c04`.
- S5 (gh-forgejo-shim-c06) depends on `gh-forgejo-shim-c04`.
- S6 (gh-forgejo-shim-c07) depends on `gh-forgejo-shim-c05` and `gh-forgejo-shim-c06`.
- C2 (gh-forgejo-shim-c08) depends on `gh-forgejo-shim-c07`.
- S7 (gh-forgejo-shim-c09) depends on `gh-forgejo-shim-c08`.
- S8 (gh-forgejo-shim-c10) depends on `gh-forgejo-shim-c09`.
- S9 (gh-forgejo-shim-c11) depends on `gh-forgejo-shim-c10`.
- C3 (gh-forgejo-shim-c12) depends on `gh-forgejo-shim-c11`.
- S10 (gh-forgejo-shim-c13) depends on `gh-forgejo-shim-c12`.
- S11 (gh-forgejo-shim-c14) depends on `gh-forgejo-shim-c12`.
- C4 (gh-forgejo-shim-c15) depends on `gh-forgejo-shim-c13` and `gh-forgejo-shim-c14`.
- S12 (gh-forgejo-shim-c16) depends on `gh-forgejo-shim-c15`.
- C5 (gh-forgejo-shim-c17) depends on `gh-forgejo-shim-c16`.

## Settled Decisions

- Preserve `cwd + env + stdin + gh argv` to `exit status + stdout + stderr` as the external interface.
- Resolve a generic HostRegistry before routing, credentials, API URLs, links, or delegation.
- Use one shared five-second command deadline, 50-item pagination, a 20-page error cap, and verified real GitHub CLI 2.96 delegation.
- Keep one external integration PR. Only S4 with S5 and S10 with S11 may run in parallel.
- Use the adaptive ownership profile selected in the loop creation command.

## Open Questions

- No product or architecture questions remain. Live second-host credentials and fresh desktop traces are run-time evidence. Do not create credentials or repositories without separate authority.

## Risks

- Host alias ambiguity, credential crossover, unsafe Forgejo-to-GitHub fallthrough, partial timeout results, stale checks, package path damage, and rollback failure stop the run.

## Replanning Triggers

- A fresh desktop trace changes the build-6720 contract.
- Forgejo cannot support a claimed field or check semantic.
- A required live fixture is unavailable or has drifted without an authorized replacement.
- A checkpoint reaches the three-pass repair limit.

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

## Closeout

The final closeout artifact is:

`docs/implementation/chatgpt-forgejo-pr-compatibility/storyboard-post-run-08-22-2026.html`
