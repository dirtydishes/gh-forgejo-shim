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
- help for any command: `help …`, or a help flag as gh parses it. That means `--help`, or `-h`
  in a command that doesn't define its own `-h`, as a flag and not as another flag's value, before
  a `--`. Several `auth` commands define `-h` as `--hostname`, so `gh auth token -h forge.test`
  isn't help. The wrapper decides this with the invocation parser's flag grammar, not by
  scanning argv. Help needs no token, so `gh pr view --help` prints help even where the forge
  has no stored token;
- `config …`, `alias …`;
- `auth login`, `auth logout`, `auth switch`, `auth refresh`, `auth setup-git`;
- `extension …`, because install, upgrade, browse and search call github.com.

**Commands with no repository.** `gh api` without `--hostname`, and `gh auth token` without
`--hostname`, don't use the checkout's repository to pick a host. They use gh's default host
(`gh/pkg/cmd/api/api.go:417-420`, `gh/pkg/cmd/auth/token/token.go:71-73`). A launched gh's
`hosts.yml` holds several keys whenever the user has a host of their own, so its default host is
then github.com. Routing these by the checkout's remote, as today, would make the launched gh
print the placeholder for `gh auth token`, and send `gh api` to github.com, where the gateway
refuses it with 421. So when routing sends a host-less `gh api` or `gh auth token` to a Forgejo
host, the launcher sets `GH_HOST` to that host for the call, which makes it gh's default host.
They then talk to the forge, as the shim's versions of them always have. This is §13 decision 4,
waiting for Kell. The alternative is to follow gh, so they go to github.com like plain gh in a
GHES checkout. `GH_HOST` is restored for nested calls like the other launcher variables (§7).
`auth status` and `auth git-credential` keep their current routing. L3 owns the exact table and
proves R4 (`jko.5`: `gh --version` in a Forgejo checkout delegates).

**Server mode never launches.** In server mode (D1, `cwe.42`) the forge host serves `/api/v3` and
`/api/graphql` itself, so plain gh works against it with a normal `gh auth login`. A host configured
for server mode routes as delegate. The config key belongs to `cwe.42` and the config bridge (L4).

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
- **`hosts.yml`:** one entry for every host in the user's own `hosts.yml`, and one for every
  Forgejo host on the shim's allowlist, by gh's portless hostname. Each entry has an
  `oauth_token`, plus the `git_protocol` from the user's entry for that host, if any. A host-level
  `oauth_token` needs no `user` key for gh to find it. `gh auth status` reads the account name from
  `user`, though, and fetches it only for tokens from the environment
  (`gh/pkg/cmd/auth/status/status.go:314-320,484-490`). For an `auth status` call, the launcher
  therefore looks up each served host's login before it starts gh. It uses the gateway's own
  `authenticate`, which warms the session cache, so gh's own requests repeat nothing. It writes
  that login as `user`. If the lookup fails, the entry has no `user`, and gh reports the failure
  itself. Other commands never show the account, so they get no `user` and no lookup.
  - A served host (the set *S*, below) gets its real token.
  - Every other host gets the placeholder `gfj-not-served`.
  - An allowlist entry with a port, such as `forge.test:3443`, always gets two entries, because
    gh looks its token up two ways. gh's API client drops the port before the lookup
    (`gh/api/http_client.go:284-290`), so it finds the portless key. gh's git credential helper
    looks up the host git passes, port included (`gh/pkg/cmd/auth/gitcredential/helper.go:119`),
    so a fetch from `https://forge.test:3443/…` finds `forge.test:3443`.
    - The `host:port` entry gets the real token, which git then sends only to that port.
    - The portless entry gets a random token instead, 32 bytes from the OS RNG as hex, made for
      this call. The gateway accepts it for the host and authenticates upstream with the real
      token (§5).

    A git fetch from the same hostname on another port, such as a portless remote on 443 that
    may be a different service, then gets only the random token, which nothing else accepts.
    Routing counts both keys (L2, `GhHosts::load`), so gh's default host still matches the
    wrapper's model.
  - Two allowlist entries that share a hostname, such as `forge.test:3443` and
    `forge.test:8443`, can't share the portless key. That key gets the placeholder, and each entry
    is reachable only with its port spelled out. Routing already leaves a portless host on a shared
    hostname unrouted (L2).

