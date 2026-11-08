# Apps and the shim

The shim matters most for the desktop apps' own pull request features: the status icons, PR
panels, review comments you add to the chat, and merge buttons. They work only when something
answers the app's `gh` calls (client-only mode) or GitHub's API (server mode) with Forgejo's data.
Agents that can run `gh` from a shell may also work, but most are untested. The
[table in the README](../README.md#works-with) shows what has been tested and which mode each app
needs. Tried an app? Tell us how it went in
[#271](https://github.com/dirtydishes/gh-forgejo-shim/issues/271).

## Agents that can run `gh`

- **GitHub Copilot CLI.** On GitHub Enterprise Server, GitHub's docs say it can do issue and pull
  request work through `gh`, signed in to that server
  ([set up Copilot CLI](https://docs.github.com/en/enterprise-server@3.22/copilot/copilot-on-ghes/set-up-copilot-cli)).
  Untested with the shim.
- **Cursor's agent.** Untested; [#242](https://github.com/dirtydishes/gh-forgejo-shim/issues/242)
  tracks it.
- **Others:** Gemini CLI, Cline, Roo Code, Kilo Code, Continue, Windsurf (Cascade), Kiro, Goose,
  Amp, OpenCode, Warp, Junie CLI and Zed's agent. They're listed only because each can run shell
  commands. We know of no docs on how they handle pull requests with `gh`, and none has been tested
  with the shim.

You may need to tell any of them that `gh` works in a Forgejo repository. The two lines in the
[`CLAUDE.md` tip](../README.md#tell-claudes-agent-to-use-gh) likely help in whichever file your
agent reads, such as `AGENTS.md`.

## Editors and Git apps

- **VS Code's
  [GitHub Pull Requests](https://marketplace.visualstudio.com/items?itemName=GitHub.vscode-pull-request-github)
  extension** would need server mode. Untested; we don't know whether its sign-in works against
  the gateway.
- **[GitHub Desktop](https://github.com/apps/desktop):** untested. We don't know whether its pull
  request features work with a host other than GitHub.

## You may not need the shim

These tools speak Forgejo or Gitea themselves:

- **VS Code:** the [Forgejo Integration](https://marketplace.visualstudio.com/items?itemName=maxking.forgejo-vscode)
  extension browses pull requests, issues and Actions on Forgejo, Gitea and Codeberg.
- **JetBrains IDEs:** the [Gitea & Forgejo Pull Requests](https://plugins.jetbrains.com/plugin/34605-gitea--forgejo-pull-requests)
  plugin, from a third party, can review, comment on, approve and merge pull requests.
- **Zed** links commits, issues and pull requests to a self-hosted Forgejo or Gitea, which it detects
  from the remote URL or which you add under `git_hosting_providers`
  ([Git integrations](https://zed.dev/docs/git)), and opens Forgejo's
  new pull request page for a branch ([source](https://github.com/zed-industries/zed/blob/d2a4fb6ded7c470be25d1541281da3882495b65b/crates/git_hosting_providers/src/providers/forgejo.rs#L231-L244)).
- **lazygit** opens Gitea's new pull request page once you set the `gitea` provider
  ([custom pull request URLs](https://github.com/jesseduffield/lazygit/blob/bd4062a1e745b47d87ee8d0b24649529ed28b499/docs/Config.md#custom-pull-request-urls)).
- **Tower** for Windows 9.1 added Gitea and Gitea Self-Hosted, with pull request review
  ([release notes](https://www.git-tower.com/release-notes)).
- **CLIs:** [`fj`](https://codeberg.org/forgejo-contrib/forgejo-cli), a Forgejo CLI from
  forgejo-contrib, and [`gt`](https://gitea.com/gitea/cli), the official Gitea CLI, which replaced
  the archived `tea`.

A terminal agent can be told in `AGENTS.md` to use `fj` for Forgejo repositories. The shim matters
most for the desktop apps' PR panels and buttons, which nothing else fills.

## Can't help

These features run in a vendor's cloud or through a GitHub App and talk to GitHub from the vendor's
servers, so the shim never sees their requests. The same product's local agent may still work; see
the [README table](../README.md#works-with) and the list above.

- **Cursor:** cloud agents and Bugbot. [Bugbot](https://cursor.com/docs/bugbot) supports GitHub
  Enterprise Server, and [#243](https://github.com/dirtydishes/gh-forgejo-shim/issues/243) asks
  whether the gateway could stand in for one.
- **GitHub Copilot:** the [cloud agent](https://docs.github.com/en/copilot/concepts/copilot-surfaces/copilot-on-github),
  which works only with repositories hosted on GitHub, and
  [Copilot code review](https://docs.github.com/en/copilot/concepts/agent-capabilities/agent-powered-workflows-and-operations/code-review).
- **Warp:** [cloud agents](https://docs.warp.dev/platform/integrations/github/), through its GitHub
  App.
- **Kiro:** the [Kiro Web agent](https://kiro.dev/docs/web/github/), through the
  Kiro Agent GitHub app.
- **Amp:** [hosted agents](https://ampcode.com/docs/github), which reach GitHub through a GitHub App
  connection.
- **OpenCode:** its [GitHub Action](https://docs.opencode.ai/docs/github/).
- **Factory:** the [Droid action](https://github.com/Factory-AI/droid-action).
- **Windsurf:** [PR Reviews](https://docs.devin.ai/desktop/windsurf-reviews/windsurf-reviews), a
  GitHub bot for GitHub Cloud.
- **Augment:** [Code Review](https://docs.augmentcode.com/codereview/overview), through the Augment
  GitHub App.
- **Kilo:** [Code Reviews](https://kilo.ai/docs/automate/code-reviews/github), through a GitHub App.
