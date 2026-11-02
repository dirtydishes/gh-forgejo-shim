# Core rewrite plan review, 2026-11-01

This is an adversarial review of the core-rewrite plan (beads epic `gh-forgejo-shim-cwe`), done before more
of it gets built. Three GPT-6.1-Sol reviewers (reasoning effort xhigh) each took one lens: Forgejo and
GitHub semantics, failure modes, and sequencing and scope. Their findings were leads. Opus checked each one
against primary sources: this repo, Forgejo v15.0.0 source, gh v2.103.3 and go-gh v2.17.0 source, and
read-only GETs against Codeberg and git.dirtydishes.dev.

Round 1 produced 33 findings: 10 on semantics, 11 on failure modes and 12 on sequencing. Opus confirmed
32 of them, some at a lower severity, and rejected one as stated (F23). After merging duplicates and
adding Opus's own checks, 25 findings remain, listed below. One rebuttal round covered the six disputed
calls. The reviewer accepted all five severity changes and added detail. It also successfully argued
that F21 hides more than URL layout.

Each finding carries a verdict:

- **Confirmed.** Opus reproduced the evidence.
- **Plausible.** The mechanism is real but depends on a design choice that hasn't been made yet.
- **Rejected.** The evidence didn't hold. The reason is given.

Severity scale:

- **critical:** data loss or a credential leak in the default path.
- **high:** a break users will see on the default path, or a plan defect that will get the wrong thing built.
- **medium:** a real defect in a narrower case, or one with an easy workaround.
- **low:** a cosmetic issue, doc drift, or a non-default configuration.

Paths cite this repo's `rewrite` branch at `e17a07e` unless stated otherwise. Gateway paths
(`crates/fga-core`, `crates/fga-server`) are the same at forgejo-github-api `22c0b69` and at the unmerged
move commit `b5f0c62`.

## 1. The plan as it stands after option B

Nothing in this repo or its beads records the decision below yet, which is finding 0. This section
writes it down.

**History.** The 2026-10-30 plan was a contract-first emulator of the gh CLI. The shim parsed gh's argv,
called Forgejo, and reproduced gh's output, `--json`, `--jq` and error text. That work was split into
tiers T0 to T3 and phases:

- A: contract
- B: skeleton work packages R1 to R5
- C: operations work packages C1 to C5
- D: T3

Client captures taken the same day changed the picture.

- **ChatGPT desktop (build 22943)** reads PR state through six fixed GraphQL documents. It uses `pr list`
  only to map a branch to a PR.
- **Claude desktop and Claude Code** run `gh auth token --hostname <forge>`. They then call GitHub
  Enterprise REST directly at `https://<forge>/api/v3/repos/{o}/{r}` and `…/pulls?head=…`. No gh
  process is involved in those calls.
- **Real gh 2.103.3**, pointed at a GitHub Enterprise-shaped host, ran all of T0 to T2. That needed 21
  named GraphQL operations, including schema introspection, and 7 REST routes.

A separate gateway repo, forgejo-github-api, was built after that. It is a Rust GraphQL and REST server
for a subset of GitHub's API, backed by Forgejo `/api/v1`. Its milestones G1 to G3 shipped, and the
pre-move audit merged as its PR #6 (`22c0b69`).

**Decision, 2026-11-01 (Kell, option B):**

- The shim becomes **real gh plus an in-process gateway reached over a unix socket**. It is built in
  this repo.
- The gateway crates come into this repo by a history-keeping merge, and forgejo-github-api is then
  archived with a pointer here.
- The install command, binary names (`gh-forgejo-shim`, `gfj`, and the managed `gh` wrapper), config
  file and token storage stay the same for existing users.
- Standalone server mode, with the gateway behind a reverse proxy on the forge host, becomes a secondary
  README section. Decision D1 below supersedes this: server mode is a first-class install path.
- If real gh becomes newly required, the release notes say so. It does become newly required (F14).

**Decisions on this review, 2026-11-01 (Kell).** Section 3 has the full text.

- D1: client-only mode supports Claude's agent running gh, and server mode becomes first-class.
- D2: PR URLs use `/pull/N`.
- D3: the minimum gh is v2.103.3, and install and upgrade refuse anything older.
- D4: no interim release from `rewrite`. The shim's `pr checks --required` path is deleted in the
  restructure, and the rule is that nothing ships from `rewrite` until the launcher slice (L3) passes.
- D5: seeding the Forgejo twin and adding a non-admin e2e user are approved.

**State on 2026-11-01:**

- `rewrite` has PRs #32 to #37 merged:
  - R2 config and credentials
  - R1 contract harness
  - R3 strict grammar
  - C2c `pr checks` on `fga-core` pinned at `0199b3d`
  - the branch-protection paging fix
  - credential hardening
- The move is **built and gated but unmerged**. It is local merge commit `b5f0c62` on
  `foreman/gateway-move`, with parents `e17a07e` and forgejo-github-api `22c0b69`. It changes the
  following:
  - adds `crates/fga-core`, `crates/fga-server`, `fixtures/`, `scripts/e2e*`, `scripts/oracle/` and
    `docs/gateway/`, each at its original path, so `git log` follows the history;
  - switches the shim's `fga-core` dependency to a path dependency;
  - adds a `gateway-e2e` CI job.
- The move passes fmt, clippy, `cargo test --workspace` (256 passed), `scripts/contract/run.sh`, the
  release package smoke, and the gateway e2e oracles with the same counts as forgejo-github-api main.
