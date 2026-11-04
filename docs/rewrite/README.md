# Core rewrite: design history

Read these in this order. The plan review is the current plan. Where the older documents disagree with
it, the plan review wins.

| File | Date | What it is |
|---|---|---|
| [`assessment.md`](assessment.md) | 2026-10-30 | Read-only assessment of the shim: why it was unstable, and the case for rewriting the gh-emulation core |
| [`clients.md`](clients.md) | 2026-10-30 | Which clients call `gh` and how: a static scan, a survey of other tools, and live captures of ChatGPT, Claude desktop and Claude Code |
| [`rewrite-plan.md`](rewrite-plan.md) | 2026-10-30 | The first rewrite plan: tiers, contract mechanics, phases A to D, and §7 on emulating gh's CLI or GitHub's API |
| [`gateway-spike.md`](gateway-spike.md) | 2026-10-30 | The spike that ran real gh against a GitHub Enterprise-shaped host |
| [`plan-review-2026-11-01.md`](plan-review-2026-11-01.md) | 2026-11-01 | Option B (real gh plus an in-process gateway), the adversarial review of the plan, the amended order of work, and Kell's decisions D1 to D6 |
| [`launcher.md`](launcher.md) | 2026-11-02 | L0: how the `gh` wrapper runs real gh against the in-process gateway: config directory, socket, tokens, signals, exit codes, timeouts |
| [`release-notes-server-mode.md`](release-notes-server-mode.md) | 2026-11-03 | Draft of the release notes' server-mode section, to merge into the full notes |

The client captures that `clients.md` describes were taken with a logging gh wrapper, gh-capture.
The wrapper and the raw captures, redacted, arrive with bead A1 (`cwe.1`).
