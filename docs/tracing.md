# Tracing

When Codex.app reports `GitHub CLI unavailable`, `Pull request status unavailable`, or stale branch data, enable tracing before guessing at the fix. The shim can record the exact `gh` commands it handles or delegates:

```sh
export FJ_SHIM_TRACE="$PWD/codex-probe-trace.jsonl"
export FJ_SHIM_TRACE_BODY=1
```

`FJ_SHIM_TRACE` is opt-in. With it unset, runtime behavior is unchanged. Trace records include timestamp, cwd, argv, route decision, host/repo, duration, exit code, stdout/stderr byte counts, relevant environment keys, and redacted excerpts when `FJ_SHIM_TRACE_BODY=1` is also set. Redaction hides credentials in common forms but not every form (see [#232](https://github.com/dirtydishes/gh-forgejo-shim/issues/232) and [#233](https://github.com/dirtydishes/gh-forgejo-shim/issues/233)), so read a trace before you share it; `gh auth token` stdout is always suppressed in trace excerpts. A new trace file is created owner-only (0600), and an existing one is narrowed to 0600 before records are added. If the file belongs to another user, so it can't be narrowed, no records are written to it.

## Redaction

Redaction errs toward hiding. Any value under a key with `password`, `secret`, `token`, `credential`, `api_key`, `apikey` or `private_key` in its name is hidden (brackets count as part of a name, as in `config[secret]`), and so is any key with `pass` anywhere in it, unless the key part is exactly one of a few ordinary words such as `bypass`, `compass` or `lowpass`. Outside JSON, a hidden value takes the rest of its line with it, and runs past the line's end while a quote is still open. In JSON, a quoted value ends at its closing quote when a comma, a bracket or the line's end follows, and an array or object runs to its closing bracket, across lines. In the gh trace, a flag with one of those words in its name hides its value, and so do `--body` and `-b` for `gh secret`. URL passwords are hidden too, and so are tokens that start with a known prefix (`ghp_`, `github_pat_`, `glpat-`, `gitea_` and others). The cost: some harmless text shows as `<redacted>` (`[REDACTED]` in git traces), such as `byPassEnabled=true`, or `status=200` after `password=x` on the same line. A hidden value costs a little readability; a leaked one costs a credential.

## Summarize a trace

```sh
gfj trace summarize codex-probe-trace.jsonl
```

## Smoke probes

Run the local Codex-style smoke probes directly:

```sh
gfj trace smoke
```

The smoke command runs the observed read-only probe family: `gh --version`, `gh auth status`, `gh api user`, PR status/list JSON checks, and the local Git repo/default/upstream/current-branch/ref checks that branch and PR UI usually depend on.

## Record raw git calls

Some Codex branch UI checks use raw `git`, not `gh`. To capture those calls for one launch session, create a temporary `git` recorder wrapper and use the printed launch environment for that Codex session:

```sh
gfj trace git-recorder create "$PWD/codex-probe-trace.jsonl"
```

Remove it when the session is done:

```sh
gfj trace git-recorder remove /path/to/printed/wrapper-dir
```

The recorder wrapper is temporary, prepends one directory to `PATH`, forwards real `git` stdout/stderr, and appends JSONL records to the same trace file.