- Building has stopped until this review is accepted.

**How option B would run.** No document specifies this yet, so the reviewers attacked a working model.
The installed `gh` wrapper decides whether an invocation targets a Forgejo host. If it doesn't, the
wrapper execs real gh unchanged. If it does, the wrapper starts the gateway in-process on a unix socket,
runs real gh as a child with `http_unix_socket` pointing at the socket and the Forgejo token supplied,
and exits when gh exits. The gateway translates `/api/v3` and `/api/graphql` into Forgejo `/api/v1`
calls over real HTTPS.

## 2. Findings

### F0. Nothing records option B, and the backlog still commissions an emulator (high, confirmed)

No plan document, bead, README or gateway doc mentions option B, the unix socket or the move. The epic is
titled "Core rewrite: contract-first gh emulation", and its description links to the old plan in a
private repo. The open children still ask for emulator work:

- `cwe.8` (R3) wants "strict errors; never delegate Forgejo ops". The merged R3 code validates argv
  before anything else runs (`crates/gh-forgejo-shim/src/routing.rs:267`).
- `cwe.10` (R5) wants "JSON projection, jaq, non-TTY text". Real gh produces all of that.
- `cwe.4` (A4) builds a gh grammar table, and C1 to C4 (`cwe.11` to `.18`) are argv emulation per command.
- `cwe.21` (C2g) describes a lookup table of normalized GraphQL documents. The gateway already
  implements a real GraphQL schema, which replaced that idea (F7).
- Bead `65s` cites a file in a private repo.

The plan's size estimate was "2–3k lines replacing ~8.5k". The move alone adds about 8.6k lines of
gateway crates, and the old emulator is still in place. An agent working through the beads in order
would build a second implementation of gh's behavior next to real gh.

**Fix.** Record the decision. This document does that, and section 3 restructures the backlog. Retitle
the epic, and rewrite or retire each child as listed in section 3.

### F1. The launcher is unspecified, and gh's socket setting covers every host (high, confirmed)

gh doesn't scope `http_unix_socket` per host. go-gh reads it as a global key
(`go-gh/pkg/api/client_options.go:131-132`, `cfg.Get([]string{"http_unix_socket"})`). The unix
transport also replaces both the plain and the TLS dialer with the same unix dial
(`go-gh/pkg/api/http_client.go:238-249`). Once the key is set, every API request that gh process
makes goes down the socket as plaintext HTTP, whatever its host:

- github.com requests;
- every host `gh auth status` checks when run without `--hostname`
  (`gh/pkg/cmd/auth/status/status.go:279,310-329`);
- `gh api` with an absolute URL (`gh/pkg/cmd/api/http.go:19-21`).

Where that key lives is an open design choice, and every option breaks something:

- **The user's real `~/.config/gh`.** A crash or uninstall leaves plain gh dialing a dead socket.
  Concurrent invocations overwrite each other's socket path. `docs/rollback.md` assumes that removing the
  wrapper restores plain gh, and it wouldn't.
- **A private, empty `GH_CONFIG_DIR`.** go-gh keeps both `config.yml` and `hosts.yml` there
  (`go-gh/pkg/config/config.go:251-256,274-285`). The user's aliases (`gh/pkg/cmd/root/root.go:207-228`),
  `git_protocol`, extensions and github.com login vanish inside Forgejo checkouts.
- **Environment only** (`GH_HOST`, `GH_ENTERPRISE_TOKEN`). This still needs the socket key from a config
  file.

No bead owns any of the remaining launcher work:

- socket readiness, stale socket files, permissions, and macOS's 104-byte `sun_path` limit;
- concurrent invocations (ChatGPT polls about every 60 s per thread);
- signal forwarding and exit codes;
- how the token reaches gh.

**Fix.** Before any other launcher code, write a launcher spec (L0 in section 3), then build one
installed-wrapper vertical slice. Acceptance must prove three things. The user's gh config is
byte-identical after a run, a crash and an uninstall. Aliases still work inside a Forgejo checkout.
github.com requests never reach the gateway.

### F2. The gateway forwards any host's credential to its one Forgejo (high, confirmed)

The gateway's authentication middleware reads `Authorization` and authenticates it against its single
configured Forgejo upstream. It never checks which host the request was meant for
(`crates/fga-server/src/lib.rs:83-89`, `crates/fga-core/src/forgejo.rs:109-143,157-167`). gh attaches
the token matching each request's own hostname (`gh/api/http_client.go:248-251`). Combine that with F1
and you get a leak. Run `gh api https://api.github.com/user` or a host-less `gh auth status` inside a
routed Forgejo checkout. gh sends the GitHub token, the socket delivers it to the gateway, and the
gateway sends it to Forgejo's `/api/v1/user`. A second Forgejo host's token goes the same way. For
hosted-forge users, the receiving server belongs to a third party.

This is high and not critical only because the launcher doesn't exist yet. The gateway code that would
leak is already merged.

**Fix.** Make host isolation a blocking gate before the launcher (L1). The gateway checks the request's
Host or authority before authenticating, rejects other hosts without contacting upstream, and gets tests
proving that GitHub and second-Forgejo tokens never reach the selected upstream.

### F3. The wrapper and real gh can choose different repositories (high, confirmed)