The placeholders matter because of how gh finds a token. With no token in `hosts.yml` for a
host, gh falls back to the Keychain's unkeyed slot for it (`gh:<host>`, user `""`;
`gh/internal/config/config.go:320-324,382-384`). Every normal secure-storage login fills that
slot (`config.go:651-652`), and moving `GH_CONFIG_DIR` doesn't move the Keychain. Without a
placeholder, a github.com request from a launched gh would carry the user's real GitHub token
down the socket. A non-empty token in `hosts.yml` wins over the Keychain
(`config.go:301-304`), so the placeholder blocks that fallback.

The placeholders cover every host the user's `hosts.yml` lists, and nothing else. The Keychain is
shared by every gh config directory, though. Say the user once logged in to github.com under
another `GH_CONFIG_DIR`, and this one doesn't list github.com. A launched gh asked for github.com
by an explicit URL, such as an alias expanding to `api https://api.github.com/user`, finds that
Keychain token and sends it down the socket. It reaches only the local gateway, which refuses it
with 421 and forwards nothing (§5). No credential leaves the machine for the wrong host, but the
gateway does see it. §13 decision 2 records this.

The key set, the user's hosts plus the allowlist hosts, is also what routing (L2,
`repo::GhHosts::load`) assumes gh knows. A launched gh therefore resolves the same default host
(`go-gh/pkg/auth/auth.go:146-156`) and the same remote as the wrapper predicted.

There is one exception. go-gh's default host depends on how many keys `hosts.yml` holds. Plain gh
counts only the user's keys, so when the user's own `hosts.yml` holds exactly one host, and it is
an allowlist host, plain gh defaults to that Forgejo host. Before delegating, routing checks plain
gh's own choice (`GhHosts::user`). When that choice is a served Forgejo host, routing launches
with the user's key set instead, marked "user's gh hosts" in the route reason. That call's
`hosts.yml` then lists only the user's hosts, the served one with its real token and the others
with the placeholder. The launched gh then resolves exactly as plain gh would.

*S* is the routed host. For a host-less `gh auth status` it is every allowlist host that has a
token, so the status reports all of them.

What this guarantees:

- **The user's gh config is byte-identical after a run, a crash and an uninstall**, because the
  launcher only ever opens it to read. There is nothing to restore after a crash.
  `docs/rollback.md` stays true: removing the wrapper restores plain gh.
- **Concurrent invocations can't collide**, because each one has its own directory and socket.
- **Aliases and settings work inside a Forgejo checkout.** Installed extensions run through plain
  gh (§2), so the ones that talk to GitHub keep working there.
- **The launched gh holds credentials only for the hosts in *S*.** github.com and every other host
  have the placeholder, which no server accepts, and the gateway rejects their requests with 421
  without contacting any upstream (L1).

The cost:

- A write that gh makes inside a launched process lands in the copy and is lost when the run ends.
  The delegate list in section 2 keeps the commands that write config out of the launcher. An
  alias that expands to `config set` inside a Forgejo checkout loses its write. This is rare, and
  L3's docs name it.
- A host-less `gh auth status` inside a Forgejo checkout reports the Forgejo hosts as logged in.
  The user's other hosts, github.com included, show as failing, because a launched gh can't reach
  them. Outside a Forgejo checkout, it delegates and shows the user's normal hosts.
- For a forge on a custom port, `gh auth token --hostname forge.test` prints the random per-call
  token, which works only through the gateway. `gh auth token --hostname forge.test:3443` prints
  the real one. A client that hands the token to its own HTTP calls has to spell the port.
