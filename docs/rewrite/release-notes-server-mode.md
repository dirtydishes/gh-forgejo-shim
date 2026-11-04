# Release notes draft: server mode

A draft of the server-mode section of the rewrite's release notes, to merge into the full notes. The
client-only changes and the gh v2.103.3 requirement have their own sections.

## Server mode: the gateway on your forge

This release adds a second install path. Server mode runs the gateway, `fga-server`, next to Forgejo
and mounts the GitHub Enterprise API on the forge's own hostname. The forge then answers
`https://forge.example.com/api/v3` and `/api/graphql`, and clients need no wrapper: a plain
`gh auth login --hostname forge.example.com` with a Forgejo token is enough.

**Who needs it.** Use server mode if you use Claude Code or Claude desktop. Their pull request status
calls the forge's GitHub API itself, outside any `gh` process, so only a gateway on the forge can
answer it. The client-only wrapper still covers agents that run `gh`, including Claude's agent in a
shell. You can run both. Server mode needs control of the forge's reverse proxy, so a hosted forge
such as Codeberg can't use it.

**What it serves.** The `gh` commands that coding tools use against pull requests, issues, checks and
Actions runs, through gh 2.103.3's own REST and GraphQL requests: `pr view`, `list`, `status`,
`checks`, `diff`, `checkout`, `create`, `edit`, `comment`, `ready`, `close`, `reopen` and `merge`
(including `--auto`); `issue list`, `view`, `create` and `comment`; `run list` (including
`--workflow`) and `run view`. It also serves Claude's REST lookups for a branch's pull request, and
the read documents ChatGPT desktop's pull request view sends, and its merge; the review pane's own
mutations (review threads, pending reviews) aren't served yet.

**Pull request links.** Every pull request URL uses GitHub's `/{owner}/{repo}/pull/{n}` layout, which
ChatGPT and Codex.app look for. In server mode the gateway redirects those links to Forgejo's
`/pulls/{n}` page. The README has the same redirect for nginx, Caddy and Nginx Proxy Manager, for
client-only users who run their own forge.

**Setting it up.** Build the image from the release's checkout, start it with the example compose
file (`packaging/gateway/compose.yaml`), and route `/api/v3/`, `/api/graphql` and
`/{owner}/{repo}/pull/{n}` to it in the forge's own site block. The repository ships an nginx
snippet, which also works in Nginx Proxy Manager, and a Caddy site block. See the README's
"Server Mode" section and `docs/gateway/server-mode.md`.

**Upgrading and rolling back.** The gateway keeps no state. Tag each image with its release and keep
the previous image and compose file: an upgrade switches to the new tag, and a rollback switches back.

**Security.** The gateway forwards each client's own Forgejo token and stores none. It answers only the
forge's own host and port, and refuses any other host with `421` before a token is used, so a GitHub
or second forge's token never reaches Forgejo. Every API route needs a token.

**Known differences from GitHub**, documented in `docs/gateway/MAPPING.md` and `docs/gateway/DESIGN.md`:

- A closed or merged pull request, or one whose branch is no longer its head, checks out from its own
  ref (`refs/pull/{n}/head`) rather than its branch, so `gh pr checkout` never takes a branch that was
  recreated or moved. An open pull request whose branch isn't its head counts as cross-repository, so
  `gh pr merge -d` and `gh pr close -d` leave that branch alone.
- Forgejo answers "still checking" and "conflicts" the same way, so a conflicting merge takes about
  5 s to report.
- Actions runs have no jobs, logs or artifacts through Forgejo 15's API, and every run is attempt 1.
- One gateway serves one forge.

Follow-up work is tracked in the repository's issues labelled `post-release`.
