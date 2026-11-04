# Audit: code that survives the gateway restructure (2026-11-01)

The shim is becoming real `gh` plus an in-process GitHub-API gateway. This
audit covers the code existing users keep: install, binary names, config,
token storage, routing, diagnostics and tracing. The gh-emulation core
(`read_only.rs`, `checks.rs`, `create.rs`, `normalize.rs`, `invocation.rs`,
`forgejo.rs`, `repo.rs`) and the gateway crates were out of scope, except
where surviving code calls into them.

Base commit: `e17a07e`. Tracking epic: `gh-forgejo-shim-jko`.

## Method

Three reviewers each took one slice: credentials and config, routing and the
`gh` wrapper, and diagnostics, tracing and install. They could add failing
tests only. Every finding below has a test that fails on `e17a07e` on macOS
and on Linux (`rust:1.97.1`); D14 and D15 run on Linux only. Each test was read
and run against the unmodified code before being kept. One rebuttal round
settled the disputed severities and rejections.

The tests live in `crates/gh-forgejo-shim/tests/audit_{credentials,routing,diagnostics}.rs`.
Each is `#[ignore]`d with the bead that tracks its fix, so CI stays green. To
run them:

```sh
cargo test -p gh-forgejo-shim --test audit_credentials --test audit_routing \
  --test audit_diagnostics -- --ignored --test-threads=1
```

A fix is done when its test passes and the `#[ignore]` is removed.

## High

| ID | Bead | Location | Finding | Test |
|---|---|---|---|---|
| R3 | jko.1 | `shim.rs:63` | `install-shim --force` writes through a `gh` symlink and overwrites the real `gh` it points to. | `audit_route_force_install_symlink` |
| R2 | jko.2 | `repo.rs:71` via `routing.rs:135` | `-R owner/repo` and `GH_REPO=owner/repo` without `GH_HOST` are ignored inside a checkout, so the command acts on the checkout's repo instead. | `audit_route_short_repo_override`, `audit_route_short_gh_repo_override` |
| D1 | jko.3 | `git_recorder.rs:101` | `trace git-recorder remove` runs `remove_dir_all` on the wrapper directory, deleting unrelated files when that directory was pre-existing. | `audit_diag_recorder_remove_preserves_unrelated_files` |
| D18 | jko.4 | `git_recorder.rs:307` | Recreating a recorder while its directory is on `PATH` selects the wrapper as real git, so it execs itself without bound. | `audit_diag_recorder_recreate_prevents_self_recursion` |

## Medium

| ID | Bead | Location | Finding | Test |
|---|---|---|---|---|
| R4 | jko.5 | `routing.rs:135` | `gh --version` in a Forgejo checkout routes to Forgejo and fails. | `audit_route_version_in_forgejo_checkout` |
| R5 | jko.6 | `repo.rs:155` | HTTPS remotes lose their port, so an allowlisted `host:port` never matches. | `audit_route_https_port_allowlist` |
| R7 | jko.7 | `routing.rs:90` | A non-UTF-8 value in any environment variable panics the wrapper. | `audit_route_non_utf8_environment_passthrough` |
| C3 | jko.8 | `auth.rs:245` | A non-object `hosts` in the auth file is silently replaced, losing stored tokens. | `audit_cred_malformed_hosts_does_not_reset_store` |
| C5 | jko.21 | `config.rs:270` | Config writes truncate in place, so a write error leaves an empty `config.toml` and every setting is lost. | `audit_cred_failed_config_write_preserves_original_file` |
| D3 | jko.9 | `git_recorder.rs:498` | Redaction misses `"token": value` and `token: value` written with a space. | `audit_diag_git_trace_json_secret` |
| D4 | jko.10 | `git_recorder.rs:487` | Only the first `Authorization` header in an excerpt is redacted. | `audit_diag_git_trace_multiple_authorization_headers` |
| D6 | jko.11 | `trace.rs:399` | Concurrent writers interleave inside JSONL records (30 of 240 survived on Linux). | `audit_diag_trace_concurrent_jsonl` |
| D13 | jko.12 | `doctor.rs:297` | `doctor` passes when the wrapper's target binary is missing. | `audit_diag_doctor_checks_wrapper_target` |
| D14 | jko.13 | `cli.rs:390` | `uninstall-shim` ignores the directory `bootstrap` chose (for example `~/.cargo/bin`) and leaves the active wrapper behind. | `audit_diag_uninstall_reverses_default_bootstrap` (Linux only) |
| D15 | jko.14 | `docs/installation.md:59` | The documented `tar -xzf … gh-forgejo-shim gfj` fails on GNU tar because archive entries are `./`-prefixed. | `audit_diag_release_documented_extract` (Linux only) |

