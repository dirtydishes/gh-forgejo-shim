# gh-forgejo-shim assessment (Opus, read-only)

Date: 2026-10-30. Subject: this repo at `main` = `211bdec` (2026-07-03), plus the
unmerged `lavender/chatgpt-support` branch at `69cb1c4` (2026-08-25). File references are to `main` and
relative to `crates/gh-forgejo-shim/src/` unless noted. Nothing in the shim repo was modified. The branch
was tested in a throwaway clone.

## TL;DR

- **The history needs one correction.** The original implementation was **Python/pipx**, not TypeScript:
  `6fc0bfe` adds `pyproject.toml` and `src/gh_forgejo_shim/*.py`. Ten phased PRs ported it to Rust in a
  single day (2026-06-20, `#17`–`#26`), with the Python output frozen as the contract.
- **It is not installed anywhere.** It was rolled back on 2026-07-09, and beads `l43` and `ltq` (closed
  2026-08-18) record a full purge from the maintainer's machines. The review machine agreed: `gh` resolved
  to Homebrew's gh 2.103.0 with no wrapper, config, or LaunchAgent. Keeping it therefore has no
  migration cost.
- **Main builds and all 155 tests pass, but clippy `-D warnings` fails** on current stable, and CI uses
  unpinned `@stable`.
- **The instability is mostly a design problem.** The shim has no oracle for "correct gh behavior", and
  when it doesn't know something it silently degrades instead of failing. The tests make this worse
  because they pin the old Python shim's output, not gh's. Upstream churn is real but moderate, and the
  design turns each change into a silent "status unavailable" instead of a visible error.
- **Recommendation: rewrite the gh-emulation core, contract-first. Keep the installation and diagnostics
  shell. Harvest `lavender/chatgpt-support`; don't merge it.** Details in [Recommendation](#repair-or-rewrite).

## 1. What it does today

**Invocation and interception.** `gfj install-shim` writes `~/.local/bin/gh` as a shell wrapper that
`exec`s `gh-forgejo-shim gh "$@"` and carries a marker comment (`shim.rs:24-29`). For GUI apps (Codex.app,
ChatGPT desktop) it also installs a LaunchAgent that sets the launchd PATH (`gui_path.rs:12-56`).
`gfj bootstrap` does both and allowlists the repo's host. When argv[1] is `gh`, `cli.rs:30-35` hands off
to `routing::run_gh_from_env`.

**Routing** (`routing.rs:140-175`). A small allowlist of commands is eligible
(`routing.rs:15-20`: `pr {checks,checkout,co,comment,create,diff,list,new,status,view}`,
`issue {create,list,ls,new,view}`, `repo view`, `auth {status,token}`, and `api user` only). The repo is
detected from `-R`, a URL argument, `GH_REPO`, `GH_HOST`+remote, or the git remote (`repo.rs:75-113`).
If the host is in the allowlist and is not github.com, the invocation is handled natively by
`read_only::run`. **Anything else goes to the real gh**, found by `external.rs:50-80`, which skips the
managed wrapper.

**Native handling.** `read_only.rs` has 3,549 lines, including the mutating commands despite the name. It
contains a hand-written argv parser for each command (`read_only.rs:1353-1982`), calls Forgejo REST
through a `reqwest` client (`forgejo.rs`), and reshapes the responses into GitHub-style JSON
(`normalize.rs`). It also hand-rolls subsets of `--jq` and `--template` (`normalize.rs:396-485`).

**Where the code goes.** Main has about 16.2k lines of Rust, including inline tests:

| Area | Files | ~LOC |
|---|---|---|
| gh emulation (routing, parsing, REST, normalization, auth, config) | routing, repo, read_only, normalize, forgejo, create, auth, config | 8.5k |
| Install, discovery, diagnostics | cli, bootstrap, doctor, setup, gui_path, shim, external | 4.7k |
| Tracing and probe tooling | trace, trace_summary, git_recorder, codex_smoke | 1.8k |

Roughly 40% of the code isn't gh emulation. It has its own bug stream (below).

## 2. Build and test status

Toolchain: rustc/cargo 1.97.1.

| Gate | main `211bdec` | `chatgpt-support` `69cb1c4` |
|---|---|---|
| `cargo build --workspace --locked` | pass | pass |
| `cargo test --workspace --locked` | **155 pass** (124 unit, 25 CLI, 6 normalization) | **235 pass, 2 ignored** |
| `cargo clippy … -D warnings` | **FAIL**: `normalize.rs:414` `question_mark` (new lint) | pass (fixed by bead `9l1`) |
| `cargo fmt --check` | pass | pass |

