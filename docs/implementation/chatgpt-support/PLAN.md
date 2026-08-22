# ChatGPT desktop Forgejo pull request compatibility

Status: accepted plan, execution paused

Source research: [`docs/reports/chatgpt-forgejo-pr-compatibility.html`](../../reports/chatgpt-forgejo-pr-compatibility.html)

This plan restores the native ChatGPT desktop pull request screen for Codeberg
and user-configured Forgejo hosts. It preserves GitHub behavior through the real
GitHub CLI and keeps `git.dirtydishes.dev` as a test fixture, not product
configuration.

Do not create or run the dirtyloop, create or update Beads issues, start
implementation, install a build, or change runtime state until the user gives a
new instruction to begin.

## Outcome

ChatGPT desktop can open and poll a Forgejo pull request through the installed
`gh` process without showing "Pull request status unavailable." The same
installed command continues to delegate GitHub repositories to a verified real
GitHub CLI.

The accepted external interface remains:

```text
cwd + env + stdin + gh argv
             |
             v
exit status + stdout + stderr
```

The accepted build-6720 read contract is:

- `gh --version`
- `gh auth status --active [--hostname H]`
- `gh pr list --head B --author @me --state all --json ...`
- `gh pr view N --json <25 fields>`
- `gh pr checks N --json ...`

Existing mutation commands must keep working, but this work does not expand
them. General GraphQL support, caching, a daemon, an HTTP proxy, ChatGPT's cloud
GitHub connector, and new mutation behavior remain out of scope.

## Confirmed TDD seams

These seams are accepted. Tests must observe behavior through them instead of
calling private helpers or asserting internal call order.

1. **Managed `gh` process seam.** Invoke the shipped command with `cwd`,
   environment, stdin, and argv. Assert exit status, stdout, stderr, request
   destination, and elapsed deadline.
2. **Management CLI seam.** Invoke `gfj config`, `gfj auth`, `gfj bootstrap`,
   `gfj doctor`, install, and uninstall commands as child processes. Assert
   their output and isolated filesystem effects.
3. **HTTP provider seam.** Run the managed command against a fake Forgejo HTTP
   server with literal request and response fixtures. The fake server is an
   adapter at the same seam used by live Forgejo.
4. **Real GitHub delegation seam.** Give the shim an explicit fake or verified
   real `gh` executable. Assert unchanged argv, exit status, stdout, and stderr.
5. **Native desktop acceptance seam.** Observe the ChatGPT PR screen and the
   redacted process trace. This is the final product check, not a unit test.

Tests may use internal fake adapters only to create deterministic conditions at
these seams. They must not mock private functions, duplicate the implementation
in expected-value builders, or inspect private state as the main assertion.

## TDD working rules

Every implementation slice uses a vertical red to green cycle:

1. Write one failing behavior test at a confirmed seam.
2. Run the narrow test and save the expected failure as red evidence.
3. Commit and push the red test on the slice branch.
4. Add only enough implementation to pass that test.
5. Run the narrow test and the affected regression set.
6. Commit and push the green implementation.
7. Repeat with the next behavior in the same slice.
8. Do not refactor during a red to green cycle. Refactoring belongs in the
   checkpoint review and combined repair stage.

Expected values must come from the captured ChatGPT command, the verified
GitHub CLI 2.96 output, the Forgejo schema, or a literal worked fixture. A test
must fail when the behavior it names is broken.

## Design decisions

1. `routing::run_gh_from_env` and `routing::run_gh_captured` remain the process
   interface.
2. A new internal `HostRegistry` resolves transport names before allowlisting,
   credential lookup, REST URL construction, generated links, or delegation.
3. Existing `hosts = [...]` configuration remains readable. New profiles add
   optional aliases, API roots, and credential hosts without preventing an old
   binary from reading the canonical host list.
4. Unknown operations for a resolved Forgejo repository fail locally. They
   never reach GitHub.
5. Each observed ChatGPT command receives one shared five-second monotonic
   deadline. Pagination and enrichment share that deadline.
6. Pull request pagination fetches 50 entries per page. It stops at the
   requested limit or the terminal page. A 20-page safety cap returns an error
   instead of incomplete data.
7. Real GitHub behavior uses an explicit, checksum-verified GitHub CLI 2.96
   path. The package records approved versions and checksums but does not
   redistribute `gh`.
