# Which clients call `gh`, and how (2026-10-30)

Two sources. The first is a static scan of an installed ChatGPT.app (Opus). The second is a web and
source survey of other tools (an Opus research subagent, which cites primary sources). Static evidence
says what a client *can* send. A live capture (rewrite plan, phase A1) says what it actually sends, and
in what order. The live captures below were taken with gh-capture, a logging gh wrapper, on a
separate test Mac. The wrapper and the raw captures aren't in this repo yet. They arrive, redacted,
with bead A1 (`cwe.1`).

## ChatGPT.app 26.1016.62342 (build 21477): static scan

Sources: `Contents/Resources/app.asar` (minified renderer) and
`Contents/Resources/plugins/openai-bundled/plugins/code-review/server.mjs` (the readable "Code Review"
plugin, version 26.1016.62342).

- **Scope grew a lot since build 6720.** The shim's last contract (6720, August) had 5 probes: version,
  auth status, pr list, a 25-field pr view, and pr checks. Build 21477 contains about 38 distinct `gh`
  argv shapes. These include PR mutations, REST through `gh api`, and at least 14 `gh api graphql`
  operations for a full PR-review pane: review threads, pending reviews, resolve or unresolve, reviewer
  search, and merge permissions.
- **`gh` is found through PATH.** Locally it spawns `["gh", ...args]`, and on remote hosts it runs
  `bash -lc "gh ..."` (`server.mjs:35514-35521`). A PATH shim still works. The Codex-bundled gh at
  `~/.codex/tools/gh_*` from openai/codex#38773 wasn't present on the scanned Mac.
- **The host comes from the argv, not an allowlist.** Order: `--hostname`, else a 3-part `--repo`, else
  `GH_HOST`, else `github.com` (`server.mjs:25845-25847`). A Forgejo host passes through.
- **A service mode bypasses `gh` entirely.** When Code Review uses OpenAI's GitHub service,
  `gh` execution is disabled (`server.mjs:25840`). In that mode no shim can help. Users on the GitHub
  connector won't benefit, and the docs must say so.
- **The PR inbox defaults to github.com.** Many argv shapes have no `--hostname` and no `--repo` (the
  `pr list --state` inbox query, `search/users`), so they resolve from cwd or default to github.com.

Argv shapes, normalized (`{…}` is an interpolated value, and `{n}` is a PR number):

```
--version
auth status [--active] [--hostname H]        auth status --json hosts --jq '.hosts | keys[]'
auth token --hostname H                      api user --hostname H        api rate_limit --hostname H
pr list --head B --author @me --state all --json number,url,state,headRefName
pr list --state S --limit N --json <list fields>          (inbox)
pr view {n} --json headRefOid --jq .headRefOid            pr diff {n}
pr create                                    pr comment {n} --body T [...]
pr edit {n} --title T | --body T             pr ready {n} [--undo]        pr reopen {n}    pr close {n}
pr merge {n} --squash|--merge --match-head-commit SHA [...]
pr merge {n} --auto --squash|--merge         pr merge {n} --disable-auto
api [--hostname H] repos/{o}/{r}/{path} [...]               api repos/{o}/{r}/releases/tags/{t}
api repos/{o}/{r}/pulls/{n}/comments --method POST -f body= -f commit_id= -f path= -F line= -f side=
api repos/{o}/{r}/pulls/{n}/reviews --method POST -f commit_id= -f event= [...]
api repos/{o}/{r}/pulls/{n}/requested_reviewers --method POST|DELETE -f reviewers[]=...
api --method GET search/users -f q='{…} in:login' -f per_page=10
api --hostname H markdown --method POST -f text= -f mode=gfm -f context={o}/{r}
api graphql [--hostname H] -f query=<doc> [-f/-F vars]      (14+ distinct documents, below)
```

GraphQL documents seen: `viewer{login}`; PR head/base/state/mergeable; PR permissions and merge
methods; reviewDecision + latestReviews + reviewRequests + reviews (paginated); pending review by login;
mentionableUsers/participants search; collaborators search; node lookups for review threads and
comments; and the mutations `addPullRequestReview`, `addPullRequestReviewThread`,
`addPullRequestReviewThreadReply`, `submitPullRequestReview`, `deletePullRequestReview`, and
`resolveReviewThread`/`unresolveReviewThread`. Some documents are module constants outside the scanned
region, so the count is a lower bound.

## Other tools (research survey)