Today the shim picks `origin` first and then the other remotes in listed order
(`crates/gh-forgejo-shim/src/repo.rs:124-139`). It ignores `gh repo set-default`. Real gh ranks
`upstream`, then `github`, then `origin` (`gh/context/remote.go:60-77`), and it honors the saved default
(`gh/context/context.go:70-79`). Take a fork checkout with a GitHub `origin` and a Forgejo `upstream`.
The wrapper treats it as GitHub and execs plain gh. gh picks `upstream` and calls Forgejo's
nonexistent `/api/v3`. The reverse layout sends GitHub traffic into the gateway, which is F2.

**Fix.** Routing parity is a prerequisite for the launcher (L2). Either reuse gh's own resolution, for
example by asking gh, or port its ordering and saved-default rules exactly. Add process cases for
mixed-provider remotes and saved defaults that assert the wrapper and gh pick the same host and repo.

### F4. Multi-commit PRs report the oldest commit's checks (high, confirmed)

gh reads check state through `statusCheckRollup: commits(last: 1)` (`gh/api/query_builder.go:210,238,280`).
Forgejo returns PR commits newest first. It builds the list with a plain `git log base..head`
(`modules/git/repo_compare.go:103`), and Codeberg's live API shows the same order (forgejo/forgejo
PR 15097, two commits, newest first). The gateway keeps that order (`crates/fga-core/src/forgejo.rs:315`,
`graphql/pulls.rs:406-425`), and connection slicing takes the tail for `last`
(`graphql/connections.rs:52-55`). So `last: 1` returns the **oldest** commit. Push a fix on top of a
failing commit and `gh pr checks`, `pr view --json statusCheckRollup` and `pr status` all show the old
result. The e2e seed creates one commit per PR, so no test catches it.

**Fix.** Reverse PR commits into GitHub's oldest-first order in fga-core. Add a seeded PR with two commits
and different check results to the e2e oracle.

### F5. Client-only mode can't serve Claude, which is the gateway's first goal (high, confirmed; decided D1)

Claude desktop and Claude Code get a token from gh and then call `https://<forge>/api/v3` themselves.
Those requests never go through gh, so a socket that lives only as long as one gh process can't receive
them. `docs/gateway/DESIGN.md` lists "Claude Code and Claude desktop show PR status and checks" as goal 1
and assumes the gateway runs on the forge hostname. Under option B the default install is client-only.
A Claude user would get a successful token probe and still no PR status.

**Decision D1.** Client-only mode supports Claude's agent running `gh`. Claude's built-in PR status needs
server mode. Server mode gets real weight rather than a secondary README section:

- its own install path;
- its own tests;
- a README note telling anyone who uses Claude to use server mode.

**Fix.** Build server mode as its own work package, `cwe.42`. It covers the container image, proxy
snippets that include the `/pull/N` redirect, and tests for real gh and Claude's REST lookups against
fga-server. Acceptance is the Claude footer and Claude desktop showing the PR on clover. Release notes
and the README state support per mode.

### F6. Client-only mode can't make ChatGPT's PR links work (high, confirmed; decided D2)

ChatGPT 22943 ignores a PR whose URL ends `/pulls/N` and accepts `/pull/N`. Its bundle parses PR URLs with
`\/pull\/(\d+)`. Forgejo has no `/pull/N` route: `/pull/N` returns 404 on both Codeberg and
git.dirtydishes.dev, and Codeberg returns 200 for `/pulls/N`. `docs/gateway/MAPPING.md` §8 resolves this
with a redirect served by the deployment's proxy. Client-only installs have no proxy, and a browser never
uses gh's socket. Client-only users therefore get one of two outcomes:

- `/pull/N`: ChatGPT shows the PR, but "Open pull request in browser" 404s.
- `/pulls/N`: the link works, but ChatGPT shows no PR.

**Decision D2.** PR URLs use `/pull/N` in every mode. In client-only mode, the wrapper sets
`GH_BROWSER` to a helper that rewrites `/pull/` to `/pulls/`, so gh's own `--web` opens work. The
README carries the redirect recipe for anyone who runs their own forge. "Open pull request in browser" in
ChatGPT still 404s for client-only users without that redirect, and the README says so.

**Fix.** Bead `65s` is rewritten with acceptance for each mode:

- **Both modes.** A raw `/pull/N` in `pr list`/`pr view --json url` (with no normalizer), and ChatGPT
  shows the PR.
- **Client-only.** `gh pr view --web` opens a working `/pulls/N` page.
- **Server mode.** `/pull/N` redirects to `/pulls/N` with the query and fragment intact.

### F7. ChatGPT's six documents fail against the schema, and nothing tests them (high, confirmed)

The captured documents ask for things the schema lacks:

- `bodyHTML`;
- `reviewThreads`;
- `reviews(author:, states:)`. The resolver accepts only paging arguments
  (`crates/fga-core/src/graphql/pulls.rs:336-343`).

GraphQL validation rejects any document that uses one of them. Neither test suite would notice:

- The contract case validator allows only `pr view|list|checks|diff|status`, `repo view`, `api user`
  and `auth status` (`scripts/contract/common.py:70-73`). It excludes `api graphql`.
- The imported `scripts/oracle` cases run gh's own commands, not the app's documents.
- No `contract/clients/` captures exist in this repo.

Meanwhile C2g (`cwe.21`) still specifies a document-hash dispatcher. It depends on R3, R4, R5 and C2b,
which option B makes obsolete, so it can never be unblocked as written.

**Fix.** Rewrite C2g as the gateway's G4 work: close the schema gaps, then replay all six captured
documents, with their variables, the literal and alias batch query, and paging. Commit the redacted
captures to this repo. Schema tests can come first. Closing C2g needs a replay through the installed
wrapper.