- Inside a launched gh, git fetches that gh starts, such as `gh pr checkout`, get only the
  placeholder for other hosts. gh's fetch also clears the user's own git credential helpers for
  every host (`-c credential.helper=`, `gh/git/client.go:153-155`;
  `gh/pkg/cmd/pr/checkout/checkout.go:369-370`). A private GitHub submodule fetched by
  `gh pr checkout` in a Forgejo repo therefore fails to authenticate. No credential leaks in this
  case: the placeholder is all git ever sends.

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
Keychain or `auth.json`, then the configured env names). It writes the token into `hosts.yml`
and hands the same token to the gateway for that host. The launched gh therefore holds the real
Forgejo token, as today's shim does. It needs the real one, because gh hands it to git for HTTPS
fetches and prints it for `gh auth token`, and both of those talk to the forge directly.

The launcher removes `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN` and
`GITHUB_ENTERPRISE_TOKEN` from gh's environment. Otherwise they would win over `hosts.yml`
(fact 3), and a user's GitHub Enterprise token would be sent for the Forgejo host.

If the routed host has no token, the launcher doesn't start gh. It prints
`gh-forgejo-shim: no token for <host>; run: gfj auth login <host>` and exits 4, which is gh's own
code for missing authentication.

**The token is on disk for the length of the call.** On macOS the stored token normally lives in
the Keychain. While a launched call runs, a copy also sits in `hosts.yml`, mode 0600, in a 0700
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
| `panic` | only if a gateway task panicked with tracing off (section 5) |

The order is fixed:
1. `mkdir` the directory.
2. Create the lock as `.lock-<random>` with `create_new`, take the exclusive lock on it, write
   the marker line `gh-forgejo-shim run directory v1` into it, then rename it to `lock`. The lock belongs to the open file, so the rename keeps it. A sweeper only
   opens the name `lock`, so it never finds an unlocked one in a live directory, and can't win a
   race against a launcher that is still setting up. If `try_lock` on the fresh file fails, which
   no other process can cause, the launcher stops with exit 1.
3. Write `hosts.yml`, then `config.yml`.
4. Bind the socket.

The token is never on disk without a held lock named `lock` beside it.

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
grace period (their client is gone), removes `s`, `config.yml`, `hosts.yml`, `panic` and `lock` by name, and
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
  of the user's own, say `/tmp/gfj-backup`, fails the name check. One that passes the name check
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

- `Host::new(public_url, forgejo, wip_prefixes)`. `forgejo` is that host's own upstream: base URL
  and HTTP client, with the CA and `api_url` settings from L4. `public_url` is `https://<entry>`,
  the allowlist entry with its port if it has one, so the gateway's PR, repo and check links point
  at the forge's web UI. Per D2, PR links end in `/pull/N`.
- `.expecting(token)` with the token gh will present, the one under the key gh's API client
  looks up, which is the portless key. Any other credential for that host gets 401 before any
  upstream contact: a GitHub token, another forge's token, the placeholder, a `basic` scheme, or
  none. GraphQL returns 401 too, not 200 with an error body. For a default-port entry, that is the
  real token, which the gateway forwards upstream. For an entry with a port, it is §3's random
  token, and the gateway must send the real token upstream instead. That needs one more L1
  builder method, say `.upstream_token(real)`, requested from the gateway thread for L3.
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
isn't forwarded, and gh keeps running. That is rare, and SIGTERM still works.

The handlers are installed before the run directory exists. Any of these four signals arriving
before gh is spawned aborts the launch. The launcher cleans up what it made and dies by that
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
| run directory, socket path length, bind, unreadable or unparseable user config | 1 |

After gh starts, the launcher writes nothing to the terminal. A cleanup failure is recorded in the
trace when tracing is on, and is otherwise silent. Its leftovers fall to the next sweep.

## 7. Environment

The launcher builds gh's environment from `std::env::vars_os()` and changes only the variables
named here. A non-UTF-8 value elsewhere passes through untouched. That is R7 (`jko.7`):
`routing.rs:90` collects `std::env::vars()`, which panics on one. L2 fixes it in `routing.rs`, and
the launcher never converts the environment to `String`.

