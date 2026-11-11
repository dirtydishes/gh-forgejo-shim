# Agent-guided install

Steps for a coding agent, such as Claude Code or Codex, that a user has asked to install
gh-forgejo-shim. A person can follow them too. Each step ends with how to tell it's done; finish
one before starting the next.

## Rules for the whole install

- **Tokens stay out of the chat.** Ask for a token, password or passphrase only through a secret
  tool, if you have one: a tool that asks the user for the value and hands it to you as a file or
  a reference. Pass the value to the command on stdin (`--with-token < "$file"`, `sudo -S`), and
  never echo it, print it, put it in a command's arguments or write it to a file the user didn't
  choose. Without such a tool, give the user the command to run in their own terminal. In Claude
  Code, they can type `!` before a command to run it in their shell.
- **Ask before each change**, and show the command first. Reading, such as `git remote -v`,
  `gh --version` or `gfj doctor`, needs no permission.
- **This guide changes nothing on the forge's server.** Server mode's steps are for whoever runs
  the forge; step 5 checks them.
- **Stop and hand over** when a command fails in a way this guide doesn't cover. Show the user the
  command, its output and the output of `gfj doctor`, and point them to
  [the issue tracker](https://github.com/dirtydishes/gh-forgejo-shim/issues/new).

## 1. Find out what the user needs

Work out what you can first, then ask the user only what's left, in this order:

1. **The forge.** Run `git remote -v` in the repository the user wants to work in. A remote that
   isn't on `github.com` names the forge's host, such as `forge.example.com`. With no such remote,
   ask for the forge's address.
2. **The apps.** Ask which apps will work with the forge: Codex CLI, ChatGPT desktop, Claude Code,
   Claude desktop, or others.

**Server mode** is needed when the apps include Claude Code or Claude desktop: they look up a
branch's pull request on the forge directly, outside `gh`. Every other app needs only the
client-only wrapper. Install the wrapper in either case; Claude's agent uses it when it runs `gh`.

Done when you know the forge's host and whether server mode is needed.

## 2. Install

1. Run `gh --version`. The shim needs gh 2.103.3 or newer. If gh is missing or older, install or
   upgrade it with Homebrew (`brew install gh` or `brew upgrade gh`), or on Debian and Ubuntu from
   [GitHub's apt repository](https://github.com/cli/cli/blob/trunk/docs/install_linux.md).
2. With Homebrew, run `brew install dirtydishes/tap/gh-forgejo-shim`. Without it, install a
   release tarball as [installation.md](installation.md#github-release-tarballs) shows.
3. Tell the user that the shim sends one anonymous usage report a day, which counts the GitHub
   features their agents ask for that the shim lacks, and that `gfj telemetry off` stops it.
   [telemetry.md](telemetry.md) lists every field.

Done when `gfj --version` prints a version.

## 3. Set up the wrapper

Run `gfj bootstrap` inside the user's Forgejo checkout. It installs the `gh` wrapper, adds the
forge to the shim's list of hosts and checks the setup. It prints repair commands for anything it
can't fix itself. Show them to the user and run them once the user agrees, except the login
command, which step 4 covers.

Done when `bootstrap` reports the wrapper installed and every repair command but the login one
has run.

## 4. Log the shim in

The user creates a token in Forgejo under **Settings → Applications**, with the four scopes in the
[README's token scopes table](../README.md#token-scopes). Point them there. You can't create it for
them.

- **With a secret tool:** ask for the token through it, then run
  `gfj auth login --hostname HOST --with-token < "$file"`, where `$file` is the path the tool gave.
- **Without one:** ask the user to run `gfj auth login HOST` themselves and paste the token at its
  prompt.

Done when `gfj auth status HOST` prints `HOST: logged in`.

## 5. Server mode

Skip this step unless step 1 found server mode is needed.

Run `gfj server check HOST`. It sends no token.

- **Every line reads `[ok]`:** the forge serves the gateway. Log gh in to the forge with the same
  token. With a secret tool, run `gh auth login --hostname HOST --with-token < "$file"`; without
  one, ask the user to run `gh auth login --hostname HOST` and choose **Paste an authentication
  token**. Done when `gh api --hostname HOST /meta` prints `installed_version`.
- **Any line reads `[fix]`:** the forge doesn't serve the gateway yet. Tell the user what the check
  printed, and that whoever runs the forge sets it up with
  [server-mode.md](gateway/server-mode.md); you'll check again once they have. On a hosted forge
  such as Codeberg, server mode isn't possible: tell the user that Claude Code and Claude desktop
  won't show pull requests, though Claude's agent can still use `gh` through the wrapper.

Done when the check passes and gh is logged in, or the user knows what the forge still needs.

## 6. Finish

1. Run `gfj doctor`. Done when it prints no `[fix]` line. `[warn]` lines can stay; tell the user
   what each says.
2. On macOS, if `bootstrap` said it changed the GUI PATH, ask the user to quit and reopen their GUI
   apps, such as ChatGPT desktop and Claude desktop. Running apps don't see the new PATH, and you
   can't restart them.
3. If the apps include Claude Code or Claude desktop, offer to add the two lines in
   [Tell Claude's agent to use `gh`](../README.md#tell-claudes-agent-to-use-gh) to the repository's
   `CLAUDE.md`.
4. Tell the user what you installed, which mode they're in, and anything still open.
