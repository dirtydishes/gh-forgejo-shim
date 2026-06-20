# Rust Rewrite Compatibility Contract

This directory freezes the Python behavior that the Rust rewrite must either preserve or intentionally migrate.

`compatibility-contract.v1.json` is the source of truth for phase 01. It contains:

- the supported and delegated command matrix
- Codex-critical golden probes
- routed Forgejo API request expectations
- trace and git-recorder JSONL schemas
- intentionally accepted-but-ignored flags
- baseline Python/real-`gh` timing numbers and Rust budgets

Future Rust phases should consume this JSON directly in tests. The fixture is intentionally static so Rust validation can run without launching Codex or hitting live Forgejo/GitHub services.

When behavior changes intentionally, add a new contract version and reference the bead or decision that authorizes the migration.