### F8. The contract harness fails the design it's supposed to check (high, confirmed)

The R1 runner puts a decoy `gh` on PATH that exits 97, and it fails any case where the decoy runs
(`scripts/contract/runner.py:44-46,65-66`). Two cases require the shim to refuse locally:

- `contract/cases/local-unsupported-merge.json` expects `"gh pr merge" is not supported for Forgejo host`;
- `local-unsupported-api-repos.json` does the same for the PR list REST call.

Under option B, a correct wrapper runs real gh and serves `pr merge`, so the suite goes red for the right
behavior. The pressure that creates is to keep emulation or keep operations disabled.

**Fix.** Rewrite R1 (`cwe.6`) and A2 (`cwe.2`) to compare pinned real gh through the gateway against
the committed GitHub oracle. Keep the scenario data and expected output. Keep a decoy only to catch the
wrong gh binary being selected. Replace the local-refusal cases with gateway behavior cases, and fold
the imported `scripts/oracle` suite in rather than running two.

### F9. Cutover can unblock before the work it ships (high, confirmed)

C5 (`cwe.19`) depends only on R3, R4 and R5 (`bd show gh-forgejo-shim-cwe.19`). It doesn't depend on
C1 to C4, C2g, R1, A2, or anything about launching, migration or rollback. Close those three and C5 is
ready while ChatGPT's card is still unsupported. Its acceptance, "its contract cases pass and the old code
it replaces is deleted", names no evidence.

**Fix.** Give C5 named prerequisites: the launcher slice (L3), host isolation (L1), routing parity (L2),
the config bridge (L4), ChatGPT replay (C2g), the rewritten contract runner, and release work (R). Then
list the acceptance evidence it needs.

### F10. Upgrading drops private-CA support (high, confirmed)

The shim honors `[host.<name>].ca_file` and `FJ_SHIM_CA_FILE` (`crates/gh-forgejo-shim/src/forgejo.rs:207-222,235-258`),
and `docs/configuration.md` promises it. The private-CA support arrived in PR #30, from an outside
contributor who uses it. The gateway's upstream client is a bare `Client::builder()` with a fixed 10 s
timeout and no custom roots (`crates/fga-core/src/forgejo.rs:98-106`). Its server config has no CA
setting either. If the launcher reuses that client unchanged, existing private-CA users fail TLS after
upgrading with an unchanged config.

**Fix.** Write a config bridge (L4) that maps the existing config into the gateway client: CA files,
`api_url`, host-scoped token variables and the real-gh path. Acceptance runs a private-CA forge with
certificate and hostname checks still on. "The config file is unchanged" doesn't prove compatibility on
its own.

### F11. `pr checks --required` on `rewrite` is broken for non-admins and can pick the wrong rule (medium, confirmed)

This was found by the gateway thread and two reviewers independently. It only affects `rewrite`, and
`main` is clean.

- Forgejo serves `GET /repos/{o}/{r}/branch_protections` only to repo admins: the route group has
  `reqToken(), reqAdmin()` in `routers/api/v1/api.go` at about lines 1212-1226. The shim calls it with no
  fallback (`crates/gh-forgejo-shim/src/forgejo.rs:371-389`). Its only caller, the `--required` path,
  propagates the 403 with `?` (`read_only.rs:917-924`). Plain `gh pr checks` isn't affected.
- It takes the first rule that `glob::Pattern` matches. Forgejo sorts plain names ahead of globs
  (`models/git/protected_branch_list.go:27-35`) and matches plain names case-insensitively
  (`models/git/protected_branch.go:100-107`). Its globs don't let `*` cross `/`. So a `release/*` rule
  listed before `release/**` wins in the shim and loses in Forgejo for `release/a/b`.
- The head name comes from `/head/ref` (`read_only.rs:909-912`). After the head branch is deleted,
  Forgejo returns `refs/pull/N/head` there (`services/convert/pull.go:108,229`).

fga-core already handles all three (`Session::protection`, `Protection::for_branch`).

**Decision D4.** Delete, don't patch. Nothing ships from `rewrite` before the launcher slice, so no
release will ever carry this path.

**Fix.** `cwe.40` owns deleting the shim's `pr checks --required` path once L3 serves `pr checks` through
real gh and the gateway. That covers `get_branch_protection` in `forgejo.rs`, the `--required` branch in
`read_only.rs`, and the protection handling in `checks.rs`. The gateway's `Session::protection` replaces
it. A non-admin `--required` test stays, run against the new path as the non-admin e2e user approved
under D5. C5 (`cwe.19`) depends on `cwe.40`, so the old path can't survive by accident.

### F12. The older ChatGPT epic is still an active queue for the old architecture (medium, confirmed)

`6y9` is in progress, and its children `c09` to `c17` (S7 to S12, plus checkpoints labelled C3, C4 and
C5) are open. Each one rules out "General GraphQL support … a daemon, an HTTP proxy". `c14` and `c16`
require gh 2.96, while this repo pins 2.103.3. The labels C3, C4 and C5 also mean different things in
`cwe`, so two agents could each "do C3" against different contracts. A5 (`cwe.5`) promises to supersede
the old queue but is still open. `9r8` (pr list filters) and `mte` (live Forgejo coverage) predate the
gateway's filters and the imported real-Forgejo e2e.

**Fix.** Make A5's supersession an early step. Move the useful requirements from `c13` to `c16`
(multi-host, packaging, rollback, native-screen checks) into successor beads. Then mark `6y9` and its
children superseded, naming each ID. Reconcile `9r8` with gateway search filters, and `mte` with
`scripts/e2e`.