8. `git.dirtydishes.dev` is a fixture. CI proves two independent host profiles,
   and release review includes an unrelated live Forgejo host such as Codeberg.
9. The run uses one external integration PR. Parallel slice work uses separate
   worktrees and internal branches.
10. Deployment begins only after the code, review, and packaging slices pass.

## Review and repair policy

### Reviewer roles

Checkpoint reviews use two independent roles when listed below:

- **Thermonuclear reviewer.** Uses the
  `thermo-nuclear-code-quality-review` skill. It focuses on module depth,
  ownership, giant files, condition growth, naming, test seams, and long-term
  maintainability.
- **Adversarial reviewer.** Challenges correctness, security, failure modes,
  compatibility claims, unsafe fallthrough, credential isolation, deadlines,
  and rollback. Its prompt must say: `Do not invoke or consult the
  thermo-nuclear-code-quality-review skill.`

The adversarial reviewer must never invoke the thermonuclear skill. A review
that violates this rule is invalid and must be rerun with a new reviewer.

Reviewers do not repair their own findings. They return evidence with file,
line, severity, reproduction, and expected behavior.

### Combined repair pass

At each checkpoint, the orchestrator waits for all required reviews, then:

1. Combines both finding sets into one list.
2. Deduplicates findings without dropping distinct failure paths.
3. Separates blockers, required repairs, and rejected findings.
4. Gives one repair owner the combined list.
5. Runs the affected TDD seam tests, the full local quality gate, and both
   checkpoint reviewers again.
6. Commits and pushes the repair as one checkpoint repair commit.

A checkpoint gets at most three combined repair passes. Passes count even if a
repair introduces a new finding. After the third pass, any remaining blocker,
required finding, red test, or red CI result stops the program runner. The
orchestrator notifies the user with the remaining evidence and does not start a
fourth pass.

### Review checkpoints

| Checkpoint | After | Required reviews | Reason |
| --- | --- | --- | --- |
| C1 | S3 | Thermonuclear and adversarial | Establish host, identity, routing, and module patterns before read behavior grows around them. |
| C2 | S6 | Thermonuclear and adversarial | Check the new pull request module, test seam, deadline, and pagination shape early. |
| C3 | S9 | Adversarial | Attack the complete read contract, stale check handling, timeouts, and GitHub fallthrough. |
| C4 | S11 | Thermonuclear and adversarial | Review the finished code shape, packaging, install safety, and rollback before live use. |
| C5 | S12 | Adversarial and manual product review | Confirm the native screen, live traces, and release claims. |

No dependent slice may begin until its preceding checkpoint passes.

## Common slice completion gate

Every slice must meet all of these rules:

- One implementation owner owns the slice branch and worktree.
- Red evidence exists before implementation evidence for each TDD cycle.
- All narrow acceptance tests pass.
- `cargo fmt --all -- --check` passes.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` passes.
- `cargo test --workspace --locked` passes.
- GitHub Actions job `rust` is green for the exact commit.
- `release package smoke` is also green when packaging changes.
- Review findings required at that slice or checkpoint are resolved.
- The turn record contains commit IDs, commands, results, CI links, review
  results, and any redacted live evidence.
- The branch is pushed and remote parity is verified before the slice closes.

Any failed review, red CI result, checkout mismatch, dirty ownership conflict,
or missing acceptance evidence stops the run.

## S0. Make the test baseline deterministic

**User-visible outcome.** Contributors get the same test result whether or not
the host already has `/usr/bin/gh`.

**Exact scope and ownership.** Change only
`crates/gh-forgejo-shim/tests/support/mod.rs` and the two affected cases in
`tests/cli_scaffold.rs`. Give test processes a fixture-only `PATH` with an
explicit Git fixture instead of inherited executable directories.

**Dependencies.** None.

**Acceptance tests.** All 25 CLI integration tests and all 124 current unit
tests pass on a machine that has a system `gh`. Add a named regression proving
that an installed system `gh` cannot affect the fixture.

**Review gate.** Confirm the fix isolates the environment and does not weaken
either existing assertion.

**CI evidence.** Green `cargo test --workspace --locked` and the named isolation
regression in the `rust` job.

**Stop conditions.** Stop if isolation requires a product behavior change,
skips an assertion, or relies on the developer machine's executable layout.

**Rollback boundary.** Revert the S0 commits. No product or user state changes.

## S1. Freeze the ChatGPT build-6720 process contract

**User-visible outcome.** A maintainer can run one suite and see the exact
process behavior ChatGPT expects, including native GitHub delegation.

**Exact scope and ownership.** Add `tests/chatgpt_build_6720.rs`,
`tests/fixtures/chatgpt/build-6720/`, and narrow fake-server support under
`tests/support/`. Store exact argv, exit status, stdout shape, stderr shape,
empty values, and the GitHub 2.96 control.

**Dependencies.** S0.

**Acceptance tests.** The GitHub control passes. Forgejo cases fail only at the
known gaps: alias identity, `@me`, missing fields, and checks.

**Review gate.** Compare every fixture with the report's captured command and
GitHub control. Fixtures describe external behavior, not Rust implementation
details.

**CI evidence.** Green `cargo test --test chatgpt_build_6720` attached to the
red fixture commit and later green commits.

**Stop conditions.** Stop if a fresh desktop trace differs from the recorded
command contract. Replan against the new trace before implementation.

**Rollback boundary.** Revert the S1 test and fixture commits.

## S2. Add typed, backward-compatible host profiles

**User-visible outcome.** A user can configure one canonical Forgejo host with
transport aliases and inspect the result with `gfj config list`.

**Exact scope and ownership.** Add `src/provider.rs`. Update `src/config.rs`,
`src/cli.rs`, `src/lib.rs`, and `docs/configuration.md`. Introduce:

```rust
HostProfile {
    canonical_host,
    aliases,
    api_root,
    credential_host,
}

