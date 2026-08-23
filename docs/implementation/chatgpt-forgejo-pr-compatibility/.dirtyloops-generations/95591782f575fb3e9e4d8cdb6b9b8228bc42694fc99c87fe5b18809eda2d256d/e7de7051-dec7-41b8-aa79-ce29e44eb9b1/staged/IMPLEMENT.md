# ChatGPT desktop Forgejo pull request compatibility Implementation Loop

Dirtyloop version: `2`

Execution profile: `adaptive`

Canonical tracker: Beads epic `gh-forgejo-shim-6y9`

Accepted plan: `docs/implementation/chatgpt-support/PLAN.md`

Beads owns state. These docs preserve accepted intent and execution context.

## Goal

Restore ChatGPT desktop's native pull request screen for Codeberg and user-configured Forgejo hosts without changing real GitHub behavior.

## Scope And Non-Goals

- Implement the observed build-6720 read contract, generic host profiles, canonical identity, auth, deadlines, PR list and view projections, checks, two-host proof, packaging, and native desktop validation.
- Preserve existing mutation commands without expanding them.
- Exclude general GraphQL support, caching, daemons, HTTP proxies, ChatGPT cloud GitHub workflows, and new mutation behavior.

## Settled Decisions

- Keep the installed process interface and the accepted TDD seams.
- Use a data-driven HostRegistry and provider-first fail-closed routing.
- Use one shared five-second monotonic deadline for an observed command.
- Delegate GitHub repositories through an explicit checksum-verified GitHub CLI 2.96 path.
- Use one external integration PR and the accepted checkpoint review policy.
- The user's create command selects adaptive ownership and supersedes only the older orchestrator-callback profile clause in the plan.

## Stream Acceptance Evidence

- The exact Dirtypages discovery command returns an authoritative empty list without contacting loopback HTTPS.
- Alias and canonical auth use the right host credential without token output.
- The full 25-field PR view and checks contracts match the verified fixtures, including real no-check behavior.
- Two unrelated Forgejo host profiles do not share aliases, credentials, API roots, repositories, or links.
- Real GitHub commands delegate unchanged through verified `gh` 2.96.
- The approved package installs and rolls back without replacing `/usr/bin/gh`.
- The native ChatGPT screen and redacted trace pass the S12 and C5 checks.

## Sources Of Truth

- Beads epic: `gh-forgejo-shim-6y9`
- Accepted plan: `docs/implementation/chatgpt-support/PLAN.md`
- Roadmap: `docs/implementation/chatgpt-forgejo-pr-compatibility/00-roadmap.md`
- Phase docs linked from Beads
- Turn docs: `docs/implementation/chatgpt-forgejo-pr-compatibility/turn-docs/`
- Resume mirror: `docs/implementation/chatgpt-forgejo-pr-compatibility/loop-state.md`
- Runtime generation: `docs/implementation/chatgpt-forgejo-pr-compatibility/runtime/generation.json`
- Harness binding: `docs/implementation/chatgpt-forgejo-pr-compatibility/runtime/binding.json`
- Pi adapter policy when present: `docs/implementation/chatgpt-forgejo-pr-compatibility/runtime/adapter.json`

## Control-Plane Invariants

- Select one ready phase, or one approved Wave of dependency-ready qualified tracer-bullet leaves, only when the accepted plan explicitly permits it.
- When broad Phase milestones group work, never claim them. Record phase membership in metadata rather than Beads `parent-child` edges; execute only their qualified tracer-bullet leaves.
- Require `runtime/activation.json` to be ready for the current generated builder closure and coordinator before phase ownership; this state does not certify a separate Program runtime adapter.
- Require a `ready` execution-readiness report before phase mutation. Qualified tracer-bullet Beads leaves may narrow an immutable shared phase doc only when both sources are hashed and the leaf has explicit bounded ownership; clarification or re-slicing is a visible plan amendment, never an automatic rewrite.
- Read its phase doc and write an orchestration brief before broad work.
- Enforce the persisted execution profile as the ownership boundary.
- Within that boundary, choose model, effort, helper missions, concurrency, and coordination from current evidence and capabilities.
- Strongly encourage useful helper subagents during implementation and review. Use at most 20 helper missions per stage; bound active concurrency by user configuration, certified runtime capacity, and available slots. Record a rationale when a non-trivial stage uses none.
- Keep helper missions evidence-driven rather than using a permanent persona catalog.
- In `orchestrator-callback`, keep this control thread orchestrator-only and launch separate implementation and independent review owners with the concrete run-time orchestrator thread ID and one logical terminal-result identity each.
- Keep one owner per mutable checkout and verify repo/worktree/symbolic branch before child mutation or review.
- Bind callback targets at run time whenever callbacks are required or used.
- Acknowledge emitted completion handles and nonces before another child launch.
- Use structured completion callbacks as the normal path with no status, sleep, or wait polling. Reasoned recovery is exceptional, rate-limited, and durable.
- Renew degraded coordinators only at a safe ownership boundary with Beads, turn-doc, PR, branch, checkout, and activation evidence. Do not prescribe a fixed coordinator lifetime or topology.
- Keep one active external implementation PR unless Beads and the accepted plan explicitly permit more. Parallel owner branches may feed that one integration PR.
- Use independent review and resolve CI before completion.
- Update the existing phase turn doc and Beads; file follow-ups instead of widening scope.
- Continue phase-by-phase unless complete, blocked, interrupted, unresolved, or explicitly `--once`.