### F13. The gateway docs contradict the code and each other (medium, confirmed)

- **`warning` status.** `docs/gateway/MAPPING.md` §3 maps `warning` to `NEUTRAL` (CheckRun) and
  `SUCCESS` (StatusContext). Since PR #6, the code maps it to failure. The move updates two shim tests
  in `src/checks.rs`, so `gh pr checks` now exits 1 on a warning. An agent following MAPPING would
  revert a deliberate change.
- **Stale approvals.** MAPPING §4 drops stale approvals when `dismiss_stale_approvals` is set. Forgejo
  counts with `IgnoreStaleApprovals` (`models/issues/pull.go:901-915`). The code
  (`crates/fga-core/src/map/pulls.rs:163`) is right and the doc is wrong.
- **`docs/gateway/README.md`** links `docs/DESIGN.md` relative to its new directory, which breaks. It
  still says mutations are unsupported and that PR URLs use `/pulls/N`.
- **`docs/gateway/DESIGN.md`** still names a private repo's paths as its sources.

**Fix.** Add a doc pass to the move PR, or right after it. Record the move's behavior changes in its PR
body, since the move changes behavior as well as layout. The changes are the `warning` mapping, partial
protection rules for non-admins (`mergeStateStatus` `UNKNOWN` and `reviewDecision` null when the
visible gates pass), and the auto-merge refusals.

### F14. Release, migration and archival have no owner (medium, confirmed)

- **Real gh becomes required.** Forgejo commands currently run in-process
  (`crates/gh-forgejo-shim/src/routing.rs:293-307`), and a missing real gh only makes `doctor` warn
  (`doctor.rs:312`). Under option B, every Forgejo command needs real gh. No bead sets a supported gh
  version range or a check at install or upgrade time.
- **Release notes.** They are still the fixed string "Rust-first gh-forgejo-shim release artifacts."
  (`.github/workflows/release.yml:139`).
- **No owned task** covers the README restructure, upgrade and rollback proof, or archiving
  forgejo-github-api with a pointer. That repo has no open issues or PRs, and PR #6 is already merged,
  so archiving it is simple.

**Decision D3.** The minimum supported gh is **v2.103.3**, the version this review checked. Install and
upgrade refuse with a clear message when real gh is missing or older.

**Fix.** The release and retirement bead (`cwe.39`) blocks C5 and carries:

- the D3 check at install and upgrade time, with tests for a missing gh, an older gh and v2.103.3;
- release notes that say real gh v2.103.3 or newer is now required;
- a README that puts the client-only flow and server mode (`cwe.42`) side by side, with the Claude note
  from D1;
- upgrade and rollback proof using the existing config and token store;
- the forgejo-github-api pointer.

### F15. The live Forgejo twin has no PRs, and the e2e user is an admin (medium, confirmed)

The plan's live checks assume the twin holds PR states:

- the live contract runner;
- C5's "live-verify against the Forgejo twin";
- DESIGN's live acceptance.

But `GET https://git.dirtydishes.dev/api/v1/repos/dirtydishes/gfj-oracle/pulls?state=all` returns `[]`,
`open_pr_counter` is 0, and `main` is the only branch.

Separately, `scripts/e2e.sh:43-44` creates the e2e user with `--admin`. Every e2e read goes through the
admin protection path, and the non-admin fallback that PR #6 added is covered only by fixtures.

**Decision D5.** Kell approved both seeding the twin and adding a non-admin e2e user. The seeding is
build work for the bead that owns it, not something to do during this review.

**Fix.** `cwe.32` seeds the twin from the same scenario script the e2e uses. `cwe.31` adds a non-admin
e2e user and runs the read oracle as that user too.

### F16. Auto-merge can show as enabled after Forgejo dropped it (medium, confirmed)

The gateway rebuilds `autoMergeRequest` from the latest scheduled-merge or cancel event on the timeline
(`crates/fga-core/src/forgejo/reads.rs:29-40`). Forgejo's worker deletes the schedule *before* it tries
the merge (`services/automerge/automerge.go:227-236`). If the merge then fails, no cancel event is
written. One way it fails is a scheduled merge style that was disallowed in the meantime
(`services/pull/merge.go:246`). Another is a base update racing the push, which Forgejo handles as a
non-fast-forward failure at `merge.go:398`. Every poll keeps reporting auto-merge as enabled. Forgejo's own UI has the same blind
spot, per the FIXME at `automerge.go:231-233`. The PR stays visibly open, and
`gh pr merge --disable-auto` still works.

**Fix.** Add a worker-failure case to the mapping contract. Don't claim a schedule is active without
proof. One option is a cheap check that some current state still implies a schedule. If none exists,
document the limit in MAPPING §6.

### F17. Draft and ready need the server's WIP prefixes, which client mode can't learn (medium, confirmed)

Forgejo decides draft status from title prefixes that the server configures
(`models/issues/pull.go:798`), and no API exposes them. fga-core defaults to `WIP:,[WIP]`
(`crates/fga-core/src/map/pulls.rs:76`), and only `fga-server`'s config can override that. On a forge
configured with, say, `DRAFT:`, `gh pr ready` strips nothing, and the PATCH succeeds. gh then prints
success (`gh/pkg/cmd/pr/ready/ready.go:106`), but the PR is still a draft. The mutation never checks the
`draft` field it gets back (`graphql/mutations.rs:305-335`).

