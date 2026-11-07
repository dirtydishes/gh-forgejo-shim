# Usage reports

gh-forgejo-shim and its gateway, fga-server, send one anonymous report a day. The reports count
installs, show how many keep running week after week, and show which agents and forges people use
the shim with. They hold as little as that needs, and the totals are public.

Turn them off with `gfj telemetry off`, `GFJ_TELEMETRY=0` or `DO_NOT_TRACK=1`. See
[Turning reports off](#turning-reports-off).

## When a report goes

- **The gh wrapper** sends at most one report a UTC day, and only on a day it runs. The first `gh`
  call of the day starts a separate background process and doesn't wait for it. That process has
  no input or output, sends the report with a 2-second timeout, and stops. A `gh` call is never
  slowed, changed or made to fail, and nothing is printed, whether the report goes or not.
- **fga-server** sends one report a day while it runs, from ten minutes after it starts, so a
  server restarting in a loop sends nothing.
- **A debug build** (`cargo build` without `--release`) sends nothing unless `GFJ_TELEMETRY_URL` or
  `FGA_TELEMETRY_URL` names an address. Tests and development runs never reach the collector.

`gfj telemetry show` prints the exact report the wrapper would send today, byte for byte.

## What a report holds

Every field, and nothing else. The collector refuses a report with any other field, or with a value
outside [`contract/telemetry.json`](../contract/telemetry.json).

From the gh wrapper:

```json
{"schema":1,"version":"0.3.0","os":"macos","arch":"aarch64","mode":"client","client":"codex","forge":"forgejo","forge_major":15,"gh":"2.103","first_week":"2026-W45","unsupported":{"graphql:Repository.projectsV2":2}}
```

From fga-server:

```json
{"schema":1,"version":"0.3.0","os":"linux","arch":"x86_64","mode":"server","forge":"forgejo","forge_major":15,"first_week":"2026-W45","unsupported":{}}
```

| Field | Values | Where it comes from |
|---|---|---|
| `schema` | `1` | The report's format. |
| `version` | `0.3.0` | The shim's release. |
| `os` | `macos`, `linux`, `other` | The build. |
| `arch` | `x86_64`, `aarch64`, `other` | The build. |
| `mode` | `client`, `server` | The gh wrapper, or fga-server. |
| `client` | `codex`, `chatgpt`, `claude-code`, `claude-desktop`, `cursor`, `other` | The wrapper only. The nearest process above `gh` that it recognises, read from the local process list. Only the name from this list leaves the machine, never a program's name or command line. |
| `forge` | `forgejo`, `gitea`, `other` | The first allowlisted forge's version route, asked without a token. Left out when it doesn't answer. |
| `forge_major` | `15` | The same answer's major version. |
| `gh` | `2.103` | The wrapper only. Real gh's major and minor version. |
| `first_week` | `2026-W45` | The ISO week of the first report, kept locally. It shows how long installs keep running, without any ID. |
| `unsupported` | `{"rest:repos.releases": 1}` | How often the gateway lacked a GitHub REST resource or GraphQL field since the last report. |

A report never holds a host, repository, owner, path, command, argument, token, address, user name
or any kind of install ID. The `unsupported` names come only from the fixed list in
`contract/telemetry.json`: GitHub's own names for its REST resources (`rest:repos.releases`) and
GraphQL fields (`graphql:Repository.projectsV2`). Anything not on the list counts as `rest:other`,
`graphql:Repository.other` or `graphql:other`, so text from a request never reaches a report.
[`scripts/telemetry/contract.py`](../scripts/telemetry/contract.py) builds the list from GitHub's
public schema.

## What's kept locally

The wrapper keeps a few small files in `$XDG_STATE_HOME/gh-forgejo-shim/telemetry`, or
`~/.local/state/gh-forgejo-shim/telemetry`:

- `first-week`: the week of the first report.
- `sent-YYYY-MM-DD`: today's report has gone.
- `unsupported`: names and counts waiting for the next report.
- `notice-shown`: `bootstrap`, `install-shim` or `doctor` has printed the notice.

While reports are off, nothing is written there.

fga-server keeps its first week and last day sent in memory. Set `FGA_STATE_DIR` to a writable
directory to keep them across restarts. Otherwise each restart counts as a new install from that
week.

## Turning reports off

Any of these turns the wrapper's reports off:

- `gfj telemetry off`, which writes `telemetry = false` to `~/.config/gh-forgejo-shim/config.toml`.
  `gfj telemetry on` turns them back on.
- `GFJ_TELEMETRY=0` (or `false`, `off`, `no`).
- `DO_NOT_TRACK=1`.
- `CI` set to anything but `0` or `false`, as continuous-integration services set it.

`gfj telemetry status` says whether reports are on, and if not, why.

For fga-server, `FGA_TELEMETRY=0` turns them off, and so do `DO_NOT_TRACK=1` and `CI`. It logs one
line at startup when reports are on.

`bootstrap`, `install-shim` and `doctor` print a one-time notice about reports. The gh wrapper never
prints one, because agents read its output.

## What the collector keeps

Reports go to `https://telemetry.dirtydishes.dev/v1/report`, a Cloudflare Worker whose code is in
[`telemetry/collector`](../telemetry/collector). For each report, it adds one to a counter for that
UTC day and that combination of values, and one to a counter for each `unsupported` name. That's
all it stores. It keeps no IP address, header, user agent or report body, and its request logs are
off. It deletes counters older than 400 days.

## Published totals

`https://telemetry.dirtydishes.dev/v1/stats` publishes the totals as JSON, for anyone to read:

- `totals`: installs and reports per day (the last 90 days) and per week (the last 26 weeks).
- `by_field`: the same, for each value of `client`, `forge`, `mode`, `os` and `version` on its own.
- `combinations`: the same, for each combination of `mode`, `client`, `forge`, `forge_major`, `os`,
  `arch`, `version` and `gh`.
- `retention`: for each `first_week`, the installs seen in each later week.
- `unsupported`: per week, how many reports named each unsupported name, and how many times in all.

Each install sends at most one report a day, so a day's reports count the installs that ran that
day. A week's `installs` is its busiest day's count, the most the counts can prove ran that week.

Any count below 3 is left out, with its labels, so no published figure can describe just one or two
installs. The rule applies to every figure in every section.