`.github/workflows/ci.yml` uses `dtolnay/rust-toolchain@stable` with no `rust-toolchain.toml`, so main's
CI would now fail. The branch plan (`docs/implementation/chatgpt-support/PLAN.md`, "Current baseline")
also records **2 CLI tests failing on one of the maintainer's Linux hosts**, because the "isolated"
fixture picks up the host's `/usr/bin/gh`. On the review machine (macOS) they pass, so the result depends
on the machine.

**How much behavior the tests pin down: little of what matters.**

- **The oracle is the old shim, not gh.** `compatibility-contract.v1.json` "freezes the Python behavior"
  (its `README.md`). Its golden probes are Python-shim outputs. Nothing on main compares against real
  `gh` output. Python-isms persist as a result: `python_text`, `value_to_python_string`,
  `python_json_string` in `normalize.rs`, and tests named `*_python_compatible_*` in `forgejo.rs` and
  `auth.rs`.
- **The contract isn't executed.** `tests/cli_scaffold.rs:775-799` only asserts that some contract IDs
  exist. The 36 `command_matrix` entries and 8 `quirks` drive no tests. Golden stdout is compared only at
  the normalization-function level (`tests/http_normalization.rs`, 6 tests).
- **No process-level Forgejo test.** On main, no test runs `gh pr list|view|checks` end to end through
  the binary against a fake Forgejo HTTP server. The single Forgejo process test is `auth status`
  (`tests/cli_scaffold.rs:427-448`, despite its "repo_selects_native_route" name).
- **The unit tests use a test-only twin of the entry point.** `read_only.rs:390-427`
  (`run_with_client`, `#[cfg(test)]`) duplicates `run_with_context` (`read_only.rs:346-387`) and
  bypasses real token discovery and the real HTTP client.
- **Nothing tests the failure policy.** No test asserts that unknown flags, fields, or jq expressions
  fail.

## 3. Where the instability comes from

### Evidence

Beads on main have 70 issues (28 bugs). Grouping the bugs from titles and close reasons:

- **GUI and PATH discovery of `gh` (~9 bugs):** `bjt`, `2je`, `aux`, `5uv`, `dge`, `k6t`, `b95`, `btf`,
  `wp8`, plus feature `vbc`. These come from macOS GUI apps launching with a minimal PATH and are
  unrelated to gh fidelity. They recur because each fix lands in a different layer (wrapper, launchd,
  doctor, bootstrap).
- **Output shape didn't match the client (~7):** `5ab` (pr status envelope), `ufb` (PR board fields,
  `--state merged`), `ini`, `ewk` and `xva` (PR URLs), `qg5` (relative status URLs), `2ey`. In every case
  the symptom was **"pull request status unavailable"** in the UI with no error anywhere. Someone had to
  reverse-engineer what the client wanted.
- **Routing broke GitHub (~3):** `xva` (GitHub PR info stopped passing through), `bh7` (stale allowlist),
  and `ot5`, which even planned to rewrite *delegated GitHub* `pr create` argv.
- **Fallout from the port (~6):** `shb`, `y0g`, `nou`, `cjr`, `yov`, `ex1`, all from the one-day port.

Unmerged branches: 31 `lavender/*` branches exist, and nearly all are merged or abandoned (`+0` to `+5`
ahead). Four drafts from 2026-06-17 (`#9`–`#12`) are superseded. The one that matters is
**`lavender/chatgpt-support` (draft PR #29, +172 commits, +27.7k/−1.3k lines)**. About 6.1k lines of that
are under `crates/` (code, tests, fixtures). The rest is loop machinery: duplicated schema snapshots, two copies of a 5,293-line
`PLAN.html`, and turn docs. It completed slices S0–S6 and stopped at checkpoint C2 after exhausting its
3-pass repair limit (`docs/implementation/chatgpt-forgejo-pr-compatibility/loop-state.md`). The open
finding concerned the recursive JSON traversal that **scrapes other tools' credential files** (details
below).

Other open PRs: `#28` (docs publishing) and `#30` (an external contributor, `pikos-apikos`: private CA
certs, CI `action_required`).

### Design problems, confirmed

These were checked by running the main binary with an isolated `HOME`, a fake real `gh` that logs and
exits 42, and a repo whose remote is `git.example.com`.