**Fix.** Add a per-host `wip_prefixes` setting to the shim's config. Make draft changes check the returned
`draft` value and fail if it didn't flip.

### F18. Protected-file rules don't count against CLEAN (medium, confirmed)

Forgejo refuses a merge with "Changed protected files" (`services/pull/merge.go:544-547`). The rule's
patterns appear in the API (`services/convert/convert.go:181-183`), but fga-core's `Protection` type
doesn't read them. Its `mergeStateStatus` can therefore reach `CLEAN` on a PR that Forgejo will refuse.
The rule is opt-in, and gh shows Forgejo's refusal when you try to merge.

Checking this needs more than a generic path glob. The API doesn't expose `ChangedProtectedFiles`
(`modules/structs/pull.go`). Forgejo lowercases the patterns, splits them on `;`, and compiles globs
with both `.` and `/` as separators (`models/git/protected_branch.go:214`). It also lowercases file
names before matching (`services/pull/patch.go:511`).

**Fix.** Read the patterns. If a rule has any, report `UNKNOWN` until a matcher that copies Forgejo's
rules, built like the existing gobwas branch-glob port, can report `BLOCKED` exactly.

### F19. A failed PR edit can leave earlier changes applied (medium, confirmed)

Forgejo's edit endpoint applies several changes before it checks that a new base branch exists
(`routers/api/v1/repo/pull.go:757-759`):

- title (`:649-650`)
- body (`:656`)
- deadline
- assignees
- milestone
- labels
- open or closed state (`:738-750`)

The gateway's `updatePullRequest` forwards title, body, state and base in one PATCH
(`crates/fga-core/src/graphql/mutations.rs:163-177`). So `gh pr edit --title X --base missing` returns
NOT_FOUND with the title already changed. A client that sends `state: CLOSED` with a bad base closes the
PR, and Forgejo sends the close notifications, before the error comes back. GitHub fails that mutation
without changing anything. gh's own `pr close` uses a different mutation, so this needs a non-gh client
or `gh api graphql`.

**Fix.** Check that the base branch exists before sending the PATCH. Add a case asserting the PR's state
after the error.

### F20. Upstream responses have no size limit (medium, confirmed)

Every upstream body is read whole: `response.bytes().await?.to_vec()` (`crates/fga-core/src/forgejo.rs:180`).
The "bounded bodies" commit `72ab657` limits read time, not size. Diffs, comments and timelines are read
unpaged. In server mode, any authenticated user can request a huge diff and spike the gateway's memory.

**Fix.** Cap response bytes per request and in total, and add an oversized-diff case that expects a clear
error.

### F21. A test normalizer hides the URL regression ChatGPT depends on (medium, confirmed)

The `pr_urls` normalizer rewrites both `/pulls/N` and `/pull/N` to `/pull/N`
(`scripts/contract/common.py:134-135`), and the imported `scripts/oracle/compare.py` does the same. The
case modelled on ChatGPT's lookup (`contract/cases/pr-list-head-chatgpt.json`) enables it. If the URL
layout ever regressed, both suites would stay green while ChatGPT dropped the PR (F6).

The SHA normalizer has the same problem. `scripts/oracle/compare.py:19` replaces every 40-character hex
string, and the `wide-1` case requests `headRefOid` (`scripts/oracle/cases.json:137`). So a gateway that
returned the base SHA as `headRefOid` would pass. ChatGPT 22943 hands that value to
`gh pr merge --match-head-commit <sha>`, gh sends it as `expectedHeadOid`
(`gh/pkg/cmd/pr/merge/http.go:77`), and Forgejo refuses the merge on a mismatch
(`services/pull/merge_prepare.go:66`). The Merge button would fail.

**Fix.** Add an unnormalized URL assertion to that case, and a check that `headRefOid` equals the
backend's actual PR head. More generally, wherever a normalizer replaces a value a client uses (host,
head SHA, node IDs), add a check on the real value next to it.

### F22. Actions run paging assumes 50 items per Forgejo page (low, confirmed)

`action_runs` computes offsets and when to stop from a fixed `PAGE_SIZE` of 50
(`crates/fga-core/src/actions.rs:75-92`). Forgejo clamps each page to the server's `MAX_RESPONSE_ITEMS`
(`services/convert/utils.go:15-20`). With a cap below 50, runs go missing without any error. Codeberg and
git.dirtydishes.dev both report `max_response_items: 50`, the default, so they aren't affected.

**Fix.** Read `/api/v1/settings/api` once per session and use its `max_response_items` as the page size.

### F23. "Per-command deadline" is emulator wording (low; the high-severity claim was rejected)

One reviewer argued that gateway deadlines don't bound the gh process, and rated it high. That's
**rejected** as stated:

- Each gateway request carries one 10 s `Session` deadline. Every upstream call in the request shares it
  (`crates/fga-core/src/forgejo.rs:143,194-200`), and it covers body reads.
- The `/user` lookup for authentication is limited by the client's own 10 s timeout.
- Real gh has no process-wide deadline against GitHub either.
- A stalled `git fetch` in `pr checkout` hangs the same way against GitHub.
- The merge retry loops are bounded and don't reset the deadline: three inner retries
  (`crates/fga-core/src/forgejo/writes.rs:237`) and one base-race retry (`graphql/mutations.rs:398`).

What remains is wording. The plan's "shared per-command deadline" (and R4's "slow server finishes within
the deadline plus 0.5 s") was written for an emulator. Restate it as a per-gateway-request deadline, and
say the wrapper doesn't kill gh.

