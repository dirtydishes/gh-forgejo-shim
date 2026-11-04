# gh-forgejo-shim core rewrite: plan (draft for Kell)

Date: 2026-10-30. Inputs: `assessment.md` §4 (core design), `clients.md` (who calls `gh`), and decisions
in `assessment.md` §7. This plan sets the order of work and the contract mechanics. The design itself
is assessment §4 and isn't repeated here.

## 1. Scope: tiers, not clients

The contract has two halves. **Client captures** say what each app sends; per-app build, exact argv.
**The tier** says what the shim promises any caller, including agents. A shim release states the tier
it supports. Everything above that tier fails locally with a clear error and is never delegated.

| Tier | Commands | Who needs it | Status in plan |
|---|---|---|---|
| **T0 probes** | `--version`; `auth status [--active] [--hostname] [--json hosts] [--jq]`; `auth token [--hostname]`; `api user`; `api rate_limit` | every client gates on these | phase C1 |
| **T1 PR status** | `repo view --json`; `pr list` (`--head --author @me --state --limit --json --jq --repo`); `pr view [N] --json` incl. `statusCheckRollup`, `reviewDecision`; `pr checks [N] --json`; `pr diff`; `api repos/{o}/{r}/commits/{sha}/pulls` | ChatGPT PR card, Claude desktop, Codex CLI, Cursor, dashboards, agents | phase C2 |
| **T2 PR actions** | `pr create`, `pr comment`, `pr edit --title/--body`, `pr ready [--undo]`, `pr merge [--merge/--squash] [--match-head-commit] [--auto/--disable-auto]`, `pr close/reopen`, `pr checkout` | ChatGPT buttons, Claude "merge when ready", agents | phase C3 |
| **T-agent** | `issue list/view/create/comment`, `run list/view` (Forgejo Actions), plain-text (non-`--json`) output for T1/T2 | Claude Code, Codex CLI, and in-app agents running gh themselves | phase C4 |
| **T3 review pane** | `gh api` REST path translation (reviews, review comments, requested reviewers, `search/users`, `markdown`) and `gh api graphql` for a fixed table of documents | ChatGPT review pane, Nimbalyst, Sculptor, Superset | **deferred: decision after A1 captures** |

Why T3 waits: GraphQL emulation means a table keyed by query document, with one handler per document.
Each client ships its own documents and changes them per build, so this is the exact churn that sank the
old shim, only larger. It's only worth doing if captures show Kell's actual tools need the review pane
on Forgejo. Until then, T3 calls get a gh-shaped error (stdout `{"errors":[…]}`, exit 1), so the UI
shows "unavailable" for that pane while the T0–T2 features keep working.

**Non-TTY output is the oracle for agents.** Agents run gh without a TTY, and gh's non-TTY output is
plain and tab-separated (no color, no truncation). The shim matches that byte-for-byte. TTY formatting
is out of scope.

## 2. Contract v2 mechanics

All contract data lives in the shim repo under `contract/`:

```
contract/
  tiers.toml                      # tier → operations; the grammar table's source of truth
  clients/<client>/<build>.json   # captured argv templates, feature, failure effect, provenance
  cases/<op>/<case>.json          # one scenario: argv, oracle result, fake-Forgejo exchanges
```

- **Oracle = real gh against twin repos.** One scenario script builds the same state on a GitHub repo
  and on a Forgejo repo (git.dirtydishes.dev). States to cover:
  - PRs: open, draft, merged, closed, from a fork, no checks, with checks passing, failing, and pending.
  - Reviews: approved, changes requested, review requested.
  - Repos: more than one page of PRs.

  The script runs pinned real `gh` against GitHub and records exit code, stdout and stderr. Recorded
  Forgejo REST responses for the twin become the fake-server exchanges. Declared normalizations (host,
  owner, URLs, timestamps, IDs) make the two comparable.
- **Two runners, one case format.**
  - **The CI runner** runs the binary with a fake Forgejo server and a decoy real `gh` that fails the
    test if it's ever called. It asserts the oracle output.
  - **The live runner** (`gfj contract verify --live`, run by hand) runs the same argv against the real
    Forgejo twin. That catches mapping errors the recorded exchanges hide.
- **Coverage is enforced.** A test fails if any argv template in `clients/` or any operation in
  `tiers.toml` lacks a case. Unsupported means a case whose expected result is the error.
- **gh version is pinned in the cases.** Re-capturing on a new gh release is a deliberate step, and its
  diff shows exactly what changed.

## 3. How the code gets there: strangler, not big bang

The new core grows in new modules next to the old one. `routing` gains one dispatch table: operation →
new handler, else the old path. Each WP moves a set of operations and deletes the old code paths they
replace. Main stays green and releasable throughout. The last WP deletes `read_only.rs`, `normalize.rs`
and the routing parser.

The new core's modules follow assessment §4:
- `grammar` and `invocation`: one parser, fed by `tiers.toml`.
- `resolve`: host and repo resolution, producing either a Forgejo target or a delegate to real gh.
- `forgejo`: one HTTP client with a deadline and pagination.
- `project`: one projection function per GitHub field.
- `output`: JSON, `jaq`, and non-TTY text.
- `errors`: error text identical to gh's.
- `credentials` and `config`: config parsed with the toml crate.