1. **Two config parsers that disagree.** The dispatcher hand-parses TOML (`routing.rs:434-536`; it needs
   `hosts = [...]` on one line and splits on `#` even inside strings). `gfj config`/`doctor` use the real
   `toml` crate (`config.rs:77-138`). With a multi-line `hosts` array, `gfj config list` prints
   `git.example.com`, but `gh pr list` **delegates to the real gh**. `normalize_host`,
   `is_known_github_host`, and `is_forgejo_host` are also duplicated (`routing.rs:47-57,198-216` and
   `config.rs:41-53,215-232`).
2. **Unknown Forgejo operations reach real gh.** `gh pr merge`, `gh pr edit`, and
   `gh api repos/o/r/pulls` in a Forgejo checkout all printed `REAL-GH: …` (`routing.rs:158-160`), so gh
   then works against the wrong forge. The branch's own plan names this as decision 4: "unknown operations
   … fail locally".
3. **Silent degradation instead of errors.** gh itself rejects unknown fields and flags; the shim doesn't.
   - Unknown `--json` fields are dropped (`normalize.rs:487-503`). `auth status --json hosts,bogus`
     exits 0.
   - ChatGPT build 6720 requests a 25-field `pr view` (`tests/fixtures/chatgpt/build-6720/process-contract.json`
     on the branch). Main supports 16 of those fields (`normalize.rs:5-23`) and silently omits
     `autoMergeRequest`, `body`, `comments`, `commits`, `headRefOid`, `latestReviews`, `mergedAt`,
     `mergedBy`, and `reviews`. `pr checks` omits `event`.
   - `pr list --author/--label/--assignee/--search` are parsed and **ignored** (`read_only.rs:1497-1506`;
     bead `9r8`, open since 2026-05-31). ChatGPT sends `--author @me`. The contract itself lists
     "accepted_but_ignored" quirks.
   - The jq subset returns the **whole input** for any expression that doesn't start with `.`
     (`normalize.rs:405-407`). `--jq 'keys'` printed the full object, `--jq '.hosts | keys'` printed
     nothing, and both exited 0.
   - `pr checks` with no checks exits 0 (`read_only.rs:878-893`). Real gh 2.96 exits 1 with
     `no checks reported on the '<branch>' branch` (control fixture on the branch).
   - `auth status --json` reports `"login": ""` (`read_only.rs:2002-2018`).

   Each of these produces "status unavailable" in a GUI rather than an error a trace would show.
4. **No pagination.** `list_pulls` fetches one page and then filters `--head` on the client side
   (`forgejo.rs:386-410`). In a repo with more PRs than Forgejo's default page size, ChatGPT's
   `pr list --head B --state all` can miss the PR.
5. **Heuristic credential scraping.** At runtime, `auth.rs:100-121` reads 7 third-party config files
   (fj, tea, gitea). It recursively searches JSON for any `token`/`access_token` near a host-like key
   (`auth.rs:272-350`). For YAML it takes the first `token:` line within 2,000 characters after the first
   mention of the host (`auth.rs:352-360`), which can be the *next* host's token. Separately,
   `FJ_SHIM_TOKEN`/`FORGEJO_TOKEN`/`GITEA_TOKEN`/`FJ_TOKEN` are host-agnostic and override host-specific
   stored tokens (`auth.rs:84`). With two Forgejo hosts, one host's token is sent to the other. This is
   the code the stalled C2 review kept finding new edge cases in. The heuristic should go, not be
   hardened.
6. **Three argv parsers on the branch.** Main has the routing parser (`routing.rs:363-432`) and per-command
   parsers in `read_only.rs`. The branch adds a third (`invocation.rs`, 742 lines, "certified grammar")
   while leaving `read_only.rs` at 3,623 lines. **Two HTTP stacks:** `reqwest` (`forgejo.rs:4`) and
   `ureq` (`auth.rs:208`).

### Verdict

- **Design: primary.** There is no contract stating which gh behavior is emulated and against what
  oracle, and the default for anything unknown is to delegate or drop silently. Every client change
  therefore surfaces as a silent UI failure. It is then fixed by adding one more field or flag to an
  allowlist, which is the pattern in the bug list.
- **Testing: amplifier.** The tests pin the old shim's output, skip the process seam for Forgejo, and
  depend on the machine they run on. They passed throughout every regression above.
- **Upstream churn: real but moderate.** Five client-driven changes landed in roughly three months: the
  pr status envelope, PR board fields, PR URLs, availability probes (`auth status`/`api user`), and
  ChatGPT build 6720 (25-field view, `@me`, `--active`). A strict shim plus the existing trace recorder
  would have turned each into one obvious error line.