### F24. Old turn logs contain local paths (low, confirmed)

`docs/turns/*.html` (7 files) and `docs/dev/testing.html` contain 13 lines with a local home-directory
path. Scrub them in a separate small change.

## 3. Proposed plan amendment

### Status of the existing work

| Bead | Now | Action |
|---|---|---|
| A1 `cwe.1` captures | still needed | Extend to direct HTTP clients (Claude), and commit redacted captures here |
| A2 `cwe.2` oracle | rewrite | Real gh through the gateway against the GitHub oracle; seed the Forgejo twin (F15) |
| A3 `cwe.3` mapping spec | largely done in `docs/gateway/MAPPING.md` | Close after the F13 doc pass, adding F4, F16, F17 and F18 |
| A4 `cwe.4` grammar table | obsolete | Retire; real gh owns argv |
| A5 `cwe.5` harvest | still needed, moved earlier | Also supersede `6y9` (F12) |
| R1 `cwe.6` contract runner | rewrite | F8 |
| R2 `cwe.7` config and credentials | done, kept | The config bridge (L4) builds on it |
| R3 `cwe.8` strict grammar | obsolete | Retire the merged validator and renderer after L3 passes; keep its fixtures as gateway cases |
| R4 `cwe.9` HTTP client | rewrite | The gateway client plus F10, F20, F22 and F23 |
| R5 `cwe.10` output, jq | obsolete | Retire |
| C1–C4 `cwe.11`–`.18` | rewrite | Gateway coverage and acceptance per tier, reconciled with G1–G3; C3c (`.17`) needs a real git round-trip |
| C2g `cwe.21` | rewrite | G4 schema gaps plus replay of the six documents (F7) |
| C5 `cwe.19` cutover | keep with new prerequisites | F9 |
| D `cwe.20` | still deferred | Drop "REST passthrough", which the gateway already serves |
| `cwe.22` serde_yaml | keep | |
| `65s` PR URLs | rewritten | `/pull/N` with per-mode acceptance (F6, D2) |

None of these beads is closed or deleted by this review. The retire and rewrite actions are proposals for
Kell, recorded as bead comments.

### New order

1. **M1. Land the move.** Open the `b5f0c62` merge as a PR into `rewrite`, with the F13 doc pass and a
   body that lists its behavior changes. **M2. Archive forgejo-github-api** with a pointer to this
   repo, after M1 merges.
2. **L0. Launcher spec.** Cover the process model, where gh's config lives (F1), socket lifecycle,
   token handoff, signals and exit codes, timeouts (F23), and concurrent calls. Opus writes it and Kell
   approves it.
3. **L1. Host isolation** in the gateway (F2), and **L2. routing parity** with gh (F3). Neither depends
   on the other, and both block L3.
4. **L3. Installed-wrapper vertical slice.** `gh pr view` and `gh pr checks` on a Forgejo repo, through
   real gh, the socket and the gateway. Acceptance includes the F1 config-immutability and alias tests,
   plus a multi-host `gh auth status`.
5. **L4. Config bridge** (F10), and **T. test rework**:
   - the R1 runner rewrite (F8);
   - the URL assertion (F21);
   - a two-commit seed (F4);
   - a non-admin e2e user (F15, approved under D5);
   - the twin seed (`cwe.32`, approved under D5).
6. **G. Gateway correctness:**
   - F4 commit order, which is the most urgent: it's a real bug in code that is already merged;
   - F16 to F20;
   - C2g / G4 (F7).

   These can run alongside L, because none of them depends on the launcher.
7. **S. Server mode** (`cwe.42`, D1). Its own install path and tests. It starts after M1 and L1, and
   runs alongside L2 to L4.
8. **C-tier acceptance** (rewritten C1 to C4), then **R. Release and retirement** (F14, D3), then
   **C5 cutover**.

### Release freeze on `rewrite` (D4)

Nothing ships from `rewrite` until the launcher slice (L3, `cwe.28`) passes. That means no release, no
tag, and no merge to `main`. Code may land on `rewrite` before then, including the audit fixes in
section 4, but it reaches users only after L3. The freeze is a blocking note on C5 (`cwe.19`) and on the
release bead (`cwe.39`), and both already depend on `cwe.28`. Lifting it early takes a new decision from
Kell.

The freeze doesn't cover `main` itself. A patch release cut from `main` is open question D6 in section 4.

### Decisions (Kell, 2026-11-01)

- **D1: client-only mode supports Claude's agent running `gh`; Claude's built-in PR status needs server
  mode.** Server mode gets real weight: its own install path (`cwe.42`), its own tests, and a README
  note telling Claude users to use it. It is not a secondary section. (F5)
- **D2: `/pull/N`.** The wrapper also sets a `GH_BROWSER` rewrite for gh's `--web`, and the README has a
  redirect recipe for people who run their own forge. Bead `65s` is rewritten with acceptance per
  mode. (F6)
- **D3: the minimum supported gh is v2.103.3.** Install and upgrade refuse with a clear message when gh
  is missing or older. This is in the release bead `cwe.39`. (F14)
- **D4: no interim release from `rewrite`.** F11's path is deleted in the restructure (`cwe.40`), not
  patched, and the release freeze above applies. A non-admin `--required` test runs against the new path.
  (F11)
- **D5: approved.** Seeding PR states on `dirtydishes/gfj-oracle` and adding a non-admin e2e user are
  both OK. Both are build work for their beads (`cwe.32`, `cwe.31`), not part of this review. (F15)

