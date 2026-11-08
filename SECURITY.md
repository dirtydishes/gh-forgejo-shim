# Security

## Reporting a vulnerability

Please report security problems privately, through GitHub's
[private vulnerability reporting](https://github.com/dirtydishes/gh-forgejo-shim/security/advisories/new),
not as a public issue. Only the maintainer sees the report, and we can work on a fix together
before anything is public.

A useful report says:

- the version or commit you tested, and whether you used client-only or server mode;
- what an attacker needs (network position, a token, a crafted repository name and so on) and what
  they gain;
- how to reproduce it, ideally against a local mock server with a made-up token;
- what you did not show, so we don't overstate it.

Never include a real token, password or private host in a report. If you found one in a trace or
log, say where, and leave the value out.

## What happens next

This is a small project with one maintainer, so replies are best effort, but security reports come
first. You'll get an answer saying whether we could reproduce the problem, and then the fix's pull
request and the release it ships in. Each fix credits the reporter in its pull request and the
release notes, unless you'd rather not be named.

## Scope

Reports are welcome for anything in this repository: the `gh` wrapper and `gfj`, the gateway
(`fga-server`), the release archives and gateway images, and the install and packaging scripts.
Problems in Forgejo itself or in `gh` belong with those projects.

## Supported versions

Fixes go into the latest release. Please test against it, or against `main`, before reporting.