- **Environment discovery: a separate, real problem.** It is unrelated to gh fidelity, and churn in it
  shouldn't be blamed on the emulation core.

## 4. Repair or rewrite

**Recommendation: rewrite the gh-emulation core in Rust, in the same repo. Keep the install, discovery,
and trace shell. Use `chatgpt-support` as a source of fixtures and ideas, and close PR #29 unmerged.**

Why not repair:

- Fixing it means flipping the global policy from silently degrading to failing strictly, collapsing three
  parsers into one, and replacing the oracle and the tests. That touches every command in `read_only.rs`,
  so little of it would survive anyway.
- The real surface is small: about 5 read operations, plus create, comment, and checkout, for 2–3 clients.
  A contract-first core is plausibly 2–3k lines.
- The branch shows the risk of repairing this incrementally: 172 commits and heavy process, and it
  stalled hardening a heuristic that a design review would have deleted.

Why not a full greenfield rewrite: the shell (wrapper install, real-gh discovery, the launchd PATH
handling, trace redaction) encodes lessons hard-won from about 9 bugs. Its problems are environmental,
not architectural.

**Core design for the rewrite:**

1. **The contract is data, and the oracle is real gh.** Each client (Codex.app, ChatGPT desktop build N,
   T3 Code) gets a capture file listing the exact argv it sends, taken from `gfj trace`. Each supported
   operation has a fixture containing **real gh output against a GitHub repo** (exit code, stdout,
   stderr), like the branch's `github-pr-31.json` and its no-checks control. A contract entry without a
   passing process test fails CI.
2. **One invocation parser, one grammar table.** The table gives each operation its flags (with arity)
   and its allowed `--json` fields. Routing and execution both consume the same parsed result
   (`(provider, repo, operation, args)` or `Delegate`).
3. **Strict by default.** For a resolved Forgejo repo, an unknown operation, flag, field, or jq/template
   construct gives a non-zero exit with gh's own error text (for example `Unknown JSON field: "x"`), and
   is traced. Forgejo-resolved invocations never go to real gh. No accepted-but-ignored flags.
4. **Field projection as a table.** Each GitHub field gets one function from Forgejo data, with a fixture
   per field. Hard semantic mappings (`reviewDecision`, `mergeStateStatus`, `autoMergeRequest`, check
   `bucket`) are specified by Opus up front.
5. **Real jq.** Embed `jaq` (or shell out to `jq` when present) instead of the hand-rolled subset.
   `--template` returns an error until a real Go-template implementation is needed.
6. **Explicit credentials.** Tokens come only from the shim's store (Keychain or file), keyed by host. An
   env token applies only to the host named in a host-scoped variable, or only when exactly one host is
   configured. `gfj auth import --from fj|tea` reads the *documented* schema once. No runtime scraping.
7. **One config loader** (the `toml` crate), **one HTTP client** with a shared per-command deadline (the
   branch uses 5s) and pagination.
8. **Tests at the process seam:** the binary is run against a fake Forgejo HTTP server with literal
   fixtures, with a decoy real gh that fails if it's called. These are the branch plan's "confirmed TDD
   seams" 1–4, which are good and worth adopting.

**Keep:** `shim.rs` (wrapper and marker), `external.rs` (real-gh discovery), `gui_path.rs`, `trace.rs`
redaction and `trace_summary.rs` (the capture pipeline the contract depends on), the status→check state
and bucket mapping in `normalize.rs:560-720` as reference, URL building and percent-encoding in
`forgejo.rs:494-560`, and `scripts/package-release.sh` and the release workflow.

**Harvest from the branch:** `process-contract.json` and `github-pr-31.json`; fake-server test support
(`tests/support/mod.rs`); the S0 determinism fix; the `HostRegistry` alias/api-root concept
(`provider.rs`); pagination and `@me` (`pull_requests/list.rs`); and the PLAN's seams and decisions 2–6.

**Drop:** all Python compatibility (formatting helpers, `-m gh_forgejo_shim` marker detection, the
migration docs), the hand-rolled TOML and jq, heuristic credential scraping, `ureq`, `codex_smoke.rs`
(superseded by contract tests), and the `ot5`-style rewriting of delegated GitHub argv.
`git_recorder.rs` is optional.

## 5. Candidate work packages for Sol

These are candidates only. Each one is verifiable by machine. Standard gate (**G**):
`cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`.
"Opus first" marks work that needs a spec or fixture from Opus or Kell before Sol can start. These
packages are useful whether Kell chooses repair or rewrite. WP1–2 are good calibration runs for the
review workflow.