## Low

| ID | Bead | Location | Finding | Test |
|---|---|---|---|---|
| R1 | jko.15 | `external.rs:260` | A copied or hardlinked dispatcher passes the self-check as real `gh`. There is no recursion. | `audit_route_copied_dispatcher_on_path`, `audit_route_hardlinked_real_gh_override` |
| R6 | jko.16 | `external.rs:104`, `git_recorder.rs:177` | A child killed by a signal is reported as exit 1. | `audit_route_signal_passthrough`, `audit_diag_recorder_preserves_signal_exit` |
| R8 | jko.17 | `shim.rs:57` | Install without `--force` writes through a dangling `gh` symlink. | `audit_diag_install_refuses_dangling_symlink` |
| C1 | jko.18 | `config.rs:303` | `github.com:443` bypasses the GitHub-host exclusion and receives the Forgejo token. | `audit_cred_github_default_port_does_not_receive_forgejo_token`, `audit_cred_import_rejects_github_default_port` |
| C2 | jko.19 | `config.rs:97` | TOML parse errors echo the offending line, including URL passwords. | `audit_cred_config_parse_error_redacts_url_password` |
| C4 | jko.20 | `cli.rs:994` | `auth import` stores the token, then fails on a corrupt config. | `audit_cred_failed_import_preserves_previous_token` |
| C6 | jko.22 | `config.rs:229` | Config rewrites drop unknown keys and tables. | `audit_cred_config_update_preserves_unknown_settings` |
| D2 | jko.23 | `git_recorder.rs:512` | `access_token` inside a URL query survives argv redaction. | `audit_diag_git_trace_query_token` |
| D5 | jko.24 | `shim.rs:115` | Install keeps group/other write bits on an existing wrapper. | `audit_diag_install_drops_other_write_permissions` |
| D7 | jko.25 | `trace.rs:395` | A pre-existing trace file keeps a world-readable mode. | `audit_diag_trace_existing_file_permissions` |
| D9 | jko.26 | `trace.rs:156` | URL redaction stops at the first `@` in userinfo. | `audit_diag_gh_trace_url_password_at_sign` |
| D10 | jko.27 | `trace.rs:206` | JSON redaction ends at an escaped quote. | `audit_diag_gh_trace_json_escaped_secret` |
| D11 | jko.28 | `codex_smoke.rs:199` | `trace smoke` prints probe stderr without redaction. | `audit_diag_smoke_redacts_stderr` |
| D12 | jko.29 | `codex_smoke.rs:208` | `trace smoke` panics when a byte slice splits a UTF-8 character. | `audit_diag_smoke_utf8_stderr` |
| D19 | jko.30 | `trace_summary.rs:25` | A truncated UTF-8 tail makes `trace summary` reject the whole file. | `audit_diag_summary_invalid_utf8_tail` |

R2 and R5 sit in `repo.rs`, which the restructure replaces. Fix them there
only if the replacement lands later than the fix. Otherwise carry the tests
over to the new routing code.

## Rejected

- **Empty `PATH` entry skipped during real-`gh` discovery.** This is the safe
  behaviour. Honouring it would run a `gh` planted in an untrusted checkout.
- **Trace writers follow symlinks.** The trace path is always chosen by the
  user, and nothing defaults to a shared directory. Following a symlink there
  is ordinary append semantics.

## Merged

- The dangling-symlink install finding was reported by two reviewers. R8 keeps
  the end-to-end CLI test.
- Signal flattening in the wrapper and in the git recorder is one bug class,
  tracked as R6.

## Not found

Round 1 found no token on child argv, no keychain or `security` misuse, and no
auth-file permission problems. The changes from `b48ff8b` held up, apart from
the nested `hosts` reset (C3).