HostRegistry::resolve(&RepoRef) -> ProviderResolution
```

Extend `config add-host` with repeatable `--alias`, optional `--api-root`, and
optional `--credential-host` flags.

**Dependencies.** S1.

**Acceptance tests.** Legacy `hosts = ["codeberg.org"]` creates a default
profile. A `git.dirtydishes.dev` fixture profile resolves both `127.0.0.1` and
`127.0.0.1:2222`. Duplicate or ambiguous aliases fail.

**Review gate.** C1 later reviews the pattern. Slice review confirms no default
Dirtydishes entry, GitHub cannot be registered, URL schemes are restricted to
HTTP and HTTPS, and old config remains readable.

**CI evidence.** Unit and CLI round-trip tests for legacy config, new config,
invalid roots, ambiguous aliases, and `FJ_SHIM_HOSTS`.

**Stop conditions.** Stop on a lossy config rewrite, implicit host enablement,
ambiguous resolution, or a format an older binary cannot read.

**Rollback boundary.** Revert S2. New writes retain the legacy canonical host
list, so an older binary can still read it.

## S3. Resolve identity before routing

**User-visible outcome.** The original `--repo 127.0.0.1/...` command targets
canonical Forgejo, while unknown Forgejo operations fail locally.

**Exact scope and ownership.** Update `src/routing.rs`, `src/repo.rs`, and the
repository context passed into `src/read_only.rs`. Remove duplicate host parsing
from routing and use `HostRegistry` as the sole provider decision source.

**Dependencies.** S2.

**Acceptance tests.** The exact Dirtypages command never targets loopback HTTPS
and never invokes real `gh`. GitHub repositories still delegate. An unsupported
command for an allowlisted Forgejo alias exits nonzero with a clear error.

**Review gate.** C1 requires both thermonuclear and adversarial review, followed
by the combined repair process.

**CI evidence.** A route matrix covers canonical Forgejo, alias Forgejo, unknown
Forgejo operation, unconfigured host, `github.com`, `GH_REPO`, `GH_HOST`, and
Git remotes.

**Stop conditions.** Stop if any Forgejo repository can fall through to GitHub,
if GitHub stops delegating, or if C1 fails after three repair passes.

**Rollback boundary.** Revert S2 and S3 together if the registry contract must
change. The new profile data remains inert until routing uses it.

## S4. Make authentication use canonical host identity

**User-visible outcome.** `gh auth status --active --hostname ALIAS` finds the
canonical host credential without exposing the token.

**Exact scope and ownership.** Update `src/auth.rs` and auth-specific tests. Add
Linux `fj` discovery at `~/.local/share/forgejo-cli/keys.json`. Key lookup and
stored auth by `credential_host`.

**Dependencies.** S3 and C1.

**Acceptance tests.** Alias and canonical host return the same logged-in
identity. An unrelated host cannot read that credential. Linux `fj`, shim
storage, environment variables, and macOS Keychain retain their documented
precedence.

**Review gate.** Inspect path permissions, host matching, logs, errors, stdout,
stderr, and traces for token leakage.

**CI evidence.** Auth tests for matching host, unrelated host, aliases,
precedence, owner-only storage, and secret redaction.

**Stop conditions.** Stop on any cross-host credential use, token disclosure,
or change that copies an external token into a project file.

**Rollback boundary.** Revert S4. No credential migration or copying occurs.

## S5. Enforce the command deadline and truthful tracing

**User-visible outcome.** A user gets a bounded result or clear timeout, and the
trace states which output it observed.

**Exact scope and ownership.** Update `src/routing.rs`, `src/forgejo.rs`,
`src/external.rs`, and `src/trace.rs`. Add a shared `CommandDeadline`. Count
Forgejo output through counting writers. Capture and forward output only for
the observed noninteractive ChatGPT command set. Mark inherited output as
unknown when the shim cannot count it.

**Dependencies.** S3 and C1. S4 and S5 may run in parallel in separate
worktrees because their owned implementation files do not overlap.

**Acceptance tests.** A delayed multi-request command exits within five
seconds. Trace records provider, command class, duration, exit, exact observed
byte counts, and an explicit unknown state where capture was impossible.

**Review gate.** Confirm interactive and large existing commands keep inherited
streaming and secrets remain redacted.

**CI evidence.** Fake-clock or bounded-delay tests plus literal trace schema
assertions.

**Stop conditions.** Stop if the deadline resets between requests, output order
changes, interactive behavior regresses, or trace data claims false precision.

**Rollback boundary.** Revert S5. Persistent effects remain limited to opt-in
trace files.

## S6. Make PR discovery honor `@me` and pagination

**User-visible outcome.** ChatGPT can truthfully decide whether the current
branch has a pull request.

**Exact scope and ownership.** Add `src/pull_requests/mod.rs` and
`src/pull_requests/list.rs`. Update `src/read_only.rs`, `src/forgejo.rs`,
`src/normalize.rs`, and `src/lib.rs`. Establish a private
`PullRequestSource` seam with real and fake adapters.

**Dependencies.** S4 and S5.

**Acceptance tests.** Resolve `@me` through `GET /user`, filter by stable login
and head branch, handle `state=all`, paginate 50 entries at a time, and return at
most the real `gh` default of 30 unless `--limit` changes it. Dirtypages `main`
returns an authoritative empty list.

**Review gate.** C2 requires thermonuclear and adversarial review, followed by
the combined repair process. Review author semantics, terminal-page detection,
the page cap, shared deadline, and empty-result truthfulness.

**CI evidence.** Contract tests cover a match on page 2, no match after the
terminal page, explicit limits, more than 1,000 candidates, and a redacted live
Dirtypages replay.

**Stop conditions.** Stop if the command returns partial data after timeout or
cap, compares display names, ignores `--author`, or C2 fails after three repair
passes.

**Rollback boundary.** Revert S6. The earlier routing and auth repairs remain.

## S7. Render the PR header and core fields

**User-visible outcome.** `gh pr view` renders title, body, branches, author,
merge state, timestamps, counts, and links with GitHub-compatible shapes.

**Exact scope and ownership.** Add `src/pull_requests/view.rs` and
`src/pull_requests/projection.rs`. Move only the required projection logic from
`src/normalize.rs`. Track whether projected values are exact, derived, or
unavailable inside the module.

**Dependencies.** S6 and C2.

**Acceptance tests.** A core-field request matches the GitHub 2.96 fixture for
nulls, strings, objects, arrays, field filtering, and ordering. No unsupported
value is invented.

**Review gate.** Compare every implemented field with its Forgejo source and
the GitHub fixture.

**CI evidence.** Golden core-view projection and a redacted live Islandflow
fixture.

**Stop conditions.** Stop if a field's source is unknown or its empty shape
differs from real `gh`.

**Rollback boundary.** Revert S7. S6 discovery remains usable.

## S8. Add comments, commits, reviews, and reviewer requests

**User-visible outcome.** ChatGPT's PR activity view shows the complete
discussion and review state.

**Exact scope and ownership.** Extend `src/pull_requests/view.rs`,
`src/pull_requests/projection.rs`, and `src/forgejo.rs`. Add paginated reads for
commits, issue comments, reviews, review comments, and requested reviewers.

**Dependencies.** S7.

**Acceptance tests.** The exact 25-field build-6720 `pr view` request matches
the golden shape. Islandflow PR 36, or a verified replacement, shows its body,
commits, two comments, reviews, and requests in stable order.

**Review gate.** Check pagination, chronological ordering, latest-review
derivation, deleted users, empty arrays, and unsupported review-thread
semantics.

**CI evidence.** Golden 25-field test, ordering tests, and redacted live fixture
record.

**Stop conditions.** Stop if the fixture has drifted without a replacement or
if Forgejo cannot support a claimed semantic. Do not fake review-thread
capabilities.

**Rollback boundary.** Revert S8. Core view from S7 remains.

## S9. Render current checks and real no-check behavior

**User-visible outcome.** ChatGPT shows current Forgejo checks, and a PR with no
checks produces the same nonfatal warning as real `gh`.

**Exact scope and ownership.** Add `src/pull_requests/checks.rs`. Update
`src/forgejo.rs` and check projection code in `src/normalize.rs`. Read combined
commit status and action runs.

**Dependencies.** S8.

**Acceptance tests.** Return requested `event`, timing, workflow, state,
conclusion, and links. Collapse duplicate contexts to current state. With no
checks, exit 1, write empty stdout, and write `no checks reported` to stderr.

**Review gate.** C3 requires adversarial review only. The adversarial reviewer
must not invoke the thermonuclear skill. Apply the combined repair process to
its findings.

**CI evidence.** Golden checks test, no-check contract test, stale-history
regression, and a redacted live Islandflow PR 111 record or verified
replacement.

**Stop conditions.** Stop on stale status, missing requested keys, different
no-check process behavior, or a C3 failure after three repair passes.

**Rollback boundary.** Revert S9. PR discovery and view remain intact.

## S10. Prove generic multi-host behavior

**User-visible outcome.** The same binary handles two unrelated Forgejo
identities without sharing aliases or credentials.

**Exact scope and ownership.** Add `tests/multi_host.rs` and isolated two-host
HTTP fixtures. Do not change product files unless the test exposes a defect.

**Dependencies.** S9 and C3.

**Acceptance tests.** Two profiles use distinct aliases, API roots, tokens,
repositories, and links. Requests and credentials never cross. Run a read-only
smoke test against Codeberg or another unrelated live Forgejo host.

**Review gate.** Search product and fixture files for Dirtydishes defaults and
hidden single-host assumptions.

**CI evidence.** Green two-host integration test. Attach the live smoke record
separately because CI must not depend on a public service.

**Stop conditions.** Stop if no authorized live second-host credential or
repository is available. Do not create one without separate approval.

**Rollback boundary.** Revert test files. Live work is read-only.

## S11. Package and install without disturbing the host

**User-visible outcome.** A fresh app-server child resolves the managed wrapper,
while GitHub still runs through verified `gh` 2.96.

**Exact scope and ownership.** Update `src/setup.rs`, `src/bootstrap.rs`,
`src/doctor.rs`, `src/shim.rs`, `src/external.rs`,
`scripts/package-release.sh`, `.github/workflows/ci.yml`,
`docs/installation.md`, and `docs/rollback.md`. Add
`packaging/real-gh-compat.toml` with approved versions and target checksums.

**Dependencies.** S9 and C3. S10 and S11 may run in parallel in separate
worktrees because their owned implementation files do not overlap.

**Acceptance tests.** Package smoke verifies archive checksum, wrapper
precedence, explicit real-`gh` path, GitHub control, private config and auth
permissions, and uninstall. Do not replace `/usr/bin/gh`.

**Review gate.** C4 requires thermonuclear and adversarial review, followed by
the combined repair process. Inspect every filesystem write, managed-file
marker, checksum source, permission, and uninstall path.

**CI evidence.** Both GitHub Actions jobs are green, including release archive
install and rollback.

**Stop conditions.** Stop if the wrapper target is unmanaged, checksum
verification fails, system `gh` would change, active ChatGPT work requires a
restart, or C4 fails after three repair passes.

**Rollback boundary.** Before live installation there is no runtime state. After
installation, `gfj uninstall-shim` removes the managed wrapper. Reinstalling the
prior binary rolls back its version. `FJ_SHIM_HOSTS=` disables Forgejo routing
for one process.

## S12. Validate the native ChatGPT PR screen

**User-visible outcome.** The desktop app opens and keeps the Forgejo PR screen
current without harming GitHub repositories.

**Exact scope and ownership.** Make no source changes unless validation finds a
defect. Install the approved build through the existing managed path, then run
read-only desktop checks.

**Dependencies.** S10, S11, and C4.

**Acceptance tests.** All of the following must pass:

- Dirtypages `main` reports no matching PR without "Pull request status
  unavailable."
- Islandflow PR 36, or a verified replacement, shows header, body, activity,
  and checks.
- Lyricslab PR 31 still opens through real `gh` 2.96.
- A no-check PR displays the native nonfatal warning.
- Repeated polling stays within five seconds per process.
- Trace data contains no token and no Forgejo request in GitHub traffic.

**Review gate.** C5 requires adversarial review and manual product review. The
adversarial reviewer must not invoke the thermonuclear skill. Compare the UI,
process trace, and API truth.

**CI evidence.** Exact green S12 commit plus a redacted desktop trace and
screenshots tied to that commit.

**Stop conditions.** Stop on unavailable status, stale data, timeout, token
leak, Forgejo-to-GitHub request, GitHub regression, or a C5 failure after three
repair passes. Optional GraphQL enrichment and mutations require separate
accepted plans.

**Rollback boundary.** Remove the managed wrapper or reinstall the prior
binary. Do not restart unrelated services or active tasks.

## Dependency and parallel execution map

```text
S0 -> S1 -> S2 -> S3 -> C1
                         |-- S4 auth -------|
                         `-- S5 deadline ---|
                                             v
                                           S6 -> C2 -> S7 -> S8 -> S9 -> C3
                                                                          |-- S10 multi-host --|
                                                                          `-- S11 packaging ---|
                                                                                                v
                                                                                               C4 -> S12 -> C5
