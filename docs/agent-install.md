# Agent-guided install

Steps for a coding agent, such as Claude Code or Codex, that a user has asked to install
gh-forgejo-shim. A person can follow them too. Each step ends with how to tell it's done; finish
one before starting the next.

## Rules for the whole install

- **Tokens stay out of the chat.** Ask for a token only through a secret tool, if you have one: a
  tool that asks the user for the value and hands it to you. Ask it to deliver the value as a file,
  pass that file to the command on stdin (`--with-token < "$file"`), and don't copy, print or keep
  it. Never put a token in a command's arguments. Without such a tool, the user runs the login
  command in their own terminal; a command you run for them has no terminal to type into.
- **Ask before each change**, and show the command first. Reading, such as `git remote -v`,
  `gh --version` or `gfj doctor`, needs no permission.
- **In a sandbox**, such as Codex's default one, installing needs the network and setting up writes
  outside the repository (`~/.config`, `~/.local/bin`, on macOS `~/Library/LaunchAgents` and the
  keychain). When a command fails for that reason, ask the user to approve running it outside the
  sandbox.
- **This guide changes nothing on the forge's server.** Server mode's steps are for whoever runs
  the forge; step 5 checks them.
- **Stop and hand over** when a command fails in a way this guide doesn't cover. Show the user the
  command, its output and the output of `gfj doctor`, and point them to
  [the issue tracker](https://github.com/dirtydishes/gh-forgejo-shim/issues/new).

## 1. Find out what the user needs

1. **The repository.** Work in the checkout the user wants to use with the forge. Run
   `git remote -v`: a remote on a host other than `github.com` is probably the forge. It may be
   GitHub Enterprise or another service instead, so confirm with the user. If the checkout has no
   such remote, ask the user to open the agent in one.
2. **The apps.** Ask which apps will work with the forge: Codex CLI, ChatGPT desktop, Claude Code,
   Claude desktop, or others.

**Server mode** is needed when the apps include Claude Code or Claude desktop: they look up a
branch's pull request on the forge directly, outside `gh`. Every other app needs only the
client-only wrapper. Install the wrapper in either case; Claude's agent uses it when it runs `gh`.

Done when you're in a checkout of the forge and know whether server mode is needed.

## 2. Install

1. Run `gh --version`. The shim needs gh 2.103.3 or newer. If gh is missing or older, install it
   as [installation.md](installation.md#requirements) shows: Homebrew where the user has it, gh's
   apt repository on Debian and Ubuntu, otherwise gh's
   [Linux install guide](https://github.com/cli/cli/blob/trunk/docs/install_linux.md) or a release
   from [gh's releases page](https://github.com/cli/cli/releases). Don't install Homebrew itself
   unless the user asks for it.
2. Install the shim. With Homebrew, run `brew install dirtydishes/tap/gh-forgejo-shim`. Without it,
   install the latest release tarball as
   [installation.md](installation.md#github-release-tarballs) shows. That puts `gfj` in
   `~/.local/bin`, which may not be on PATH yet: until step 3 fixes that, run it as
   `~/.local/bin/gfj`.
3. Tell the user that the shim sends one anonymous usage report a day, which counts the GitHub
   features their agents ask for that the shim lacks, and that `gfj telemetry off` stops it.
   [telemetry.md](telemetry.md) lists every field.

Done when `gfj --version`, or `~/.local/bin/gfj --version`, prints 0.6.0 or newer. An older shim
lacks the commands steps 4 and 5 use: upgrade it with `brew upgrade gh-forgejo-shim`, or install the
latest tarball over it.

## 3. Set up the wrapper

Run `gfj bootstrap` in the checkout. It installs the `gh` wrapper, adds the forge to the shim's list
of hosts and checks the setup, one line per check. It exits 1 on a fresh install, because nothing
is logged in yet; that's expected. Only its `[fix]` lines matter.

**HOST** from here on is the entry in bootstrap's `repository` line,
`[ok] repository: detected … and allowlisted HOST`. It can differ from the remote's host, such as
when the remote uses SSH. If that line doesn't end in `and allowlisted HOST`, stop and hand over:
gh resolves this checkout to another repository, such as a `github.com` remote or `GH_HOST`, and
the wrapper won't send it to the forge.

Bootstrap ends with a list of repair commands. Handle them this way:

- **Login commands** (`auth login`, `auth import`): skip them; step 4 logs in.
- **`export PATH="DIR:$PATH"`**: run in your shell, it lasts only for this session. Ask the user to
  add that line to their shell's startup file (`~/.zshrc` on macOS, usually `~/.bashrc` on Linux)
  and to open a new terminal, since one already open doesn't read it again. Run the line in your
  shell too. Skip the `bootstrap --bin-dir` command printed with it.
- **`git` commands**: they fix how this checkout tracks the forge. Run `git fetch origin` and
  `git remote set-head origin -a`; if that fails, show the user the
  `git remote set-head origin BRANCH` command bootstrap printed and ask which branch is the default.
  Run `git branch --set-upstream-to=…` only if that branch exists on the forge. Never run a command
  that names `git.example.com`: it's a placeholder.
- **`doctor`**: step 6 runs it.

If bootstrap says `Codex-style restricted PATH`, it ran with the agent's PATH rather than the
user's. Ask the user to run `gfj bootstrap` once in their own terminal, so the wrapper goes where
their shell looks, and to paste its output to you; it holds no secrets. Use that output from here
on, for its repair commands and for step 6.

Done when `gfj doctor` prints no `[fix]` line except the one for auth.

## 4. Log the shim in

The user creates a token in Forgejo under **Settings → Applications**, with the four scopes in the
[README's token scopes table](../README.md#token-scopes). Point them there. You can't create it for
them.

- **With a secret tool:** ask for the token through it, as a file, then run
  `gfj auth login --hostname HOST --with-token < "$file"`.
- **Without one:** ask the user to run `gfj auth login HOST` in their own terminal and paste the
  token at its prompt.

Done when `gfj auth status HOST` prints `HOST: logged in`.

## 5. Server mode

Skip this step unless step 1 found server mode is needed.

Run `gfj server check HOST`. It sends no token.

- **It exits 0** and ends with `The gateway answers on HOST`: the forge serves the gateway. Log gh
  in to the forge with the same token. With a secret tool, ask for the token through it again, as
  a file, and run `gh auth login --hostname HOST --with-token < "$file"`. Without one, ask the
  user to run `gh auth login --hostname HOST` in their own terminal and answer **HTTPS**, **No**
  to logging git in with these credentials, and **Paste an authentication token**. Done when
  `gh api --hostname HOST /meta` prints `installed_version`.
- **Its only line is `[fix] Forgejo:`**, or it fails before checking anything: it couldn't reach
  the forge or refused the address. Follow the sandbox rule if you're in one; otherwise show the
  user the output, and stop and hand over if they can't fix it.
- **Any other `[fix]` line:** the forge doesn't serve the gateway right yet. Ask the user whether
  they, or someone they can ask, run the forge and its reverse proxy.
  - If so, tell them what the check printed, and that whoever runs the forge sets the gateway up
    with [server-mode.md](gateway/server-mode.md); you'll check again once they have.
  - If not, as on a hosted forge such as Codeberg, server mode isn't possible. Tell the user that
    Claude Code and Claude desktop won't show pull requests, though Claude's agent can still use
    `gh` through the wrapper.

Done when the check passes and gh is logged in, or the user knows what the forge still needs.

## 6. Finish

1. Run `gfj doctor`. Done when it prints no `[fix]` line. `[warn]` lines can stay; tell the user
   what each says.
2. On macOS, look at bootstrap's `macOS GUI PATH` line:
   - `updated launchd PATH … restart existing GUI apps to inherit it`: ask the user to quit and
     reopen their GUI apps, such as ChatGPT desktop and Claude desktop. Running apps don't see the
     new PATH, and you can't restart them.
   - `could not apply launchd PATH immediately … after the next login`: ask the user to log out and
     back in; reopening the apps isn't enough.
3. If the apps include Claude Code or Claude desktop, offer to add the two lines in
   [Tell Claude's agent to use `gh`](../README.md#tell-claudes-agent-to-use-gh) to the repository's
   `CLAUDE.md`. Change it only if the user agrees.
4. Tell the user what you installed, which mode they're in, and anything still open.
