# Contract tests

These cases compare the shim, invoked as `gh`, with committed output from official gh 2.103.3.
`scripts/contract/run.sh` starts real Forgejo 15 and uses only local data. CI needs no GitHub login.

Run from the checkout root with Rust from `rust-toolchain.toml`, Python 3.9+, git, and Docker available:

```sh
scripts/contract/run.sh
scripts/contract/ratchet-self-test.sh
```

The Cargo entry is `tests/contract.rs`, registered in the crate manifest because this workspace has
no root package. Its live test is ignored by default. It calls the standard-library Python driver;
ordinary `cargo test --workspace --locked` runs the driver's safety and comparison tests.

## Updating the oracle

Run this only when you intend to replace the committed GitHub observations:

```sh
scripts/contract/capture-oracle.sh
```

Use repeated `--case <id>` options to capture selected cases and preserve the other captures and scenarios.
Cases with `kind = "shim-local"` use `expected` instead of `oracle`. Capture skips these cases, and
`oracle_argv` rejects them. The exact local allowlist covers unsupported merge, edit, and repository API calls.
The runner still checks exit code, both streams, and the real-gh decoy for these cases.

Capture requires curl and the host's existing gh login for `github.com`. It validates every case
before reading the login through `gh auth token --hostname github.com`. It downloads the official
gh 2.103.3 archive and checks it against the committed upstream release checksums in
`gh-checksums.txt`. Linux releases use tar.gz; official macOS releases use zip.

The capture commands are read-only and limited to `dirtydishes/gfj-oracle`. The case validator rejects
writes, API method/body flags, other repos, web/browser flags, token disclosure, and environment
overrides that could redirect or authenticate a request. The token stays in memory and the child
environment. Captures refuse output containing it. Refreshing also records PRs 1-7 in `scenarios.json`
and the downloaded CLI's version in `oracle-version.txt`. Review those changes before committing.

## Cases and comparison

Each `cases/<id>.json` records `argv`, `repo`, optional `pr` and formatting `env`, raw `oracle`
exit/stdout/stderr, and its ordered `normalize` list. `--repo forge.test/dirtydishes/gfj-oracle`
becomes the GitHub twin during capture. The runner compares both streams and the exit code.

Normalizers run only when a case names them:

| Name | What it replaces |
| --- | --- |
| `host` | Exact `github.com` or `forge.test` host names with `<host>` |
| `pr_urls` | Forgejo `/pulls/N` and GitHub `/pull/N` with `/pull/N`, preserving N |
| `timestamps` | ISO timestamps with `<timestamp>` |
| `durations` | gh elapsed durations such as `5s` and `1m4s` with `<duration>` |
| `shas` | 7-40 digit hexadecimal commit IDs with `<sha>` |
| `node_ids` | JSON `id`, `node_id`, and `nodeId` string values with `<node-id>` |
| `check_details_urls` | Check-run Actions job URLs and JSON `detailsUrl` values with `<check-url>` |
| `json` | JSON object key order and whitespace, preserving values and array order |

`json` goes beyond the brief's normalizer list. Each case using it includes a one-line justification
in `normalize_reason`. It avoids failures due solely to JSON serialization. Invalid or missing JSON
still differs from valid output. Plain text spacing, row order, streams, and exit codes stay significant.

An `xfail` marks an observed product gap; `xfail_diff` records its initial one-line diff. Cases without
`xfail` must match. Cases with it must differ. A match fails with `remove xfail from <id>`. The runner
prints every diff and a `pass N, xfail N, unexpected N` summary. The scratch-copy self-test proves
both directions without editing the committed cases.

## Local twin and isolation

`seed-forgejo.sh <command> [args...]` seeds Forgejo, gives the child `GFJS_TOKEN`, `GFJS_SEED_HOME`,
and `GFJS_WORK_DIR`, then tears down. Tokens never enter the config file. Each run owns a unique
`gfjs-contract-*` container, network, and data volume. Cleanup removes only those exact names, including
on ordinary failure, Ctrl-C, or SIGTERM. Docker or host crashes may require removing that run's resources.
There is no global prune. Work files stay under ignored `target/contract` and get removed after the run.

The seed copies the captured PR numbers, branches, titles, bodies, draft flags, and merged/closed states.
Forgejo detects drafts from a title prefix, so the disposable server uses `scenario: draft` as its prefix
to preserve PR #4's exact title. Status contexts on #1 and #2 use Forgejo Actions names and relative job
URLs. #5 has no statuses. #3's old pending job is stale, so no case tests it. PRs 8-10 are not seeded.
Seeding verifies source states and statuses before starting the contract test.

Each case gets a fresh HOME/config and a git repo with origin
`https://forge.test/dirtydishes/gfj-oracle.git`. It invokes an absolute `gh` wrapper for the built binary.
PATH contains git and a failing real-gh decoy; it contains no host `security` command, so macOS cannot
consult the user's Keychain. The child gets no host GitHub token, proxy, or Git config. Calling the decoy,
timing out, or crashing always fails, even on an expected-failure case. Python unit tests exercise this
fallback check with a deliberately broken shim.

The exact ChatGPT `--author @me` probe currently passes because both the viewer and PR #1's author are
`dirtydishes`. The added `pr-list-author-other` case uses `github-actions[bot]` and exposes the ignored
author filter. The case still records that handler gap.

## Invocation grammar

`tiers.toml` contains the generated gh 2.103.3 grammar. Only its `support` labels change in this checkout.
`usage-errors.json` supplies the help descriptions and available command names needed for gh's exact errors.
`python3 scripts/contract/capture-usage.py` rebuilds that file from checksum-pinned official gh with local
`gh help` calls. It reads no login and executes none of the named commands.

After `cargo build --workspace --locked`, `python3 scripts/contract/check-parser.py` compares 107 local
argument errors against the pinned CLI. It uses a temporary HOME, dummy credentials, read-only commands,
and a closed proxy. Ordinary Cargo tests compare committed error captures and check routing through a decoy.