| Tool | App calls `gh`? | What it sends | Forgejo support | Source |
|---|---|---|---|---|
| Codex CLI (TUI) | yes | `pr view --json number,url,state`, `repo view --json nameWithOwner,parent`, `api repos/R/commits/SHA/pulls` | no | `codex-rs/tui/src/branch_summary.rs` @7a030b97 |
| Claude desktop (Code tab) | yes | `pr create`; `pr view N --json number,title,state,isDraft,statusCheckRollup,reviewDecision,additions,deletions`; `pr checks`; repo from cwd | no (GHE partial) | code.claude.com/docs/en/desktop; anthropics/claude-code#59182, #96000 |
| Claude Code CLI | yes | `pr view` (footer badge). Some modes call `gh auth token`, then the API directly, which a shim can't intercept | GitLab via `glab` | interactive-mode docs; CHANGELOG 2.1.234, 2.1.251 |
| Cursor 3.11–3.13 | fallback | `--version`; `pr list --repo R --head B --state all --json url` | not found | forum.cursor.com 166725 |
| Conductor | yes | only `auth status` documented (closed source) | none documented | conductor.build troubleshooting |
| Superset/Paradev | yes | 16-field `pr view`, `pr list --head`, `repo view`, `api …/deployments`, `api graphql`, `api --paginate --slurp` | no | `apps/desktop/.../github/*.ts` @a229f62 |
| Nimbalyst | yes | `auth token --user`, `api graphql`, `api -X POST …/reviews` | no | `GhApiService.ts` @031a6e6 |
| Sculptor | yes | `api graphql` (PR polling), PR create | no | `sculptor/web/pr_status.py` @ea2a9dff |
| Vibe Kanban | yes | `pr create/view/list/checkout`, `api repos/…/pulls/N` | Azure only | `crates/git-host/src/github/cli.rs` @7c380708 |
| T3 Code, Emdash | token only | `auth status --json hosts`, `auth token` | **native Forgejo**, out of scope | `packages/source-control-*` @6fc9144a |
| GitButler, Zed, VS Code + Copilot, OpenCode | no | OAuth or browser | Zed: Gitea/Forgejo hosting | — |
| Warp, Windsurf, Kiro, Jules, Amp, Factory | no evidence | — | — | low confidence |

