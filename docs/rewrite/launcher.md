# Launcher spec (L0)

Status: approved by Kell on 2026-11-02, including the section 13 decisions; in review. Bead `cwe.25`. Findings F1 and F23 in the
[plan review](plan-review-2026-11-01.md), decisions D1 and D2, and audit bugs R1 (`jko.15`),
R6 (`jko.16`) and R7 (`jko.7`).

The launcher is the part of the managed `gh` wrapper that runs real gh against a Forgejo host. It
starts the gateway in-process on a unix socket, runs real gh as a child pointed at that socket, and
exits with gh's status. This spec fixes how it does that before any launcher code is written. L3
(`cwe.28`) builds it, and its acceptance tests are listed in [section 11](#11-acceptance-tests).

Upstream citations are gh v2.103.3 (`gh/…`) and go-gh v2.17.0 (`go-gh/…`). Repo paths cite
`rewrite` at `68d0f24`.

## 1. Constraints from gh

These facts decide most of the design. Each one was checked in the source.

1. **The socket covers every host.** gh's own HTTP client passes no socket option to go-gh
   (`gh/api/http_client.go:66-71`), so go-gh reads `http_unix_socket` as a global config key
   (`go-gh/pkg/api/client_options.go:131-132`). gh core has a host-scoped lookup,
   `HTTPUnixSocket(hostname)` (`gh/internal/config/config.go:168-170`), but its only caller is
   telemetry, which passes an empty host. With the key set, the unix dialer replaces both the plain
   and the TLS dialer (`go-gh/pkg/api/http_client.go:238-249`). Every request that gh process makes
   goes down the socket as plaintext HTTP, with the original `Host` header, whatever its host. Keep-alives
   are off, so each request opens a new connection.
2. **The key can only come from a config file.** There is no environment variable for it. go-gh keeps
   `config.yml` and `hosts.yml` together in one directory, chosen by `GH_CONFIG_DIR`, then
   `XDG_CONFIG_HOME`, then `~/.config/gh` (`go-gh/pkg/config/config.go:251-256,273-285`).
   Extensions live in the data directory and update state in the state directory, so moving
   `GH_CONFIG_DIR` touches neither.
3. **Env tokens beat the config, and the enterprise one covers every non-GitHub host.**
   `GH_TOKEN` and `GITHUB_TOKEN` apply to github.com, `*.ghe.com` and `github.localhost`
   (`go-gh/pkg/auth/auth.go:26,68`). `GH_ENTERPRISE_TOKEN` and `GITHUB_ENTERPRISE_TOKEN` apply to
   every other host, plain `localhost` included. Both win over
   `hosts.<host>.oauth_token` in `hosts.yml` (`go-gh/pkg/auth/auth.go:63-100`). A host-level
   `oauth_token` resolves without a `user` key (`gh/internal/config/config.go:295-304`).
4. **gh hands its token to git.** For fetches and clones gh injects
   `credential.<host>.helper=!"<gh path>" auth git-credential` (`gh/git/client.go:148`). The gh path
   is `GH_PATH` if set, else `argv[0]` if it contains a slash, else a PATH lookup
   (`gh/internal/ghcmd/cmd.go:413-425`). Git then sends that token to the real forge over HTTPS,
   not through the socket. Extensions also get `GH_PATH` (`gh/pkg/cmd/extension/manager.go:134`).
5. **gh no longer migrates old configs.** v2.103.3 keeps the multi-account migration only as
   documentation, and production startup never runs it
   (`gh/internal/config/migration/multi_account.go:70-78`). The private `config.yml` still gets
   `version: "1"`, the value every current gh writes, so it looks to gh like any config gh made.
6. **The browser comes from `GH_BROWSER`, then the config `browser` key, then `BROWSER`**
   (`go-gh/pkg/browser/browser.go:82-93`). gh splits the value with shlex and appends the URL
   (`browser.go:64`).
7. **Telemetry turns itself off for enterprise users.** If any host in `hosts.yml` is not GitHub,
   or an enterprise token is set, gh disables telemetry (`gh/internal/ghcmd/cmd.go:481-494`).
   Update checks call api.github.com unless `GH_NO_UPDATE_NOTIFIER` and
   `GH_NO_EXTENSION_UPDATE_NOTIFIER` are set (`gh/internal/update/update.go:42,83`).
8. **gh's exit codes:** 0 ok, 1 error, 2 cancelled, 4 authentication required, 8 pending checks
   (`gh/internal/ghcmd/cmd.go:44-48`).

## 2. Process model

One wrapper invocation is one process. There is no daemon and nothing shared between invocations.

```text
client ── exec ──> ~/.local/bin/gh  (sh wrapper, shim.rs)
                     └─ exec ──> gh-forgejo-shim gh ARGS        pid P
                                   revert launcher env (§7)
                                   route(argv, env, cwd)        routing.rs, L2
                                   ├─ delegate ──> exec real gh ARGS          P becomes gh
                                   └─ launch
                                        create run dir, write config (§3), bind socket (§4)
                                        spawn real gh ARGS as child C, same process group
                                        current-thread tokio runtime on P's main thread:
                                          serve the gateway on the socket
                                          wait for C
                                          forward SIGTERM, SIGHUP, and SIGINT/SIGQUIT
                                            sent without a terminal, to C (§6)
                                        C exits: stop serving, remove run dir, write trace
                                        exit with C's status, or die by C's signal (§6)
```

**Delegate means exec.** When the route is delegate and tracing is off, the wrapper replaces itself
with real gh (`CommandExt::exec`). gh then owns the pid, the signals and the exit status, and the
launcher has nothing to translate. When tracing is on, the wrapper spawns and waits so that it can
write the trace record afterwards. That spawn path passes status through per section 6, as R6 does
for today's delegation.

**Launch is chosen per invocation.** The wrapper launches when routing (L2) resolves a Forgejo host
in client mode and the command isn't on the delegate list below. Everything else delegates.
Unknown commands launch, because gh aliases arrive as unknown commands and expand only inside gh.
An alias such as `gh co 12` in a Forgejo checkout has to reach a gh that can talk to the forge.

**Transition: the launch list.** Until the gateway serves every command, the rule above applies
only to the commands on one list in code, `launcher::LAUNCH_LIST`, matched on the command after
alias expansion (L2). Every other Forgejo route keeps the shim's built-in path, as before the
launcher. So do usage errors and a listed command's forms the built-in path refuses as
unsupported, such as `pr checks --watch`: their answers need no token, and they stay as they are
until a test shows the gateway serves them. L3 starts the list with `pr view` and `pr checks`, then adds `pr list`, `api`, `auth token` and `auth status`. C3a adds `pr create`, `pr comment` and `pr edit`, each launching only with the flags a trace showed the gateway serves (`launcher::LAUNCH_FLAGS`); any other flag keeps the built-in path, which serves or refuses that form before writing. For these commands the flag list replaces the built-in path's own test for the forms it serves. C3b adds `pr merge` (`--merge`, `--squash`, `--rebase`, `--auto`, `--match-head-commit`), `pr ready` (with `--undo`), `pr close` and `pr reopen`, the same way. `--delete-branch` joins `pr merge` and `pr close` once the gateway deletes a branch as gh asks: gh 2.103.3 sends the whole ref as one escaped segment, `git/refs/heads%2F<branch>`. `pr merge` takes a 404 or 422 as already deleted; `pr close` reports either as a failure. A merge with `--auto` keeps the built-in path, which refuses it: gh skips the delete when it schedules the merge. On a pull request merged before the call, gh sends no delete and still says, in a terminal, that it deleted the branch, as on GitHub. `pr close` deletes in the base repository unless the gateway reports the pull request as cross-repository, so one whose fork was deleted must read as cross-repository (`cwe.16.2`). C3c adds `pr checkout`, with `--branch`, `--detach` and `--force`, all git-side (`cwe.17`). The e2e environment runs it end to end for a same-repository pull request, with each of those flags and `-R`: a launched `gh pr checkout` fetches from the forge over TLS with only the launcher's credential helper, even with other helpers configured. That is test 14's first clause, a default-port fetch; its other clauses run as process tests or wait for `cwe.43`. C1 adds `api rate_limit` (`-X GET`, `--jq`, `--include`, `--hostname`), and `api graphql` launches
for ChatGPT's PR card and pane: a query (`-f query=…`, variables as `-f` or `-F`, `--hostname`, a
POST), never a document naming `mutation` or `subscription` or read from a file the shim can't see. C4 adds `run list` (`--json`, `--jq`, `--limit`, `--status`, `--branch`, `--event`, `--commit`, `--all`), `run view` with a run ID and `--json` (with `--jq`; without an ID gh prompts in a terminal, and its text view lists the run's jobs) and `issue comment` with a body (`--body`, `--body-file`; without one gh asks for an editor or a browser, and `pr comment` gets the same rule), as traced (`cwe.18.1`). A `--json` naming `jobs` or `attempt` keeps the built-in path: the gateway can't list a run's jobs, and its attempt reads 0 where GitHub's first is 1. `run view --verbose` shows jobs and stays off; `--exit-status` matters only for a completed run, which no trace has covered. `run list --workflow` waits on the gateway, which can't take a workflow's file name yet; `--user`, `--created`, `run view --attempt`, `--log` and `issue comment --edit-last` or `--delete-last` stay off. C4 also adds `issue list` (`--state`, `--limit`, `--json`, `--jq`, `--author`, `--assignee`, `--label`, and `--mention` without `--label`), `issue view` with `--json` (and `--jq`) and `issue create` with a title and a body (`-t` with `-b` or `-F`; otherwise gh prompts in a terminal), now that the gateway serves `createIssue` and most of Issue's fields. It leaves `blockedBy`, `blocking` and `reactionGroups` empty though Forgejo has dependencies and reactions, so a `--json` naming them keeps the built-in path, and so does `issue view`'s text, which lists the dependencies. `issue list --search` stays off: the gateway reads GitHub's query syntax its own way (`label:a,b` as one label). `issue list --milestone`, `--type` and `--app` fail on the gateway; `issue create`'s `--type`, `--parent`, `--blocked-by` and `--blocking` fail after the issue is made (`r0t`), and its labels, assignees, milestone, projects, templates and `--web` stay off too. Each later gateway command adds its name to the list, with a test, in the change that
makes the gateway serve it. Once the list covers everything, C5 (`cwe.19`) deletes the built-in
path and the list with it, and this rule becomes the one above.

**Installed extensions delegate.** A first argument *name* with an installed extension, a
directory `<data dir>/extensions/gh-<name>`, runs through plain gh. The data directory is
`$XDG_DATA_HOME/gh`, else `~/.local/share/gh` (`go-gh/pkg/config/config.go` `DataDir`; gh's
`pkg/cmd/extension/manager.go:594-595`). Inside a launched gh, every host but *S* has only the
placeholder token (§3). An extension that talks to github.com would therefore fail there, where it
works through plain gh. Plain gh can't reach the forge, so an extension that works on the Forgejo
repo doesn't work in either mode. A gh alias with the same name as an extension wins inside gh,
but the wrapper can't tell the two apart without parsing the alias list. It treats the name as an
extension. Kell accepted this (§13, decision 2).

