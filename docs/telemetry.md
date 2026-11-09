# Usage reports

gh-forgejo-shim has one maintainer, and I can't test it with every agent and forge people use.
When an agent asks the gateway for a GitHub feature it doesn't have yet, the call fails, and I only
hear about it if someone files an issue. So the shim and its gateway, fga-server, send one
anonymous report a day: which agent and forge it ran with, and which missing features were asked
for. That tells me what to fix first, and in which apps. The reports also count installs and how
many keep running, so I can tell whether the work reaches people.

A report holds nothing more. Rounded weekly totals are public; the detailed counts
stay private. Turn reports off with `gfj telemetry off`, `GFJ_TELEMETRY=0` or `DO_NOT_TRACK=1`. See
[Turning reports off](#turning-reports-off).

## When a report goes

- **The gh wrapper** sends at most one report a UTC day, and only on a day it runs. The first `gh`
  call of the day starts a separate background process and doesn't wait for it. That process has
  no input or output, sends the report with a 2-second timeout, and stops. A `gh` call is never
  slowed, changed or made to fail, and nothing is printed, whether the report goes or not.
- **fga-server** sends one report a day while it runs, from ten minutes after it starts, so a
  server restarting in a loop sends nothing.
- **Only release builds report.** The release archives, the Homebrew formula's downloads and the
  gateway image are built with `GFJ_TELEMETRY_RELEASE` set. Any other build, from `cargo build`,
  `cargo install` or `cargo test --release`, sends nothing unless `GFJ_TELEMETRY_URL` or
  `FGA_TELEMETRY_URL` names an address, so tests and development runs never reach the collector.

`gfj telemetry show` prints the report the wrapper would send today, as it would send it, with one
difference: its `client` names whatever ran `show`, where the day's report names whatever ran `gh`.

## What a report holds

Every field, and nothing else. The collector refuses a report with any other field, or with a value
outside [`contract/telemetry.json`](../contract/telemetry.json).

From the gh wrapper:

```json
{"schema":1,"version":"0.4.0","os":"macos","arch":"aarch64","mode":"client","client":"codex","forge":"forgejo","forge_major":15,"gh":"2.103","first_week":"2026-W45","unsupported":{"graphql:Repository.projectsV2":2}}
```

From fga-server:

```json
{"schema":1,"version":"0.4.0","os":"linux","arch":"x86_64","mode":"server","forge":"forgejo","forge_major":15,"first_week":"2026-W45","unsupported":{}}
```

| Field | Values | Where it comes from |
|---|---|---|
| `schema` | `1` | The report's format. |
| `version` | `0.4.0` | The shim's release: its numbers, and a pre-release suffix only of the form `-rc.1`, `-beta.1` or `-alpha.1`. Any other text in a custom build's version is dropped. |
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

Turning reports off, in any of the ways below, deletes these files, and while reports are off
nothing is written there. Each file is opened only if it's a regular file, so nothing put there can
hold up a `gh` call.

fga-server keeps its first week and last day sent in `FGA_STATE_DIR`. The image sets that to
`/var/lib/fga-server`, and the example `compose.yaml` keeps it on a volume. Without it, the server
uses `$XDG_STATE_HOME/fga-server` or `~/.local/state/fga-server`. If none can be written, it keeps
them in memory, so each restart counts as a new install from that week.

## Turning reports off

Any of these turns the wrapper's reports off:

- `gfj telemetry off`, which writes `telemetry = false` to `~/.config/gh-forgejo-shim/config.toml`.
  `gfj telemetry on` turns them back on.
- `GFJ_TELEMETRY=0` (or `false`, `off`, `no`).
- `DO_NOT_TRACK=1`.
- `CI` set to any value, as continuous-integration services set it.

`gfj telemetry status` says whether reports are on, and if not, why.

For fga-server, `FGA_TELEMETRY=0` turns them off, and so do `DO_NOT_TRACK=1` and `CI`. It logs one
line at startup when reports are on.

`bootstrap`, `install-shim` and `doctor` print a one-time notice about reports. The gh wrapper never
prints one, because agents read its output.

## What the collector keeps

Reports go to `https://telemetry.dirtydishes.dev/v1/report`, a Cloudflare Worker whose code is in
[`telemetry/collector`](../telemetry/collector). For each report, it adds one to a counter for that
UTC day and that combination of values, one to a counter for that day and the report's first week,
and one to a counter for each `unsupported` name. That's all it stores. The first week is counted
apart from the other values, so no stored counter ties a combination to the week an install began.
It keeps no IP address, header, user agent or report body, and its request logs are off. After
each week ends, it also stores that week's published totals. It deletes counters and published
totals older than 400 days.

The counters are private. Only the maintainer reads them, straight from the database, with the
queries in [`telemetry/collector/README.md`](../telemetry/collector/README.md#reading-the-detailed-counts).
Nothing serves them over the web.

## Published totals

`https://telemetry.dirtydishes.dev/v1/stats` publishes a small summary as JSON, for anyone to read:
for each of the last 26 finished ISO weeks, the week's installs, and its installs by each of
`client`, `forge`, `mode`, `os` and `version` on its own.

```json
{
  "generated": "2026-11-16",
  "rounded_to": 10,
  "weeks": [
    {
      "week": "2026-W46",
      "installs": 120,
      "client": {"codex": 50, "chatgpt": 20, "claude-code": 40, "claude-desktop": "under 10", "cursor": "under 10", "other": "under 10", "none": 10},
      "forge": {"forgejo": 110, "gitea": 10, "other": "under 10", "none": "under 10"},
      "mode": {"client": 110, "server": 10},
      "os": {"macos": 80, "linux": 40, "other": "under 10"},
      "version": {"0.3.1": 30, "0.4.0": 90, "other": "under 10"}
    }
  ]
}
```

- Every figure gets a small random number added, then is rounded to the nearest 10. A figure that
  comes to 9 or less reads `"under 10"`, never a number. See
  [Noise and rounding](#noise-and-rounding).
- Each install sends at most one report a day, so a day's reports count the installs that ran that
  day. `installs` is the week's busiest day's count, the most the counts can prove ran that week.
  In a breakdown, each value's figure is its own busiest day's, so a field's figures needn't add
  up to `installs`.
- Every value of `client`, `forge`, `mode` and `os` is listed every week, so `"under 10"` may mean
  none. `none` counts reports without that field: fga-server's have no `client`, and `forge` is
  absent when the forge didn't answer. Versions whose count with noise is below 10 that week are
  counted together under `other`.
- Nothing else is published: no daily figures or sums of days, no combinations of values, no
  retention, no `unsupported` names, and no `arch`, `forge_major` or `gh`. The maintainer reads those from the
  counters.

A week's totals appear once it ends, Monday to Sunday in UTC. The collector works them out once,
early on the Monday after, and never changes them, so two responses can't be compared to find a
change.

### Noise and rounding

Before rounding, each figure gets its own random whole number, drawn so that 0 is likeliest and
about three draws in four fall between −8 and 8; larger ones are rarer the larger they are
(`NOISE_SCALE` in [`stats.ts`](../telemetry/collector/src/stats.ts) is 6). The collector draws them
once, when it works the week out, and stores the result, so asking again returns the same figure
and the noise can't be averaged away.

Reports carry no ID, so anyone can send them, and a sender knows how many of a figure's reports are
theirs. Without noise, someone could send just enough reports to put a figure one short of where
its rounding changes, and so tell whether one more real report came. Noise moves that point by more
than one report, so one real report more or less changes a figure's odds by about 1.2 times at
most, and never decides it. A report shows in at most six figures, `installs` and one value of each
field, so even read together they change the odds by under 3 times. The exception is the rare week
when a report tips its version's count over 10: the version's name then appears, and its count
leaves `other`.

What's left: each week's noise is new, so someone who repeats this every week for a long time can
slowly learn which values a quiet install tends to carry. A version one install ran can, rarely,
show by name as `10`. With more
installs reporting, a single report matters less, and the noise adds little error to the figures.
