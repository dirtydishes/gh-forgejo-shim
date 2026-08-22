# Run Loop: ChatGPT desktop Forgejo pull request compatibility

Dirtyloop version: `2`

Canonical tracker: Beads epic `gh-forgejo-shim-6y9`

Harness binding: `docs/implementation/chatgpt-forgejo-pr-compatibility/runtime/binding.json`

Runtime generation: `docs/implementation/chatgpt-forgejo-pr-compatibility/runtime/generation.json`

Execution profile: `adaptive`

Pi adapter policy when present: `docs/implementation/chatgpt-forgejo-pr-compatibility/runtime/adapter.json`

Start from:

- `docs/implementation/chatgpt-forgejo-pr-compatibility/IMPLEMENT.md`
- `docs/implementation/chatgpt-forgejo-pr-compatibility/loop-state.md`
- `docs/implementation/chatgpt-forgejo-pr-compatibility/runtime/generation.json`
- the ready Beads phase and its linked phase doc

## Run Contract

1. Select one ready phase, or one accepted dispatch set of dependency-ready qualified tracer-bullet leaves, and read every selected contract's outcome, constraints, decisions, open questions, acceptance evidence, and replanning triggers. When a plan materializes broad Phase milestones, never claim one; record phase membership as metadata rather than a Beads `parent-child` edge and execute only qualified leaves.
2. Resolve and print the exact loop root, repository root, Beads epic, coordinator identity, branch, HEAD, loaded skill path, and generated adapter identity before selecting work. Validate every deterministic generated artifact hash, validate mutable `loop-state.md` against Beads semantics, and stop on a mixed generation, loaded-skill closure mismatch, unknown builder contract, or wrong checkout. Before a declared builder bootstrap completes, require `node bin/skills.js doctor` to report stable Dirtyloops source parity current when available, or require equivalent full loaded-source identity evidence. After bootstrap, never compare with or install moving candidate source; require the B8/B9 verifier to prove the loaded frozen builder against `runtime/builder-certificate.json` and `runtime/builder-activation.json`. A matching harness-reference hash alone is insufficient. `run` never repairs these mismatches implicitly.
3. Require `runtime/activation.json` to be ready for this generation and coordinator, but interpret it only as builder-control activation. It does not certify a future Program runtime. For Pi, use `dirtyloops_runtime action=activate`, launch its exact vanilla probe, and acknowledge the production completion with the exact absolute loop `doc_root`. Same-session reload is not fresh. Unsupported destination activation is blocked.
4. Run the bundled execution-readiness audit for the selected phase as a boundary check. If Beads already re-sliced a shared broad phase, pass the exact selected tracer-bullet issue JSON while preserving hashes for both sources. Then run `scripts/launch-readiness.mjs` for that leaf. Proceed only when the boundary is `ready` and the live result is `launchable`; the latter requires the exact ID in `bd ready`, concrete ownership, matching dependencies, and the active generation.
5. Inspect the capabilities required by the chosen builder strategy and write a compact orchestration brief to the existing phase turn doc. Direct coordinator work may proceed without a separate launch adapter only after exact root, prepared worktree, symbolic branch, HEAD, status, and exclusive ownership are proved. Until an executable adapter is certified, do not launch mutable owners; read-only helpers or reviewers must report and verify the exact absolute checkout, branch, and HEAD before inspection. If this accepted loop declares a builder bootstrap, require its immutable `runtime/builder-certificate.json` and separate fresh `runtime/builder-activation.json`; builder generation readiness alone is insufficient. After bootstrap, compare the loaded builder to that frozen certificate, not to moving candidate source.
6. Enforce the persisted execution profile. In `adaptive`, choose direct work, bounded delegates, durable sessions, native multi-agent execution, or a hybrid. In `orchestrator-callback`, remain orchestrator-only and appoint separate implementation and independent review owners with the concrete current orchestrator thread ID and one logical terminal-result identity each; delivery may repeat.
7. For both implementation and review, enumerate independent risks and evidence questions, then strongly encourage useful bounded helper missions. Use at most 20 helper missions per stage. Bound active concurrency by the user's configured limit, certified runtime capacity, and currently available slots; use waves when needed. Do not assume an undocumented default, create filler agents, or use a permanent persona catalog. Record why when a non-trivial stage uses no helpers.
8. Keep one owner per mutable checkout. Any child that mutates or reviews code must start in the intended repo/worktree and symbolic branch/ref. Bind concrete completion delivery at run time when required or used. Compile broad phases into qualified tracer-bullet leaves before dispatch; review their branch diffs, integrate them in dependency order, and keep them as internal contributions to one external implementation PR unless the accepted plan says otherwise.
9. Persist each valid completion handle and nonce before acknowledging it through the bound adapter. Return a replayable acknowledgement response for duplicate delivery and never duplicate downstream effects. Do not launch a replacement owner while delivery or acknowledgement remains unresolved.
10. Under structured completion, use callbacks as the normal path with no status, sleep, or wait polling. Use reason-required, rate-limited recovery only from adapter-observed missed-heartbeat evidence, unacknowledged delivery, or explicit operator intervention.
11. After degraded delivery, renew at a verified safe ownership boundary and record Beads, turn-doc, PR, branch, checkout, and fresh activation evidence. Context-pressure renewal is adaptive and sets no fixed lifetime or topology.
12. Implement within scope. If evidence invalidates accepted intent or phase structure, record a proposed plan amendment instead of silently rewriting the plan.
13. Obtain proportionate independent review using `thermo-nuclear-code-quality-review` when a reviewer agent is used. Resolve CI to an allowed terminal state.
14. Update the existing turn doc, Beads, export when applicable, and `loop-state.md`.
15. Continue until complete, blocked, interrupted, unresolved, or explicitly `--once`.

## User Constraints

- See the accepted plan and Beads metadata.

## Start Prompt

Run the `adaptive` dirtyloop for Beads epic `gh-forgejo-shim-6y9`. Preserve accepted intent, enforce the ownership profile, choose helper missions from current evidence and capabilities, and record the orchestration brief before broad work.