The delegate list holds commands that call no API, or that write gh's own state, which a launched
gh would write to a throwaway copy (section 3):

- `--version`, `version`, `completion`;
- help: a help flag as gh parses it, for a command group or a command in the shim's grammar (a
  command outside both keeps the Forgejo path's refusal; see below). Not the `help` command: an
  alias can take that name, since cobra adds `help` after gh registers aliases, and gh reads alias
  files the shim's reader rejects, so `gh help …` routes like any other call. That means `--help`, or `-h`
  in a command that doesn't define its own `-h`, as a flag and not as another flag's value, before
  a `--`. Several `auth` commands define `-h` as `--hostname`, so `gh auth token -h forge.test`
  isn't help. The wrapper decides this with the invocation parser's flag grammar, not by
  scanning argv. Help needs no token, so `gh pr view --help` prints help even where the forge
  has no stored token;
- `config …`, `alias …`;
- `auth login`, `auth logout`, `auth switch`, `auth refresh`, `auth setup-git`;
- `extension …`, because install, upgrade, browse and search call github.com;
- `codespace …`, `copilot …`, `skill …` and `agent-task …` (with gh's aliases `cs`, `skills`,
  `agent-tasks`, `agent` and `agents`), which serve only github.com and,
  for `codespace ssh --config`, write `GH_PATH` into a lasting file;
- `auth git-credential`, except from the launcher's own helper (§3, "Git's credentials"). Inside a
  launched call (`FJ_SHIM_CREDENTIALS` set, even empty), a call the helper's exact match doesn't
  take runs only when cobra's command word is one of gh's own commands other than `auth`. Any
  other `auth` form (`auth git-credential -- get`), an alias or an extension is refused rather
  than delegated: plain gh would answer from the user's own logins. The wrapper doesn't try to
  read the user's aliases here, since gh accepts some its YAML reader can't (an unquoted `true:`
  name, a repeated one).

The list applies to the command gh will run: routing expands a plain alias first (L2), so
`gh sg` with alias `sg: auth setup-git` delegates too. The list applies only when the command word comes
first: a flag before it may take a value the parser doesn't know, as in `gh --notes config
release create v1`, where gh runs `release create`; such a call keeps the Forgejo path. Help delegates after routing has chosen a
forge, so the route trace still names it, with a reason saying plain gh ran; plain gh then prints
the help. Only a command in the shim's grammar knows its own `-h` and which flags take a value, so
a help flag delegates only there, and for command groups, which have no `-h` of their own:
`repo edit -h URL` sets a homepage, and keeps the refusal.

**Near a forge, only the delegate list runs plain gh.** Which host gh sends a call to depends on
how it parses the call and on the command. The shim can't match gh on either everywhere. A word
it takes for `-R` may be another flag's value to gh (`gh release create v1 --notes -R
github.com/o/r` creates the release where the checkout points), and `gh gist create` ignores
`GH_REPO` and posts to `GH_HOST`. So a call off the delegate list that routing would delegate
runs plain gh only when no Forgejo host is in reach: none in its words, `GH_HOST`, `GH_REPO`,
any of the checkout's remotes, whichever one the command would pick, or gh's default host when it
comes from the user's `hosts.yml` (the only host there, with `GH_HOST` unset: go-gh's
`defaultHost`), or a `hosts.yml` the shim can't read. Otherwise the wrapper refuses it with exit
1 and says to run it where no Forgejo host is in reach. Separately, if the user's `config.yml` has
aliases the shim can't read (a repeated name, or YAML it rejects), any command word that could be
an alias, `help` included, is refused with a pointer to `config.yml`, near a forge or not. The cost is every
non-Forgejo call made from near a forge: `gh pr view 1 -R github.com/o/r` with a Forgejo
`GH_HOST`, `gh variable list` in a checkout with both a Forgejo and a GitHub remote, `gh api
https://api.github.com/…` in a Forgejo checkout. Grammar coverage doesn't change this. gh's root
(`gh`, `gh --version`) still delegates, but only when cobra finds no command word either.

**Commands with no repository.** `gh api` without `--hostname`, and `gh auth token` without
`--hostname`, don't use the checkout's repository to pick a host. They use gh's default host
(`gh/pkg/cmd/api/api.go:417-420`, `gh/pkg/cmd/auth/token/token.go:71-73`). A launched gh's
`hosts.yml` holds several keys whenever the user has a host of their own, so its default host is
then github.com. Routing these by the checkout's remote, as today, would make the launched gh
print the placeholder for `gh auth token`, and send `gh api` to github.com, where the gateway
refuses it with 421. So when routing sends a host-less `gh api` or `gh auth token` to a Forgejo
host, the launcher sets `GH_HOST` to that host's hostname without its port (`gh_hostname`), which
makes it gh's default host. Both commands then talk to the forge, as the shim's versions of them
always have. Kell chose this (§13, decision 4). The alternative, following gh to github.com, was
declined. The value has no port because gh drops the port from remotes before comparing them
with `GH_HOST`: with `forge.test:3443`, gh refuses with "none of the git remotes … correspond to
the GH_HOST environment variable". The portless value also keeps `gh auth token`'s lookup on the
same key as gh's API requests. The launched `hosts.yml` has that key, and the gateway answers to
the portless spelling (§5). When the user's own `GH_HOST` already has that value, it stays as it
is. `GH_HOST` is restored for nested calls like the other launcher variables (§7).
`auth status` keeps its current routing. `auth git-credential` under a launcher is answered by
the wrapper itself (§3, "Git's credentials"); otherwise it keeps its current routing. L3 owns the
exact table and
proves R4 (`jko.5`: `gh --version` in a Forgejo checkout delegates).