## 4. Where the shell audit's bugs land

A separate audit of the code that survives the restructure found 34 bugs. It covered credentials and
config, routing and the `gh` wrapper, and diagnostics, tracing and install. Each bug has a failing,
`#[ignore]`d test. The audit tracks them under epic `jko` and documents them in
`docs/audits/2026-11-01-surviving-shell.md`, both on local branch `audit/shell-sweep` at `8d2bfd9`,
which isn't merged yet. This section says what happens to each bug under option B.

**On `main`** means the file is byte-identical on `main` and `rewrite`, so released users have the bug
today. For files that differ between the branches, nobody has run the tests against `main` yet. Those
rows say "not checked".

The audit's finding IDs (R, C or D plus a number) are its own labels for routing, credentials and
diagnostics. They're unrelated to the decisions D1 to D6. Audit D6 (trace concurrency) and decision D6
(a patch from `main`) are different things.

Because of the release freeze (D4), fixes made now on `rewrite` reach users only after L3.

### Fix now

This code survives the restructure, and none of these fixes depends on the launcher. They are listed in
fix order: data loss first, then crashes and broken commands, then leaks and low-severity items.

| ID | Bead | Where | On `main` | Note |
|---|---|---|---|---|
| R3 | jko.1 | `shim.rs:63` | yes | `install-shim --force` overwrites the real `gh` through a symlink |
| D1 | jko.3 | `git_recorder.rs:101` | yes | recorder removal deletes its whole directory |
| D18 | jko.4 | `git_recorder.rs:307` | yes | the recorder execs itself without bound |
| C5 | jko.21 | `config.rs:270` | not checked | a failed write empties `config.toml` |
| C3 | jko.8 | `auth.rs:245` | not checked | a malformed `hosts` resets stored tokens |
| R7 | jko.7 | `routing.rs:90` | not checked | a non-UTF-8 env value panics the wrapper; the launcher passes env through too |
| D14 | jko.13 | `cli.rs:390` | not checked | uninstall misses bootstrap's directory; part of the rollback proof in `cwe.39` |
| D15 | jko.14 | `docs/installation.md:59` | yes | the documented Linux extract fails on GNU tar |
| D13 | jko.12 | `doctor.rs:297` | not checked | `doctor` passes with a missing wrapper target; `cwe.39` reworks the same checks for D3 |
| D6 | jko.11 | `trace.rs:399` | yes | concurrent trace writes interleave; L3 runs concurrent calls, so fix this first |
| D3, D4 | jko.9, jko.10 | `git_recorder.rs` | yes | redaction gaps |
| C1 | jko.18 | `config.rs:303` | not checked | `github.com:443` gets the Forgejo token; the config-side half of F2 (L1 is the gateway side) |
| C2, C4, C6 | jko.19, .20, .22 | `config.rs`, `cli.rs` | not checked | |
| R8, D5 | jko.17, jko.24 | `shim.rs` | yes | |
| D2, D7, D9, D10 | jko.23, .25, .26, .27 | `git_recorder.rs`, `trace.rs` | yes | |
| D19 | jko.30 | `trace_summary.rs:25` | yes | |

### Fix before the launcher slice

These two survive, and option B makes them matter more. L3 (`cwe.28`) now depends on both.

- **R1, `jko.15`** (`external.rs:260`). A copied or hardlinked dispatcher passes the "real gh" check.
  Today delegation drops the `gh` argument, so it can't loop. Under option B the launcher runs real gh as
  a child, with the socket configured. If discovery picks a copy of the shim instead, the copy sees a
  Forgejo invocation and starts the launcher again. That loop is **speculation**, since the launcher
  doesn't exist yet. It is the same class of bug as the old C2c hang.
- **R6, `jko.16`** (`external.rs:104`, `git_recorder.rs:177`). A child killed by a signal is reported as
  exit 1. The L0 spec has to define signal and exit-code passthrough for the gh child. This fix is the
  pattern it should copy.

### Absorbed by L2 (routing parity)

L2 (`cwe.27`) replaces repo resolution in `repo.rs`, so these two are fixed there. Their tests become
part of L2's acceptance. Under D4 nothing ships from `rewrite` before L3, so neither gets a separate fix
in `repo.rs`.

- **R2, `jko.2`.** `-R owner/repo` and `GH_REPO=owner/repo` are ignored inside a checkout.
- **R5, `jko.6`.** HTTPS remotes lose their port.

### Gone after the restructure

- **R4, `jko.5`** (`routing.rs:135`). `gh --version` in a Forgejo checkout routes to the Forgejo path.
  Under option B, real gh answers it. Keep its test as an L3 acceptance case rather than fixing the
  emulator path.

### Fix or drop with `trace smoke`

- **D11, `jko.28`, and D12, `jko.29`** (`codex_smoke.rs`). The 2026-10-30 assessment proposed dropping
  `codex_smoke.rs`, because contract tests supersede it. If `trace smoke` survives, fix both. If it's
  dropped, the tests go with it. Decide this at cutover (`cwe.19`).

### Open question for Kell (D6)

Audit bugs R3, D1 and D18 are high, and they ship in released code today. Should they go out as a patch
release cut from `main` ahead of the restructure? D4 freezes releases from `rewrite`, so without a patch
from `main`, released users keep these three until after L3. The fixes are small and don't touch code the
restructure replaces. They would be cherry-picked to `rewrite` afterwards.
