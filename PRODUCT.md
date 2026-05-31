# Product

## Register

product

## Users

Developers and coding agents working in Forgejo repositories with tools that expect GitHub-style repository metadata, pull request state, and issue workflows through the GitHub CLI.

## Product Purpose

`gh-forgejo-shim` gives GitHub-oriented tools enough `gh` compatibility to work inside allowlisted Forgejo checkouts without pretending to be a complete GitHub CLI replacement. Success means repository, pull request, issue, branch, and auth probes return practical GitHub-shaped answers while real GitHub repositories continue to delegate to the real `gh`.

## Brand Personality

Practical, calm, and explicit. The product should feel like trustworthy glue for developer workflows: small surface area, clear failure modes, no theatrical claims.

## Anti-references

Avoid marketing-page drama, ornamental UI, vague compatibility promises, and broad emulation claims that hide unsupported behavior. Avoid docs that say "updated" or "improved" without naming the exact command, flag, payload, or fallback.

## Design Principles

- Make compatibility honest: support the commands tools actually probe, and delegate everything else.
- Prefer GitHub-shaped output where tooling expects it, but keep Forgejo-specific limits visible.
- Keep setup recoverable with plain commands, clear paths, and predictable rollback.
- Optimize for non-interactive developer tools as much as human terminal use.
- Document before and after behavior so future sessions do not have to rediscover the workflow.

## Accessibility & Inclusion

User-facing documentation should be readable in dark mode, use high-contrast body text, keep line lengths comfortable, and avoid relying on color alone to communicate status. CLI output should remain plain text and useful in non-interactive or screen-reader-adjacent workflows.