**A forge remote that SSH sends elsewhere keeps its own name** (#334). gh reads an SSH remote's
host through `ssh -G`, so when the user's SSH config gives the forge's name another `HostName`,
such as a VPN address for a forge whose proxy doesn't carry SSH, gh sees that address, knows no
such host, and drops the remote. An `insteadOf` rule that rewrites the forge's HTTPS URLs to SSH
does the same to an HTTPS remote. Routing instead reads such a remote under its own host when
that host is on the allowlist and gh knows none by the name `ssh -G` gives; a remote that `ssh
-G` sends to a host gh knows, such as github.com, stays there, as it does for gh. It then picks
among the remotes as gh would. When the remote it picks is one gh drops, the launched or
stepped-aside gh gets the repository as `GH_REPO`, `HOST/OWNER/REPO` with the portless hostname
that gh reads from a remote, as if the call had said `-R`; without it, gh would find no
repository. A host-less `gh api` gets it only when it uses `{owner}` or `{repo}` (or `:owner`,
`:repo`), since gh refuses `{branch}` whenever `GH_REPO` is set. The route reason names the host
`ssh -G` gave. `GH_REPO` supplies only the base repository: gh still looks up the current branch's
remote by name among the remotes it kept, so a command that finds a PR from the branch (`pr view`,
`pr checks`, `pr status` with no selector, on a branch that tracks the remote) fails with gh's
"could not resolve remote". gh also reads `GH_REPO` as a repository the user chose: `gh browse
PATH` takes the path from the repository root, `gh repo fork --remote` forks before it fails to
find the remote, `gh secret` skips its question about several remotes, and `gh pr create` skips
local templates. A user's own `GH_REPO`, `-R` or URL
argument decides first, as for gh, so the wrapper never sets `GH_REPO` then. Such a remote also
counts as a forge in reach (above), and as a forge remote for the push check (below), its host
read as gh reads it, a leading `www.` dropped: git pushes to the address SSH gives, so `gh pr
create` is refused unless the remote's push URL stays on the forge. Plain gh without the wrapper can't be
helped; `gfj doctor` warns, and the README's Known gaps gives a proxy-command setup that keeps the
forge's name in `ssh -G`. `gfj bootstrap` never allowlists a host on these grounds alone: the
remote's own host may just as well be an SSH alias for the forge's real name.

**Server mode never launches.** In server mode (D1, `cwe.42`) the forge host serves `/api/v3` and
`/api/graphql` itself, so plain gh works against it with a normal `gh auth login`. A host configured
for server mode routes as delegate. The config key belongs to `cwe.42` and the config bridge (L4).

**Nor does a forge the user's gh is logged in to.** gh saves a login only after the host answers
its API: `gh auth login` checks the token against the host before writing anything
(`gh/pkg/cmd/auth/login/login.go:239`, `gh/pkg/cmd/auth/shared/login_flow.go:237`). A Forgejo
host without server mode has no `/api/v3`, so that check fails there. A login in the user's own
`hosts.yml`, an entry for the forge with an `oauth_token` or a `user`, therefore means the forge
serves gh itself. The wrapper reads the entry gh's API client would use: the key equal to the
forge's hostname without its port, spelled exactly, because gh drops the port before looking up a
token (`gh/api/http_client.go:284-290`). A login stored only under `host:port` doesn't count,
since plain gh's API requests would never find it. When routing picks such a forge, the
wrapper steps aside and runs plain gh as on the delegate path, for every form, the built-in path's
included: those would otherwise answer with the shim's own token, or refuse a form plain gh serves
there, such as ChatGPT's `gh api graphql --hostname <forge>`. A form that doesn't launch steps
aside only when nothing in the call names another host: `GH_HOST` naming one, or a word or
`GH_REPO` naming another allowlisted forge, or holding a URL, an SSH address (`host:o/r`, `git@host:o/r`) or a
`HOST/OWNER/REPO` whose host isn't the forge, or a `--hostname` value that isn't (`gh attestation
download --hostname github.com`). Any `x:` counts as an SSH host, whatever its
characters. Routing doesn't model every argument gh takes a host from (`gh repo clone
forge-b/o/r` in forge-a's checkout routes to forge-a), so such a call keeps the built-in path.
The token rule below already keeps every token variable from leaving; this check also keeps
the call where routing and the trace say it goes.
`gh api` and `gh auth` skip the word check: gh sends them where routing says, and their other
words are data, such as a GraphQL document. That is no run directory, no gateway, and
the user's environment unchanged apart from `FJ_SHIM_DEPTH` (§8) and two cases. Every form
gets `GH_HOST` set to the forge's portless hostname, the default host gh then uses: for a host-less
`gh api` or `gh auth token` that is decision 4, as on the launch path (above), and for the rest it
covers what routing doesn't model. Without it, plain gh would send to its default host, github.com
whenever the user is also logged in there, every host-less `OWNER/REPO` (`gh repo delete o/r`,
`gh repo clone o/r`) and every command that only uses the default host (`gh search`, `gh gist`,
`gh status`), while routing and the trace named the forge; `gh auth token` would print the GitHub
token. gh still sends a call that names a host (`-R host/o/r`, a URL, `--hostname`) there, and
with `GH_HOST` set it uses only the checkout's remotes on the forge, the ones routing chose from.
Commands that only make sense on github.com reach the forge too: `gh search` searches the forge,
and `gh gist`, which Forgejo doesn't have, fails with the forge's answer instead of silently
reaching github.com. A user `GH_HOST` naming the forge stays; one naming another host keeps a form
that doesn't launch on the built-in path (above), and gives way to the forge on the launch path,
where routing already chose it. `GH_HOST` doesn't steer git: `gh pr create` pushes the branch's
remote and `gh repo create --push` the remote it adds, and a remote's push URL or
`url.<base>.pushInsteadOf` can send that push to another host, as can an SSH host alias. So a
forge-routed `pr create` (or `pr new`), launched or not, is refused before gh runs when a forge
remote's push URL in the checkout names another host (a forge remote is one git fetches from the
forge, or one whose URL names the forge though `ssh -G` resolves it elsewhere), an `insteadOf` or `pushInsteadOf` rule whose
prefix overlaps one of the forge's URL forms rewrites to another host, or, unless gh uses
`https` there, SSH resolves `git@<forge>`, the remote gh adds for a fork, to another host. gh's
protocol is read from its files, without running gh: the forge's portless entry, then the
top-level `git_protocol`, then `https`, both as the launched gh reads them (`hosts.yml`) and as
plain gh does (a legacy `hosts` key in `config.yml` first); both must say `https`. When ssh
can't resolve an SSH host and git connects through its own SSH command (`GIT_SSH_COMMAND`,
`GIT_SSH` or `core.sshCommand`), the push is refused; with neither, git can't push over SSH.
A `repo create` (or `repo new`) that pushes, with
`--push` or when it prompts (no argument and no flag; a lone `--` counts as none), is refused
outright: it pushes from a `--source` checkout the wrapper would have to read as gh does. Without
`--push` it creates the repository, and git pushes after. The command is found as cobra finds
it: a flag without `=` takes the next word as its value unless it's `--help`, `-h` or `--version`,
so `gh --source x repo create` and `gh repo --source view create` are `repo create`, and
`gh pr view new` is `pr view`. An SSH push URL resolves with its own user (`ssh -G git@forge`),
which a `Match user` rule can change. The trace names the portless
hostname gh reaches. For a forge entry with a port, that holds only when the call doesn't spell
the port: gh keeps it for a full name (`-R forge.test:3443/o/r`, `GH_REPO`, `gh api`'s URL or
`--hostname`) and drops it for a URL or a remote, so a call or `GH_REPO` naming `forge.test:3443`
doesn't step aside. Here the portless value also matters
for the token: a user's own `hosts.yml` has the login under the portless key only, and
`GH_HOST=forge.test:3443` gives "no oauth token found for forge.test:3443". For a forge on a
custom port, stepping aside therefore sends gh to the forge's default port 443, where the user's
login was checked, while the launch path reaches the entry's own port through the gateway.
gh also uses a token
variable, if set, for the forge ahead of the user's login there (fact 3): `GH_ENTERPRISE_TOKEN`
or `GITHUB_ENTERPRISE_TOKEN` for most forges, and `GH_TOKEN` or `GITHUB_TOKEN` for a forge named
`github.localhost` or `*.ghe.com`. Either would send another site's login to the forge, the
enterprise ones whatever `GH_HOST` says. So stepping aside removes those of the four token
variables that are set, as a launch does, even when the user's own `GH_HOST` already names
the forge, where it sets nothing. Stepping aside needs the
user's stored login for the forge, so gh never needs the variables there, and without them a call
that reaches another host carries only the user's own login for it, as plain gh would.
It records the `GH_HOST` change with §7's
`FJ_SHIM_SAVED`, `FJ_SHIM_ORIG_<NAME>` and `FJ_SHIM_SET_<NAME>`, so a nested call, such as one
from `gh api`'s pager, gets the user's value back. It doesn't record the token variables: a
nested call, such as git's credential helper running `gh auth git-credential` for a remote on
another host, would otherwise get them back and hand one to that host. Plain gh would do the
same (fact 3), so here the wrapper is stricter than plain gh: a nested gh that relied on
`GH_TOKEN` for GitHub uses the user's stored GitHub login instead, and fails without one. It never sets `FJ_SHIM_LAUNCHER`: the wrapper
execs gh here, so gh has the wrapper's pid, and guard 2 would stop a wrapper that gh runs directly
(§8). That holds for every route to the host,
the "user's gh hosts" route included (§3), and such a host is never in *S*. Until `cwe.42` adds
the server-mode key, this is how the shim recognizes server mode.

## 3. Where gh's config lives

**Decision: a private config directory per invocation, built from the user's config, which the
launcher reads and never writes.**

The launcher sets `GH_CONFIG_DIR` to its run directory (section 4), which holds:

- **`config.yml`:** the user's `config.yml` from their effective config directory (the same
  precedence go-gh uses, computed from the reverted environment of section 7), parsed as YAML,
  with `http_unix_socket` set to the socket path and `version` set to `"1"` if absent. Aliases,
  `git_protocol`, `editor`, `pager`, `prompt` and `browser` carry over. With no user
  `config.yml`, it holds only those two keys. If the user's file doesn't parse, the launcher stops
  with an error naming it (section 6); plain gh would fail on it too.
- **`hosts.yml`:** one entry for every host in the user's own `hosts.yml`, keyed exactly as the
  user's file spells it, and one for every Forgejo host on the shim's allowlist, by gh's portless
  hostname, unless that exact key is already there. gh counts keys as written, so `Forge.Test`
  and `forge.test` are two, and routing counts them the same way (L2). Each entry has an
  `oauth_token`, plus the `git_protocol` from the user's entry for that host, if any. A host-level
  `oauth_token` needs no `user` key for gh to find it. `gh auth status` reads the account name from
  `user`, though, and fetches it only for tokens from the environment
  (`gh/pkg/cmd/auth/status/status.go:314-320,484-490`), and `gh auth token --user <login>` finds
  a token only under `users.<login>.oauth_token` (`gh/internal/config/config.go:693-709`). For an
  `auth status` call, or an `auth token` call with a `--user`, the launcher therefore looks up
  each served host's login before it starts gh. It uses the gateway's own `authenticate`, which
  warms the session cache, so gh's own requests repeat nothing. It writes that login as `user`,
  and the entry's token again under `users.<login>.oauth_token`, as gh's own plain-text login
  writes it. gh then prints the token for that login and refuses any other, as it would. One
  exception is gh's own: a named-user lookup reads the system keyring (`gh:<host>`, keyed by login)
  before any config file (`gh/internal/config/config.go:693-705`), whatever `GH_CONFIG_DIR` says. A
  keyring login for that host, left by another gh config, therefore wins, for that login or
  another. It is the user's own credential for the same host, which plain gh would print too. For a
  plain keyring token, `auth token` sends nothing anywhere; a refreshable credential near expiry
  (`gh:_refreshable:<host>`) makes gh refresh it with that host's OAuth endpoint first
  (`gh/pkg/cmd/auth/token/token.go:198`), as plain gh would. The launcher can't hide it without
  answering `auth token --user` itself. If the lookup fails, the entry has neither, and gh reports
  the failure itself. Other commands never show the account, so they get no `user` and no lookup.
  - A served host (the set *S*, below) gets a per-call token: 32 random bytes from the OS RNG as
    hex, made for this call. The gateway accepts it for that host and authenticates upstream
    with the real token (§5). Outside the gateway it opens nothing, so no gh credential helper,
    gh's own or one a nested git reaches, can hand a usable login to anyone.
  - In a `gh auth token` call, a served host's keys get its real token, which that command
    prints for clients that call the forge themselves. That call makes no API request, runs no
    git, and starts nothing.
  - In a `gh auth status` call with `-t` or `--show-token`, a served host's keys also get its
    real token, because gh prints the token from `hosts.yml`
    (`gh/pkg/cmd/auth/status/status.go:258,363`). A per-call token there would print a value
    that dies with the call. That command does call the API, so for that call the gateway
    expects the real token in place of a per-call one (§5). It runs no git and starts nothing
    (`status.go` imports nothing that spawns a process). The wrapper decides "with
    `--show-token`" on the argv after alias expansion (L2's `expand_alias`), with the invocation
    parser's flag grammar, which declares the flag as gh's pflag does
    (`BoolVarP(…, "show-token", "t", …)`, `status.go:258`): `Invocation::flag("show-token")`,
    the last occurrence, read with Go's `strconv.ParseBool` rules. So `-t`, `-at`, `-ta`,
    `--show-token`, `--show-token=1` and `--show-token=t` count, and `--show-token=false`,
    `-t=false`, `=0` and `-t --show-token=false` don't. A plain scan for `-t` would miss `-at`
    and print a token that dies with the call. No other
    call puts the real token in `hosts.yml`.
  - Every other host gets the placeholder `gfj-not-served`.
  - An allowlist entry with a port, such as `forge.test:3443`, always gets two entries, its
    hostname and `host:port`, with the same token. gh's API client drops the port before the
    lookup (`gh/api/http_client.go:284-290`), and `gh auth token --hostname forge.test:3443` and
    `gh auth status --hostname forge.test:3443` look up `host:port`. Routing counts both keys
    (L2, `GhHosts::load`), so gh's default host still matches the wrapper's model.
  - Two allowlist entries that share a hostname, such as `forge.test:3443` and
    `forge.test:8443`, can't be served at all. gh's API client looks up the portless key for any
    spelling, even `https://forge.test:3443/…`, so a launched gh could present neither entry's
    token. Routing leaves every target on such a hostname unrouted, port or not (L2, `34023a8`),
    and those entries are never in *S*. Their keys get the placeholder. A user who needs both
    forges serves one of them in server mode (§2), where plain gh logs in to each normally.

The placeholders matter because of how gh finds a token. With no token in `hosts.yml` for a
host, gh falls back to the Keychain's unkeyed slot for it (`gh:<host>`, user `""`;
`gh/internal/config/config.go:320-324,382-384`). Every normal secure-storage login fills that
slot (`config.go:651-652`), and moving `GH_CONFIG_DIR` doesn't move the Keychain. Without a
placeholder, a github.com request from a launched gh would carry the user's real GitHub token
down the socket. A non-empty token in `hosts.yml` wins over the Keychain
(`config.go:301-304`), so the placeholder blocks that fallback.

The placeholders cover every host the user's `hosts.yml` lists, and nothing else. The Keychain is
shared by every gh config directory, though. Say the user once logged in to github.com under
another `GH_CONFIG_DIR`, and this one doesn't list github.com. A launched gh can then find that
Keychain token two ways:
- **Through the API**, when it's asked for github.com by an explicit URL, such as an alias
  expanding to `api https://api.github.com/user`. The token goes down the socket and reaches
  only the local gateway, which refuses it with 421 and forwards nothing (§5).