```

Only S4 with S5, and S10 with S11, may run in parallel. Each parallel slice
uses a separate worktree and phase branch from the same accepted integration
commit.

S0 through S3 remain serial because each defines assumptions used by the next
slice. S6 through S9 remain serial because they deepen the same pull request
module and each accepted result becomes the next slice's input. S12 is the
final serial release gate.

## Dirtyloops execution contract for a later run

When the user gives a new instruction to begin:

- Reverify host, repository realpath, symbolic branch, starting commit,
  remotes, upstream parity, and cleanliness before creating anything.
- Materialize this accepted plan as a Beads-canonical
  `orchestrator-callback` dirtyloop bound to Codex.
- Keep the control task orchestration-only.
- Use separate implementation and review owners.
- Reuse `lavender/chatgpt-support` as the integration branch only if it still
  contains the accepted plan and has the verified base. Stop on drift.
- Keep one external PR. Reuse an existing PR for the branch if one exists.
- Use the goal only to keep the long-running Program Runner alive. Goal state
  must not grant admission, claim a slice, accept results, advance Beads, merge
  code, or prove completion.
- Beads owns work status, ordering, dependencies, blockers, and completion.
- Git owns source truth. Review and CI evidence own acceptance.
- Never advance a slice because an implementation owner merely reported
  completion.
- Stop at any failed review, red CI result, dependency drift, contract drift,
  credential problem, ambiguous host, missing live fixture, rollback failure,
  or exhausted three-pass repair limit.

## Commit and push protocol for a later run

- Use lowercase, short, human commit messages.
- Commit and push each red seam test on its phase branch.
- Commit and push each minimal green implementation.
- Commit and push each combined checkpoint repair pass separately.
- Never mix unrelated slices in one commit.
- After a reviewed slice lands, push the integration branch and verify remote
  parity before closing the slice.
- Before session close, run the full quality gate, update Beads, pull with
  rebase where safe, push, and prove the branch is clean and up to date.
- If push fails, resolve it and retry. Do not report the slice complete while
  commits remain local.

## Current baseline recorded when the plan was accepted

- Repository: `/home/delta/dev/gh-forgejo-shim`
- Branch: `lavender/chatgpt-support`
- Commit: `8874d3c7da7ad6c71f32221892853f8f44f91184`
- Formatting: passing
- Clippy with warnings denied: passing
- Unit tests: 124 passing
- CLI integration tests: 23 passing, 2 failing
- Failing tests:
  - `doctor_runs_native_diagnostics`
  - `bootstrap_default_current_target_falls_back_to_user_local_without_safe_visible_dir`
- Known baseline cause: the integration fixture sees this host's `/usr/bin/gh`
  despite describing the environment as isolated.

S0 is therefore a hard prerequisite, not optional cleanup.