**GUI PATH:**
- Codex/ChatGPT loads the login-shell environment with a 5 s timeout. On timeout it falls back to
  `/usr/bin:/bin:…` and shows "GitHub CLI unavailable" (openai/codex#27740).
- Nix-installed gh isn't found (#34046).
- The shim's existing `gui_path.rs` work remains relevant.

## What this means for the rewrite

1. **One surface serves nearly every client:** `--version`; `auth status` (incl. `--json hosts`);
   `auth token`; `api user`; `repo view --json`; `pr list`, `pr view`, `pr checks` and `pr diff`
   with `--json`/`--jq`; and `pr create`. That surface covers ChatGPT's PR card, Claude desktop, Codex CLI,
   Cursor, Vibe Kanban and Crystal-style dashboards. It is tier T0–T1 in the plan.
2. **PR actions** (merge, ready, edit, comment, close and reopen) map directly to Forgejo REST. They
   form tier T2.
3. **Review panes are a different product.** ChatGPT's review UI, Nimbalyst, Sculptor and Superset need
   `gh api` REST path translation and **GitHub GraphQL emulation**. Every client ships its own GraphQL
   documents, and they change with each build. This is tier T3, and it gets its own decision.
4. **Some paths can't be shimmed:** ChatGPT service mode, and Claude Code's token-then-direct-API mode.
   Document them; don't chase them.

## Live capture: ChatGPT 26.1027.30824 (build 22943), 2026-10-30

32 calls. A Codex-mode project on a local checkout of the GitHub twin `dirtydishes/gfj-oracle`, branch
`s/pass` (PR #1, checks passing). One chat ("check this branch's PR"),
then Attach PR, then open the PR pane.

Distinct calls made by the app itself (process `Codex (Service)`):

| Call | Purpose (inferred) |
|---|---|
| `--version` | probe |
| `auth status --active [--hostname github.com]` | probe |
| `api user --hostname github.com` | probe / viewer |
| `pr list --head <branch> --author @me --state all --json number,url,state,headRefName --repo <owner>/<repo>` | branch → PR lookup |
| graphql `44345376`: PR `body bodyHTML` | PR description |
| graphql `d9f1954f`: `reviewDecision`, `latestReviews`, `reviewRequests`, `reviews` (paginated) | review state |
| graphql `cfad5414`: `viewerPermission`, merge methods allowed, PR additions/deletions/changedFiles, `viewerCan*`, `autoMergeRequest` | merge button and permissions |
| graphql `0788b4d1`: **batched literal query** (`p0: repository(owner:"…",name:"…") { pullRequest(number:1) {…} }`) with `statusCheckRollup` contexts (CheckRun and StatusContext) | PR card and checks |
| graphql `f7a31c84`: head/base oid, state, isDraft, mergeable, mergeStateStatus | mergeability |
| graphql `12df611e`: summary, commits, comments, reviews, review threads (5 `@include` flags) | PR pane timeline |

The agent inside the app (process `codex`) ran
`pr view --json number,title,url,state,isDraft,mergeable,mergeStateStatus,statusCheckRollup,headRefName,baseRefName`.

**Consequence:**
- **The app no longer calls `pr view` or `pr checks`.** Even the basic PR card (open/merged state and
  checks) comes from GraphQL.
- **A shim limited to T0–T2 shows no PR status in ChatGPT 22943.** It only finds that a PR exists.
- **The app's GraphQL is fixed per build.** Each document is a constant (hashes above). One of them is
  built with interpolated literals and aliases (`p0:`), so matching has to normalize whitespace and
  literals, not just hash the raw text.

### Same build, all PR states (176 calls)

For each of `s/fail`, `s/pending`, `s/draft`, `s/nochecks`, `s/merged` and `s/closed`, Opus switched
the checkout to that branch, started a one-line chat, and attached the PR.

- **The same 6 GraphQL documents and `pr list --head` throughout.** No new call shapes appeared. The
  batched status query is the same document every time once literals are normalized (`d1dcfb35`), and
  it always carries one `p0:` alias.
- **Polling:** the batched status query repeats about every 60 s per open thread (54 of 176 calls), and
  `api user` repeats too. The shim's latency budget matters at that rate.
- **What the UI shows** (screenshot crops arrive with A1):
  - The PR item's badge shows failing (red), pending (yellow), draft, no-checks, and merged (purple).
  - **A closed PR isn't offered** for attaching: `pr list --state all` finds it, and the app ignores it.
  - The PR pane has a Merge dropdown and "Open pull request in browser".
- The PR-state captures never call `--version` or `auth status` again after startup; those calls
  happen once per app session.

### Same build, actions (included in `chatgpt-22943-states.jsonl`)

| UI action | gh call |
|---|---|
| PR pane → Merge ▾ → **Merge** (PR #8) | `pr merge 8 --merge --match-head-commit <head sha> --repo github.com/dirtydishes/gfj-oracle` (3-part `--repo` with host) |
| Merge ▾ menu | offers only **Merge** and **Enable auto-merge**. Squash isn't offered, which follows from `squashMergeAllowed` in graphql `cfad5414`. |
| In-app agent asked to open a PR (PR #9) | `pr create --base main --head s/nopr --title <t> --body <b>` (process `codex`) |

No app-side Create PR button was found for a pushed branch with no uncommitted changes. The static scan's
bare `pr create` probably belongs to the commit → push → PR flow after the agent edits files. That flow
isn't captured yet.

## Live capture: Claude desktop 4.4212.0 (bundled claude-code 2.1.309), 2026-10-30

Sources: the gh-capture log of gh calls, the Forgejo-side access log, and a screenshot of the PR
status. They arrive with A1.

- **GitHub remote (a checkout of the GitHub twin on `s/fail`):** the only gh calls are
  `auth token --hostname github.com`, from both the app (`Claude`) and its agent process (`claude`),
  repeated each session. The UI still shows "scenario: checks fail · #2 · 1 failing", so PR and check
  data come from **GitHub's API directly**, with gh's token. A gh shim never sees those calls.
- **Forgejo remote (a checkout of the Forgejo twin):** `auth token --hostname git.dirtydishes.dev` exits 1, and
  the UI shows nothing PR-related.
- **Forgejo remote with a fake token** (`gh-capture` probe mode, `~/.gh-capture/fake-token-hosts`): Claude
  calls the **GitHub Enterprise REST layout on the Forgejo host**:
  `GET https://git.dirtydishes.dev/api/v3/repos/{owner}/{repo}` and
  `GET …/api/v3/repos/{owner}/{repo}/pulls?head={owner}:{branch}&state=open&per_page=1`.
  Both return 401, because Forgejo has no `/api/v3`.

**Consequence:** Claude desktop can't be supported by any gh CLI shim. It *could* be supported by
something that answers GitHub-Enterprise-style `/api/v3` (and presumably `/api/graphql`) on the forge's
own hostname. See rewrite plan §7.

## Live capture: Claude Code CLI 2.1.309 → 2.1.314, 2026-10-30

Sources: the gh-capture log, the Forgejo-side access log and a screenshot of the footer, arriving with
A1. The CLI auto-updated between the two runs. Auth went through an API-key-style login, not a
claude.ai login. That doesn't explain the behavior: Claude desktop on a normal claude.ai Pro login
showed the same path, `gh auth token` and then the API
directly. (A research-survey claim that Claude Code picks its PR path by auth mode couldn't be found in
the changelog and is unverified.)

- **GitHub remote (`s/fail`):** `auth token` and `auth token --hostname github.com` ran, then the footer
  showed `PR #2`. There was no `pr view`, so PR lookup goes through the API directly. There was also an
  unrelated startup call:
  `issue list -R anthropics/claude-code --author @me --state closed --search closed:>… --json …`.
- **Forgejo remote, fake token:** the same GHE REST layout as desktop, `GET /api/v3/repos/{o}/{r}` and
  `GET /api/v3/repos/{o}/{r}/pulls?head={o}:{branch}&state=open&per_page=1`, with
  `User-Agent: claude-code/2.1.314`.

**Claude desktop and Claude Code share one client path:** a gh token, then GitHub REST at
`https://<remote host>/api/v3`. Supporting Claude means answering those endpoints on the forge's
hostname, not shimming the gh CLI. Claude's agent also runs `gh` itself through Bash for PR work, and
on a GHE-configured host those calls would reach the same gateway.