| Variable | In launched gh |
|---|---|
| `GH_CONFIG_DIR` | the run directory |
| `GH_PATH` | real gh's absolute path, so gh's git credential helper and extensions run real gh, not the wrapper (fact 4) |
| `GH_BROWSER` | the URL helper (section 9) |
| `GH_HOST` | the routed host, for a host-less `gh api` or `gh auth token` only (section 2); otherwise unchanged |
| `GH_NO_UPDATE_NOTIFIER`, `GH_NO_EXTENSION_UPDATE_NOTIFIER` | `1`; these checks would call api.github.com through the socket |
| `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN` | removed (section 3) |
| `FJ_SHIM_LAUNCHER` | the launcher's pid |
| `FJ_SHIM_DEPTH` | the inherited depth plus 1. Every wrapper invocation sets it, on the delegate path too (section 8) |
| `FJ_SHIM_SAVED` | the names of the variables above that the launcher changed |
| `FJ_SHIM_ORIG_<NAME>` | the user's original value of each changed variable that was set |
| `FJ_SHIM_SET_<NAME>` | the value the launcher set for each changed variable it didn't remove |

The launcher spawns gh by absolute path, so `argv[0]` is that path too.

**Nested calls get the user's environment back.** gh's children inherit the launched environment:
alias shells, extensions, git, pager, editor and credential helpers. When one of them runs `gh`
again, it reaches the wrapper. Before routing, the wrapper reverts the launcher's changes. For each
name in `FJ_SHIM_SAVED`, it restores the variable from `FJ_SHIM_ORIG_<NAME>`, or unsets it if
there's no saved value. It skips any variable whose current state no longer matches what the
launcher left: a different value from `FJ_SHIM_SET_<NAME>`, or, for a removed variable, any value
at all. Something in between changed it on purpose. It then removes the
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

Guard 2 runs before the environment revert in section 7, which keeps `FJ_SHIM_DEPTH` for guard 3.

## 9. PR URLs and the browser (D2)

PR URLs from the gateway end in `/pull/N` in every mode (D2, bead `65s`). Forgejo serves
`/pulls/N`. In client mode no proxy exists to redirect, so the launcher sets `GH_BROWSER` to a
helper. Every gh `--web` open and every `gh browse` goes through it.

- `GH_BROWSER` is the shim's absolute path, shell-quoted for shlex, plus a hidden subcommand. gh
  appends the URL (fact 6).
- The helper rewrites the URL only when its host names a host in *S*, and its path is
  `/<owner>/<repo>/pull/<N>`, optionally followed by `/…`, a query or a fragment. For an
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
Each names the section it proves. On macOS the tests send only SIGTERM, SIGKILL, SIGINT and SIGHUP,
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
11. **Tokens (§3).** `gh auth token --hostname <forge>` prints the stored token. With no stored
    token, a launched `gh pr view` exits 4 with the login hint.
12. **Delegation (§2).** `gh --version` in a Forgejo checkout delegates (R4, `jko.5`).
    `gh config set` in a Forgejo checkout changes the user's real `config.yml`.
13. **Timeout (§10).** Once the session is cached, a stalled upstream answers within 10.5 s:
    a REST request (`gh api repos/{owner}/{repo}`) with 504, and a GraphQL request (`gh pr view`)
    with 200 and an `errors` body. A slow `/user` (9 s) followed by a stalled call answers within
    20.5 s. gh exits 1 in every case, and the launcher doesn't kill it.

## 12. Ownership

| Piece | Owner |
|---|---|
| launcher module, `routing.rs`, `repo.rs`, wrapper entry point | launcher thread (L2, L3, L4) |
| host isolation and multi-host router (§5) | gateway thread, L1 `cwe.26` |
| R1 identity check, R6 signal and exec pattern, browser subcommand registration in `cli.rs` | survivor-fixes thread, through the coordinator |
| per-host CA, `api_url` and server-mode key | L4 `cwe.29` and S `cwe.42` |

## 13. Decisions for Kell

1. **The token sits in a private `hosts.yml` for the length of a launched call** (§3). The
   alternative, the environment, leaks across hosts through gh's git credential helper.
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
3. **gh writes inside a launched call are discarded** (§3). The delegate list keeps the config-writing
   commands out, but an alias that expands to one inside a Forgejo checkout loses its write.