| # | Package | Opus first | Gate (in addition to G) |
|---|---|---|---|
| WP1 | Add `rust-toolchain.toml` pinned to 1.97.1; fix `normalize.rs:414`; CI reads the pin | — | G on a clean clone; `grep -q 'channel' rust-toolchain.toml` |
| WP2 | Make the test harness hermetic: no test sees host `gh`/`git`/`HOME` (port branch S0) | — | G run with a decoy `gh` first in PATH that `exit 99`s, plus `env -i HOME=/nonexistent PATH=/usr/bin:/bin cargo test --workspace --locked` |
| WP3 | One config loader: delete `routing.rs:434-536`, route through `config::load_config_with_env` | — | New tests: multi-line `hosts`, `#` inside strings, `[paths]`; probe script: multi-line config + `gh pr list` must not reach the decoy gh |
| WP4 | Contract-driven process test runner: one test iterates over contract entries (argv, fake-server exchanges, expected exit/stdout/stderr) | Schema v2 | Runner passes on seed entries; a mutated copy of one expected stdout makes it fail (`cargo test … 2>&1 \| grep -q FAILED`) |
| WP5 | Fail locally: Forgejo-resolved invocations never delegate | — | Process tests for `pr merge`, `pr edit`, `api repos/...` with a decoy gh that exits 99 if called; exit ≠ 99 and stderr matches the expected text |
| WP6 | Strict `--json`: unknown field gives exit 1 and gh's error text | gh error fixture | Contract entries from WP4 for each command; existing tests updated, none deleted |
| WP7 | Replace the jq subset with `jaq` | jq fixture table | Table test: about 30 expressions × inputs, expected output generated by the real `jq` binary (committed), plus `cargo tree -i jaq-core` |
| WP8 | PR list pagination, `--author @me`, and the remaining list filters (or explicit errors) | — | Fake-server tests: 3 pages, limit cut, safety cap, `@me` → `/user` lookup; no `accepted_but_ignored` entries left |
| WP9 | `pr view`: the 9 missing fields; `pr checks`: `event` and no-checks exit 1 | Field-mapping spec | Un-ignore the branch's `forgejo_pr_view_does_not_yet_emit_all_25_fields` / `…checks…_all_9_fields`; field-by-field fixture tests |
| WP10 | One HTTP client (drop `ureq`) and a shared deadline | — | `! cargo tree -i ureq`; a slow fake server test finishes in under 5.5s with a deadline error |
| WP11 | Credential scoping and removal of runtime scraping | Kell's decision (below) | Two-host test: host A's token never appears in requests to host B; `auth.rs` contains no `find_token_in_text` |

Not for Sol: capturing the oracle (needs live GitHub, the ChatGPT app, and judgment), the field-mapping
semantics, the trace-to-contract workflow, anything touching `bootstrap`/`doctor`/launchd behavior (it
can't be checked in CI and depends on the user's environment), and the final ChatGPT screen acceptance.

## 6. Decisions log

2026-10-30, from Kell:

- **Rewrite the core** (section 4). Close PR #29 unmerged after harvesting it.
- **Client scope changed.** T3 Code now supports Forgejo natively and is out of scope. Codex.app and
  ChatGPT have merged into one ChatGPT.app, which many developers use by default even though Kell
  doesn't use it. The scope widens to other agentic GUI tools developers commonly use, possibly
  including Claude's apps. Which of these call `gh`, and how, is unknown; it will be measured with the
  trace recorder, not guessed. Two client classes follow:
  - **GUI apps** send a fixed set of probes. The contract is an exact capture per app build.
  - **Agents** (Claude Code, Codex CLI, and the agents inside these apps) use `gh` open-endedly. The
    contract is broad, honest coverage of common commands, with clear errors for everything else.
- Tokens: the shim's own per-host store is primary. A generic env token is allowed only when exactly
  one host is configured; otherwise use a per-host `token_env`. No runtime scraping; use
  `gfj auth import --from fj|tea` instead.
- PR #30 (private CA): merged as `a7e4200`. WP1 and WP2 shipped together as PR #31 (`1eb0461`), which
  turned main's CI green.

2026-10-30, later:

- Sol runs may use a local Linux container as a gate.
- Work packages are tracked in the shim's beads as well. `bd` 1.3.2 is installed. It is blocked on the
  beads remote, which was unreachable at the time (rewrite plan §5).
- Rewrite planning started: see `rewrite-plan.md` and `clients.md`.