## 4. Phases

### A. Contract (Opus and Kell; no Sol)

| Step | Who | Output |
|---|---|---|
| A1 Client captures | Opus writes `tools/gh-capture`, a PATH wrapper that logs argv, cwd, exit and stdout size, then execs real gh. Kell (or Opus via cua-driver, if Kell prefers) uses each app against a GitHub repo. | `contract/clients/*`: ChatGPT 21477, Claude desktop, Codex CLI, Claude Code CLI, plus any of Cursor/Conductor/Superset Kell wants to install |
| A2 Twin repos and oracle capture | Opus | scenario script, `contract/cases/*` for T0–T2 |
| A3 Semantic mapping spec | Opus | one table: GitHub field or state ← Forgejo source, with the hard ones decided: `reviewDecision`, `mergeStateStatus`, `mergeable`, check `bucket`, `isDraft`/`pr ready` (Forgejo WIP prefix vs `draft`), `--auto` (Forgejo merge-when-checks-succeed), `autoMergeRequest`, and author/`@me`. Each mapping is checked against the live Forgejo API, not taken from memory. |
| A4 Grammar table | Opus | `tiers.toml` for T0–T2: flags, arity, `--json` fields per command |
| A5 Harvest | Opus | Branch fixtures and test support copied in, then PR #29 closed with a note |

### B. Skeleton (Sol work packages; serial, each needs the previous one)

| WP | Package | Old WPs it replaces | Gate in addition to fmt/clippy/test plus the Linux gate |
|---|---|---|---|
| R1 | Contract runner, fake Forgejo server and decoy gh. Seed with 3 T0 cases. | WP4 | Runner passes; a mutated expected stdout makes it fail; the decoy fails the run if it's called |
| R2 | `config` (toml crate, one loader) and `credentials` (host-keyed store, env rules, `gfj auth import`) | WP3, WP11 | Multi-line hosts; `#` in strings; two-host token isolation; grep confirms no scraping code |
| R3 | `grammar` and `invocation` from `tiers.toml`; strict errors; dispatch table in routing; Forgejo-resolved invocations never delegate | WP5, WP6 | Unknown flag, field or op gives gh's error text with exit ≠ 0, and the decoy is never called |
| R4 | `forgejo` client: deadline, pagination; drop `ureq` | WP8 (part), WP10 | 3-page fake server; slow server finishes within the deadline plus 0.5 s; `! cargo tree -i ureq` |
| R5 | `output`: JSON projection plumbing, `jaq`, non-TTY text writer | WP7 | jq table generated by the real `jq` binary |

### C. Operations (Sol; can run in parallel worktrees, at most 2 at once because review is the bottleneck)

Each WP's gate is "these contract cases pass, and the old code they replace is deleted".

- **C1** T0 probes.
- **C2a** `pr list` and `repo view`. **C2b** `pr view`. **C2c** `pr checks`, `pr diff` and
  `commits/{sha}/pulls`.
- **C3a** `pr create`, `pr comment` and `pr edit`. **C3b** `pr merge`, `pr ready`, `pr close` and
  `pr reopen`. **C3c** `pr checkout`.
- **C4** issues, runs, and text output for the T1/T2 commands.
- **C5 cutover:** delete the old core and Python-isms, then docs and a release. Live-verify against the
  Forgejo twin, and Kell checks each app's UI against the capture.

### D. T3 decision

This needs the A1 captures and the shipped C phase. The question is whether review panes on Forgejo are
worth a per-client GraphQL table. If yes, it is per client and per build, starting with whichever tool
Kell actually uses.

## 5. Decisions needed from Kell

1. **Twin oracle repos.** Create a public `dirtydishes/gfj-oracle` on GitHub and a matching repo on
   git.dirtydishes.dev. Both would hold throwaway PRs, reviews and CI runs.
2. **Who drives A1.** Either Kell clicks through the apps, or Opus drives them with cua-driver while
   Kell is logged in. Either way the actions run against the GitHub oracle repo only.