- **Not through git.** gh's own credential helper would find it, and would also retry a
  `gist.<host>` request as `<host>` (`gh/pkg/cmd/auth/gitcredential/helper.go:123-126`). The
  launcher replaces that helper, and the user's own, with its own for every git in the call. It
  knows only the served hosts ("Git's credentials", below).

So a token goes only to the local gateway, which refuses it. That is the guarantee: the
launcher never sends a login to a site it doesn't belong to. Kell accepted it (§13,
decision 2).

### Git's credentials

gh installs its credential helper for every host on its fetches and pushes, as
`!%q auth git-credential` built from `GH_PATH` (`gh/git/client.go:148-155,905-913`). That helper
reads `hosts.yml` and the Keychain, and retries `gist.<host>` as `<host>`. In a launched call,
`hosts.yml` holds only per-call tokens and placeholders (above). The Keychain fallback and the
`gist.` retry still let that helper give a login to a site it doesn't belong to, and a git that an
alias shell or an editor starts reaches it through the user's own git config, under the launched
call's `GH_CONFIG_DIR`. Git that gh starts also needs the real token for the forge. So the
launcher answers every git in the call itself:
- **The script.** The run directory holds `gh`, mode 0700:
  `#!/bin/sh` and `FJ_SHIM_CREDENTIALS='<run dir>/credentials' exec '<shim>' gh "$@"`, with the
  shim path single-quoted. `GH_PATH` names it, so gh's own helper invocations run it.