## Phase Ledger

| Beads Issue | Phase | Outcome | Phase Doc | Depends On | Status |
|---|---|---|---|---|---|
| `gh-forgejo-shim-c00` | S0 | Contributors get the same test result whether or not the host already has `/usr/bin/gh`. | `00-s0-make-the-test-baseline-deterministic.md` | none | open |
| `gh-forgejo-shim-c01` | S1 | A maintainer can run one suite and see the exact process behavior ChatGPT expects, including native GitHub delegation. | `01-s1-freeze-the-chatgpt-build-6720-process-contract.md` | `gh-forgejo-shim-c00` | open |
| `gh-forgejo-shim-c02` | S2 | A user can configure one canonical Forgejo host with transport aliases and inspect the result with `gfj config list`. | `02-s2-add-typed-backward-compatible-host-profiles.md` | `gh-forgejo-shim-c01` | open |
| `gh-forgejo-shim-c03` | S3 | The original `--repo 127.0.0.1/...` command targets canonical Forgejo, while unknown Forgejo operations fail locally. | `03-s3-resolve-identity-before-routing.md` | `gh-forgejo-shim-c02` | open |
| `gh-forgejo-shim-c04` | C1 | The host registry, canonical identity, and provider-first routing pattern can support later read behavior without unsafe GitHub fallthrough. | `04-c1-pass-the-host-identity-and-routing-checkpoint.md` | `gh-forgejo-shim-c03` | open |
| `gh-forgejo-shim-c05` | S4 | `gh auth status --active --hostname ALIAS` finds the canonical host credential without exposing the token. | `05-s4-make-authentication-use-canonical-host-identity.md` | `gh-forgejo-shim-c04` | open |
| `gh-forgejo-shim-c06` | S5 | A user gets a bounded result or clear timeout, and the trace states which output it observed. | `06-s5-enforce-the-command-deadline-and-truthful-tracing.md` | `gh-forgejo-shim-c04` | open |
| `gh-forgejo-shim-c07` | S6 | ChatGPT can truthfully decide whether the current branch has a pull request. | `07-s6-make-pr-discovery-honor-me-and-pagination.md` | `gh-forgejo-shim-c05`, `gh-forgejo-shim-c06` | open |
| `gh-forgejo-shim-c08` | C2 | The pull request module, test seam, shared deadline, and pagination rules are sound before view projections grow around them. | `08-c2-pass-the-pr-discovery-checkpoint.md` | `gh-forgejo-shim-c07` | open |
| `gh-forgejo-shim-c09` | S7 | `gh pr view` renders title, body, branches, author, merge state, timestamps, counts, and links with GitHub-compatible shapes. | `09-s7-render-the-pr-header-and-core-fields.md` | `gh-forgejo-shim-c08` | open |
| `gh-forgejo-shim-c10` | S8 | ChatGPT's PR activity view shows the complete discussion and review state. | `10-s8-add-comments-commits-reviews-and-reviewer-requests.md` | `gh-forgejo-shim-c09` | open |
| `gh-forgejo-shim-c11` | S9 | ChatGPT shows current Forgejo checks, and a PR with no checks produces the same nonfatal warning as real `gh`. | `11-s9-render-current-checks-and-real-no-check-behavior.md` | `gh-forgejo-shim-c10` | open |
| `gh-forgejo-shim-c12` | C3 | The full read contract returns current, truthful PR and check data without leaking Forgejo work to GitHub. | `12-c3-pass-the-complete-read-contract-checkpoint.md` | `gh-forgejo-shim-c11` | open |
| `gh-forgejo-shim-c13` | S10 | The same binary handles two unrelated Forgejo identities without sharing aliases or credentials. | `13-s10-prove-generic-multi-host-behavior.md` | `gh-forgejo-shim-c12` | open |
| `gh-forgejo-shim-c14` | S11 | A fresh app-server child resolves the managed wrapper, while GitHub still runs through verified `gh` 2.96. | `14-s11-package-and-install-without-disturbing-the-host.md` | `gh-forgejo-shim-c12` | open |
| `gh-forgejo-shim-c15` | C4 | The reviewed integration commit can be packaged, installed, and removed without changing the system GitHub CLI or unrelated host state. | `15-c4-pass-the-packaging-and-rollback-checkpoint.md` | `gh-forgejo-shim-c13`, `gh-forgejo-shim-c14` | open |
| `gh-forgejo-shim-c16` | S12 | The desktop app opens and keeps the Forgejo PR screen current without harming GitHub repositories. | `16-s12-validate-the-native-chatgpt-pr-screen.md` | `gh-forgejo-shim-c15` | open |
| `gh-forgejo-shim-c17` | C5 | Independent review and manual product checks confirm the native ChatGPT PR screen works for Forgejo and still works for GitHub. | `17-c5-accept-the-native-chatgpt-product-result.md` | `gh-forgejo-shim-c16` | open |

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

## Branch And PR Constraints

- Use `lavender/chatgpt-support` as the integration branch only while it retains the accepted plan and verified base.
- Use one phase branch and one owned worktree per active slice.
- Keep one external implementation PR.
- Commit and push each red test, green implementation, and combined checkpoint repair separately.

## Storyboard

On epic completion, generate `docs/implementation/chatgpt-forgejo-pr-compatibility/storyboard-post-run-08-22-2026.html`. Use `impeccable` when available and `@pierre/diffs/ssr` for every diff.