3. **T3 deferred**, with gh-shaped errors until captures justify it. This is the recommendation.
4. **Which extra apps to install for capture.** Cursor, Conductor and Superset aren't on the capture machine.
   ChatGPT and Claude desktop are the minimum (Claude desktop isn't installed either).
5. **Beads remote.** The beads database's sync remote isn't reachable, so `bd` can't sync. Fixing it
   means changing the maintainer's server, so it waits for Kell's go-ahead. Until then, the beads epic
   and issues for R1–C5 can't be created without forking the database.

### Answers (Kell, 2026-10-30)

1. Twin repos: yes. Both are created: `dirtydishes/gfj-oracle` on GitHub and on git.dirtydishes.dev.
2. A1: Opus drives the apps on a separate test Mac, as far as it can alone.
3. T3: deferred for now. **It still has to be built** once the captures give enough evidence to do it
   right. Deferral means "after evidence", not "never".
4. Install whatever capture needs. Kell doesn't use Cursor, Conductor or Superset, so they're skipped.
5. Beads remote: Sol (xhigh) fixes the server. It was repaired the same day.

### Branches

- `rewrite` on GitHub is the integration branch, cut from main `a7e4200`. CI runs on pushes to it and
  on PRs into it.
- Each work package gets its own branch, cut from `rewrite`, with a PR into `rewrite` merged by merge
  commit.
- `rewrite` merges into main at cutover (C5), or earlier at a tier milestone if Kell wants a release.
  (Superseded by the release freeze, decision D4 in `plan-review-2026-11-01.md`.)
- The repo ruleset `copilot-review` blocks branch deletion and force-push on every branch, so merged WP
  branches stay on the remote.

### Amendment after the first live capture (2026-10-30)

ChatGPT build 22943 renders its PR card from 6 GraphQL documents. Only branch→PR lookup still uses
`pr list` (see `clients.md`, "Live capture"). Kell's call on T3 ("defer, but build it once there's
evidence") needs splitting:

- **T1g, GraphQL reads for the PR card:** the read-only documents a GUI app needs for PR status, checks,
  reviews and mergeability. This is now the evidence-backed core of ChatGPT support, and it should move
  ahead of T2 for ChatGPT. Design: parse each document (graphql-parser crate), normalize it, look up a
  handler keyed by its normalized shape, and resolve fields from the same projection table T1 uses.
  An unknown document gets a gh-shaped GraphQL error.
- **T3, review-pane mutations and REST passthrough:** these stay deferred until captured.

**Kell approved this split on 2026-10-30.** T1g is in scope. Only review-pane mutations and REST passthrough (T3) stay deferred.

## 6. Size estimate

- **Phase A:** Opus, 2–3 sessions, plus about an hour of Kell's time at the GUI.
- **Phase B:** 5 Sol WPs, serial, about 1–2 fix rounds each based on WP1/WP2.
- **Phase C:** about 9 Sol WPs, 2 in parallel.
- **New core:** 2–3k lines plus contract data. It replaces about 8.5k.

## 7. Open design question raised by the captures: emulate gh's CLI, or GitHub's API?

The captures show the clients reaching GitHub in three ways:

- **ChatGPT 22943:** gh argv (`pr list`, `pr merge`), but mostly `gh api graphql` with fixed documents.
- **Claude desktop:** `gh auth token`, then **GitHub Enterprise REST** (`https://<host>/api/v3/...`)
  directly.
- **Agents (codex, claude):** open-ended gh argv such as `pr view --json`, `pr create`, and so on.

**Option S (current plan): a gh CLI shim.** It emulates gh's argv grammar, output formatting,
`--json`/`--jq`, error text, plus GraphQL documents. It can't help Claude desktop. It needs the
PATH/launchd wrapper machinery, which has its own bug class.

**Option G: a GitHub-API gateway for the forge host.** It's a small service answering `/api/v3/*` and
`/api/graphql` on the forge's hostname, translating to Forgejo's `/api/v1` and forwarding the user's
Forgejo token. It's deployed as a reverse-proxy rule next to Forgejo. The forge already owns that
hostname, and Forgejo doesn't use those paths. Clients would then talk to real gh, configured with
`gh auth login --hostname <forge>` as a GHE host, or to the API directly as Claude does.

- **What G removes:** the argv grammar, output formatting, jq, templates and error-text fidelity. Real
  gh produces all of those, byte-for-byte. It also removes the PATH wrapper and LaunchAgent, and with
  them the whole GUI-PATH bug class.
- **What G adds:** Claude desktop. Any agent running arbitrary gh commands works as far as API coverage
  goes.
- **What stays:** the GraphQL work for ChatGPT's documents, which sits in the gateway instead of the
  CLI. So does the field-projection table (A3).
- **What changes:**
  - **The contract** becomes HTTP: requests clients and gh send, captured at the proxy, as in the
    access log above.
  - **The oracle** becomes GitHub's API responses.
  - **Deployment** needs server-side access. That's fine for self-hosters but not for hosted Forgejo
    like Codeberg, where the CLI shim is the only option.
- **Unknowns to spike before deciding (Opus, about 1 session):**
  - Does gh 2.103 accept a GHE host backed by a stub, for `auth login --hostname`, `pr list` and
    `api graphql`?
  - Which `/api/v3` endpoints does gh hit for T0–T2?
  - Do ChatGPT's GraphQL calls follow the PR's host (`--hostname <forge>`)?

A hybrid is possible: G for self-hosters, plus a thin CLI shim that maps a hosted forge onto a local
gateway.

**Spike result (2026-10-30):** [`gateway-spike.md`](gateway-spike.md). Real gh works against a GHE-shaped host for all of T0–T2. That needs 21 named GraphQL operations, including schema introspection, and 7 REST endpoints. The recommendation is G, in its own repo, built as a GraphQL server over a subset of GitHub's schema. Phase B work packages (R1–R5) are on hold until Kell decides.