- **Every git in the call uses it.** The launcher appends two entries to `GIT_CONFIG_PARAMETERS`,
  after any value it inherited (§7): `'credential.helper='` and
  `'credential.helper=!<run dir>/gh auth git-credential'`. It uses git's older `'key=value'`
  quoting, which every git version parses; git before 2.31 rejects the whole variable in the
  `'key'='value'` form. Like git's own code, it puts a space before the new entries only when
  the inherited value is non-empty. git rejects a value with leading whitespace ("bogus format
  in GIT_CONFIG_PARAMETERS"), so an unset or empty inherited value is replaced, not appended to.
  The run directory's path is shell-safe (§4) and has no `'`, so the helper needs no quoting
  beyond the entry's own. git reads its config files, then `GIT_CONFIG_COUNT`, then
  `GIT_CONFIG_PARAMETERS` in order, and a `-c` on its own command line last. git gathers every
  helper that matches a URL, scoped `credential.<url>.helper` entries included, into one list in
  that order, and an empty value clears the list. So the launcher's helper is the only one any
  git in the call asks, unless that git has a `-c` of its own (checked against git 2.56). gh's
  own `-c` pair names the same script.
- **The answer.** Before anything else, ahead of guard 2 and the §7 revert, the wrapper checks
  for `FJ_SHIM_CREDENTIALS` and an argv of `auth git-credential <op>`. It then answers from that
  file and exits, never routing or launching:
  - It reads `key=value` lines until a blank line or EOF, as gh's helper does
    (`gh/pkg/cmd/auth/gitcredential/helper.go:77-102`), and ignores keys it doesn't use, such as
    `capability[]`, `wwwauth[]`, `state[]` and `path`. It advertises no capabilities.
  - `get` answers only for `protocol=https` with a `host` exactly matching a served host as git
    reaches it: the hostname for a default-port entry, `host:port` for a ported one. The answer
    is `protocol`, `host`, `username=x-access-token` and `password=<real token>`. Any other
    host, a `gist.` one included, gets no answer. git then asks its next source: an askpass
    program (`GIT_ASKPASS`, `core.askPass` or `SSH_ASKPASS`; VS Code's terminal sets one), else a
    terminal prompt. Either asks about the URL's own host only, so neither can carry a login to
    another site. `GIT_TERMINAL_PROMPT=0` stops the prompt but not askpass, so tests also unset
    all three askpass settings.
  - `store` and `erase` do nothing, as gh's helper does. Any other operation fails, as gh's does.

Any other `auth git-credential` call, from a git outside a launched call, delegates to plain gh
(§2), which answers from the user's own config. It never launches.

Two cases stay outside the launcher's reach. A nested git with a helper on its own command line
asks that helper as it would without the shim: after the launcher's declines for
`git -c credential.helper=X push`, and instead of it for `git -c credential.helper=
-c credential.helper=X push`, the form gh itself uses. If that helper is gh's, it runs plain gh with the user's own config (§7),
or real gh under the call's `GH_CONFIG_DIR`, where it finds per-call tokens and placeholders.
A git still running after the call ends finds no script, and git goes on without a credential.

Only a few gh paths write `GH_PATH` somewhere that outlasts the call: `auth setup-git`,
`auth login` and `auth refresh` write it into `~/.gitconfig`
(`gh/pkg/cmd/auth/shared/gitcredentials/helper_config.go:41-55`), and `codespace ssh --config`
writes it into the ssh config (`gh/pkg/cmd/codespace/ssh.go:653`). Every one of them delegates
(§2), aliases included, because routing decides on the expanded alias (L2). Installed extensions
delegate too, so their `GH_PATH` is plain gh's.

The key set, the user's hosts plus the allowlist hosts, is also what routing (L2,
`repo::GhHosts::load`) assumes gh knows. A launched gh therefore resolves the same default host
(`go-gh/pkg/auth/auth.go:146-156`) and the same remote as the wrapper predicted.

One configuration doesn't fit. go-gh's default host depends on how many keys `hosts.yml` holds.
Plain gh counts only the user's keys. So when the user's own `hosts.yml` lists a Forgejo host
the shim serves as its only host, plain gh defaults to that host, while a launched gh, with the
allowlist hosts added, may not. Before delegating, routing checks plain gh's own choice
(`GhHosts::user`). When that choice is a served Forgejo host, routing marks the route "user's gh
hosts" (L2). If the user's gh is logged in to that host, the wrapper steps aside (§2).
Otherwise the entry gives gh no login it can use: it holds only settings, such as the
`git_protocol` that `gh config set -h <host> git_protocol ssh` writes, or a login stored under
`host:port`, which gh's API client never reads. Plain gh would hit the forge's missing
`/api/v3`, and every launch with a key set that keeps plain gh's default host has broken token
lookup or the default host itself. So the launcher doesn't run gh. It exits 1 with:
`gh-forgejo-shim: <config dir>/hosts.yml makes <key> gh's default host, but gh has no login it can use there; delete the <key> entry from that file to use gh-forgejo-shim there`.
`<config dir>` is the user's effective gh config directory, as §3 finds it, and `<key>` is the
entry's key as that file spells it, such as `Forge.Test`. `gh auth logout` can't help, because
it removes only logins and fails on a host without one
(`gh/pkg/cmd/auth/logout/logout.go:111-129`). The user's config is left alone (§3).

*S* is the routed host. For a host-less `gh auth status` it is every allowlist host that has a
token, doesn't share its hostname with another entry, isn't one the user's gh is logged in to
(§2), and has settings the built-in client accepts (§5), so the status reports all of them.

What this guarantees:

- **The user's gh config is byte-identical after a run, a crash and an uninstall**, because the
  launcher only ever opens it to read. There is nothing to restore after a crash.
  `docs/rollback.md` stays true: removing the wrapper restores plain gh.
- **Concurrent invocations can't collide**, because each one has its own directory and socket.
- **Aliases and settings work inside a Forgejo checkout.** Installed extensions run through plain
  gh (§2), so the ones that talk to GitHub keep working there.
- **The launcher never sends a login to a site it doesn't belong to.** Every host in the user's
  own `hosts.yml` other than *S* has the placeholder, which no server accepts, and the gateway
  refuses any request for another host with 421, without contacting any upstream (L1). A
  Keychain login the placeholders don't cover reaches only the gateway, which refuses it. No git
  in the call can use it, short of a helper named on that git's own command line (§3, "Git's
  credentials").

The cost:

- A write that gh makes inside a launched process lands in the copy and is lost when the run ends.
  The delegate list in section 2 keeps the commands that write config out of the launcher. An
  alias that expands to `config set` inside a Forgejo checkout loses its write. This is rare, and
  L3's docs name it.
- A host-less `gh auth status` inside a Forgejo checkout reports the served Forgejo hosts as
  logged in. The user's other hosts, github.com included, show as failing, because a launched gh
  can't reach them. So does a forge the user's gh is logged in to (§2), though plain gh would show
  it as logged in. Outside a Forgejo checkout, it delegates and shows the user's normal hosts.
- Inside a launched gh, git fetches and pushes that gh starts, such as `gh pr checkout`, get a
  credential only for a served host. gh's fetch also clears the user's own git credential helpers
  for every host (`-c credential.helper=`, `gh/git/client.go:153-155`;
  `gh/pkg/cmd/pr/checkout/checkout.go:369-370`). A private GitHub submodule fetched by
  `gh pr checkout` in a Forgejo repo therefore fails to authenticate.
  `gh pr checkout` picks its fetch remote by owner and name across every host gh knows, which in
  a launch includes the user's other hosts and github.com, ranked `upstream`, `github`, `origin`
  (`gh/context/remote.go:28-35`): with a `github` remote mirroring the forge's repository, the
  fetch goes to that mirror. A public mirror gives its own, possibly stale, branch; a private one
  gets no credential, so nothing leaks. `--force` resets an existing local branch with `git reset
  --hard` (`checkout.go:331-342`), as gh does, where the built-in path ran `git checkout -B`,
  which kept uncommitted changes or refused. Two kinds of pull request check out worse launched
  than on the built-in path until the gateway reports them as GitHub would, with no head
  repository (`cwe.17.1`): an AGit pull request, whose head branch name is `refs/pull/N/head`,
  fails to fetch; and one whose head branch was deleted fetches whatever branch now has that
  name.
- A git that a child of a launched gh starts, such as an alias shell or an editor, also gets a
  credential only for a served forge, and none for any other host over HTTPS: GitHub, GitHub
  Enterprise, GitLab, or a host the user's Keychain helper would answer. A shell alias that
  pushes to one of them fails, unless that git names a helper on its own command line (§3) or an
  askpass program answers for it, as VS Code's terminal does. SSH
  remotes are unaffected. Git outside a launched call is unaffected. For GitHub with the usual
  gh setup, the push would fail without the launcher's helper too: `gh auth setup-git` names
  real gh, which reads the call's `hosts.yml` and finds GitHub's placeholder ahead of the
  Keychain login.

Rejected:

- **The user's real config directory.** F1: a crash or uninstall leaves plain gh dialing a dead
  socket, and concurrent calls overwrite each other's socket path.
- **An empty private directory.** F1: aliases and settings vanish inside Forgejo checkouts.
- **Tokens in the environment instead of `hosts.yml`.** `GH_ENTERPRISE_TOKEN` applies to every
  non-GitHub host (fact 3). gh hands it to git for any such host (fact 4), and every child inherits
  it: git, pager, editor, extensions and alias shells. A `gh pr checkout` whose submodule lives on
  a second enterprise host would send the Forgejo token there over HTTPS. `hosts.yml` scopes each
  token to its own host.

### Token handoff

The launcher resolves each host's token with the shim's existing resolver (`auth::resolve_token`,
Keychain or `auth.json`, then the configured env names). The real token goes to the gateway, as
the upstream token, and into `credentials`, for git that gh starts (above). `hosts.yml` gets it
only in a `gh auth token` call, or a `gh auth status --show-token` call, which print it for
clients that talk to the forge directly, as today's shim does. Every other call's `hosts.yml`
holds the per-call token.

The launcher removes `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN` and
`GITHUB_ENTERPRISE_TOKEN` from gh's environment. Otherwise they would win over `hosts.yml`
(fact 3), and a user's GitHub Enterprise token would be sent for the Forgejo host.

If the routed host has no token, the launcher doesn't start gh. It prints
`gh-forgejo-shim: no token for <host>; run: gfj auth login <host>` and exits 4, which is gh's own
code for missing authentication.

**The token is on disk for the length of the call.** On macOS the stored token normally lives in
the Keychain. While a launched call runs, a copy also sits in `credentials`, and in a
`gh auth token` or `gh auth status --show-token` call in `hosts.yml`, mode 0600, in a 0700
directory under the user's temporary directory. The launcher deletes it on every exit it controls
(section 6). A SIGKILL, crash or power loss leaves it behind until the next launch sweeps it
(section 4). I think this is acceptable: the shim's own `auth.json` fallback is the same kind of
file. The shim also creates its Keychain item through `/usr/bin/security` without `-T`
(`auth.rs:445`), so any process running as the user can already read it silently by running that
same tool. Kell approved this (§13, decision 1).

## 4. Run directory and socket lifecycle

**Location.** The parent is `$XDG_RUNTIME_DIR` if set (Linux), else `$TMPDIR`, else `/tmp`. The
run directory is `gfj-<16 hex chars from the OS RNG>`, created with `mkdir` mode 0700. `mkdir` fails
if the name exists, so another user can't pre-create it; on that failure the launcher picks a new
name. It holds:

| Name | What |
|---|---|
| `lock` | held with an exclusive `File::try_lock` (std, Rust 1.89+) for the launcher's lifetime |
| `config.yml`, `hosts.yml` | section 3, created with `create_new` and mode 0600 |
| `s` | the socket |
| `credentials` | served hosts as git reaches them, with their real tokens, mode 0600, for the launcher's git credential helper (section 3) |
| `gh` | the script `GH_PATH` names, mode 0700, running `<shim> gh "$@"` |
| `panic` | only if a gateway task panicked with tracing off (section 5) |

The order is fixed:
1. `mkdir` the directory.
2. Create the lock as `.lock-<random>` with `create_new`, take the exclusive lock on it, write
   the marker line `gh-forgejo-shim run directory v1` into it, then rename it to `lock`. The lock belongs to the open file, so the rename keeps it. A sweeper only
   opens the name `lock`, so it never finds an unlocked one in a live directory, and can't win a
   race against a launcher that is still setting up. If `try_lock` on the fresh file fails, which
   no other process can cause, the launcher stops with exit 1.
3. Write `credentials` and the `gh` script, then `config.yml`, and make gh's environment edits
   (§7), all before any thread starts.
4. Start the gateway's runtime. For an `auth status` call or an `auth token --user`, look up each served host's login
   through the gateway's own client (§3). Write `hosts.yml`, with those logins.
5. Bind the socket.

The token is never on disk without a held lock named `lock` beside it.

**Path characters.** gh builds its git credential helper from the run directory path with Go's
`%q`, which isn't shell quoting (`gh/git/client.go:148`), and git hands it to `sh`. So the run
directory's full path may hold only ASCII letters, digits, `/`, `.`, `_` and `-`. A `TMPDIR` or
`XDG_RUNTIME_DIR` with anything else stops the launch with exit 1 and a message naming it.

**Path length.** `sun_path` holds 104 bytes on macOS and 108 on Linux, including the terminating
NUL. A typical macOS `$TMPDIR` (`/var/folders/xx/…/T/`, about 49 bytes) plus `gfj-<16>/s` comes to
about 70. If the full socket path doesn't fit, the launcher stops before creating anything and
says so, naming `TMPDIR`. It doesn't silently fall back to another directory.

**Readiness needs no wait.** The launcher calls `bind` and `listen` before it spawns gh. A
connection that arrives before the accept loop runs waits in the listen backlog. There is no
readiness poll and no startup timeout.

**Permissions.** The 0700 directory is the access control: connecting requires search permission
on it. As a cheap second check, the gateway drops a connection whose peer uid (`peer_cred`) isn't
the launcher's own. This isn't a boundary against processes running as the same user, since those
can read the token store anyway.

**Shutdown.** When gh exits, the launcher stops accepting, drops in-flight connections without a
grace period (their client is gone), removes `s`, `config.yml`, `hosts.yml`, `credentials`, `gh`, `panic` and `lock` by name, and
removes the directory. It removes only names it created. It never removes a directory recursively,
the mistake behind audit D1.

**Stale directories.** At each launch, before creating its own directory, the launcher sweeps
every parent a launch could have used, not just its own. That means each of `$XDG_RUNTIME_DIR`,
`$TMPDIR` and `/tmp` that exists, once each. A call from cron or an SSH session, or a client harness
with its own `TMPDIR`, can use a different parent from a desktop call. Without this, a token left
by a killed launch under one parent would never be swept by launches under another. One case
remains: a parent set only for one run (a `TMPDIR` that later calls don't share) is swept again
only when a call with that environment runs. In each parent it looks at `gfj-*` entries:

- skip anything whose name isn't exactly `gfj-` plus 16 lowercase hex characters, or that isn't a
  real directory owned by the current uid (`symlink_metadata`, no following links);
- skip a directory with no `lock` file. That is either a launcher before step 2's rename, or one
  that died before it; neither has written a token yet;
- open `lock` (`O_NOFOLLOW`) and try the exclusive lock. If it's held, a launcher is still
  running there: skip. If the lock is taken, its owner is dead, since the kernel releases the lock
  when a process dies, SIGKILL included;
- read `lock`, and skip the directory unless its content is exactly the marker line. A directory
  of the user's own named `gfj-backup` fails the name check. One that passes the name check
  by chance still lacks the marker. Nothing in it is touched either way;
- remove the known names, then `rmdir`. If anything unknown remains, `rmdir` fails and the
  directory stays.

The lock answers "is the owner alive" without pids, so pid reuse can't fool it. Two launchers
sweeping at once can't both remove the same directory, and the one that loses the lock skips it.
Errors such as ENOENT during a sweep are ignored. The sweep never delays or fails the launch.

## 5. The gateway in the launcher

The launcher serves the fga-server router on the socket with `axum::serve` over a
`tokio::net::UnixListener` (axum 0.8 implements `Listener` for it). One current-thread runtime on
the main thread runs the accept loop, the wait for gh and the signal streams.

L1 (`cwe.26`, gateway PR #46) provides the router. The launcher builds it as
`AppState::new(hosts, ghes_version)`, with one `Host` per host in *S*:

- `Host::new(public_url, forgejo, wip_prefixes)`. `forgejo` is that host's own upstream,
  `Forgejo::new_with_roots(base, roots)` (L4, `launcher::upstream`). `base` is the host's
  `api_url`, path included, or `https://<entry>`. `roots` are the certificates in the host's
  `ca_file`, then in `FJ_SHIM_CA_FILE`; they join the built-in trust store, and verification stays
  on. The launcher checks both as the built-in client does: a CA file it couldn't read, one with no
  PEM certificate, or one the HTTP client refuses, leaves the call to the built-in path, which
  reports it when the command calls the forge; the built-in `auth status` and `auth token` don't
  read CA settings, so they answer as before. `wip_prefixes` is the host's
  `[host.<entry>].wip_prefixes` from the shim config, or the gateway's default (`WIP:`, `[WIP]`)
  when unset. Forgejo decides what is a draft, from its own WIP setting, which the list should
  match. The gateway strips these prefixes from a draft's title, and adds the first one when it
  makes a draft or turns a pull request into one, so order matters. `public_url` is
  `https://<entry>`, the allowlist entry with its port if it has one, so the gateway's PR, repo and
  check links point at the forge's web UI. Per D2, PR links end in `/pull/N`.
- `.expecting(per_call).upstream_token(real)` (gateway PR #66), for every served host. gh
  presents §3's per-call token; Forgejo receives only the real one. In a
  `gh auth status --show-token` call, where §3 puts the real token in `hosts.yml`, it is
  `.expecting(real).upstream_token(real)` instead, and the per-call token is never made. Outside
  that call, any other credential for that host gets 401 before any upstream contact: the real
  token itself, a GitHub token, another
  forge's token, the placeholder, a `basic` scheme, or none. GraphQL returns 401 too, not 200
  with an error body. `gh auth status --hostname forge.test:3443` sets the Authorization header
  itself from the `host:port` key (`gh/pkg/cmd/auth/status/status.go:500`,
  `gh/pkg/cmd/auth/shared/oauth_scopes.go:39`), and gh's transport keeps it
  (`gh/api/http_client.go:232-233`). That key holds the same token as the portless key, the
  per-call one or, in a `--show-token` call, the real one, so it works.
- For an allowlist entry with a port, `.answering_to(<portless hostname>)`, unless another entry
  in *S* shares that hostname (§3). gh drops the port from
  a remote URL, so a launched gh usually sends `Host: forge.test`. It keeps the port when the user
  writes it: a full-URL `gh api https://forge.test:3443/…`, `--hostname forge.test:3443`, a
  three-part `-R`, or `GH_HOST`. The `Host` admits both spellings, `forge.test` (port 443) and
  `forge.test:3443`, and refuses any other port. It is one `Host`, not two: L1 refuses an empty
  host list and two hosts that answer to the same name.

A request for any host not in the set gets 421 with no upstream contact. The gateway crates deny
print macros, so the gateway writes nothing to stdout or stderr, which gh owns and clients parse.
The peer-uid check in section 4 is the launcher's.

**Panics stay off gh's stderr.** A panic in a gateway task still reaches stderr through the
process's panic hook. Before it starts the runtime, the launcher replaces that hook with one that
writes the panic message to the trace, when tracing is on, or else to a `panic` file in the run
directory. That file is removed with the directory, and the sweep removes it as a known name.
The panic ends only that connection's task, so gh sees that one request fail and carries on.

## 6. Signals and exit codes

gh shares the launcher's process group and inherits stdin, stdout and stderr directly, not through
pipes. gh's TTY detection, colour, prompts and pager then behave as in a plain run. Job control
works because the terminal stops and continues the whole group.

| Signal to the launcher | What it does |
|---|---|
| SIGINT, SIGQUIT | Caught by a handler, then forwarded to gh unless a terminal already sent it to the whole group (below). The launcher keeps serving and waiting. |
| SIGTERM, SIGHUP | Forward the same signal to gh, keep serving, keep waiting. |
| SIGTSTP, SIGCONT | Default. The terminal handles the group. |
| SIGPIPE | Ignored, as Rust's runtime already does. The launcher never writes to stdout after spawning gh. |
| SIGKILL | Can't be handled. gh's next request fails at once, because nothing listens on the socket. The run directory stays until the next sweep. |

**Caught, never ignored.** SIGINT and SIGQUIT get real handlers, not `SIG_IGN`. Left at the
default, Ctrl-C would kill the launcher at once, leaving the token file behind and cutting off gh's
request in flight, and Ctrl-\ would dump core. Set to `SIG_IGN`, gh would inherit the ignore,
since exec keeps ignored signals and Rust's `Command` resets only SIGPIPE, and Go keeps an
inherited ignored SIGINT ignored. Ctrl-C would then stop working on gh. A caught signal reverts
to the default across exec, so gh starts with its normal handling.

**When SIGINT and SIGQUIT are forwarded.** A terminal sends Ctrl-C and Ctrl-\ to its whole
foreground process group, gh included, so forwarding those would deliver them twice. Most of this
shim's callers have no terminal, though. ChatGPT, Claude's agent, `timeout -s INT`, Node's
`child.kill('SIGINT')` and Python's `send_signal` all signal the wrapper's pid directly, and
without forwarding, gh would keep running. The rule:
- don't forward when the launcher has a controlling terminal (`/dev/tty` opens) and its process
  group is that terminal's foreground group (`tcgetpgrp` equals `getpgrp`; rustix has safe
  versions, if L3 takes it back as a dependency);
- forward in every other case.

One case is lost. A process that signals a launcher which is in the foreground of a terminal
isn't forwarded, and gh keeps running. That is rare, and SIGTERM still works. A second: a
terminal's Ctrl-C or Ctrl-\ in the instant between the launcher's last pre-spawn check and gh's
spawn reaches no gh and isn't forwarded; pressing it again works. Forwarding it from the
pre-spawn flag can't tell that instant from the next, when the terminal already reached gh.

The handlers are installed before the run directory exists. Any of these four signals arriving
before the launcher's final pre-spawn check aborts the launch; one after it reaches gh through the run loop. The launcher cleans up what it made and dies by that
signal.

gh gets the default SIGPIPE disposition, because Rust's `Command` resets it in the child. That is
what makes `gh pr list | head -1` behave as it does with plain gh.

**Exit status.** After cleanup and the trace record:

- gh exited with code *n*: the launcher exits *n*.
- gh died by signal *s*: the launcher sends *s* to itself, so its parent sees the same death. The
  audit test `audit_route_signal_passthrough` asserts the signal itself, not 143. It exits
  128 + *s* instead in two cases:
  - as pid 1 (a container's init, in any PID namespace), the kernel drops a signal the process
    sends itself under the default action, so #44 skips the send. Otherwise signal-hook would
    fall back to `abort()`, and the launcher would die of the wrong signal;
  - a Linux real-time signal the launcher inherited as ignored stays ignored. gh inherited that
    ignore too, though, so it couldn't have died of that signal.

L3 uses R6's mechanism (`jko.16`, survivor-fixes PR #44) and doesn't build a second one.
`external::inherited_exit_code` turns gh's status into its code, or 128 + *s*, and remembers *s*.
`external::exit_like_child` is called last, after the trace is written. When the code is 128 + *s*,
it calls signal-hook's `low_level::emulate_default_handler(s)`, which puts back the default action
for *s*, overriding any handler the launcher installed, then unblocks *s* and raises it. It then
calls `low_level::raise(s)` as well. That covers the signals `emulate_default_handler` returns on
without raising: those missing from its table, such as Linux real-time signals, and those it lists
as ignored, such as SIGIO, which terminates on Linux. The launcher therefore needs no reset of its
own before the call. It restores nothing else either, since the process ends there.

The same rule covers SIGPIPE. The launcher ignores it while it runs, but when gh dies of SIGPIPE,
the launcher dies of SIGPIPE too, as plain gh would. A shell reports 141 either way.

`exit_like_child` reads *s* from the calling thread, so the launcher calls `inherited_exit_code`
and `exit_like_child` on the same thread. The current-thread runtime of section 5 runs on the main
thread, so the main thread does both. `cli::run_from_env` already wraps the `gh` branch in
`exit_like_child(routing::run_gh_from_env(..))`. A launcher that returns its code through that
branch needs nothing more. One that exits another way calls `exit_like_child` itself, as its last
step.

The exec path in section 2 needs none of this: after exec, gh's death is the wrapper's.

**Launcher failures** happen before gh starts and print one line to stderr, prefixed
`gh-forgejo-shim:`.

| Failure | Exit |
|---|---|
| no real gh found, every candidate is the shim, or guard 2 or 3 tripped (section 8) | 127 |
| real gh found but not executable | 126 |
| routed host has no token | 4 |
| a "user's gh hosts" route to a forge the user's gh has no usable login for (section 3) | 1 |
| run directory, path characters or length, bind, unreadable or unparseable user config | 1 |

After gh starts, the launcher writes nothing to the terminal. A cleanup failure is recorded in the
trace when tracing is on, and is otherwise silent. Its leftovers fall to the next sweep.

## 7. Environment

Real gh inherits the wrapper's own environment, and the wrapper changes only the variables named
here, in place, with `std::env::set_var` and `remove_var`. Then it execs or spawns gh with no
`Command` environment call. That matters because any `Command::env`, `env_remove` or `env_clear`
call makes Rust rebuild the child's environment from `vars_os()` into a sorted map, which drops
entries without `=` after their first byte (a bare `NAME`, an empty entry, `=value`), keeps only
one of a repeated name, and reorders the rest. Untouched, every other entry reaches gh byte for
byte and in order, non-UTF-8 values included (R7, `jko.7`; `z9u`).

Editing the environment is sound only while no other thread can read it, so the wrapper does it
while single-threaded: at entry (the §7 revert below, and `FJ_SHIM_DEPTH`, §8), and on the launch
path after the run directory and its files exist but before the gateway's runtime, or any other
thread, starts. The signal handlers that §6 requires before the run directory are signal-hook
flags, registered without a runtime. A debug build on Linux checks that the process has one
thread at the edit point. Building gh's environment by hand and spawning it with `posix_spawn` or
`execve` instead would need `unsafe`, which the workspace forbids, or a new dependency.

| Variable | In launched gh |
|---|---|
| `GH_CONFIG_DIR` | the run directory |
| `GH_PATH` | the run directory's `gh` script, so gh's git credential helper is the launcher's (section 3, "Git's credentials") |
| `GIT_CONFIG_PARAMETERS` | the inherited value and a space, if that value is non-empty, then `'credential.helper=' 'credential.helper=!<run dir>/gh auth git-credential'` (section 3, "Git's credentials") |
| `GH_BROWSER` | the URL helper (section 9) |
| `GH_HOST` | the routed host's hostname without its port, for a host-less `gh api` or `gh auth token` (section 2); otherwise unchanged |
| `GH_REPO` | `HOST/OWNER/REPO`, when routing chose a forge remote gh drops because `ssh -G` sends it elsewhere (section 2); otherwise unchanged |
| `GH_NO_UPDATE_NOTIFIER`, `GH_NO_EXTENSION_UPDATE_NOTIFIER` | `1`; these checks would call api.github.com through the socket |
| `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN` | removed (section 3) |
| `FJ_SHIM_LAUNCHER` | the launcher's pid |
| `FJ_SHIM_DEPTH` | the inherited depth plus 1. Every wrapper invocation sets it, on the delegate path too (section 8) |
| `FJ_SHIM_SAVED` | the names of the variables above that the launcher changed |
| `FJ_SHIM_ORIG_<NAME>` | the user's original value of each changed variable that was set |
| `FJ_SHIM_SET_<NAME>` | the value the launcher set for each changed variable it didn't remove |

The table is the launch path. Stepping aside (§2) sets `GH_HOST` to the forge's portless hostname
on every form, unless the user's own `GH_HOST` has that value, and `GH_REPO` as on the launch
path, both recorded with `FJ_SHIM_SAVED`, `FJ_SHIM_ORIG_<NAME>` and
`FJ_SHIM_SET_<NAME>`. It removes the four token variables on every form, unrecorded, so no
nested call gets them back. It never sets `FJ_SHIM_LAUNCHER`.

The launcher spawns gh by absolute path, so `argv[0]` is that path too.

**Nested calls get the user's environment back.** gh's children inherit the launched environment:
alias shells, extensions, git, pager, editor and credential helpers. When one of them runs `gh`
again, it reaches the wrapper. Before routing, the wrapper reverts the launcher's changes. For each
name in `FJ_SHIM_SAVED`, it restores the variable from `FJ_SHIM_ORIG_<NAME>`, or unsets it if
there's no saved value. It skips any variable whose current state no longer matches what the
launcher left: a different value from `FJ_SHIM_SET_<NAME>`, or, for a removed variable, any value
at all. Something in between changed it on purpose. `GIT_CONFIG_PARAMETERS` is the one
exception: a git that runs a child with `-c` appends a space and its own entries to it. So when
the value is `FJ_SHIM_SET_GIT_CONFIG_PARAMETERS`, a space, and a rest, the wrapper keeps the rest:
it sets the variable to the original value, a space and the rest when the original was
non-empty, and to the rest alone otherwise, never with leading whitespace. Any other value that
differs from the one the launcher set is skipped like any changed variable. It then removes the
bookkeeping variables, except `FJ_SHIM_DEPTH`. A shell alias that runs
`gh api https://api.github.com/user` therefore runs plain gh with the user's own config and token.

## 8. Recursion

R1 (`jko.15`): `external.rs:260` compares canonical paths, so a copy or hardlink of the shim passes
the "real gh" check. Under option B the launcher spawns that copy as gh. A bare copy of the binary
can't loop: `cli::run_from_env` routes only a `gh` first argument to the wrapper and never checks
its own program name, so the copy gets gh's arguments and fails with a usage error. The loop needs
a script that runs a shim build as `<shim> gh "$@"`, which is what the managed wrapper itself
looks like. Such a script sees a Forgejo invocation and launches again, and so on. Three guards
stop it:

1. **Identity.** Real-gh discovery (`external::find_program("gh", ..)`, R1 fix in survivor-fixes
   PR #44) skips managed wrapper scripts. It also skips any candidate that may be either shim
   entry point, `gh-forgejo-shim` or `gfj`: the same file by device and inode, which catches
   hardlinks; a byte-for-byte copy; or a same-length file it can't read. On Linux it compares
   against `/proc/self/exe`, so a running binary whose file was deleted still counts. The launcher
   calls the same discovery. Guard 1 can't see three cases: a copy of a *different* shim version,
   a hand-written script that runs the shim without the managed marker, and, on macOS, a copy of a
   running binary whose own file was deleted or replaced. Guards 2 and 3 catch all three, because
   they don't depend on what the candidate is.
2. **Direct child.** If `FJ_SHIM_LAUNCHER` equals the wrapper's parent pid
   (`std::os::unix::process::parent_id`), the launcher spawned this process as real gh, so
   discovery picked the shim. The wrapper exits 127 with
   `gh-forgejo-shim: the real gh at <path> is gh-forgejo-shim; set FJ_SHIM_REAL_GH`. Legitimate
   nested calls are grandchildren, through gh or a shell, and never trip this. gh never replaces
   itself with exec: v2.103.3 has no `syscall.Exec` call, and runs aliases and extensions as
   children.
3. **Depth cap.** Every wrapper invocation, delegating or launching, increments `FJ_SHIM_DEPTH`
   in the environment it passes on. If the inherited value is already 8 or more, the wrapper
   doesn't run anything. It exits 127 with
   `gh-forgejo-shim: gh resolves back to gh-forgejo-shim (depth 8); set FJ_SHIM_REAL_GH`.
   That catches a loop through an intermediate process that guard 2 can't see. An example is an
   unmarked script that runs `gh-forgejo-shim gh "$@"` without exec. Each turn of the loop then
   leaves a waiting `sh`, so the shim is a grandchild, not a child. Delegating at the cap, as an
   earlier draft did, would exec the same script again. Legitimate nesting, an alias shell
   running gh that runs another alias, stays far below 8.

The run directory's `gh` script, which `GH_PATH` names, runs the wrapper too. It sets `FJ_SHIM_CREDENTIALS`, and the wrapper answers `auth git-credential` from that
before any guard or the §7 revert, never launching (§3), so the helper can't start a chain.
Anything else reaching that script runs as a nested call only when its command word is one of gh's own other than `auth`; the rest is refused (§2, the delegate list).

Guard 2 runs before the environment revert in section 7, which keeps `FJ_SHIM_DEPTH` for guard 3.

## 9. PR URLs and the browser (D2)

PR URLs from the gateway end in `/pull/N` in every mode (D2, bead `65s`). Forgejo serves
`/pulls/N`. In client mode no proxy exists to redirect, so the launcher sets `GH_BROWSER` to a
helper. Every gh `--web` open and every `gh browse` goes through it.

- `GH_BROWSER` is the shim's absolute path, shell-quoted for shlex, plus a hidden subcommand and
  the hosts in *S*, each quoted the same way and followed by its forge's path prefix, if any
  (`forge.test/forgejo`, from the upstream base, §5). gh appends the URL (fact 6).
- The helper rewrites the URL only when its host names a host in *S*, and its path is that
  host's prefix, then `/<owner>/<repo>/pull/<N>`, optionally followed by `/…`, a query or a
  fragment. The gateway's links keep a forge's path prefix. For an
  allowlist entry with a port, both spellings count, the same two the gateway admits (§5). The
  gateway's own links carry the port, from `public_url`, while URLs gh builds itself don't. It changes
  `/pull/` to `/pulls/` and leaves everything else as it was. Other URLs pass through unchanged.
- It then opens the URL with the browser the user would have got, in gh's order: the user's
  original `GH_BROWSER` (from `FJ_SHIM_ORIG_GH_BROWSER`), the `browser` key in their
  `config.yml`, `BROWSER`, then the platform default (`open` on macOS, `xdg-open` elsewhere). It
  exits with that program's status.
- The subcommand is registered in `cli.rs`, which belongs to the survivor thread. L3 asks for it
  through the coordinator.

ChatGPT's own "Open pull request in browser" link doesn't go through gh. In client mode it still
404s without the redirect recipe, as D2 says.

## 10. Timeouts (F23) and concurrency

**Timeouts.** Per F23, the deadline belongs to each gateway request, not to the command:

- Each gateway request has one 10 s deadline, shared by every upstream call it makes, body reads
  included (`crates/fga-core/src/forgejo.rs:143,194-200`). The deadline starts after
  authentication, though, and the `/user` lookup that authenticates has its own 10 s client
  timeout. A request that has to authenticate can therefore take up to about 20 s before gh gets
  its error: a 9 s `/user`, then a stalled call. Requests that find the session cached are bounded
  by about 10 s. The cache keeps a session for 60 s from its lookup, with no extension on use
  (`crates/fga-core/src/forgejo/cache.rs:11`), so a long `gh run watch` authenticates again each
  minute and can hit the 20 s bound then too. Putting both steps under one deadline would be a
  gateway change, not a launcher one.
- What gh gets for a timeout depends on the API. A REST request gets 504. A GraphQL request,
  which is what `gh pr view` and most `pr` commands send, gets HTTP 200 with an `errors` body
  naming the timeout: the gateway's GraphQL handler always answers 200 (`graphql_request` in
  `crates/fga-server/src/lib.rs`). Either way gh prints the error and exits 1.
- The launcher has no process-wide deadline and never kills gh on its own. `gh run watch` and
  `gh pr checks --watch` poll for as long as gh likes. An interactive `gh pr create` sits in the
  editor for as long as the user likes. Plain gh against GitHub behaves the same way.
- Setup has no timeout, because nothing waits (section 4). Shutdown has no grace period.
- A client that wants a bound on the whole command kills the wrapper. SIGTERM reaches gh
  (section 6). SIGKILL leaves gh to fail on its next request.

R4's old gate, "a slow server finishes within the deadline plus 0.5 s", becomes a gateway test.
A stalled upstream answers within 10.5 s once the session is cached, and within 20.5 s when
`/user` is slow too: a 504 for REST, a 200 with an `errors` body for GraphQL. The launcher keeps
running until gh exits.

**Concurrency.** Each invocation has its own run directory, socket, config copy, gateway and
session cache. ChatGPT polls about every 60 s per thread, so several launchers run at once and share
nothing but read-only inputs: the user's gh config, the shim's config and the token store. The
remaining shared writes are:

- the trace file, which needs audit D6 (`jko.11`) fixed before L3. The fix (survivor PR #53)
  writes each record with one `O_APPEND` write and takes no file lock: a lock could wait forever
  on another process and hang every traced call. That holds on local filesystems, not NFS. The
  launcher writes trace records the same way, and adds no lock;
- the sweep, which only touches directories whose lock it holds (section 4).

Each launched call pays one `/user` lookup per served token per minute of activity: the first
request, then one more each time the 60 s cache entry lapses. The session cache lives in the
process and doesn't outlast it. Within one call, gh
sends requests in parallel (for example `gh pr status`, or `gh auth status` across hosts), and the
runtime serves them concurrently.

## 11. Acceptance tests

L3 builds these, as process tests against real gh v2.103.3 and a Forgejo stub or the e2e Forgejo.
Each names the section it proves. In tests 1 to 14 the user's `hosts.yml` holds no login for the
served forge, or every call would step aside (§2); the e2e client setup logs gh in to the forge,
so those tests can't reuse its config. On macOS the tests send only SIGTERM, SIGKILL, SIGINT and SIGHUP,
because crash-type signals (SIGSEGV, SIGQUIT, SIGABRT, SIGBUS, SIGILL) open a crash dialog on the
user's screen. Cases that need one, such as SIGQUIT in §6, run only on Linux.

1. **Config immutability (§3).** Hash every file in the user's gh config directory. Run a launched
   `gh pr view`, a launched call killed with SIGKILL, and `uninstall`. The hashes are unchanged
   after each.
2. **Aliases (§2, §3).** In a Forgejo checkout, an alias `co` = `pr view` runs through the
   launcher and prints the PR. A shell alias `!gh api https://api.github.com/user` runs plain gh
   with the user's original environment (§7).
3. **No foreign credential reaches the gateway (§3, §5).** Set up `GH_TOKEN`, a github.com entry
   in the user's `hosts.yml`, and a GitHub token in the Keychain's unkeyed `gh:github.com` slot
   (Linux gate: a test keyring). A host-less `gh auth status` in a Forgejo checkout reports the
   Forgejo hosts as logged in. Every request the gateway sees for any other host carries only the
   placeholder, and gets 421 without any upstream contact. No request carries the GitHub token.
   With `GH_ENTERPRISE_TOKEN` set, the Forgejo host still gets its own token.
4. **Signals (§6).** SIGTERM to the launcher: gh gets SIGTERM, the launcher dies by SIGTERM, and
   the run directory is gone. SIGINT to the process group, as a terminal's Ctrl-C sends it: when
   gh dies by SIGINT, so does the launcher, and the directory is gone. SIGINT to the launcher's pid
   with no controlling terminal, as ChatGPT or `timeout -s INT` sends it: the launcher forwards it,
   gh dies by SIGINT, and so does the launcher. In `gh pr list | head -1`, when gh dies of SIGPIPE,
   the launcher dies of SIGPIPE too, and its directory is gone. A gh that kills itself with
   SIGTERM: the launcher dies by SIGTERM (R6's test, on the launch path). SIGQUIT cases run on
   Linux only.
5. **Stale directories (§4).** After a SIGKILL'd launch, the next launch removes the leftover
   directory. A live launcher's directory survives another launcher's sweep. A directory with an
   unknown file in it stays.
6. **Concurrency (§10).** Eight parallel launched `gh pr view` calls all succeed, each with its own
   directory, and none is left afterwards.
7. **Recursion (§8).** Each case uses a shim build that differs from the running one, such as an
   older or patched build, so guard 1 can't see it, and runs it from an unmarked script:
   - `FJ_SHIM_REAL_GH` names `#!/bin/sh` + `exec <other build> gh "$@"`: guard 2 trips, the wrapper
     exits 127 with its message, and only one gateway ever ran;
   - the same script without `exec`: guard 3 stops the chain at depth 8 with exit 127, and no
     launch runs past that depth.
   Separately, an identical copy as `FJ_SHIM_REAL_GH`, with no real gh reachable on PATH or in the
   fallback directories, exits 127 with "could not find the real gh". Guard 1 skips the copy, and
   nothing else is left. R1's two audit tests pass.
8. **Environment (§7).** A non-UTF-8 variable reaches gh byte for byte (R7's test, on the launch
   path). gh's environment has none of the four token variables.
9. **Socket path (§4).** A `TMPDIR` long enough to overflow `sun_path`: exit 1, a message naming
   `TMPDIR`, and nothing created.
10. **Browser (§9).** `gh pr view --web` invokes the user's browser with `/pulls/N`, for a forge on
    the default port and for one on a custom port. A non-PR URL and a URL on another host pass
    through unchanged.
11. **Tokens (§3).** `gh auth token --hostname <forge>` prints the stored token, and so does
    `gh auth status --show-token --hostname <forge>`, which also reports the forge as logged
    in. So do `-at`, `--show-token=1` and an alias expanding to `auth status -t`. A plain
    `gh auth status`, `--show-token=false`, `--show-token=0` and `-t --show-token=false` print no
    token, and gh's request carries the per-call one.
    With no stored token, a launched `gh pr view` exits 4 with the login hint.
12. **Delegation (§2).** `gh --version` in a Forgejo checkout delegates (R4, `jko.5`).
    `gh config set` in a Forgejo checkout changes the user's real `config.yml`.
13. **Timeout (§10).** Once the session is cached, a stalled upstream answers within 10.5 s:
    a REST request (`gh api repos/{owner}/{repo}`) with 504, and a GraphQL request (`gh pr view`)
    with 200 and an `errors` body. A slow `/user` (9 s) followed by a stalled call answers within
    20.5 s. gh exits 1 in every case, and the launcher doesn't kill it.
14. **Git's credentials (§3).** With `GIT_TERMINAL_PROMPT=0`, and `GIT_ASKPASS`, `SSH_ASKPASS`
    and `core.askPass` unset: in a launched `gh pr checkout`, git
    fetching from the served forge, with or without a custom port, gets the real token. A push
    to `https://gist.forge.test/…` gets no credential, and neither does a fetch from a GitHub
    remote, even with a GitHub token in the Keychain. Then a shell alias runs `git push`, with
    gh's helper set in four places: a generic `credential.helper`, a
    `credential.https://github.com.helper`, a `GIT_CONFIG_COUNT` entry and an inherited
    `GIT_CONFIG_PARAMETERS` entry. The test runs once with those naming the wrapper and once
    naming real gh by absolute path. `GIT_TRACE` shows only the launcher's helper ran. The push
    gets the real token for the served forge, and none for `gist.forge.test` or GitHub. A `gh` run by a
    `git -c x.y=z` alias inside the call sees the user's own `GIT_CONFIG_PARAMETERS`, a space and
    `'x.y'='z'`. Each of these runs three times: with the user's `GIT_CONFIG_PARAMETERS` unset,
    set but empty, and set. With it unset or empty, that nested `gh` sees exactly `'x.y'='z'`,
    and every git in the call and in the nested `gh` runs without a config parse error. `gh sg`, with alias
    `sg: auth setup-git`, delegates and writes real gh's path, not the run directory's, into
    `~/.gitconfig`. `gh auth token --hostname <forge>` still prints the real token.
15. **Stepping aside (§2).** With the user's own `hosts.yml` holding a login for a served forge,
    once as an `oauth_token` and once as a `user` alone, as gh's secure-storage login writes it,
    `gh pr view`, `gh api --hostname <forge> repos/{owner}/{repo}` and the alias `co` = `pr view`
    in that forge's checkout all run plain gh: no run directory is created, no socket is bound,
    and gh's environment equals the wrapper's apart from `FJ_SHIM_DEPTH` and `GH_HOST`, set to the
    forge's portless hostname. With github.com also logged in, a host-less
    `gh api repos/{owner}/{repo}`, `gh auth token`, `gh repo delete o/target`,
    `gh repo clone o/target`, `gh search prs foo` and `gh gist list` there run plain gh with that
    `GH_HOST`: every request goes to the forge and the trace names it (`tests/step_aside_hosts.rs`
    checks the requests real gh sends), and `gh auth token` prints the forge's token from the
    user's own config. `gh attestation download f --owner o --hostname github.com` doesn't step
    aside. A forge-routed `gh pr create` stops before gh runs when the forge remote's push URL, or
    a `pushInsteadOf` rule, sends git's push to another host, and `gh repo create --push` always
    does. gh's environment then
    has `FJ_SHIM_SAVED`, `FJ_SHIM_ORIG_GH_HOST` (if the user set one) and `FJ_SHIM_SET_GH_HOST`,
    and no `FJ_SHIM_LAUNCHER`. With `GH_ENTERPRISE_TOKEN=ghe_corp` also set, `gh auth token`
    prints the forge's own token, gh's environment has no `GH_ENTERPRISE_TOKEN`, and
    `FJ_SHIM_ORIG_GH_ENTERPRISE_TOKEN` holds `ghe_corp`; a request captured at the forge stub
    never carries `ghe_corp`. With the allowlist entry `forge.test:3443` and the checkout's
    origin `https://forge.test:3443/o/r.git`, gh gets `GH_HOST=forge.test`, and both commands
    succeed: the request arrives at `forge.test` on port 443, so the forge stub listens there
    too, nothing reaches port 3443, and the trace names `forge.test`. A user `GH_HOST=forge.test`
    is left as it is, unrecorded, and `GH_ENTERPRISE_TOKEN` still goes. The same holds when that forge
    is the user's only host, and when the allowlist entry is `forge.test:3443` while the login
    sits under `forge.test`. A login stored only under `forge.test:3443` doesn't step aside: the
    call launches. A host-less `gh auth status` in another forge's checkout leaves the logged-in
    forge out of *S*.
16. **Own-hosts refusal (§3).** With the user's own `hosts.yml` listing only a served forge, and
    the allowlist `forge.test,other.test`, a call that routes "user's gh hosts" exits 1 with the
    message naming the user's config directory and the key, and leaves the user's config
    unchanged. The route comes from L2's parity rows: `gh pr view 1` in a checkout with `origin`
    on `forge.test` and `upstream` on github.com, and `gh pr view 1 -R other/target` (or
    `-R o/r`) in a checkout with only `origin` on `forge.test`. It runs with an entry holding
    only a `git_protocol`, and with one keyed `Forge.Test` (the message names `Forge.Test`). A
    third case has the user's only key `forge.test:3443`, holding a login, with the allowlist
    `forge.test:3443,other.test` and `gh pr view 1 -R other/target`: the login sits where gh's
    API client never reads it, so the call is refused too.

## 12. Ownership

| Piece | Owner |
|---|---|
| launcher module, `routing.rs`, `repo.rs`, wrapper entry point | launcher thread (L2, L3, L4) |
| host isolation and multi-host router (§5) | gateway thread, L1 `cwe.26` |
| R1 identity check, R6 signal and exec pattern, browser subcommand registration in `cli.rs` | survivor-fixes thread, through the coordinator |
| per-host CA, `api_url` and server-mode key | L4 `cwe.29` and S `cwe.42` |

## 13. Decisions for Kell

1. **The token sits in a private `hosts.yml` for the length of a launched call** (§3). The
   alternative, the environment, leaks across hosts through gh's git credential helper. Since
   Sol's round 4, the real token sits in the run directory's `credentials` file, and `hosts.yml`
   holds it only in a `gh auth token` or `gh auth status --show-token` call; every other call's
   `hosts.yml` holds a per-call token. The token is still on disk only in the private run
   directory, for the length of the call.
2. **A launched gh has no credential for any host but the served Forgejo hosts** (§3). Kell
   approved this at `cc8b38b`. Review then found the original mechanism leaked: gh fell back to the
   Keychain. The fix lists the user's other hosts in the private `hosts.yml` with a placeholder
   token, which changed two things the user sees, so it went back to Kell:
   - A host-less `gh auth status` in a Forgejo checkout shows the Forgejo hosts as logged in, and
     the user's other hosts as failing, not absent. The gateway sees those requests, carrying
     only the placeholder, and refuses them locally. Git fetches started by gh get no working
     credential for other hosts.
   - Installed gh extensions always run through plain gh (§2).

   Kell, 2026-11-02: (a), the behavior above, with extensions through plain gh accepted.

   The later step-aside rule (§2) adds one case to the first point: a forge the user's gh is
   logged in to is not served, so it too shows as failing there, though plain gh would show it as
   logged in.

   Sol's reviews then found one more gap, and it goes back to Kell. The placeholders cover only
   the hosts in the user's own `hosts.yml`, while the Keychain is shared by every gh config
   directory. A token saved by a login under another config directory, for a host this one
   doesn't list, can still be used by a launched gh (§3). It goes either down the socket to the
   local gateway, which refuses it and forwards nothing, or out through git to its own site, as
   plain gh would send it.
   - **(a) Recommended:** accept this, and state the guarantee as "the launcher never sends a
     login to a site it doesn't belong to".
   - **(b)** Always add a github.com placeholder too. That covers the usual case, a GitHub login
     made elsewhere. But routing would then treat github.com as known for every user, so a
     two-part `-R` would default to github.com even for someone with only a forge. Other hosts
     stay as in (a).

   Kell, 2026-11-02: (a). The guarantee is "the launcher never sends a login to a site it
   doesn't belong to", with no github.com placeholder.

   Sol's rounds 3 and 4 then led to two changes (§3). `hosts.yml` holds only per-call tokens,
   which open nothing outside the gateway, and the launcher supplies the helper for git that gh
   starts. That removes the git exception: no git in a launched call can hand anyone a usable
   login. The guarantee only got stronger, so it didn't go back to Kell. Its one cost, a private
   GitHub submodule failing to authenticate in a launched `gh pr checkout`, was already listed
   and approved.

   Sol's round 5 found that git started by a child of a launched gh, such as an alias shell or
   an editor, could still reach gh's own helper through the user's git config, and gh's `gist.`
   retry could then give a forge login to a `gist.` host. The launcher now forces its own helper
   on every git in the call through `GIT_CONFIG_PARAMETERS` (§3, §7). That adds a cost the user
   sees, so it went back to Kell. Inside a launched call, git that an alias shell or an editor
   starts gets credentials only for the served forge, and none for any other host over HTTPS
   beyond what an askpass program or a terminal prompt supplies for that host.
   Git that gh itself starts is unchanged by this, and git outside a launched call is
   unaffected. A nested git that names a helper with `-c` on its own command line still asks
   that helper (§3). The alternative was to keep the user's own
   helpers for nested git and narrow the guarantee's wording, leaving gh's `gist.` retry open.

   Kell, 2026-11-02: force the launcher's helper, with the cost above.
3. **gh writes inside a launched call are discarded** (§3). The delegate list keeps the config-writing
   commands out, but an alias that expands to one inside a Forgejo checkout loses its write.
4. **`gh api` and `gh auth token` without `--hostname`, inside a Forgejo checkout** (§2). gh
   itself picks these commands' host from its default host, not from the checkout.
   - **(a) Recommended:** they talk to the forge. The launcher sets `GH_HOST` to the routed host
     for the call. That is what today's shim does, and what an agent typing
     `gh api repos/{owner}/{repo}/pulls` in a Forgejo checkout expects.
   - **(b)** They follow gh and go to github.com, as plain gh does in a GitHub Enterprise
     checkout. That sends an agent's forge request to GitHub, which answers for a different
     repository or not at all.

   ChatGPT and Claude pass `--hostname` or a three-part `--repo` themselves, so neither option
   affects them.

   Kell, 2026-11-02: (a).

   The later step-aside rule (§2) runs plain gh for a forge the user's gh is logged in to, which
   would have dropped this for those forges. Kell, 2026-11-02: (a) there too. Stepping aside
   sets `GH_HOST` for these two commands as the launch path does. On both paths the value is the
   forge's hostname without its port, which is how gh finds the token and matches remotes (§2),
   and stepping aside also removes the two GitHub Enterprise token variables, so that setting
   `GH_HOST` sends no enterprise login to the forge.
